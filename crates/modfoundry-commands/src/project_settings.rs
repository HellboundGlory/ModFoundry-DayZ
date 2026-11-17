//! The app's saved settings, read outside the GUI so the CLI and agents use the
//! values the user set in the app.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diag_stage::{TestModArg, TestModRole};

/// The stored map of the app-wide setting overrides.
pub const PREFS_DEFAULTS_KEY: &str = "prefs.defaults";
/// The stored map of the project's setting overrides.
pub const PREFS_PROJECT_KEY: &str = "prefs.project";
/// The stored project record, which holds the launch settings.
pub const PROJECT_KEY: &str = "gui.project";
pub(crate) const MODS_KEY: &str = "launch.mods.test";

/// How many hex characters of the path hash identify a project.
const ID_LEN: usize = 16;

/// The id a project's settings are stored under, from its canonical path.
pub fn project_id(source_root: &Path) -> String {
    let canonical = source_root
        .canonicalize()
        .unwrap_or_else(|_| source_root.to_path_buf());
    modfoundry_vfs::sha256_hex(canonical.to_string_lossy().as_bytes())[..ID_LEN].to_owned()
}

/// Which signing key the build uses; `None` builds unsigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyChoice {
    // Written as the app's older name, so a build that still reads only that keeps working.
    #[serde(rename = "Automatic", alias = "None")]
    None,
    Named {
        authority: String,
        private_key: PathBuf,
    },
}

/// Where a test mod is loaded from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModSource {
    Workshop { id: String },
    Folder { path: PathBuf },
}

/// Which launch sides load a test mod.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModRole {
    Both,
    ServerOnly,
}

/// What a test mod is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModUse {
    #[default]
    TestOnly,
    Dependency,
}

/// One entry of the test-mod list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestMod {
    pub id: String,
    pub name: String,
    pub source: ModSource,
    pub role: ModRole,
    #[serde(default)]
    pub use_as: ModUse,
    pub enabled: bool,
}

/// The settings a command reads from the app's saved state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSettings {
    pub player_name: String,
    pub windowed: bool,
    pub window: (u32, u32),
    pub port: u16,
    pub keep_running: bool,
    pub run_secs: u64,
    pub test_mods: Vec<TestMod>,
    pub prefix: Option<String>,
    pub binarize: bool,
    pub strip_comments: bool,
    pub profile: String,
    pub key: KeyChoice,
    /// Whether the saved settings name a signing key, including an explicit none.
    pub key_saved: bool,
    pub client: bool,
    pub server_dir: Option<PathBuf>,
}

impl ProjectSettings {
    /// The settings in force for the stored maps and project record.
    pub fn from_values(
        defaults: Option<&Value>,
        project: Option<&Value>,
        gui_project: Option<&Value>,
    ) -> Self {
        let defaults = defaults.filter(|map| readable(map));
        let legacy;
        let project = match project.filter(|map| readable(map)) {
            Some(map) => Some(map),
            None => {
                legacy = gui_project.map(legacy_overrides);
                legacy.as_ref()
            }
        };
        let maps = [project, defaults];
        let key: Option<KeyChoice> = read("build.signing.key", maps, "Key");
        Self {
            player_name: read("launch.player.name", maps, "Text")
                .unwrap_or_else(|| "Survivor".to_owned()),
            windowed: read("launch.window.windowed", maps, "Bool").unwrap_or(true),
            window: read::<Size>("launch.window.size", maps, "Size")
                .map_or((1280, 720), |size| (size.width, size.height)),
            port: read("launch.window.port", maps, "Number").unwrap_or(2302),
            keep_running: read("launch.window.keep_running", maps, "Bool").unwrap_or(true),
            run_secs: read("launch.window.run_secs", maps, "Number").unwrap_or(600),
            test_mods: merge_test_mods(
                defaults.and_then(stored_mods).unwrap_or_default(),
                project.and_then(stored_mods).unwrap_or_default(),
            ),
            prefix: read::<String>("build.packing.prefix", maps, "Text")
                .filter(|prefix| !prefix.is_empty()),
            binarize: read("build.packing.binarize", maps, "Bool").unwrap_or(false),
            strip_comments: read("build.packing.strip_comments", maps, "Bool").unwrap_or(false),
            profile: read("build.packing.profile", maps, "Text")
                .unwrap_or_else(|| "default".to_owned()),
            key_saved: key.is_some(),
            key: key.unwrap_or(KeyChoice::None),
            client: launch_field::<String>(gui_project, "target").as_deref()
                == Some("ServerAndClient"),
            server_dir: launch_field(gui_project, "server_dir"),
        }
    }

    /// The `build.run` arguments the saved packing and signing settings imply.
    pub fn build_args(&self) -> serde_json::Map<String, Value> {
        let mut args = serde_json::Map::new();
        args.insert("profile".into(), self.profile.clone().into());
        if let Some(prefix) = &self.prefix {
            args.insert("prefix".into(), prefix.clone().into());
        }
        args.insert("binarize_config".into(), self.binarize.into());
        args.insert("strip_comments".into(), self.strip_comments.into());
        if let KeyChoice::Named {
            authority,
            private_key,
        } = &self.key
        {
            args.insert("authority".into(), authority.clone().into());
            let keys_dir = private_key.parent().unwrap_or(Path::new(""));
            args.insert("keys_dir".into(), serde_json::json!(keys_dir));
        }
        args
    }

    /// The `diag.run_diag` arguments the saved launch settings imply.
    pub fn diag_launch_args(
        &self,
        steam_root: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> LaunchArgs {
        let mut args = serde_json::Map::new();
        args.insert("connect_name".into(), self.player_name.clone().into());
        args.insert("windowed".into(), self.windowed.into());
        args.insert("port".into(), self.port.into());
        args.insert("client".into(), self.client.into());
        if self.windowed {
            args.insert("window".into(), self.window_arg());
        }
        if !self.keep_running {
            args.insert("timeout_secs".into(), self.run_secs.into());
        }
        if let Some(server_dir) = &self.server_dir {
            args.insert("server_dir".into(), serde_json::json!(server_dir));
        }
        let (test_mods, missing) = test_mod_args(
            self.test_mods.iter().filter(|entry| entry.enabled),
            steam_root,
            &exists,
        );
        args.insert("test_mods".into(), test_mods);
        LaunchArgs { args, missing }
    }

    /// The `diag.run` arguments the saved server settings imply.
    pub fn server_run_args(
        &self,
        steam_root: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> LaunchArgs {
        let mut args = serde_json::Map::new();
        args.insert("port".into(), self.port.into());
        args.insert("timeout_secs".into(), self.run_secs.into());
        if let Some(server_dir) = &self.server_dir {
            args.insert("server_dir".into(), serde_json::json!(server_dir));
        }
        let (test_mods, missing) = test_mod_args(
            self.test_mods.iter().filter(|entry| entry.enabled),
            steam_root,
            &exists,
        );
        args.insert("test_mods".into(), test_mods);
        LaunchArgs { args, missing }
    }

    /// The `diag.run_client` arguments the saved client settings imply.
    pub fn client_run_args(
        &self,
        steam_root: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> LaunchArgs {
        let mut args = serde_json::Map::new();
        args.insert("profile_name".into(), self.player_name.clone().into());
        args.insert("windowed".into(), self.windowed.into());
        args.insert("window".into(), self.window_arg());
        let (test_mods, missing) = test_mod_args(
            self.test_mods
                .iter()
                .filter(|entry| entry.enabled && entry.role == ModRole::Both),
            steam_root,
            &exists,
        );
        args.insert("test_mods".into(), test_mods);
        LaunchArgs { args, missing }
    }

    /// The enabled test mods that are used as a dependency.
    pub(crate) fn dependency_mods(&self) -> impl Iterator<Item = &TestMod> {
        self.test_mods
            .iter()
            .filter(|entry| entry.enabled && entry.use_as == ModUse::Dependency)
    }

    /// The `script.index` arguments the saved dependency mods imply.
    pub fn script_index_args(
        &self,
        steam_root: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> LaunchArgs {
        let mut dependencies = Vec::new();
        let mut missing = Vec::new();
        for entry in self.dependency_mods() {
            match test_mod_arg(entry, steam_root, &exists) {
                Ok(arg) => dependencies.push(serde_json::json!(arg.path)),
                Err(missing_mod) => missing.push(missing_mod),
            }
        }
        let mut args = serde_json::Map::new();
        args.insert("dependencies".into(), Value::Array(dependencies));
        LaunchArgs { args, missing }
    }

    pub(crate) fn window_arg(&self) -> Value {
        let (width, height) = self.window;
        serde_json::json!({"width": width, "height": height})
    }
}

/// The `test_mods` argument for the mods a launch can load, and the ones whose folder is missing.
fn test_mod_args<'a>(
    mods: impl Iterator<Item = &'a TestMod>,
    steam_root: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> (Value, Vec<MissingMod>) {
    let mut args = Vec::new();
    let mut missing = Vec::new();
    for entry in mods {
        match test_mod_arg(entry, steam_root, &exists) {
            Ok(arg) => {
                args.push(serde_json::to_value(arg).expect("a test mod argument serialises"))
            }
            Err(missing_mod) => missing.push(missing_mod),
        }
    }
    (Value::Array(args), missing)
}

/// The arguments a diag launch takes from the saved settings, and the enabled
/// test mods whose folder could not be found.
#[derive(Debug)]
pub struct LaunchArgs {
    pub args: serde_json::Map<String, Value>,
    pub missing: Vec<MissingMod>,
}

/// An enabled test mod the launch cannot load, and where it was looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingMod {
    pub name: String,
    /// `None` when a Workshop mod has no Steam install to look in.
    pub path: Option<PathBuf>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self::from_values(None, None, None)
    }
}

/// The value of `kind` stored under `id`, from the first map that has one.
fn read<T: DeserializeOwned>(id: &str, maps: [Option<&Value>; 2], kind: &str) -> Option<T> {
    maps.into_iter()
        .flatten()
        .find_map(|map| stored(map, id, kind))
}

fn stored<T: DeserializeOwned>(map: &Value, id: &str, kind: &str) -> Option<T> {
    T::deserialize(map.get(id)?.get(kind)?).ok()
}

fn stored_mods(map: &Value) -> Option<Vec<TestMod>> {
    stored(map, MODS_KEY, "Mods")
}

// A list that is present but malformed is an error: an empty result would let a write drop the user's rows.
pub(crate) fn stored_mods_strict(project: Option<&Value>) -> anyhow::Result<Vec<TestMod>> {
    let Some(project) = project else {
        return Ok(Vec::new());
    };
    let Some(map) = project.as_object() else {
        anyhow::bail!("the project's saved settings are not a map");
    };
    if !readable(project) {
        anyhow::bail!("the saved settings hold an entry this version cannot read");
    }
    let Some(stored) = map.get(MODS_KEY) else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_value(
        stored.get("Mods").cloned().unwrap_or(Value::Null),
    )?)
}

fn launch_field<T: DeserializeOwned>(gui_project: Option<&Value>, key: &str) -> Option<T> {
    T::deserialize(gui_project?.get("launch")?.get(key)?).ok()
}

/// The defaults' mods with the project's enabled flags, then the project's own.
fn merge_test_mods(defaults: Vec<TestMod>, project: Vec<TestMod>) -> Vec<TestMod> {
    let mut merged: Vec<TestMod> = defaults
        .iter()
        .map(|default| {
            let enabled = project
                .iter()
                .find(|entry| entry.id == default.id)
                .map_or(default.enabled, |entry| entry.enabled);
            TestMod {
                enabled,
                ..default.clone()
            }
        })
        .collect();
    merged.extend(
        project
            .iter()
            .filter(|entry| !defaults.iter().any(|default| default.id == entry.id))
            .cloned(),
    );
    merged
}

/// The bare link name of a mod folder: its file name without a leading `@`,
/// since `-mod=` names a link and a link name cannot start with `@`.
pub(crate) fn folder_link_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
        .trim_start_matches('@')
        .to_owned()
}

/// The diag argument for a test mod whose folder is there, or what is missing.
fn test_mod_arg(
    entry: &TestMod,
    steam_root: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Result<TestModArg, MissingMod> {
    let missing = || MissingMod {
        name: entry.name.clone(),
        path: None,
    };
    let (path, link_name) = match &entry.source {
        ModSource::Folder { path } => (path.clone(), folder_link_name(path)),
        ModSource::Workshop { id } => (
            steam_root
                .and_then(|root| modfoundry_platform::workshop_mod_dir(root, id))
                .ok_or_else(missing)?,
            id.clone(),
        ),
    };
    if !exists(&path) {
        return Err(MissingMod {
            path: Some(path),
            ..missing()
        });
    }
    let role = match entry.role {
        ModRole::Both => TestModRole::Both,
        ModRole::ServerOnly => TestModRole::ServerOnly,
    };
    Ok(TestModArg {
        path,
        link_name,
        role,
    })
}

#[derive(Deserialize)]
struct Size {
    width: u32,
    height: u32,
}

/// The shape of one stored setting value, mirroring the app's `Value`.
#[derive(Deserialize)]
#[allow(dead_code)]
enum StoredValue {
    Text(String),
    Bool(bool),
    Number(u64),
    Size { width: u32, height: u32 },
    Path(Option<PathBuf>),
    Key(KeyChoice),
    Mods(Vec<TestMod>),
}

/// Whether the app could load `map` at all: it reads each stored map whole, so
/// one entry it cannot read makes it drop the map.
fn readable(map: &Value) -> bool {
    std::collections::BTreeMap::<String, StoredValue>::deserialize(map).is_ok()
}

/// The run length an old project record holds unless it was changed.
const LEGACY_RUN_SECS: u64 = 90;

/// The overrides the app imports from an old project record that has no saved
/// settings map yet.
fn legacy_overrides(gui_project: &Value) -> Value {
    let mut map = serde_json::Map::new();
    let build = gui_project.get("build");
    if let Some(profile) = build.and_then(|b| b.get("profile")).and_then(Value::as_str) {
        map.insert(
            "build.packing.profile".into(),
            serde_json::json!({"Text": profile}),
        );
    }
    if let Some(authority) = build
        .and_then(|b| b.get("authority"))
        .and_then(Value::as_str)
    {
        let keys_dir = build
            .and_then(|b| b.get("keys_dir"))
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| {
                let root = gui_project.get("source_root")?.as_str()?;
                Some(Path::new(root).join("keys"))
            });
        if let Some(keys_dir) = keys_dir {
            let private_key = keys_dir.join(format!("{authority}.biprivatekey"));
            map.insert(
                "build.signing.key".into(),
                serde_json::json!({"Key": {"Named": {"authority": authority, "private_key": private_key}}}),
            );
        }
    }
    let launch = gui_project.get("launch");
    if let Some(port) = launch.and_then(|l| l.get("port")).and_then(Value::as_u64) {
        map.insert(
            "launch.window.port".into(),
            serde_json::json!({"Number": port}),
        );
    }
    if let Some(secs) = launch
        .and_then(|l| l.get("window_secs"))
        .and_then(Value::as_u64)
        .filter(|secs| *secs != LEGACY_RUN_SECS)
    {
        map.insert(
            "launch.window.run_secs".into(),
            serde_json::json!({"Number": secs}),
        );
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nothing_saved_gives_the_built_in_defaults() {
        assert_eq!(
            ProjectSettings::from_values(None, None, None),
            ProjectSettings {
                player_name: "Survivor".to_owned(),
                windowed: true,
                window: (1280, 720),
                port: 2302,
                keep_running: true,
                run_secs: 600,
                test_mods: Vec::new(),
                prefix: None,
                binarize: false,
                strip_comments: false,
                profile: "default".to_owned(),
                key: KeyChoice::None,
                key_saved: false,
                client: false,
                server_dir: None,
            }
        );
    }

    #[test]
    fn the_project_wins_over_the_defaults_and_wrong_kinds_are_skipped() {
        let defaults = json!({
            "launch.player.name": {"Text": "A"},
            "launch.window.port": {"Number": 2402}
        });
        let project = json!({
            "launch.player.name": {"Text": "Hellbound"},
            "launch.window.port": {"Text": "oops"}
        });
        let settings = ProjectSettings::from_values(Some(&defaults), Some(&project), None);
        assert_eq!(settings.player_name, "Hellbound");
        assert_eq!(settings.port, 2402);
    }

    #[test]
    fn a_saved_automatic_key_reads_as_no_key_and_a_named_one_keeps_its_file() {
        let automatic = json!({"build.signing.key": {"Key": "Automatic"}});
        let named = json!({
            "build.signing.key": {"Key": {"Named": {
                "authority": "hellbound",
                "private_key": "/keys/hellbound.biprivatekey"
            }}}
        });
        let cases = [
            (automatic, KeyChoice::None),
            (
                named,
                KeyChoice::Named {
                    authority: "hellbound".to_owned(),
                    private_key: PathBuf::from("/keys/hellbound.biprivatekey"),
                },
            ),
        ];
        for (defaults, expected) in cases {
            let settings = ProjectSettings::from_values(Some(&defaults), None, None);
            assert_eq!(settings.key, expected);
        }
    }

    #[test]
    fn test_mods_merge_like_the_app() {
        let defaults = json!({"launch.mods.test": {"Mods": [
            {
                "id": "a", "name": "A", "role": "Both", "enabled": true,
                "source": {"Workshop": {"id": "1"}}
            },
            {
                "id": "b", "name": "B", "role": "ServerOnly", "enabled": true,
                "source": {"Folder": {"path": "/mods/b"}}
            }
        ]}});
        let project = json!({"launch.mods.test": {"Mods": [
            {
                "id": "b", "name": "B", "role": "ServerOnly", "enabled": false,
                "source": {"Folder": {"path": "/mods/b"}}
            },
            {
                "id": "c", "name": "C", "role": "Both", "enabled": true,
                "source": {"Workshop": {"id": "3"}}
            }
        ]}});
        let settings = ProjectSettings::from_values(Some(&defaults), Some(&project), None);
        let merged: Vec<(&str, bool)> = settings
            .test_mods
            .iter()
            .map(|entry| (entry.id.as_str(), entry.enabled))
            .collect();
        assert_eq!(merged, [("a", true), ("b", false), ("c", true)]);
    }

    #[test]
    fn the_launch_target_and_server_folder_come_from_the_saved_project() {
        let gui_project = json!({"launch": {
            "target": "ServerAndClient",
            "server_dir": "/srv/dayz",
            "port": null,
            "window_secs": 90
        }});
        let settings = ProjectSettings::from_values(None, None, Some(&gui_project));
        assert!(settings.client);
        assert_eq!(settings.server_dir, Some(PathBuf::from("/srv/dayz")));
    }

    #[test]
    fn project_id_is_the_first_sixteen_hex_digits_of_the_path_digest() {
        assert_eq!(
            project_id(Path::new("/no/such/mod-folder")),
            "9e32a5bd1adbf93a"
        );
    }

    #[test]
    fn a_project_never_saved_by_the_settings_page_reads_its_legacy_fields() {
        let gui_project = json!({
            "source_root": "/mods/Kit",
            "build": {"profile": "release", "authority": "hellbound", "keys_dir": null},
            "launch": {"target": "ServerOnly", "server_dir": null, "port": 2402, "window_secs": 300}
        });
        let settings = ProjectSettings::from_values(None, None, Some(&gui_project));
        assert_eq!(settings.profile, "release");
        assert_eq!(
            settings.key,
            KeyChoice::Named {
                authority: "hellbound".to_owned(),
                private_key: PathBuf::from("/mods/Kit/keys/hellbound.biprivatekey"),
            }
        );
        assert_eq!((settings.port, settings.run_secs), (2402, 300));
    }

    #[test]
    fn one_unreadable_entry_drops_the_whole_map_as_the_app_does() {
        let bad_payload =
            json!({"launch.player.name": {"Text": "B"}, "launch.window.size": {"Size": 5}});
        let settings = ProjectSettings::from_values(Some(&bad_payload), None, None);
        assert_eq!(
            settings.player_name, "Survivor",
            "a malformed payload drops the map too"
        );
        let defaults = json!({"launch.player.name": {"Text": "A"}, "x": {"Colour": 1}});
        let project = json!({"launch.player.name": {"Text": "Hellbound"}, "y": {"Colour": 1}});
        let gui_project = json!({"launch": {"port": 2402, "window_secs": 90}});
        let settings =
            ProjectSettings::from_values(Some(&defaults), Some(&project), Some(&gui_project));
        assert_eq!(settings.player_name, "Survivor");
        assert_eq!(
            settings.port, 2402,
            "an unreadable project map falls back to the legacy fields"
        );
    }

    #[test]
    fn build_args_follow_the_saved_key_and_packing() {
        let packing = ProjectSettings::from_values(
            Some(&json!({
                "build.packing.prefix": {"Text": "JM/CF"},
                "build.packing.binarize": {"Bool": true},
                "build.packing.strip_comments": {"Bool": true},
            })),
            None,
            None,
        );
        let cases = [
            (
                KeyChoice::None,
                json!({
                    "profile": "default",
                    "prefix": "JM/CF",
                    "binarize_config": true,
                    "strip_comments": true
                }),
            ),
            (
                KeyChoice::Named {
                    authority: "Me".to_owned(),
                    private_key: PathBuf::from("/k/Me.biprivatekey"),
                },
                json!({
                    "profile": "default",
                    "prefix": "JM/CF",
                    "binarize_config": true,
                    "strip_comments": true,
                    "authority": "Me",
                    "keys_dir": "/k"
                }),
            ),
        ];
        for (key, expected) in cases {
            let settings = ProjectSettings {
                key,
                ..packing.clone()
            };
            assert_eq!(Value::Object(settings.build_args()), expected);
        }
    }

    #[test]
    fn diag_launch_args_match_the_apps_launch() {
        let settings = ProjectSettings {
            player_name: "Hellbound".to_owned(),
            window: (1600, 900),
            port: 2402,
            keep_running: false,
            run_secs: 300,
            client: true,
            server_dir: Some(PathBuf::from("/srv/dayz")),
            ..ProjectSettings::default()
        };
        let launch = settings.diag_launch_args(None, |_| true);
        assert_eq!(
            Value::Object(launch.args),
            json!({
                "connect_name": "Hellbound",
                "windowed": true,
                "port": 2402,
                "client": true,
                "window": {"width": 1600, "height": 900},
                "timeout_secs": 300,
                "server_dir": "/srv/dayz",
                "test_mods": []
            })
        );
        assert!(launch.missing.is_empty());

        let full_screen = ProjectSettings {
            windowed: false,
            ..ProjectSettings::default()
        };
        let args = full_screen.diag_launch_args(None, |_| true).args;
        assert!(!args.contains_key("window"), "no window size full screen");
        assert!(
            !args.contains_key("timeout_secs"),
            "Keep running sends no timeout"
        );
    }

    #[test]
    fn unpacked_settings_still_send_binarize_off() {
        let args = ProjectSettings::default().build_args();
        assert_eq!(args.get("binarize_config"), Some(&json!(false)));
    }

    #[test]
    fn test_mods_resolve_and_report_what_is_missing() {
        let test_mod = |id: &str, name: &str, source, role, enabled| TestMod {
            id: id.to_owned(),
            name: name.to_owned(),
            source,
            role,
            use_as: ModUse::TestOnly,
            enabled,
        };
        let folder = |path: &str| ModSource::Folder {
            path: PathBuf::from(path),
        };
        let settings = ProjectSettings {
            test_mods: vec![
                test_mod("a", "A", folder("/mods/a"), ModRole::Both, true),
                test_mod("off", "Off", folder("/mods/off"), ModRole::Both, false),
                test_mod(
                    "123",
                    "W",
                    ModSource::Workshop {
                        id: "123".to_owned(),
                    },
                    ModRole::ServerOnly,
                    true,
                ),
                test_mod("gone", "Gone", folder("/mods/gone"), ModRole::Both, true),
            ],
            ..ProjectSettings::default()
        };
        let exists = |path: &Path| path != Path::new("/mods/gone");

        let launch = settings.diag_launch_args(Some(Path::new("/steam")), exists);
        assert_eq!(
            launch.args["test_mods"],
            json!([
                {"path": "/mods/a", "link_name": "a", "role": "both"},
                {
                    "path": "/steam/steamapps/workshop/content/221100/123",
                    "link_name": "123",
                    "role": "server_only"
                }
            ])
        );
        let missing = |name: &str, path: Option<&str>| MissingMod {
            name: name.to_owned(),
            path: path.map(PathBuf::from),
        };
        assert_eq!(launch.missing, [missing("Gone", Some("/mods/gone"))]);

        let without_steam = settings.diag_launch_args(None, exists);
        assert_eq!(
            without_steam.missing,
            [missing("W", None), missing("Gone", Some("/mods/gone"))]
        );
    }

    #[test]
    fn script_index_args_list_only_enabled_dependency_mods() {
        let dependency_mod = |id: &str, name: &str, source, enabled| TestMod {
            id: id.to_owned(),
            name: name.to_owned(),
            source,
            role: ModRole::Both,
            use_as: ModUse::Dependency,
            enabled,
        };
        let folder = |path: &str| ModSource::Folder {
            path: PathBuf::from(path),
        };
        let settings = ProjectSettings {
            test_mods: vec![
                dependency_mod("a", "A", folder("/mods/a"), true),
                dependency_mod("off", "Off", folder("/mods/off"), false),
                TestMod {
                    use_as: ModUse::TestOnly,
                    ..dependency_mod("test", "Test", folder("/mods/test"), true)
                },
                dependency_mod(
                    "123",
                    "W",
                    ModSource::Workshop {
                        id: "123".to_owned(),
                    },
                    true,
                ),
                dependency_mod("gone", "Gone", folder("/mods/gone"), true),
            ],
            ..ProjectSettings::default()
        };
        let exists = |path: &Path| path != Path::new("/mods/gone");
        let missing = |name: &str, path: Option<&str>| MissingMod {
            name: name.to_owned(),
            path: path.map(PathBuf::from),
        };

        let with_steam = settings.script_index_args(Some(Path::new("/steam")), exists);
        assert_eq!(
            with_steam.args["dependencies"],
            json!(["/mods/a", "/steam/steamapps/workshop/content/221100/123"])
        );
        assert_eq!(with_steam.missing, [missing("Gone", Some("/mods/gone"))]);

        let without_steam = settings.script_index_args(None, exists);
        assert_eq!(without_steam.args["dependencies"], json!(["/mods/a"]));
        assert_eq!(
            without_steam.missing,
            [missing("W", None), missing("Gone", Some("/mods/gone"))]
        );
    }

    #[test]
    fn a_folder_mods_link_name_drops_the_leading_at() {
        let settings = ProjectSettings {
            test_mods: vec![TestMod {
                id: "/mods/@CF".to_owned(),
                name: "CF".to_owned(),
                source: ModSource::Folder {
                    path: PathBuf::from("/mods/@CF"),
                },
                role: ModRole::Both,
                use_as: ModUse::TestOnly,
                enabled: true,
            }],
            ..ProjectSettings::default()
        };
        let args = settings.diag_launch_args(None, |_| true).args;
        assert_eq!(args["test_mods"][0]["link_name"], "CF");
    }

    #[test]
    fn server_and_client_run_args_carry_the_keys_each_launch_takes() {
        let test_mod = |name: &str, role| TestMod {
            id: name.to_owned(),
            name: name.to_owned(),
            source: ModSource::Folder {
                path: PathBuf::from(format!("/mods/{name}")),
            },
            role,
            use_as: ModUse::TestOnly,
            enabled: true,
        };
        let mut settings = ProjectSettings {
            test_mods: vec![
                test_mod("Both", ModRole::Both),
                test_mod("Srv", ModRole::ServerOnly),
            ],
            run_secs: 120,
            ..ProjectSettings::default()
        };
        let cases = [
            (
                true,
                Some(PathBuf::from("/srv")),
                &["port", "server_dir", "test_mods", "timeout_secs"][..],
                &["profile_name", "test_mods", "window", "windowed"][..],
            ),
            (
                false,
                None,
                &["port", "test_mods", "timeout_secs"][..],
                &["profile_name", "test_mods", "window", "windowed"][..],
            ),
        ];
        for (windowed, server_dir, server_keys, client_keys) in cases {
            settings.windowed = windowed;
            settings.server_dir = server_dir;
            let server = settings.server_run_args(None, |_| true);
            let client = settings.client_run_args(None, |_| true);
            assert_eq!(
                server.args.keys().map(String::as_str).collect::<Vec<_>>(),
                server_keys
            );
            assert_eq!(
                client.args.keys().map(String::as_str).collect::<Vec<_>>(),
                client_keys
            );
            assert_eq!(client.args["windowed"], windowed);
            assert_eq!(client.args["window"], json!({"width": 1280, "height": 720}));
            assert_eq!(server.args["test_mods"].as_array().unwrap().len(), 2);
            assert_eq!(client.args["test_mods"].as_array().unwrap().len(), 1);
            assert_eq!(server.args["timeout_secs"], 120);
        }
    }

    #[test]
    fn dependency_mods_are_the_enabled_rows_used_as_a_dependency() {
        let project = json!({"launch.mods.test": {"Mods": [
            {"id": "on", "name": "On", "source": {"Folder": {"path": "/mods/on"}},
             "role": "Both", "use_as": "Dependency", "enabled": true},
            {"id": "off", "name": "Off", "source": {"Folder": {"path": "/mods/off"}},
             "role": "Both", "use_as": "Dependency", "enabled": false},
            {"id": "test", "name": "Test", "source": {"Folder": {"path": "/mods/test"}},
             "role": "Both", "use_as": "TestOnly", "enabled": true},
        ]}});
        let settings = ProjectSettings::from_values(None, Some(&project), None);

        let ids: Vec<&str> = settings
            .dependency_mods()
            .map(|row| row.id.as_str())
            .collect();

        assert_eq!(ids, ["on"]);
    }
}
