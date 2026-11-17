//! Finding and starting the helper programs that ship beside ModFoundry's own binaries.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The program `name` in the running executable's folder, with the host's executable suffix.
pub fn sibling_program(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(
        exe.parent()?
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
    )
}

/// Puts `dir` first on the `LD_LIBRARY_PATH` of the process `cmd` starts; nothing changes off Linux.
pub fn prepend_library_dir(cmd: &mut Command, dir: &Path) {
    #[cfg(target_os = "linux")]
    {
        const VAR: &str = "LD_LIBRARY_PATH";
        let existing = cmd
            .get_envs()
            .find(|(key, _)| *key == VAR)
            .map(|(_, value)| value.map(ToOwned::to_owned))
            .unwrap_or_else(|| std::env::var_os(VAR));
        cmd.env(VAR, library_path(dir.as_os_str(), existing));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (cmd, dir);
}

/// `dir` ahead of an existing library path; an empty entry would make the loader search the
/// current folder.
#[cfg(target_os = "linux")]
fn library_path(dir: &std::ffi::OsStr, existing: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let mut path = dir.to_owned();
    if let Some(existing) = existing.filter(|existing| !existing.is_empty()) {
        path.push(":");
        path.push(existing);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_program_sits_beside_the_running_executable() {
        let exe = std::env::current_exe().unwrap();
        let sibling = sibling_program("mf-embed-gpu").unwrap();
        assert_eq!(sibling.parent(), exe.parent());
        assert_eq!(
            sibling.file_name().unwrap().to_string_lossy(),
            format!("mf-embed-gpu{}", std::env::consts::EXE_SUFFIX)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_library_path_skips_an_empty_entry() {
        let dir = std::ffi::OsStr::new("/opt/mf");
        let cases = [
            (None, "/opt/mf"),
            (Some(""), "/opt/mf"),
            (Some("/usr/lib"), "/opt/mf:/usr/lib"),
        ];
        for (existing, want) in cases {
            assert_eq!(
                library_path(dir, existing.map(std::ffi::OsString::from)),
                std::ffi::OsString::from(want),
                "{existing:?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prepend_library_dir_keeps_a_value_set_on_the_command() {
        let _spawning = crate::spawn_lock();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf %s \"$LD_LIBRARY_PATH\""]);
        cmd.env("LD_LIBRARY_PATH", "/old/a:/old/b");
        prepend_library_dir(&mut cmd, Path::new("/new/lib"));
        let out = cmd.output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "/new/lib:/old/a:/old/b"
        );
    }
}
