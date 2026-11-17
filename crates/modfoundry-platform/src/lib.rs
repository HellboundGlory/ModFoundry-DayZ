//! Platform abstraction: what is host-OS specific lives here so the
//! rest of ModFoundry stays portable and testable.
//!
//! Scope today: host platform detection, best-effort Steam library discovery (the
//! DayZ game/tools/workshop roots are inputs to the VFS), the Wine-prefix path
//! bridge used when launching Windows-only engine binaries from Linux,
//! launch staging (links, files and folders a launch puts into a game or server install), and the
//! Steam helper wire types.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod archive;
pub mod cuda;
pub mod process;
pub mod program;
pub mod proton;
pub mod stage;
pub mod steam;

pub use process::{
    classify_dayz_process, diag_server_running, end_with_parent, stop_dayz_client,
    stop_diag_client, stop_diag_server, stop_process_tree, DayzRole,
};
#[cfg(not(windows))]
pub use process::{dayz_client_pid, dayz_client_running};
pub use proton::{
    find_dayz_exe, pick_proton, pick_runtime, proton_launch, ProtonLaunch, ProtonLaunchError,
};

/// Steam app id for the DayZ client (the dedicated server is a separate app).
pub const DAYZ_APP_ID: u64 = 221100;

/// Host OS ModFoundry is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Linux,
    Windows,
    MacOs,
}

impl HostPlatform {
    pub fn detect() -> Self {
        match std::env::consts::OS {
            "linux" => HostPlatform::Linux,
            "windows" => HostPlatform::Windows,
            "macos" => HostPlatform::MacOs,
            _ => HostPlatform::Linux,
        }
    }

    /// Lowercase wire/display name, used by `mf doctor` and agent-facing
    /// reports so the host is spelled the same everywhere.
    pub fn name(self) -> &'static str {
        match self {
            HostPlatform::Linux => "linux",
            HostPlatform::Windows => "windows",
            HostPlatform::MacOs => "macos",
        }
    }
}

/// A discovered Steam installation: its root and the `steamapps` directory.
#[derive(Debug, Clone)]
pub struct SteamInstall {
    pub root: PathBuf,
    pub steamapps: PathBuf,
}

static HOST_DISCOVERY: AtomicBool = AtomicBool::new(true);

/// Turns host Steam discovery on or off for the whole process; a hermetic test
/// shell switches it off so no job reads the machine's real DayZ install or starts
/// the Steam helper.
pub fn set_host_discovery(on: bool) {
    HOST_DISCOVERY.store(on, Ordering::Relaxed);
}

/// Whether this process may look at the host's Steam: off in a hermetic test shell.
pub(crate) fn host_discovery_on() -> bool {
    HOST_DISCOVERY.load(Ordering::Relaxed)
}

/// Best-effort Steam install discovery for the current host. Returns all found
/// library roots (`<root>/steamapps`), starting with the primary library.
///
/// Linux: `~/.local/share/Steam` + `libraryfolders.vdf` extra libraries.
/// Windows: registry-free scan of `C:\Program Files (x86)\Steam` + steamapps.
/// (Extra libraries on Windows come from libraryfolders.vdf too.)
pub fn discover_steam_installations() -> Vec<SteamInstall> {
    let mut out = Vec::new();
    if !HOST_DISCOVERY.load(Ordering::Relaxed) {
        return out;
    }
    let push = |root: PathBuf, out: &mut Vec<SteamInstall>| {
        let steamapps = root.join("steamapps");
        if steamapps.is_dir() && !out.iter().any(|i| i.steamapps == steamapps) {
            out.push(SteamInstall {
                root: root.clone(),
                steamapps,
            });
        }
    };

    match HostPlatform::detect() {
        HostPlatform::Linux => {
            if let Ok(home) = std::env::var("HOME") {
                push(PathBuf::from(home).join(".local/share/Steam"), &mut out);
            }
        }
        HostPlatform::Windows => {
            for base in ["C:\\Program Files (x86)\\Steam", "C:\\Program Files\\Steam"] {
                push(PathBuf::from(base), &mut out);
            }
        }
        HostPlatform::MacOs => {
            if let Ok(home) = std::env::var("HOME") {
                push(
                    PathBuf::from(home).join("Library/Application Support/Steam"),
                    &mut out,
                );
            }
        }
    }

    // Extend with extra library folders declared in libraryfolders.vdf, if any:
    // each listed path is a library ROOT whose `steamapps` dir holds content.
    let mut extra_roots: Vec<PathBuf> = Vec::new();
    for inst in out.iter() {
        if let Some(mut extra) = extra_library_roots(&inst.steamapps.join("libraryfolders.vdf")) {
            extra_roots.append(&mut extra);
        }
    }
    for root in extra_roots {
        push(root, &mut out);
    }

    out
}

/// Minimal `libraryfolders.vdf` parser: returns the `path` values of the numbered
/// entries (secondary libraries), ignoring everything else. Format:
/// `"1" { "path" "X:\SteamLibrary" ... }`.
fn extra_library_roots(vdf: &Path) -> Option<Vec<PathBuf>> {
    let text = std::fs::read_to_string(vdf).ok()?;
    let mut roots = Vec::new();
    let mut in_entry = false;
    for line in text.lines() {
        let t = line.trim();
        // Entry headers appear either as `"1" {` (single line) or as `"1"`
        // followed by a lone `{` (real Steam layout).
        if !in_entry {
            let header = t
                .strip_prefix('"')
                .and_then(|r| r.split_once('"'))
                .map(|(_, rest)| rest);
            if let Some(rest) = header {
                if rest.contains('{') {
                    in_entry = true;
                }
            } else if t == "{" {
                in_entry = true;
            }
            if !in_entry {
                continue;
            }
        }
        if t == "}" {
            in_entry = false;
        } else if t.contains("\"path\"") {
            // VDF is quote-delimited: fields between quotes, whitespace/keyspace
            // trimmed. Expect `"path" <ws> "<value>"`.
            let fields: Vec<&str> = t
                .split('"')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if fields.first() == Some(&"path") {
                if let Some(v) = fields.get(1) {
                    roots.push(PathBuf::from(v));
                }
            }
        }
    }
    (!roots.is_empty()).then_some(roots)
}

/// Locate `steamapps/common/DayZ` (the game) and `DayZ Tools` across discovered
/// libraries.
pub fn find_dayz_installs() -> (Option<PathBuf>, Option<PathBuf>) {
    let mut game = None;
    let mut tools = None;
    for inst in discover_steam_installations() {
        let common = inst.steamapps.join("common");
        let g = common.join("DayZ");
        let t = common.join("DayZ Tools");
        if game.is_none() && g.join("DayZ_x64.exe").exists() {
            game = Some(g);
        }
        if tools.is_none() && t.join("Bin").is_dir() {
            tools = Some(t);
        }
        if game.is_some() && tools.is_some() {
            break;
        }
    }
    (game, tools)
}

/// Locate the native DayZ dedicated-server install
/// (`steamapps/common/DayZServer`) across discovered libraries. The server is
/// a separate Steam app (223350) from the client (221100).
pub fn find_dayz_server() -> Option<PathBuf> {
    for inst in discover_steam_installations() {
        let d = inst.steamapps.join("common").join("DayZServer");
        let exe = if cfg!(windows) {
            d.join("DayZServer_x64.exe")
        } else {
            d.join("DayZServer")
        };
        if exe.is_file() {
            return Some(d);
        }
    }
    None
}

/// Workshop content root for a given Steam app id (e.g. DayZ = 221100).
pub fn workshop_content_root(steam: &SteamInstall, app_id: u64) -> PathBuf {
    steam
        .steamapps
        .join("workshop/content")
        .join(app_id.to_string())
}

/// `<steam_root>/steamapps/workshop/content/221100`, the DayZ workshop items dir.
pub fn dayz_workshop_content_dir(steam_root: &Path) -> PathBuf {
    steam_root
        .join("steamapps/workshop/content")
        .join(DAYZ_APP_ID.to_string())
}

/// The installed workshop item dir `steamapps/workshop/content/221100/<id>`.
/// `None` when `workshop_id` is not all ASCII digits.
pub fn workshop_mod_dir(steam_root: &Path, workshop_id: &str) -> Option<PathBuf> {
    if workshop_id.is_empty() || !workshop_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(dayz_workshop_content_dir(steam_root).join(workshop_id))
}

/// `steamapps/compatdata/<app id>`, the folder Proton keeps an app's prefix in.
pub(crate) fn compatdata_dir(steamapps: &Path, app_id: u64) -> PathBuf {
    steamapps.join("compatdata").join(app_id.to_string())
}

/// The Proton/Wine prefix for a Steam app: `steamapps/compatdata/<id>/pfx`.
/// Returns `None` when the app has never run under Proton here.
pub fn proton_prefix(steam: &SteamInstall, app_id: u64) -> Option<PathBuf> {
    let pfx = compatdata_dir(&steam.steamapps, app_id).join("pfx");
    pfx.is_dir().then_some(pfx)
}

/// Locate the DayZ client's log dir inside a Proton prefix, where the engine
/// writes `DayZ_x64_<ts>.RPT` + `script_<ts>.log` (verified live).
/// On Windows this is `%LOCALAPPDATA%\DayZ`; under Proton it is
/// `<pfx>/drive_c/users/<user>/AppData/Local/DayZ`: the first user (common
/// names, then by name) that has it, else the only user, else `steamuser`.
/// `None` only when `pfx` is not a directory.
pub fn proton_client_log_dir(pfx: &Path) -> Option<PathBuf> {
    if !pfx.is_dir() {
        return None;
    }
    let users = pfx.join("drive_c/users");
    let dayz = |user: &str| users.join(user).join("AppData/Local/DayZ");
    for u in ["steamuser", "user", "Public"] {
        let d = dayz(u);
        if d.is_dir() {
            return Some(d);
        }
    }
    let mut user_dirs: Vec<PathBuf> = std::fs::read_dir(&users)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    user_dirs.sort();
    if let Some(d) = user_dirs
        .iter()
        .map(|user| user.join("AppData/Local/DayZ"))
        .find(|d| d.is_dir())
    {
        return Some(d);
    }
    let named: Vec<&PathBuf> = user_dirs
        .iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|name| !name.eq_ignore_ascii_case("Public"))
        })
        .collect();
    if let [only] = named[..] {
        return Some(only.join("AppData/Local/DayZ"));
    }
    Some(dayz("steamuser"))
}

/// Wine-prefix bridge: represent a host path the way a Windows process inside a
/// Wine prefix would address it.
///
/// Wine maps host drives to letters via `<prefix>/dosdevices/`. Common conventions:
/// `Z:` → `/` (host root), `C:` → the prefix itself. We emit the `Z:\…` form for a
/// prefix that keeps the default mapping; callers with custom dosdevices must
/// override. Drive letters are case-insensitive in Windows semantics, but the
/// engine's path handling is its own concern — this helper only produces paths.
pub fn host_path_in_wine(host: &Path) -> String {
    let s = host.to_string_lossy().replace('/', "\\");
    format!("Z:\\{}", s.trim_start_matches('\\'))
}

/// The program and arguments that show `path` in the host file manager.
fn reveal_command(platform: HostPlatform, path: &Path) -> (String, Vec<String>) {
    let target = path.display().to_string();
    match platform {
        HostPlatform::Linux => ("xdg-open".to_owned(), vec![target]),
        HostPlatform::Windows => ("explorer".to_owned(), vec![target]),
        HostPlatform::MacOs => ("open".to_owned(), vec![target]),
    }
}

/// Show `path` in the host file manager. The process is spawned, never waited
/// on: the file manager owns it from there.
pub fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    let (program, args) = reveal_command(HostPlatform::detect(), path);
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// The host-wide ModFoundry state directory — where the application-level
/// database lives.
///
/// `MODFOUNDRY_DATA_DIR` overrides it; otherwise the host convention
/// (`%LOCALAPPDATA%\ModFoundry` on Windows, `$XDG_DATA_HOME/modfoundry`,
/// falling back to `~/.local/share/modfoundry`). `None` when the host exposes
/// no such location, so the caller — not this function — decides whether it can
/// run without durable state.
pub fn app_data_dir() -> Option<PathBuf> {
    data_dir(&|key| std::env::var_os(key).map(PathBuf::from))
}

fn data_dir(from_env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let from_env = |key: &str| from_env(key).filter(|value| !value.as_os_str().is_empty());
    if let Some(dir) = from_env("MODFOUNDRY_DATA_DIR") {
        return Some(dir);
    }
    #[cfg(windows)]
    {
        from_env("LOCALAPPDATA").map(|base| base.join("ModFoundry"))
    }
    #[cfg(not(windows))]
    {
        let base = from_env("XDG_DATA_HOME")
            .or_else(|| from_env("HOME").map(|home| home.join(".local/share")))?;
        Some(base.join("modfoundry"))
    }
}

/// The host cache base folder (`%LOCALAPPDATA%` on Windows, else `$XDG_CACHE_HOME`,
/// else `~/.cache`). Callers add their own `modfoundry` folder. `None` when the host
/// exposes no such location.
pub fn app_cache_dir() -> Option<PathBuf> {
    cache_dir(&|key| std::env::var_os(key).map(PathBuf::from))
}

fn cache_dir(from_env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let from_env = |key: &str| from_env(key).filter(|value| !value.as_os_str().is_empty());
    #[cfg(windows)]
    {
        from_env("LOCALAPPDATA")
    }
    #[cfg(not(windows))]
    {
        from_env("XDG_CACHE_HOME").or_else(|| from_env("HOME").map(|home| home.join(".cache")))
    }
}

/// The `.agents` folder in the user's home, which agent tools share for skills and MCP entries.
pub fn agents_dir() -> Option<PathBuf> {
    agents_dir_from(&|key| std::env::var_os(key).map(PathBuf::from))
}

fn agents_dir_from(from_env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    from_env(home)
        .filter(|value| !value.as_os_str().is_empty())
        .map(|home| home.join(".agents"))
}

/// Restrict `path` to its owner: mode `0o600` on Unix. Other systems keep the
/// folder's access rules, so this is a no-op there.
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Create a new file at `path` holding `bytes`, readable by its owner only: mode `0o600` on Unix.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)
    }
}

/// Create one new folder at `path`, reachable by its owner only: mode `0o700` on Unix.
/// Fails when `path` already exists; parent folders are not created.
pub fn create_dir_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

/// Held by every test that writes an executable or starts a process: a fork from a parallel test
/// inherits an executable open for writing, and the exec of it then fails with "Text file busy".
#[cfg(test)]
pub(crate) fn spawn_lock() -> std::sync::MutexGuard<'static, ()> {
    static SPAWNING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `data_dir` environment lookup backed by a fixed table, so the
    /// resolution rules are tested without mutating the process environment.
    fn env_lookup(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<PathBuf> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn wine_path_roundtrip() {
        let host = Path::new("/home/player/DayZ/mods/MyMod/source");
        let win = host_path_in_wine(host);
        assert_eq!(win, r"Z:\home\player\DayZ\mods\MyMod\source");
    }

    #[test]
    fn workshop_mod_dir_joins_a_numeric_id() {
        let root = Path::new("/steam");
        assert_eq!(
            workshop_mod_dir(root, "123456789"),
            Some(root.join("steamapps/workshop/content/221100/123456789"))
        );
    }

    #[test]
    fn workshop_mod_dir_rejects_non_numeric_ids() {
        let root = Path::new("/steam");
        for id in ["", "abc", "12a", "a12", " 12", "-1", "12/3"] {
            assert_eq!(workshop_mod_dir(root, id), None, "{id:?}");
        }
    }

    #[test]
    fn proton_prefix_and_client_log_dir_resolve_on_disk() {
        let lib = std::env::temp_dir().join(format!("mf-proton-{}", std::process::id()));
        let steamapps = lib.join("steamapps");
        let pfx = steamapps.join("compatdata/221100/pfx");
        let log_dir = pfx.join("drive_c/users/steamuser/AppData/Local/DayZ");
        std::fs::create_dir_all(&log_dir).unwrap();
        let inst = SteamInstall {
            root: lib.clone(),
            steamapps: steamapps.clone(),
        };
        assert_eq!(proton_prefix(&inst, 221100), Some(pfx.clone()));
        assert_eq!(proton_client_log_dir(&pfx), Some(log_dir.clone()));
        // An app with no prefix -> None.
        assert_eq!(proton_prefix(&inst, 999999), None);
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn proton_client_log_dir_assumes_steamuser_before_the_game_has_run() {
        let lib = std::env::temp_dir().join(format!("mf-proton3-{}", std::process::id()));
        let pfx = lib.join("pfx");
        assert_eq!(proton_client_log_dir(&pfx), None);
        std::fs::create_dir_all(pfx.join("drive_c")).unwrap();
        assert_eq!(
            proton_client_log_dir(&pfx),
            Some(pfx.join("drive_c/users/steamuser/AppData/Local/DayZ"))
        );
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn proton_client_log_dir_falls_back_to_any_user() {
        let lib = std::env::temp_dir().join(format!("mf-proton2-{}", std::process::id()));
        let pfx = lib.join("pfx");
        let log_dir = pfx.join("drive_c/users/customuser/AppData/Local/DayZ");
        std::fs::create_dir_all(&log_dir).unwrap();
        // No steamuser/user/Public: falls back to scanning the users dir.
        assert_eq!(proton_client_log_dir(&pfx), Some(log_dir.clone()));
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn proton_client_log_dir_uses_the_only_user_before_the_game_has_run() {
        let lib = std::env::temp_dir().join(format!("mf-proton4-{}", std::process::id()));
        let pfx = lib.join("pfx");
        std::fs::create_dir_all(pfx.join("drive_c/users/player")).unwrap();
        std::fs::create_dir_all(pfx.join("drive_c/users/Public")).unwrap();
        assert_eq!(
            proton_client_log_dir(&pfx),
            Some(pfx.join("drive_c/users/player/AppData/Local/DayZ"))
        );
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn proton_client_log_dir_picks_the_first_user_by_name_when_several_have_dayz() {
        let lib = std::env::temp_dir().join(format!("mf-proton5-{}", std::process::id()));
        let pfx = lib.join("pfx");
        std::fs::create_dir_all(pfx.join("drive_c/users/zed/AppData/Local/DayZ")).unwrap();
        std::fs::create_dir_all(pfx.join("drive_c/users/amy/AppData/Local/DayZ")).unwrap();
        assert_eq!(
            proton_client_log_dir(&pfx),
            Some(pfx.join("drive_c/users/amy/AppData/Local/DayZ"))
        );
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn proton_client_log_dir_keeps_steamuser_when_the_user_is_unclear() {
        let lib = std::env::temp_dir().join(format!("mf-proton6-{}", std::process::id()));
        let pfx = lib.join("pfx");
        for user in ["player", "other", "Public"] {
            std::fs::create_dir_all(pfx.join("drive_c/users").join(user)).unwrap();
        }
        let steamuser = Some(pfx.join("drive_c/users/steamuser/AppData/Local/DayZ"));
        assert_eq!(proton_client_log_dir(&pfx), steamuser);
        std::fs::create_dir_all(pfx.join("drive_c/users/steamuser")).unwrap();
        assert_eq!(proton_client_log_dir(&pfx), steamuser);
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn app_data_dir_prefers_the_override_over_the_host_convention() {
        assert_eq!(
            data_dir(&env_lookup(vec![
                ("MODFOUNDRY_DATA_DIR", "/custom/state"),
                ("XDG_DATA_HOME", "/xdg"),
            ])),
            Some(PathBuf::from("/custom/state"))
        );
    }

    #[test]
    fn app_data_dir_falls_back_to_the_host_convention() {
        #[cfg(not(windows))]
        {
            assert_eq!(
                data_dir(&env_lookup(vec![("XDG_DATA_HOME", "/xdg")])),
                Some(PathBuf::from("/xdg/modfoundry"))
            );
            assert_eq!(
                data_dir(&env_lookup(vec![("HOME", "/home/u")])),
                Some(PathBuf::from("/home/u/.local/share/modfoundry"))
            );
        }
        #[cfg(windows)]
        assert_eq!(
            data_dir(&env_lookup(vec![(
                "LOCALAPPDATA",
                r"C:\Users\u\AppData\Local"
            )])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\ModFoundry"))
        );
        // No home, no state dir: the caller decides what to do.
        assert_eq!(data_dir(&env_lookup(vec![])), None);
    }

    #[test]
    fn the_shared_agents_folder_sits_in_the_home_folder() {
        #[cfg(not(windows))]
        let home = ("HOME", "/home/u");
        #[cfg(windows)]
        let home = ("USERPROFILE", r"C:\Users\u");
        assert_eq!(
            agents_dir_from(&env_lookup(vec![home])),
            Some(PathBuf::from(home.1).join(".agents"))
        );
        assert_eq!(agents_dir_from(&env_lookup(vec![(home.0, "")])), None);
        assert_eq!(agents_dir_from(&env_lookup(vec![])), None);
    }

    #[test]
    fn app_cache_dir_prefers_xdg_cache_home_then_home_cache() {
        #[cfg(not(windows))]
        {
            assert_eq!(
                cache_dir(&env_lookup(vec![
                    ("XDG_CACHE_HOME", "/xdg-cache"),
                    ("HOME", "/home/u"),
                ])),
                Some(PathBuf::from("/xdg-cache"))
            );
            assert_eq!(
                cache_dir(&env_lookup(vec![("HOME", "/home/u")])),
                Some(PathBuf::from("/home/u/.cache"))
            );
            assert_eq!(cache_dir(&env_lookup(vec![])), None);
        }
        #[cfg(windows)]
        assert_eq!(
            cache_dir(&env_lookup(vec![(
                "LOCALAPPDATA",
                r"C:\Users\u\AppData\Local"
            )])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local"))
        );
    }

    #[test]
    fn empty_variables_are_ignored() {
        #[cfg(not(windows))]
        assert_eq!(
            data_dir(&env_lookup(vec![
                ("MODFOUNDRY_DATA_DIR", ""),
                ("XDG_DATA_HOME", "/xdg"),
            ])),
            Some(PathBuf::from("/xdg/modfoundry"))
        );
        #[cfg(windows)]
        assert_eq!(
            data_dir(&env_lookup(vec![
                ("MODFOUNDRY_DATA_DIR", ""),
                ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ])),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\ModFoundry"))
        );
    }

    #[test]
    fn empty_cache_variables_are_ignored() {
        #[cfg(not(windows))]
        assert_eq!(
            cache_dir(&env_lookup(vec![
                ("XDG_CACHE_HOME", ""),
                ("HOME", "/home/u")
            ])),
            Some(PathBuf::from("/home/u/.cache"))
        );
        #[cfg(windows)]
        assert_eq!(cache_dir(&env_lookup(vec![("LOCALAPPDATA", "")])), None);
    }

    #[test]
    fn the_reveal_command_names_the_host_file_manager() {
        let path = Path::new("/home/u/keys");
        assert_eq!(
            reveal_command(HostPlatform::Linux, path),
            ("xdg-open".to_owned(), vec!["/home/u/keys".to_owned()])
        );
        assert_eq!(
            reveal_command(HostPlatform::MacOs, path),
            ("open".to_owned(), vec!["/home/u/keys".to_owned()])
        );
        assert_eq!(
            reveal_command(HostPlatform::Windows, Path::new(r"C:\keys")),
            ("explorer".to_owned(), vec![r"C:\keys".to_owned()])
        );
    }

    #[test]
    fn libraryfolders_vdf_parses_paths() {
        let dir = std::env::temp_dir().join(format!("mf-platform-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vdf = dir.join("libraryfolders.vdf");
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n}\n";
        std::fs::write(&vdf, text).unwrap();
        let roots = extra_library_roots(&vdf).unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[1], PathBuf::from("/mnt/games/SteamLibrary"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_private_folder_is_owner_only_and_not_replaced() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("mf-owner-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let folder = base.join("staging");

        create_dir_owner_only(&folder).unwrap();
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        assert!(create_dir_owner_only(&folder).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
