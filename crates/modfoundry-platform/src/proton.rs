//! Direct Proton launch: the command that starts DayZ under Steam's Proton
//! runtime, described purely so it can be asserted without spawning anything.
//!
//! `steam -applaunch 221100` opens the official launcher rather than the game,
//! so a client run builds the runtime command itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::DAYZ_APP_ID;

const BE_LAUNCHER_EXE: &str = "DayZ_BE.exe";
const GAME_EXE: &str = "DayZ_x64.exe";

/// A fully described Proton launch command. Constructing one touches no process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtonLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(String, OsString)>,
    pub env_remove: Vec<String>,
}

/// Why a direct Proton launch could not be described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtonLaunchError {
    /// No Proton build under `steamapps/common`.
    NoProton,
    /// No Steam Linux Runtime under `steamapps/common`.
    NoRuntime,
}

impl std::fmt::Display for ProtonLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtonLaunchError::NoProton => {
                f.write_str("no installed Proton build found under steamapps/common")
            }
            ProtonLaunchError::NoRuntime => {
                f.write_str("no Steam Linux Runtime found under steamapps/common")
            }
        }
    }
}

impl std::error::Error for ProtonLaunchError {}

/// Best Proton build under `common`, by directory scan.
pub fn pick_proton(common: &Path) -> Option<PathBuf> {
    best_proton(scan(common, "Proton", "proton"))
}

/// Best Steam Linux Runtime under `common`, by directory scan.
pub fn pick_runtime(common: &Path) -> Option<PathBuf> {
    best_runtime(scan(common, "SteamLinuxRuntime", "run"))
}

fn scan(common: &Path, dir_prefix: &str, binary: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(common) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(dir_prefix))
        })
        .map(|p| p.join(binary))
        .filter(|p| p.is_file())
        .collect()
}

fn best_proton(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().max_by_key(|p| proton_rank(p))
}

fn proton_rank(proton_binary: &Path) -> (bool, (u32, u32)) {
    let dir_name = dir_name(proton_binary);
    let experimental = dir_name.to_ascii_lowercase().contains("experimental");
    (experimental, parse_proton_version(dir_name))
}

fn parse_proton_version(dir_name: &str) -> (u32, u32) {
    let after_prefix = dir_name.strip_prefix("Proton").unwrap_or(dir_name).trim();
    let version_token = after_prefix.split_whitespace().next().unwrap_or("");
    let mut parts = version_token.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

fn best_runtime(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().max_by_key(|p| runtime_rank(p))
}

fn runtime_rank(run_binary: &Path) -> u8 {
    let dir_name = dir_name(run_binary);
    if dir_name.ends_with("_sniper") {
        2
    } else if dir_name.ends_with("_soldier") {
        1
    } else {
        0
    }
}

fn dir_name(binary: &Path) -> &str {
    binary
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// Describe the runtime command that starts `game_exe` under Proton.
pub fn proton_launch(
    steam_root: &Path,
    game_exe: &Path,
    game_args: &[String],
) -> Result<ProtonLaunch, ProtonLaunchError> {
    let common = steam_root.join("steamapps").join("common");
    let proton = pick_proton(&common).ok_or(ProtonLaunchError::NoProton)?;
    let runtime = pick_runtime(&common).ok_or(ProtonLaunchError::NoRuntime)?;

    let mut args = vec![
        OsString::from("--"),
        proton.into_os_string(),
        OsString::from("run"),
        game_exe.as_os_str().to_os_string(),
    ];
    args.extend(battleye_prefix(game_exe).into_iter().map(OsString::from));
    args.extend(game_args.iter().map(OsString::from));

    let app_id = DAYZ_APP_ID.to_string();
    Ok(ProtonLaunch {
        program: runtime,
        args,
        cwd: game_exe.parent().unwrap_or(Path::new("")).to_path_buf(),
        env: vec![
            (
                "STEAM_COMPAT_DATA_PATH".to_string(),
                crate::compatdata_dir(&steam_root.join("steamapps"), DAYZ_APP_ID).into_os_string(),
            ),
            (
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
                steam_root.as_os_str().to_os_string(),
            ),
            ("SteamAppId".to_string(), OsString::from(&app_id)),
            ("SteamGameId".to_string(), OsString::from(&app_id)),
        ],
        env_remove: [
            "LD_LIBRARY_PATH",
            "APPIMAGE",
            "APPDIR",
            "PYTHONHOME",
            "PYTHONPATH",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    })
}

/// DayZ_BE.exe runs its own updater ("Update Failed") and resolves the game exe
/// against its working directory unless told where it is.
fn battleye_prefix(game_exe: &Path) -> Vec<String> {
    let name = game_exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !name.eq_ignore_ascii_case(BE_LAUNCHER_EXE) {
        return Vec::new();
    }
    let mut args = ["0", "1", "0"].map(str::to_string).to_vec();
    if game_exe.with_file_name(GAME_EXE).is_file() {
        args.push("-exe".to_string());
        args.push(GAME_EXE.to_string());
    }
    args
}

/// The DayZ executable in `game_dir`: `DayZ_BE.exe` when present, else `DayZ_x64.exe`.
pub fn find_dayz_exe(game_dir: &Path) -> Option<PathBuf> {
    [BE_LAUNCHER_EXE, GAME_EXE]
        .into_iter()
        .map(|name| game_dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn proton(dir: &str) -> PathBuf {
        PathBuf::from(format!("/steam/steamapps/common/{dir}/proton"))
    }

    fn runtime(dir: &str) -> PathBuf {
        PathBuf::from(format!("/steam/steamapps/common/{dir}/run"))
    }

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn unique() -> String {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}-{nanos}-{n}", std::process::id())
    }

    struct Fixture(PathBuf);

    impl Fixture {
        /// A temp `steam_root` whose `steamapps/common` holds the named builds.
        fn new(proton_dir: Option<&str>, runtime_dir: Option<&str>) -> Self {
            let root = std::env::temp_dir().join(format!("mf-proton-{}", unique()));
            let common = root.join("steamapps").join("common");
            std::fs::create_dir_all(&common).unwrap();
            if let Some(dir) = proton_dir {
                let dir = common.join(dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("proton"), b"").unwrap();
            }
            if let Some(dir) = runtime_dir {
                let dir = common.join(dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("run"), b"").unwrap();
            }
            Fixture(root)
        }

        fn root(&self) -> &Path {
            &self.0
        }

        fn game_dir(&self) -> PathBuf {
            let dir = self.0.join("steamapps").join("common").join("DayZ");
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn basic_fixture() -> Fixture {
        Fixture::new(
            Some("Proton - Experimental"),
            Some("SteamLinuxRuntime_sniper"),
        )
    }

    #[test]
    fn a_higher_numeric_version_beats_a_lower_one_despite_sorting_first() {
        let candidates = vec![proton("Proton 9.0 (Beta)"), proton("Proton 5.13")];
        assert_eq!(best_proton(candidates), Some(proton("Proton 9.0 (Beta)")));
    }

    #[test]
    fn experimental_beats_every_numbered_build() {
        let candidates = vec![
            proton("Proton 9.0"),
            proton("Proton - Experimental"),
            proton("Proton 5.13"),
        ];
        assert_eq!(
            best_proton(candidates),
            Some(proton("Proton - Experimental"))
        );
    }

    #[test]
    fn a_non_numeric_edition_only_wins_when_nothing_else_is_installed() {
        let candidates = vec![proton("Proton Hotfix")];
        assert_eq!(best_proton(candidates), Some(proton("Proton Hotfix")));

        let candidates = vec![proton("Proton Hotfix"), proton("Proton 5.0")];
        assert_eq!(best_proton(candidates), Some(proton("Proton 5.0")));
    }

    #[test]
    fn sniper_beats_soldier_beats_scout() {
        let candidates = vec![
            runtime("SteamLinuxRuntime"),
            runtime("SteamLinuxRuntime_soldier"),
            runtime("SteamLinuxRuntime_sniper"),
        ];
        assert_eq!(
            best_runtime(candidates),
            Some(runtime("SteamLinuxRuntime_sniper"))
        );
    }

    #[test]
    fn version_parsing_handles_bare_and_suffixed_names() {
        assert_eq!(parse_proton_version("Proton 9.0 (Beta)"), (9, 0));
        assert_eq!(parse_proton_version("Proton 5.13"), (5, 13));
        assert_eq!(parse_proton_version("Proton Hotfix"), (0, 0));
    }

    #[test]
    fn the_scan_picks_the_best_proton_and_runtime_on_disk() {
        let fixture = Fixture::new(Some("Proton 5.13"), Some("SteamLinuxRuntime_soldier"));
        let common = fixture.root().join("steamapps").join("common");
        std::fs::create_dir_all(common.join("Proton 9.0")).unwrap();
        std::fs::write(common.join("Proton 9.0").join("proton"), b"").unwrap();
        std::fs::create_dir_all(common.join("SteamLinuxRuntime")).unwrap();
        std::fs::write(common.join("SteamLinuxRuntime").join("run"), b"").unwrap();

        assert_eq!(pick_proton(&common), Some(proton_at(&common, "Proton 9.0")));
        assert_eq!(
            pick_runtime(&common),
            Some(runtime_at(&common, "SteamLinuxRuntime_soldier"))
        );
    }

    fn proton_at(common: &Path, dir: &str) -> PathBuf {
        common.join(dir).join("proton")
    }

    fn runtime_at(common: &Path, dir: &str) -> PathBuf {
        common.join(dir).join("run")
    }

    fn expected_launch(root: &Path, game_dir: &Path, game_exe: &Path) -> ProtonLaunch {
        let common = root.join("steamapps").join("common");
        ProtonLaunch {
            program: common.join("SteamLinuxRuntime_sniper").join("run"),
            args: vec![
                OsString::from("--"),
                common
                    .join("Proton - Experimental")
                    .join("proton")
                    .into_os_string(),
                OsString::from("run"),
                game_exe.as_os_str().to_os_string(),
            ],
            cwd: game_dir.to_path_buf(),
            env: vec![
                (
                    "STEAM_COMPAT_DATA_PATH".to_string(),
                    root.join("steamapps")
                        .join("compatdata")
                        .join("221100")
                        .into_os_string(),
                ),
                (
                    "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
                    root.as_os_str().to_os_string(),
                ),
                ("SteamAppId".to_string(), OsString::from("221100")),
                ("SteamGameId".to_string(), OsString::from("221100")),
            ],
            env_remove: vec![
                "LD_LIBRARY_PATH".to_string(),
                "APPIMAGE".to_string(),
                "APPDIR".to_string(),
                "PYTHONHOME".to_string(),
                "PYTHONPATH".to_string(),
            ],
        }
    }

    #[test]
    fn the_x64_game_exe_gets_no_battleye_prefix() {
        let fixture = basic_fixture();
        let game_dir = fixture.game_dir();
        let game_exe = game_dir.join("DayZ_x64.exe");
        std::fs::write(&game_exe, b"").unwrap();

        assert_eq!(
            proton_launch(fixture.root(), &game_exe, &[]).unwrap(),
            expected_launch(fixture.root(), &game_dir, &game_exe)
        );
    }

    #[test]
    fn the_battleye_stub_gets_its_prefix_and_the_game_exe_when_beside_it() {
        let fixture = basic_fixture();
        let game_dir = fixture.game_dir();
        std::fs::write(game_dir.join("DayZ_x64.exe"), b"").unwrap();
        let game_exe = game_dir.join("DayZ_BE.exe");
        std::fs::write(&game_exe, b"").unwrap();

        let launch = proton_launch(fixture.root(), &game_exe, &[]).unwrap();
        let mut expected = expected_launch(fixture.root(), &game_dir, &game_exe);
        expected.args.push(OsString::from("0"));
        expected.args.push(OsString::from("1"));
        expected.args.push(OsString::from("0"));
        expected.args.push(OsString::from("-exe"));
        expected.args.push(OsString::from("DayZ_x64.exe"));
        assert_eq!(launch, expected);
    }

    #[test]
    fn the_battleye_stub_without_the_game_exe_gets_only_the_plain_prefix() {
        let fixture = basic_fixture();
        let game_dir = fixture.game_dir();
        let game_exe = game_dir.join("DayZ_BE.exe");
        std::fs::write(&game_exe, b"").unwrap();

        let mut expected = expected_launch(fixture.root(), &game_dir, &game_exe);
        expected.args.push(OsString::from("0"));
        expected.args.push(OsString::from("1"));
        expected.args.push(OsString::from("0"));
        assert_eq!(
            proton_launch(fixture.root(), &game_exe, &[]).unwrap(),
            expected
        );
    }

    #[test]
    fn battleye_prefix_matching_ignores_case() {
        let fixture = basic_fixture();
        let game_dir = fixture.game_dir();
        let game_exe = game_dir.join("dayz_be.EXE");
        std::fs::write(&game_exe, b"").unwrap();

        let launch = proton_launch(fixture.root(), &game_exe, &[]).unwrap();
        assert!(launch.args.contains(&OsString::from("0")));
    }

    #[test]
    fn game_args_are_passed_verbatim_and_last() {
        let fixture = basic_fixture();
        let game_dir = fixture.game_dir();
        let game_exe = game_dir.join("DayZ_x64.exe");
        std::fs::write(&game_exe, b"").unwrap();

        let game_args = vec![
            "-connect=127.0.0.1".to_string(),
            "-port=2302".to_string(),
            "-mod=@FieldKit".to_string(),
        ];
        let launch = proton_launch(fixture.root(), &game_exe, &game_args).unwrap();
        assert_eq!(
            launch.args.last(),
            Some(&OsString::from("-mod=@FieldKit")),
            "-mod= must stay the final element"
        );
        for arg in &game_args {
            assert!(
                launch.args.contains(&OsString::from(arg)),
                "{arg} was dropped"
            );
        }
    }

    #[test]
    fn a_missing_proton_is_its_own_error() {
        let fixture = Fixture::new(None, Some("SteamLinuxRuntime_sniper"));
        let game_exe = fixture.game_dir().join("DayZ_x64.exe");
        assert_eq!(
            proton_launch(fixture.root(), &game_exe, &[]).unwrap_err(),
            ProtonLaunchError::NoProton
        );
    }

    #[test]
    fn a_missing_runtime_is_its_own_error() {
        let fixture = Fixture::new(Some("Proton 9.0"), None);
        let game_exe = fixture.game_dir().join("DayZ_x64.exe");
        assert_eq!(
            proton_launch(fixture.root(), &game_exe, &[]).unwrap_err(),
            ProtonLaunchError::NoRuntime
        );
    }

    #[test]
    fn find_dayz_exe_prefers_the_battleye_stub_then_the_game_exe() {
        for (present, expected) in [
            (&["DayZ_x64.exe", "DayZ_BE.exe"][..], Some("DayZ_BE.exe")),
            (&["DayZ_x64.exe"][..], Some("DayZ_x64.exe")),
            (&[][..], None),
        ] {
            let dir = std::env::temp_dir().join(format!("mf-dayz-{}", unique()));
            std::fs::create_dir_all(&dir).unwrap();
            for name in present {
                std::fs::write(dir.join(name), b"").unwrap();
            }
            assert_eq!(
                find_dayz_exe(&dir),
                expected.map(|name| dir.join(name)),
                "{present:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
