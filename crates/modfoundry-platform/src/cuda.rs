//! Where GPU search finds its helper program and the CUDA libraries that helper loads.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The GPU embedding helper's file name, without the platform suffix; it ships beside `mf`.
pub const GPU_HELPER: &str = "mf-embed-gpu";

/// The folder `tools/setup-cuda.sh` fills with the CUDA libraries the GPU helper loads.
pub fn cuda_lib_dir() -> Option<PathBuf> {
    crate::app_data_dir().map(|dir| dir.join("cuda").join("lib"))
}

/// The `ldconfig -p` listing of cached libraries; empty off Linux or when no `ldconfig` runs.
pub fn loader_cache() -> String {
    if !cfg!(target_os = "linux") {
        return String::new();
    }
    loader_cache_from(&["ldconfig", "/usr/sbin/ldconfig", "/sbin/ldconfig"])
}

/// The listing of the first of `programs` whose `-p` run succeeds; empty when none does.
fn loader_cache_from(programs: &[&str]) -> String {
    programs
        .iter()
        .filter_map(|program| Command::new(program).arg("-p").output().ok())
        .find(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// Whether the `ldconfig -p` listing `cache` names `soname` as a cached library.
pub fn in_loader_cache(cache: &str, soname: &str) -> bool {
    cache
        .lines()
        .any(|line| line.split_whitespace().next() == Some(soname))
}

/// The NVIDIA driver library every CUDA program loads.
pub const CUDA_DRIVER: &str = "libcuda.so.1";

/// The CUDA runtime libraries the GPU helper needs beside the driver.
pub const CUDA_LIBRARIES: [&str; 5] = [
    "libcudart.so.13",
    "libcublas.so.13",
    "libcublasLt.so.13",
    "libcurand.so.10",
    "libnvrtc.so.13",
];

/// The `CUDA_LIBRARIES` names that are neither in `lib_dir` nor in the loader `cache`.
pub fn missing_cuda_libraries(lib_dir: Option<&Path>, cache: &str) -> Vec<&'static str> {
    CUDA_LIBRARIES
        .into_iter()
        .filter(|name| {
            let in_dir = lib_dir.is_some_and(|dir| dir.join(name).exists());
            !in_dir && !in_loader_cache(cache, name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs::Permissions;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn loader_cache_skips_an_ldconfig_that_fails() {
        let _spawning = crate::spawn_lock();
        let dir = std::env::temp_dir().join(format!("mf-cuda-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let failing = script(&dir, "failing", "echo wrong; exit 1");
        let working = script(&dir, "working", "echo right");
        assert_eq!(loader_cache_from(&[&failing, &working]), "right\n");
        assert_eq!(loader_cache_from(&[&failing, &failing]), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loader_cache_lookup_matches_the_whole_soname() {
        let cache = "\tlibcuda.so.1 (libc6,x86-64) => /lib64/libcuda.so.1\n\
                     \tlibcudart.so.13 (libc6,x86-64) => /usr/local/cuda/lib64/libcudart.so.13\n";
        assert!(in_loader_cache(cache, "libcuda.so.1"));
        assert!(in_loader_cache(cache, "libcudart.so.13"));
        assert!(!in_loader_cache(cache, "libcudart.so.1"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_libraries_skip_present_and_cached_names_in_order() {
        let dir = std::env::temp_dir().join(format!("mf-cuda-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libcudart.so.13"), b"").unwrap();
        std::os::unix::fs::symlink(dir.join("gone.so"), dir.join("libcublas.so.13")).unwrap();
        let cache = "\tlibcurand.so.10 (libc6,x86-64) => /usr/local/cuda/lib64/libcurand.so.10\n";
        let missing = missing_cuda_libraries(Some(&dir), cache);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            missing,
            vec!["libcublas.so.13", "libcublasLt.so.13", "libnvrtc.so.13"]
        );
    }
}
