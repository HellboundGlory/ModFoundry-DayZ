//! Driving the official DsUtils signing tools (`DSCreateKey`, `DSSignFile`,
//! `DSCheckSignatures`) through the `modfoundry-platform` process seam.
//!
//! Kept as the opt-in differential oracle and Windows-native path (`mf build
//! --tools`); the default is [`crate::native`], whose output is byte-identical
//! to these tools. A [`Runner`]
//! abstraction keeps this crate unit-testable without the tools present.

use modfoundry_platform::host_path_in_wine;
use std::path::{Path, PathBuf};

/// Which bisign version to emit. `V3` is the DayZ Tools default; `V2` is the
/// legacy escape hatch (`DSSignFile -v2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignVersion {
    V2,
    V3,
}

impl SignVersion {
    fn flag(&self) -> Option<&'static str> {
        match self {
            SignVersion::V2 => Some("-v2"),
            SignVersion::V3 => None,
        }
    }

    /// The version field a `.bisign` stores.
    pub fn number(self) -> u32 {
        match self {
            SignVersion::V2 => 2,
            SignVersion::V3 => 3,
        }
    }

    /// The version a stored field names, if it is one this crate writes.
    pub fn from_number(number: u32) -> Option<Self> {
        match number {
            2 => Some(SignVersion::V2),
            3 => Some(SignVersion::V3),
            _ => None,
        }
    }
}

/// Captured result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOut {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Executes a tool binary with args. Hidden behind the real runner so tests
/// can record calls without Wine.
pub trait Runner: Send + Sync {
    /// Run `exe` with `args` from working directory `cwd`.
    fn run(&self, exe: &Path, args: &[String], cwd: &Path) -> anyhow::Result<ProcessOut>;
}

/// Runs the DsUtils exe natively on Windows and via `wine` on other hosts,
/// translating absolute host paths to the Wine prefix (`Z:\...`).
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, exe: &Path, args: &[String], cwd: &Path) -> anyhow::Result<ProcessOut> {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new(exe);
            c.current_dir(cwd);
            c.args(args);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("wine");
            c.current_dir(cwd);
            c.arg(wine_path(exe));
            c.args(args.iter().map(|a| wine_path(Path::new(a))));
            c
        };
        let out = cmd.output()?;
        Ok(ProcessOut {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Translate an absolute host path to its Wine form; leave non-paths alone.
fn wine_path(p: &Path) -> String {
    if p.is_absolute() {
        host_path_in_wine(p)
    } else {
        p.to_string_lossy().into_owned()
    }
}

/// File-name helpers (pure, unit-tested).
pub fn private_key_name(authority: &str) -> String {
    format!("{authority}.biprivatekey")
}
pub fn public_key_name(authority: &str) -> String {
    format!("{authority}.bikey")
}
/// `<pbo_file_name>.pbo.<authority>.bisign`, e.g. `ai.pbo` + `dayz` →
/// `ai.pbo.dayz.bisign`.
pub fn bisign_name(pbo_file_name: &str, authority: &str) -> String {
    format!("{pbo_file_name}.{authority}.bisign")
}

/// Authority name is the private-key file stem (`Foo.biprivatekey` → `Foo`).
pub fn authority_of_private_key(key: &Path) -> Option<String> {
    key.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".biprivatekey"))
        .map(str::to_owned)
}

/// DsUtils tool wrapper bound to a tools directory (contains the `.exe`s and
/// their DLLs, e.g. `<DayZ Tools>/Bin/DsUtils`).
pub struct DsUtils {
    tools_dir: PathBuf,
    runner: Box<dyn Runner>,
}

impl DsUtils {
    pub fn new(tools_dir: PathBuf) -> Self {
        Self {
            tools_dir,
            runner: Box::new(ProcessRunner),
        }
    }

    /// Test-only: inject a recording runner.
    pub fn with_runner(tools_dir: PathBuf, runner: Box<dyn Runner>) -> Self {
        Self { tools_dir, runner }
    }

    fn exe(&self, name: &str) -> PathBuf {
        self.tools_dir.join(name)
    }

    fn run(&self, exe: &str, args: &[String], cwd: &Path) -> anyhow::Result<ProcessOut> {
        let exe = self.exe(exe);
        if !exe.exists() {
            anyhow::bail!(
                "signing tool not found: {} (set the DayZ Tools path)",
                exe.display()
            );
        }
        let out = self.runner.run(&exe, args, cwd)?;
        if out.code != 0 {
            anyhow::bail!(
                "{} failed (exit {}){}{}",
                exe.file_name().unwrap_or_default().to_string_lossy(),
                out.code,
                tail(&out.stdout),
                tail(&out.stderr)
            );
        }
        Ok(out)
    }

    /// `DSCreateKey <authority>` → `{authority}.biprivatekey` +
    /// `{authority}.bikey` written into `keys_dir`. Returns the private/public
    /// key paths.
    pub fn create_key(
        &self,
        keys_dir: &Path,
        authority: &str,
    ) -> anyhow::Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(keys_dir)?;
        let args = vec![authority.to_owned()];
        // DSCreateKey writes `<authority>.bikey` + `.biprivatekey` to its cwd.
        self.run("DSCreateKey.exe", &args, keys_dir)?;
        let priv_key = keys_dir.join(private_key_name(authority));
        let pub_key = keys_dir.join(public_key_name(authority));
        if !priv_key.is_file() || !pub_key.is_file() {
            anyhow::bail!(
                "DSCreateKey did not produce expected key files in {}",
                keys_dir.display()
            );
        }
        Ok((priv_key, pub_key))
    }

    /// `DSSignFile <private_key> <pbo> [-v2]`. Writes the `.bisign` sibling
    /// next to `pbo` and returns its path.
    pub fn sign(
        &self,
        private_key: &Path,
        pbo: &Path,
        version: SignVersion,
    ) -> anyhow::Result<PathBuf> {
        let authority = authority_of_private_key(private_key).ok_or_else(|| {
            anyhow::anyhow!(
                "private key filename must end in .biprivatekey: {}",
                private_key.display()
            )
        })?;
        // The tool runs with its cwd set to the PBO's directory, so both paths
        // handed to it must be absolute for `wine_path` to map them.
        let private_key = absolute(private_key)?;
        let pbo = folded(pbo)?;
        let mut args = vec![
            private_key.to_string_lossy().into_owned(),
            pbo.to_string_lossy().into_owned(),
        ];
        if let Some(flag) = version.flag() {
            args.push(flag.to_owned());
        }
        let pbo_dir = pbo.parent().unwrap_or_else(|| Path::new("."));
        self.run("DSSignFile.exe", &args, pbo_dir)?;
        let pbo_name = pbo
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("pbo path has no filename"))?;
        let bisign = pbo
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(bisign_name(pbo_name, &authority));
        if !bisign.is_file() {
            anyhow::bail!("DSSignFile did not produce expected {}", bisign.display());
        }
        Ok(bisign)
    }
}

/// The resolved path the tool is handed: `DSSignFile` rejects `..`, and the
/// Wine bridge needs it absolute. Windows' `\\?\` verbatim prefix is dropped, as
/// the tools do not read it.
fn absolute(p: &Path) -> anyhow::Result<PathBuf> {
    let resolved = std::fs::canonicalize(p)
        .map_err(|e| anyhow::anyhow!("cannot resolve {}: {e}", p.display()))?;
    let text = resolved.to_string_lossy();
    Ok(if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(plain) = text.strip_prefix(r"\\?\") {
        PathBuf::from(plain)
    } else {
        resolved
    })
}

/// `p` made absolute with `.` and `..` folded away lexically, so a symlinked
/// PBO keeps its own folder and its `.bisign` lands beside it.
fn folded(p: &Path) -> anyhow::Result<PathBuf> {
    let abs = std::path::absolute(p)
        .map_err(|e| anyhow::anyhow!("cannot resolve {}: {e}", p.display()))?;
    let mut out = PathBuf::new();
    for part in abs.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    Ok(out)
}

fn tail(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!("\n{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// (exe, args) captured per tool invocation.
    type Call = (String, Vec<String>);
    type Calls = Arc<Mutex<Vec<Call>>>;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp_seq() -> u64 {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn naming_helpers() {
        assert_eq!(private_key_name("MF"), "MF.biprivatekey");
        assert_eq!(public_key_name("MF"), "MF.bikey");
        assert_eq!(bisign_name("ai.pbo", "dayz"), "ai.pbo.dayz.bisign");
        assert_eq!(
            authority_of_private_key(Path::new("/k/MF.biprivatekey")).as_deref(),
            Some("MF")
        );
        assert_eq!(authority_of_private_key(Path::new("/k/other.pem")), None);
    }

    /// Fake runner that records calls and writes the sibling `.bisign` (so
    /// `sign()`'s post-existence check passes) based on the key+pbo args.
    struct Recording {
        calls: Calls,
    }
    impl Runner for Recording {
        fn run(&self, exe: &Path, args: &[String], _cwd: &Path) -> anyhow::Result<ProcessOut> {
            self.calls
                .lock()
                .push((exe.to_string_lossy().into_owned(), args.to_vec()));
            // args: [0] private key, [1] pbo file
            if exe.to_string_lossy().ends_with("DSSignFile.exe") {
                let authority = authority_of_private_key(Path::new(&args[0])).unwrap();
                let pbo = Path::new(&args[1]);
                let name = pbo.file_name().and_then(|n| n.to_str()).unwrap().to_owned();
                std::fs::write(
                    pbo.parent().unwrap().join(bisign_name(&name, &authority)),
                    b"sig",
                )
                .unwrap();
            }
            Ok(ProcessOut {
                code: 0,
                stdout: "Signature x.pbo.k.bisign is OK".into(),
                stderr: String::new(),
            })
        }
    }

    fn tmp_dir() -> std::path::PathBuf {
        // Unique per call: tests in one binary share `process::id()`, so a
        // pid-only key makes concurrent tests race on the same directory.
        let d = std::env::temp_dir().join(format!("mfsign-{}-{}", std::process::id(), tmp_seq()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sign_invokes_dssignfile_with_resolved_paths_and_the_version_flag() {
        for (version, flag) in [(SignVersion::V2, Some("-v2")), (SignVersion::V3, None)] {
            let tmp = tmp_dir();
            let dir = tmp.join("tools");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("DSSignFile.exe"), b"").unwrap();
            std::fs::write(tmp.join("MF.biprivatekey"), b"fake").unwrap();
            std::fs::create_dir_all(tmp.join("a")).unwrap();
            let pk = tmp.join("a").join("..").join("MF.biprivatekey");
            let pbo = tmp.join("a").join("..").join("x.pbo");
            std::fs::write(tmp.join("x.pbo"), b"pbo").unwrap();

            let calls: Calls = Arc::new(Mutex::new(Vec::new()));
            let rec = Recording {
                calls: calls.clone(),
            };
            let du = DsUtils::with_runner(dir, Box::new(rec));
            let bisign = du.sign(&pk, &pbo, version).unwrap();
            assert_eq!(
                bisign.file_name().unwrap().to_str().unwrap(),
                "x.pbo.MF.bisign"
            );

            let calls = calls.lock();
            let (exe, args) = &calls[0];
            assert!(exe.ends_with("DSSignFile.exe"));
            assert!(args[0].ends_with("MF.biprivatekey"));
            assert!(args[1].ends_with("x.pbo"));
            assert!(
                Path::new(&args[0]).is_absolute() && Path::new(&args[1]).is_absolute(),
                "tool args must be absolute so the Wine bridge maps them"
            );
            for arg in &args[..2] {
                assert!(
                    !Path::new(arg)
                        .components()
                        .any(|c| c == std::path::Component::ParentDir),
                    "DSSignFile refuses a path with `..`: {arg}"
                );
            }
            assert_eq!(args.get(2).map(String::as_str), flag, "{version:?}");
            assert_eq!(args.len(), 2 + usize::from(flag.is_some()));
        }
    }

    #[test]
    fn create_key_invokes_dscreatekey_and_checks_output() {
        let tmp = tmp_dir();
        let dir = tmp.join("tools");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DSCreateKey.exe"), b"").unwrap();
        let keys = tmp.join("keys");
        // A fake DSCreateKey that writes the two key files it would produce.
        struct KeyMaker;
        impl Runner for KeyMaker {
            fn run(&self, _exe: &Path, args: &[String], cwd: &Path) -> anyhow::Result<ProcessOut> {
                let authority = &args[0];
                std::fs::write(cwd.join(private_key_name(authority)), b"priv").unwrap();
                std::fs::write(cwd.join(public_key_name(authority)), b"pub").unwrap();
                Ok(ProcessOut {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
        let du = DsUtils::with_runner(dir, Box::new(KeyMaker));
        let (pk, pubk) = du.create_key(&keys, "MF").unwrap();
        assert_eq!(pk.file_name().unwrap().to_str().unwrap(), "MF.biprivatekey");
        assert_eq!(pubk.file_name().unwrap().to_str().unwrap(), "MF.bikey");
    }

    #[test]
    fn sign_requires_biprivatekey_name() {
        let du = DsUtils::new("/nonexistent".into());
        let err = du.sign(
            Path::new("/tmp/foo.key"),
            Path::new("/tmp/x.pbo"),
            SignVersion::V3,
        );
        assert!(err.unwrap_err().to_string().contains("biprivatekey"));
    }
}
