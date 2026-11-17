//! `tools/setup-cuda.sh` run against stub `ldconfig` and `python3` programs, so no download happens.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

static SPAWNS: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    SPAWNS.lock().unwrap_or_else(PoisonError::into_inner)
}

const SCRIPT: &str = include_str!("../../../../tools/setup-cuda.sh");

const PINNED: [&str; 4] = [
    "nvidia-cuda-runtime==13.2.86",
    "nvidia-cuda-nvrtc==13.2.86",
    "nvidia-cublas==13.6.0.2",
    "nvidia-curand==10.4.3.29",
];

const WHEEL_LIBS: [&str; 6] = [
    "libcudart.so.13",
    "libcublas.so.13",
    "libcublasLt.so.13",
    "libcurand.so.10",
    "libnvrtc.so.13",
    "libnvrtc-builtins.so.13.2",
];

const CACHE_WITH_DRIVER: &str = "\tlibcuda.so.1 (libc6,x86-64) => /lib64/libcuda.so.1\n";

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mf-setup-cuda-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_stub(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn fake_ldconfig(stubs: &Path, cache: &str) {
    write_stub(
        &stubs.join("ldconfig"),
        &format!("cat <<'EOF'\n{cache}EOF\n"),
    );
}

fn fake_python(stubs: &Path, log: &Path, skip: &str) {
    let libs = WHEEL_LIBS.join(" ");
    let body = format!(
        r#"echo "$@" >> '{log}'
if [ "$3" = install ]; then
  target=""; prev=""
  for arg in "$@"; do
    if [ "$prev" = --target ]; then target="$arg"; fi
    prev="$arg"
  done
  mkdir -p "$target/nvidia/cu13/lib"
  for lib in {libs}; do
    [ "$lib" = '{skip}' ] || : > "$target/nvidia/cu13/lib/$lib"
  done
fi
"#,
        log = log.display(),
    );
    write_stub(&stubs.join("python3"), &body);
}

fn run_script(
    sandbox: &Sandbox,
    cache: &str,
    skip: &str,
    data: impl AsRef<Path>,
    extra: &[(&str, &str)],
) -> Output {
    // A sibling test forking while a stub is still open for writing makes its exec fail with "text file busy".
    let _spawn = spawn_lock();
    let stubs = sandbox.join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    fake_ldconfig(&stubs, cache);
    fake_python(&stubs, &sandbox.join("python.log"), skip);
    for (name, body) in extra {
        write_stub(&stubs.join(name), body);
    }
    let script = sandbox.join("setup-cuda.sh");
    std::fs::write(&script, SCRIPT).unwrap();
    let path = format!(
        "{}:{}",
        stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new("bash")
        .arg(&script)
        .env("PATH", path)
        .current_dir(&sandbox.0)
        .env("MODFOUNDRY_DATA_DIR", data.as_ref())
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn installs_the_pinned_wheels_and_links_the_libraries() {
    let sandbox = Sandbox::new("ready");
    let out = run_script(&sandbox, CACHE_WITH_DRIVER, "", sandbox.join("data"), &[]);
    assert!(out.status.success(), "{}", text(&out));
    let lib_dir = sandbox.join("data/cuda/lib");
    for name in [
        "libcudart.so.13",
        "libcublas.so.13",
        "libcublasLt.so.13",
        "libcurand.so.10",
        "libnvrtc.so.13",
    ] {
        let link = lib_dir.join(name);
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "{name}"
        );
        assert!(link.exists(), "{name} is a broken link");
    }
    let calls = std::fs::read_to_string(sandbox.join("python.log")).unwrap();
    assert!(calls.contains("--no-deps"), "{calls}");
    for pin in PINNED {
        assert!(calls.contains(pin), "{calls}");
    }
}

#[test]
fn stops_before_downloading_when_the_driver_is_missing() {
    let sandbox = Sandbox::new("no-driver");
    let out = run_script(
        &sandbox,
        "\tlibc.so.6 (libc6,x86-64) => /lib64/libc.so.6\n",
        "",
        sandbox.join("data"),
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("libcuda.so.1"), "{}", text(&out));
    assert!(!sandbox.join("python.log").exists());
}

#[test]
fn names_a_library_the_install_left_out() {
    let sandbox = Sandbox::new("missing-lib");
    let out = run_script(
        &sandbox,
        CACHE_WITH_DRIVER,
        "libcurand.so.10",
        sandbox.join("data"),
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let message = text(&out);
    assert!(message.contains("missing: libcurand.so.10"), "{message}");
    assert!(!message.contains("libcudart.so.13"), "{message}");
}

#[test]
fn links_resolve_when_the_data_folder_is_relative() {
    let sandbox = Sandbox::new("relative");
    let out = run_script(&sandbox, CACHE_WITH_DRIVER, "", "data", &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(sandbox.join("data/cuda/lib/libcudart.so.13").exists());
}

#[test]
fn a_rerun_clears_links_left_dangling_by_an_older_install() {
    let sandbox = Sandbox::new("rerun");
    let lib_dir = sandbox.join("data/cuda/lib");
    std::fs::create_dir_all(&lib_dir).unwrap();
    let stale = lib_dir.join("libnvrtc-builtins.so.13.1");
    std::os::unix::fs::symlink(sandbox.join("gone"), &stale).unwrap();
    let out = run_script(&sandbox, CACHE_WITH_DRIVER, "", sandbox.join("data"), &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stale.symlink_metadata().is_err(),
        "the dangling link is still there"
    );
}

#[test]
fn a_failed_link_stops_the_script() {
    let sandbox = Sandbox::new("failed-link");
    let lib_dir = sandbox.join("data/cuda/lib");
    std::fs::create_dir_all(sandbox.join("data/cuda/wheels/nvidia/lib")).unwrap();
    let wheel = sandbox.join("data/cuda/wheels/nvidia/lib/libcudart.so.13");
    std::fs::write(&wheel, b"").unwrap();
    std::fs::create_dir_all(&lib_dir).unwrap();
    for name in [
        "libcudart.so.13",
        "libcublas.so.13",
        "libcublasLt.so.13",
        "libcurand.so.10",
        "libnvrtc.so.13",
    ] {
        std::os::unix::fs::symlink(&wheel, lib_dir.join(name)).unwrap();
    }
    let out = run_script(
        &sandbox,
        CACHE_WITH_DRIVER,
        "",
        sandbox.join("data"),
        &[("ln", "exit 1")],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        !text(&out).contains("CUDA libraries ready"),
        "{}",
        text(&out)
    );
}

#[test]
fn an_unreadable_wheel_folder_stops_the_script() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new("unreadable-wheels");
    let locked = sandbox.join("data/cuda/wheels/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let out = run_script(&sandbox, CACHE_WITH_DRIVER, "", sandbox.join("data"), &[]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        !text(&out).contains("CUDA libraries ready"),
        "{}",
        text(&out)
    );
}
