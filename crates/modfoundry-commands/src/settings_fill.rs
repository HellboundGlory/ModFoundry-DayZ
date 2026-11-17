//! Fills the arguments an agent leaves out from the app's saved settings.

use std::path::{Path, PathBuf};

use anyhow::Result;
use modfoundry_core::CommandError;
use modfoundry_platform::discover_steam_installations;
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::{json, Map, Value};

use crate::launch_sets::{read_sets, resolve_set, ResolvedMod, ResolvedSet, SetMod};
use crate::project_settings::{
    project_id, KeyChoice, LaunchArgs, MissingMod, ModRole, ModSource, ProjectSettings, TestMod,
    PREFS_DEFAULTS_KEY, PREFS_PROJECT_KEY, PROJECT_KEY,
};

/// What a call's saved settings filled in, and the folder a relative root was resolved to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SettingsUsed {
    pub(crate) keys: Vec<String>,
    pub(crate) resolved_root: Option<PathBuf>,
    /// The app's saved launch target is Server only, so the filled `client` is false.
    pub(crate) saved_server_only: bool,
}

impl SettingsUsed {
    /// Writes the filled keys, and the resolved folder when a relative root resolved, into `value`.
    pub(crate) fn write_into(&self, value: &mut Value) {
        value["settings_used"] = json!(self.keys);
        if let Some(root) = &self.resolved_root {
            value["resolved_root"] = json!(root);
        }
        if self.saved_server_only {
            value["saved_server_only"] = json!(true);
        }
    }
}

/// How a launch takes the mods and config a named set gives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetUse {
    /// `diag.run`: the set's mods as test mods, its config staged like the project's own.
    Server,
    /// `diag.run_diag`: the set's mods as test mods, its config patched as the server template.
    Diag,
    /// `diag.run_client`: the set's shared mods as the whole `mods` list.
    Client,
}

/// Fills the mods and config a `--set` names, before saved settings fill the rest; a launch that
/// stages the set's config gets its file name back.
pub(crate) fn launch_set(
    mut args: Value,
    usage: SetUse,
) -> Result<(Value, Option<PathBuf>), CommandError> {
    let Some(name) = args.get("set").and_then(Value::as_str).map(str::to_owned) else {
        return Ok((args, None));
    };
    let Some(root) = args
        .get("source_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    else {
        return Err(CommandError::InvalidArguments(format!(
            "launch set \"{name}\" needs source_root; pass --source-root"
        )));
    };
    let steam = if names_a_workshop_mod(&root, &name)? {
        steam_root(&args)
    } else {
        None
    };
    let set = resolve_set(&root, &name, steam.as_deref(), Path::exists)
        .map_err(CommandError::InvalidArguments)?;
    let config = set_config(&set, &root);
    let source_root = std::path::absolute(&root).unwrap_or(root);
    if usage != SetUse::Client && args.get("mod_dir").is_none() {
        let own = own_build(&set, &source_root).ok_or_else(|| {
            CommandError::InvalidArguments(format!(
                "launch set \"{name}\" has no local mod at {}; name the built @Mod to load",
                source_root.display()
            ))
        })?;
        set_key(&mut args, "mod_dir", json_of(&own));
    }
    let mod_dir = args.get("mod_dir").and_then(Value::as_str).map(Path::new);
    if usage == SetUse::Diag && own_is_server_only(&set, &source_root, mod_dir) {
        return Err(CommandError::InvalidArguments(format!(
            "launch set \"{name}\" lists its local mod only under server_mods; a Diag run \
             loads its own mod on the Diag client too, so list it under mods"
        )));
    }
    let typed_config = args.get("config").is_some();

    if usage == SetUse::Client {
        if !has_list(&args, "mods") {
            let (own, shared): (Vec<_>, Vec<_>) = set
                .mods
                .into_iter()
                .partition(|entry| is_own_mod(entry, &source_root, mod_dir));
            let mods = shared
                .into_iter()
                .chain(own)
                .map(|entry| json_of(&entry.arg.path))
                .collect();
            set_key(&mut args, "mods", Value::Array(mods));
        }
        if args.get("test_mods").is_none() {
            set_key(&mut args, "test_mods", Value::Array(Vec::new()));
        }
        return Ok((args, None));
    }

    let mut mods: Vec<Value> = set
        .mods
        .iter()
        .chain(&set.server_mods)
        .filter(|entry| !is_own_mod(entry, &source_root, mod_dir))
        .map(|entry| json_of(&entry.arg))
        .collect();
    if let Some(typed) = args.get("test_mods").and_then(Value::as_array) {
        mods.extend(typed.iter().cloned());
    }
    set_key(&mut args, "test_mods", Value::Array(mods));
    if usage == SetUse::Diag && !typed_config {
        if let Some(config) = &config {
            set_key(&mut args, "config", json_of(config));
        }
    }
    let staged = if usage == SetUse::Server && !typed_config {
        config
    } else {
        None
    };
    Ok((args, staged))
}

/// Whether the set names a workshop mod, so a Steam root is worth finding to resolve it.
pub(crate) fn names_a_workshop_mod(source_root: &Path, name: &str) -> Result<bool, CommandError> {
    Ok(read_sets(source_root)
        .map_err(CommandError::InvalidArguments)?
        .and_then(|mut sets| sets.sets.remove(name))
        .is_some_and(|set| {
            set.mods
                .iter()
                .chain(&set.server_mods)
                .any(|entry| matches!(entry, SetMod::Workshop { .. }))
        }))
}

/// The set's config file, relative to the project's server folder; `source_root` is the root the
/// set was resolved from, relative or not.
fn set_config(set: &ResolvedSet, source_root: &Path) -> Option<PathBuf> {
    let config = set.config.as_ref()?;
    let server = source_root.join("server");
    Some(config.strip_prefix(&server).unwrap_or(config).to_path_buf())
}

/// The build of the set's local mod whose source is the source root.
fn own_build(set: &ResolvedSet, source_root: &Path) -> Option<PathBuf> {
    set.mods
        .iter()
        .chain(&set.server_mods)
        .find(|entry| is_own_mod(entry, source_root, None))
        .map(|entry| entry.arg.path.clone())
}

fn own_is_server_only(set: &ResolvedSet, source_root: &Path, mod_dir: Option<&Path>) -> bool {
    let own = |entry: &ResolvedMod| is_own_mod(entry, source_root, mod_dir);
    !set.mods.iter().any(own) && set.server_mods.iter().any(own)
}

/// Whether a set entry is the mod this run loads itself, which loads last.
fn is_own_mod(entry: &ResolvedMod, source_root: &Path, mod_dir: Option<&Path>) -> bool {
    entry
        .local_root
        .as_deref()
        .is_some_and(|root| same_path(root, source_root))
        || mod_dir.is_some_and(|dir| same_path(&entry.arg.path, dir))
}

/// Path equality through `..` and symlinks; a path that cannot be resolved is compared absolute.
fn same_path(a: &Path, b: &Path) -> bool {
    crate::paths::resolved_or_given(a) == crate::paths::resolved_or_given(b)
}

/// Whether the caller gave a non-empty list under `key`.
fn has_list(args: &Value, key: &str) -> bool {
    args.get(key)
        .and_then(Value::as_array)
        .is_some_and(|list| !list.is_empty())
}

fn set_key(args: &mut Value, key: &str, value: Value) {
    if let Some(object) = args.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

fn json_of(serialisable: &impl serde::Serialize) -> Value {
    serde_json::to_value(serialisable).expect("a launch argument serialises")
}

/// Inserts each default whose key `args` lacks; a key given as `null` counts as given.
pub(crate) fn fill(args: &mut Map<String, Value>, defaults: Map<String, Value>) -> Vec<String> {
    let mut inserted = Vec::new();
    for (key, value) in defaults {
        if !args.contains_key(&key) {
            args.insert(key.clone(), value);
            inserted.push(key);
        }
    }
    inserted
}

/// The app's saved settings for the project at `source_root`, or `None` when the app never saved it.
pub(crate) fn load(db: &Path, source_root: &Path) -> Result<Option<ProjectSettings>> {
    // A lookup must not create the database on a machine where the app never ran.
    if !db.is_file() {
        return Ok(None);
    }
    let store = Store::open(db)?;
    let scope = Scope::Workspace(project_id(source_root));
    let project = store.get_setting(&scope, PREFS_PROJECT_KEY)?;
    let gui_project = store.get_setting(&scope, PROJECT_KEY)?;
    if project.is_none() && gui_project.is_none() {
        return Ok(None);
    }
    let defaults = store.get_setting(&Scope::Global, PREFS_DEFAULTS_KEY)?;
    Ok(Some(ProjectSettings::from_values(
        defaults.as_ref(),
        project.as_ref(),
        gui_project.as_ref(),
    )))
}

/// The app's saved settings for the project a call names under `root_key`, when a database is set.
fn saved(
    db: Option<&Path>,
    args: &Value,
    root_key: &str,
) -> Result<Option<ProjectSettings>, CommandError> {
    let (Some(db), Some(root)) = (db, args.get(root_key).and_then(Value::as_str)) else {
        return Ok(None);
    };
    load(db, Path::new(root)).map_err(|e| CommandError::Failed(format!("{e:#}")))
}

fn fill_value(args: &mut Value, defaults: Map<String, Value>) -> Vec<String> {
    args.as_object_mut()
        .map_or_else(Vec::new, |args| fill(args, defaults))
}

/// The folder a relative root under `root_key` resolved to, as the settings lookup sees it; `None`
/// for an absolute root or none.
pub(crate) fn resolved_root(args: &Value, root_key: &str) -> Option<PathBuf> {
    let root = Path::new(args.get(root_key).and_then(Value::as_str)?);
    if root.is_absolute() {
        return None;
    }
    root.canonicalize()
        .or_else(|_| std::path::absolute(root))
        .ok()
}

/// The settings a build uses: the project's, or the app-wide ones for a project the app never opened.
fn build_settings(
    db: Option<&Path>,
    args: &Value,
) -> Result<Option<ProjectSettings>, CommandError> {
    if let Some(settings) = saved(db, args, "mod_root")? {
        return Ok(Some(settings));
    }
    let Some(db) = db.filter(|db| db.is_file()) else {
        return Ok(None);
    };
    let defaults = Store::open(db)
        .and_then(|store| store.get_setting(&Scope::Global, PREFS_DEFAULTS_KEY))
        .map_err(|e| CommandError::Failed(format!("{e:#}")))?;
    Ok(defaults.map(|defaults| ProjectSettings::from_values(Some(&defaults), None, None)))
}

/// The packing and signing settings `build.run` and `publish.prepare` leave out.
pub(crate) fn build(
    db: Option<&Path>,
    mut args: Value,
) -> Result<(Value, SettingsUsed), CommandError> {
    let mut used = SettingsUsed {
        resolved_root: resolved_root(&args, "mod_root"),
        ..SettingsUsed::default()
    };
    let Some(settings) = build_settings(db, &args)? else {
        return Ok((args, used));
    };
    let mut defaults = settings.build_args();
    // The key is one choice: a caller who names either half keeps the saved key out.
    if args.get("authority").is_some() || args.get("keys_dir").is_some() {
        defaults.remove("authority");
        defaults.remove("keys_dir");
    }
    let keys = fill_value(&mut args, defaults);
    if let KeyChoice::Named {
        authority,
        private_key,
    } = &settings.key
    {
        let signs_from = private_key.with_file_name(format!("{authority}.biprivatekey"));
        if keys.iter().any(|key| key == "authority") && !signs_from.is_file() {
            return Err(CommandError::InvalidArguments(format!(
                "signing key {} not found; pick another key in the app's Settings › Signing or pass authority and keys_dir",
                signs_from.display()
            )));
        }
    }
    used.keys = keys;
    Ok((args, used))
}

/// The warning a build with no signing key earns, unless the user saved a choice.
pub(crate) fn unsigned_warning(db: Option<&Path>, args: &Value, signed: bool) -> Option<String> {
    if signed || args.get("mod_root").is_none() {
        return None;
    }
    let saved_key = matches!(build_settings(db, args), Ok(Some(settings)) if settings.key_saved);
    (!saved_key).then(|| {
        "this build is unsigned and no signing key is set; a retail server refuses it. Pick a key \
         in the app's Settings › Signing, or pass --authority and --keys-dir"
            .to_owned()
    })
}

/// The launch settings `diag.run_diag` leaves out; a saved test mod the disk lacks is refused.
pub(crate) fn diag(db: Option<&Path>, args: Value) -> Result<(Value, SettingsUsed), CommandError> {
    let (args, mut used) = launch(
        db,
        args,
        |_| true,
        |settings, steam_root| {
            let mut launch = settings.diag_launch_args(steam_root, Path::exists);
            launch
                .args
                .entry("window")
                .or_insert_with(|| settings.window_arg());
            launch
        },
    )?;
    used.saved_server_only = used.keys.iter().any(|key| key == "client")
        && args.get("client") == Some(&Value::Bool(false));
    Ok((args, used))
}

/// The launch settings `diag.run` leaves out; a saved test mod the disk lacks is refused.
pub(crate) fn server(
    db: Option<&Path>,
    args: Value,
) -> Result<(Value, SettingsUsed), CommandError> {
    launch(
        db,
        args,
        |_| true,
        |settings, steam_root| settings.server_run_args(steam_root, Path::exists),
    )
}

/// The launch settings a detached `diag.run` leaves out: with saved keep-running on it adds no
/// run length, so the run lasts until stopped.
pub(crate) fn detached_server(
    db: Option<&Path>,
    args: Value,
) -> Result<(Value, SettingsUsed), CommandError> {
    launch(
        db,
        args,
        |_| true,
        |settings, steam_root| {
            let mut launch = settings.server_run_args(steam_root, Path::exists);
            if settings.keep_running {
                launch.args.remove("timeout_secs");
            }
            launch
        },
    )
}

/// The launch settings `diag.run_client` leaves out; a saved test mod the disk lacks is refused.
pub(crate) fn client(
    db: Option<&Path>,
    args: Value,
) -> Result<(Value, SettingsUsed), CommandError> {
    launch(db, args, loads_on_client, |settings, steam_root| {
        settings.client_run_args(steam_root, Path::exists)
    })
}

/// The saved test mods a client launch keeps: server-only ones stay on the server.
fn loads_on_client(entry: &TestMod) -> bool {
    entry.role == ModRole::Both
}

/// Whether the launch's arguments may need a Steam install to find a saved workshop mod.
fn needs_steam(args: &Value, settings: &ProjectSettings, loads: impl Fn(&TestMod) -> bool) -> bool {
    args.get("test_mods").is_none()
        && settings.test_mods.iter().any(|entry| {
            entry.enabled && loads(entry) && matches!(entry.source, ModSource::Workshop { .. })
        })
}

/// Fills the launch arguments `launch_args` builds from the project's saved settings. `loads` says
/// which saved test mod the launch's own arguments keep.
fn launch(
    db: Option<&Path>,
    mut args: Value,
    loads: impl Fn(&TestMod) -> bool,
    launch_args: impl Fn(&ProjectSettings, Option<&Path>) -> LaunchArgs,
) -> Result<(Value, SettingsUsed), CommandError> {
    let mut used = SettingsUsed {
        resolved_root: resolved_root(&args, "source_root"),
        ..SettingsUsed::default()
    };
    let Some(settings) = saved(db, &args, "source_root")? else {
        return Ok((args, used));
    };
    let steam_root = if needs_steam(&args, &settings, &loads) {
        steam_root(&args)
    } else {
        None
    };
    let mut launch = launch_args(&settings, steam_root.as_deref());
    // A size asks for a window and full screen has no size, so a given choice drops the saved half.
    if args.get("window").is_some() {
        launch.args.remove("windowed");
    }
    let windowed = args
        .get("windowed")
        .or_else(|| launch.args.get("windowed"))
        .and_then(Value::as_bool);
    if windowed == Some(false) {
        launch.args.remove("window");
    }
    let keys = fill_value(&mut args, launch.args);
    if keys.iter().any(|key| key == "test_mods") && !launch.missing.is_empty() {
        return Err(CommandError::InvalidArguments(missing_test_mods(
            &launch.missing,
            "test mod",
            "fix it in the app's Settings › Launch or pass test_mods",
        )));
    }
    used.keys = keys;
    Ok((args, used))
}

pub(crate) fn steam_root(args: &Value) -> Option<PathBuf> {
    args.get("steam_dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(discovered_steam_root)
}

/// The first Steam install found on this machine.
pub(crate) fn discovered_steam_root() -> Option<PathBuf> {
    discover_steam_installations()
        .into_iter()
        .next()
        .map(|install| install.root)
}

fn missing_test_mods(missing: &[MissingMod], what: &str, fix: &str) -> String {
    let clauses: Vec<String> = missing
        .iter()
        .map(|entry| match &entry.path {
            Some(path) => format!("{what} {} not found at {}", entry.name, path.display()),
            None => format!("{what} {} needs a Steam install", entry.name),
        })
        .collect();
    format!("{}; {fix}", clauses.join("; "))
}

/// The dependency roots `script.index` leaves out: the saved dependency mods of the project.
pub(crate) fn script_index(
    db: Option<&Path>,
    mut args: Value,
) -> Result<(Value, SettingsUsed), CommandError> {
    let used = SettingsUsed {
        resolved_root: resolved_root(&args, "root"),
        ..SettingsUsed::default()
    };
    if args.get("dependencies").is_some() {
        return Ok((args, used));
    }
    let Some(settings) = saved(db, &args, "root")? else {
        return Ok((args, used));
    };
    let needs_steam = settings
        .dependency_mods()
        .any(|entry| matches!(entry.source, ModSource::Workshop { .. }));
    let steam_root = needs_steam.then(|| steam_root(&args)).flatten();
    let index = settings.script_index_args(steam_root.as_deref(), Path::exists);
    let keys = fill_value(&mut args, index.args);
    if !index.missing.is_empty() {
        return Err(CommandError::InvalidArguments(missing_test_mods(
            &index.missing,
            "dependency mod",
            "remove it with mf deps remove or from the Defaults mods list, or pass dependencies",
        )));
    }
    Ok((args, SettingsUsed { keys, ..used }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fill_adds_only_the_absent_keys_and_reports_them() {
        let cases = [
            (
                json!({"profile": "dev"}),
                json!({"profile": "default", "prefix": "JM/CF"}),
                json!({"profile": "dev", "prefix": "JM/CF"}),
                json!(["prefix"]),
            ),
            (
                json!({"prefix": null}),
                json!({"prefix": "JM/CF"}),
                json!({"prefix": null}),
                json!([]),
            ),
            (
                json!({"mod_root": "/mods/A"}),
                json!({}),
                json!({"mod_root": "/mods/A"}),
                json!([]),
            ),
        ];
        for (given, defaults, expected, inserted) in cases {
            let mut args = given.as_object().unwrap().clone();
            let reported = fill(&mut args, defaults.as_object().unwrap().clone());
            assert_eq!(Value::Object(args), expected);
            assert_eq!(json!(reported), inserted);
        }
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-fill-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_relative_root_is_named_with_the_folder_it_resolved_to() {
        let relative = json!("some/rel");
        let absolute = json!(std::env::temp_dir());
        let cases = [
            (
                relative.clone(),
                Some(std::path::absolute("some/rel").unwrap()),
            ),
            (absolute.clone(), None),
        ];
        for (given, wanted) in cases {
            let (_, used) = build(None, json!({ "mod_root": given })).unwrap();
            assert_eq!(used.resolved_root, wanted, "build with {given}");
            let (_, used) = server(None, json!({ "source_root": given })).unwrap();
            assert_eq!(used.resolved_root, wanted, "server with {given}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_root_with_saved_settings_names_the_folder_they_were_found_under() {
        let root = temp("relative-saved");
        let db = root.join("agents.db");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Workspace(project_id(&root)),
                PREFS_PROJECT_KEY,
                &json!({"launch.window.port": {"Number": 2402}}),
            )
            .unwrap();
        let rel = relative_to_cwd(&root);

        let (_, used) = server(Some(&db), json!({ "source_root": rel })).unwrap();

        assert!(used.keys.contains(&"port".to_owned()), "{used:?}");
        assert_eq!(used.resolved_root, Some(root.canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_into_names_the_resolved_folder_only_when_one_was_resolved() {
        let mut value = json!({"state": "running"});
        SettingsUsed {
            keys: vec!["port".to_owned()],
            resolved_root: None,
            ..SettingsUsed::default()
        }
        .write_into(&mut value);
        assert_eq!(
            value,
            json!({"state": "running", "settings_used": ["port"]})
        );

        let root = PathBuf::from("/project");
        let mut value = json!({});
        SettingsUsed {
            keys: vec!["port".to_owned()],
            resolved_root: Some(root.clone()),
            ..SettingsUsed::default()
        }
        .write_into(&mut value);
        assert_eq!(
            value,
            json!({"settings_used": ["port"], "resolved_root": root})
        );
    }

    /// A database saving a named key `authority` at `private_key` for the project at `root`.
    fn saved_key(dir: &Path, root: &Path, authority: &str, private_key: &Path) -> PathBuf {
        let db = dir.join("agents.db");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Workspace(project_id(root)),
                PREFS_PROJECT_KEY,
                &json!({"build.signing.key": {"Key": {"Named": {
                    "authority": authority, "private_key": private_key,
                }}}}),
            )
            .unwrap();
        db
    }

    #[test]
    fn a_detached_launch_keeps_the_saved_run_length_out_while_keep_running_is_on() {
        let dir = temp("detached");
        let db = dir.join("agents.db");
        let save = |keep_running: bool| {
            Store::open(&db)
                .unwrap()
                .set_setting(
                    &Scope::Workspace(project_id(&dir)),
                    PREFS_PROJECT_KEY,
                    &json!({
                        "launch.window.keep_running": {"Bool": keep_running},
                        "launch.window.run_secs": {"Number": 25},
                        "launch.window.port": {"Number": 2402},
                    }),
                )
                .unwrap();
        };
        let cases = [
            (true, None, None),
            (true, Some(30), Some(30)),
            (false, None, Some(25)),
            (false, Some(30), Some(30)),
        ];
        for (keep_running, given, wanted) in cases {
            save(keep_running);
            let mut args = json!({ "source_root": dir });
            if let Some(given) = given {
                args["timeout_secs"] = json!(given);
            }
            let (filled, used) = detached_server(Some(&db), args).unwrap();
            assert_eq!(
                filled.get("port").and_then(Value::as_u64),
                Some(2402),
                "the other saved launch settings still fill"
            );
            assert_eq!(
                filled.get("timeout_secs").and_then(Value::as_u64),
                wanted,
                "keep_running {keep_running}, given {given:?}"
            );
            assert_eq!(
                used.keys.iter().any(|key| key == "timeout_secs"),
                !keep_running && given.is_none(),
                "keep_running {keep_running}, given {given:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_steam_lookup_counts_only_the_mods_the_launch_loads() {
        use crate::project_settings::ModUse;

        fn keeps_all(_: &TestMod) -> bool {
            true
        }
        let saved_mod = |source: ModSource, role: ModRole| TestMod {
            id: "m".into(),
            name: "M".into(),
            source,
            role,
            use_as: ModUse::TestOnly,
            enabled: true,
        };
        let workshop = || ModSource::Workshop { id: "123".into() };
        let folder = || ModSource::Folder {
            path: PathBuf::from("/mods/@A"),
        };
        type Case = (&'static str, TestMod, Value, fn(&TestMod) -> bool, bool);
        let cases: [Case; 6] = [
            (
                "server-only workshop mod on the client",
                saved_mod(workshop(), ModRole::ServerOnly),
                json!({}),
                loads_on_client,
                false,
            ),
            (
                "both-side workshop mod on the client",
                saved_mod(workshop(), ModRole::Both),
                json!({}),
                loads_on_client,
                true,
            ),
            (
                "server-only workshop mod on the server",
                saved_mod(workshop(), ModRole::ServerOnly),
                json!({}),
                keeps_all,
                true,
            ),
            (
                "disabled workshop mod",
                TestMod {
                    enabled: false,
                    ..saved_mod(workshop(), ModRole::Both)
                },
                json!({}),
                loads_on_client,
                false,
            ),
            (
                "folder mod",
                saved_mod(folder(), ModRole::Both),
                json!({}),
                loads_on_client,
                false,
            ),
            (
                "test mods given on the call",
                saved_mod(workshop(), ModRole::Both),
                json!({"test_mods": []}),
                loads_on_client,
                false,
            ),
        ];
        for (label, entry, args, loads, wanted) in cases {
            let settings = ProjectSettings {
                test_mods: vec![entry],
                ..ProjectSettings::default()
            };
            assert_eq!(needs_steam(&args, &settings, loads), wanted, "{label}");
        }
    }

    #[test]
    fn a_given_authority_or_keys_dir_keeps_the_saved_key_out() {
        let dir = temp("pair");
        let key = dir.join("Hellbound.biprivatekey");
        std::fs::write(&key, b"k").unwrap();
        let db = saved_key(&dir, &dir, "Hellbound", &key);
        for given in [
            json!({"authority": "Other"}),
            json!({"keys_dir": "/elsewhere"}),
        ] {
            let mut args = given.clone();
            args["mod_root"] = json!(dir);
            let (filled, used) = build(Some(&db), args).unwrap();
            assert!(
                !used
                    .keys
                    .iter()
                    .any(|k| k == "authority" || k == "keys_dir"),
                "{used:?}"
            );
            assert_eq!(filled.get("authority"), given.get("authority"));
            assert_eq!(filled.get("keys_dir"), given.get("keys_dir"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_key_check_looks_where_the_build_signs_from() {
        let dir = temp("keyname");
        let key = dir.join("Hellbound.biprivatekey");
        std::fs::write(&key, b"k").unwrap();
        let db = saved_key(&dir, &dir, "H.B", &key);
        let err = build(Some(&db), json!({"mod_root": dir}))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&dir.join("H.B.biprivatekey").display().to_string()),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_the_app_never_opened_builds_with_the_app_wide_key_alone() {
        let dir = temp("defaults");
        let key = dir.join("Hellbound.biprivatekey");
        std::fs::write(&key, b"k").unwrap();
        let db = dir.join("agents.db");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({"build.signing.key": {"Key": {"Named": {
                    "authority": "Hellbound", "private_key": key,
                }}}}),
            )
            .unwrap();

        let (filled, used) = build(Some(&db), json!({"mod_root": dir})).unwrap();
        assert_eq!(filled["authority"], json!("Hellbound"));
        assert_eq!(filled["keys_dir"], json!(dir));
        assert!(used.keys.iter().any(|key| key == "authority"), "{used:?}");
        assert!(used.keys.iter().any(|key| key == "keys_dir"), "{used:?}");

        let (_, launched) = diag(Some(&db), json!({"source_root": dir})).unwrap();
        assert!(launched.keys.is_empty(), "{launched:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unsigned_warning_names_the_fix_unless_a_key_choice_is_saved() {
        let dir = temp("warning");
        let saved_project = |name: &str, value: Value| {
            let db = dir.join(name);
            Store::open(&db)
                .unwrap()
                .set_setting(
                    &Scope::Workspace(project_id(&dir)),
                    PREFS_PROJECT_KEY,
                    &value,
                )
                .unwrap();
            db
        };
        let explicit_none = saved_project(
            "none.db",
            json!({"build.signing.key": {"Key": "Automatic"}}),
        );
        let no_key = saved_project(
            "nokey.db",
            json!({"build.packing.profile": {"Text": "release"}}),
        );
        let defaults_none = dir.join("defaults.db");
        Store::open(&defaults_none)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({"build.signing.key": {"Key": "Automatic"}}),
            )
            .unwrap();
        let warning = "this build is unsigned and no signing key is set; a retail server refuses \
                       it. Pick a key in the app's Settings › Signing, or pass --authority and \
                       --keys-dir";
        let mod_root = json!({"mod_root": dir});
        let cases = [
            (
                "signed",
                Some(explicit_none.as_path()),
                mod_root.clone(),
                true,
                None,
            ),
            ("no database", None, mod_root.clone(), false, Some(warning)),
            (
                "saved no key",
                Some(explicit_none.as_path()),
                mod_root.clone(),
                false,
                None,
            ),
            (
                "app-wide no key",
                Some(defaults_none.as_path()),
                mod_root.clone(),
                false,
                None,
            ),
            (
                "no saved key",
                Some(no_key.as_path()),
                mod_root.clone(),
                false,
                Some(warning),
            ),
            (
                "no mod root",
                Some(no_key.as_path()),
                json!({}),
                false,
                None,
            ),
        ];
        for (name, db, args, signed, wanted) in cases {
            assert_eq!(
                unsigned_warning(db, &args, signed).as_deref(),
                wanted,
                "{name}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_file_that_is_not_there_fills_nothing_and_is_not_created() {
        let dir = temp("nodb");
        let db = dir.join("agents.db");
        let (_, used) = build(Some(&db), json!({"mod_root": dir})).unwrap();
        assert!(used.keys.is_empty());
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn own_name(root: &Path) -> String {
        format!("@{}", root.file_name().unwrap().to_string_lossy())
    }

    fn write_set(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join("server")).unwrap();
        std::fs::write(root.join("server/launch-sets.json"), text).unwrap();
    }

    #[test]
    fn a_set_fills_the_mods_in_order_and_keeps_typed_ones_after_them() {
        let root = temp("set-mods");
        let own = root.join("builds/default").join(own_name(&root));
        let a = root.join("mods/@FolderA");
        let b = root.join("mods/@FolderB");
        for dir in [&own, &a, &b] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [
                {"folder": a}, {"folder": b}, {"local": "."}
            ]}}})
            .to_string(),
        );
        let typed =
            json!({"path": root.join("typed/@Typed"), "link_name": "Typed", "role": "both"});

        let (filled, config) = launch_set(
            json!({"source_root": root, "mod_dir": own, "set": "session", "test_mods": [typed.clone()]}),
            SetUse::Server,
        )
        .unwrap();

        assert_eq!(
            filled["test_mods"],
            json!([
                {"path": a, "link_name": "FolderA", "role": "both"},
                {"path": b, "link_name": "FolderB", "role": "both"},
                typed,
            ])
        );
        assert!(config.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_set_names_the_runs_own_mod_when_the_run_names_none() {
        let root = temp("set-own");
        let own = root.join("builds/default").join(own_name(&root));
        let a = root.join("mods/@FolderA");
        for dir in [&own, &a] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [{"folder": a}, {"local": "."}]}}}).to_string(),
        );

        for usage in [SetUse::Server, SetUse::Diag] {
            let (filled, _) =
                launch_set(json!({"source_root": root, "set": "session"}), usage).unwrap();

            assert!(
                same_path(Path::new(filled["mod_dir"].as_str().unwrap()), &own),
                "{filled}"
            );
            assert_eq!(filled["test_mods"].as_array().unwrap().len(), 1, "{filled}");
        }
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [{"folder": a}]}}}).to_string(),
        );
        let err = launch_set(json!({"source_root": root, "set": "session"}), SetUse::Diag)
            .unwrap_err()
            .to_string();
        assert!(err.contains("has no local mod at"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_set_names_a_server_only_own_mod_as_the_runs_own() {
        let root = temp("set-own-server");
        let own = root.join("builds/default").join(own_name(&root));
        let a = root.join("mods/@FolderA");
        for dir in [&own, &a] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let both = json!({"folder": a});
        let own_entry = json!({"local": "."});
        let cases = [
            (
                "server only",
                json!({"mods": [both.clone()], "server_mods": [own_entry.clone()]}),
                vec![SetUse::Server],
            ),
            (
                "both lists",
                json!({"mods": [both, own_entry.clone()], "server_mods": [own_entry]}),
                vec![SetUse::Server, SetUse::Diag],
            ),
        ];
        for (label, set, usages) in cases {
            write_set(&root, &json!({"sets": {"session": set}}).to_string());
            for usage in usages {
                let (filled, _) =
                    launch_set(json!({"source_root": root, "set": "session"}), usage).unwrap();

                assert!(
                    same_path(Path::new(filled["mod_dir"].as_str().unwrap()), &own),
                    "{label}: {filled}"
                );
                assert_eq!(
                    filled["test_mods"],
                    json!([{
                        "path": a, "link_name": "FolderA", "role": "both",
                    }]),
                    "{label}: {filled}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_diag_set_refuses_a_server_only_own_mod() {
        let root = temp("set-own-diag");
        std::fs::create_dir_all(root.join("builds/default").join(own_name(&root))).unwrap();
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [], "server_mods": [{"local": "."}]}}})
                .to_string(),
        );

        let own = root.join("builds/default").join(own_name(&root));
        for args in [
            json!({"source_root": root, "set": "session"}),
            json!({"source_root": root, "set": "session", "mod_dir": own}),
        ] {
            let err = launch_set(args.clone(), SetUse::Diag).unwrap_err();

            assert!(
                matches!(err, CommandError::InvalidArguments(ref message) if message.contains("only under server_mods")),
                "{args}: {err:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_client_set_loads_the_runs_own_mod_last() {
        let root = temp("set-client");
        let own = root.join("builds/default").join(own_name(&root));
        let a = root.join("mods/@FolderA");
        let b = root.join("mods/@FolderB");
        for dir in [&own, &a, &b] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [
                {"folder": a}, {"local": "."}, {"folder": b}
            ]}}})
            .to_string(),
        );

        let (filled, _) = launch_set(
            json!({"source_root": root, "set": "session", "mods": []}),
            SetUse::Client,
        )
        .unwrap();

        assert_eq!(filled["mods"], json!([a, b, own]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_client_set_leaves_a_server_only_own_mod_out() {
        let root = temp("set-client-server-only");
        let a = root.join("mods/@FolderA");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(root.join("builds/default").join(own_name(&root))).unwrap();
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [{"folder": a}], "server_mods": [{"local": "."}]}}})
                .to_string(),
        );

        let (filled, _) = launch_set(
            json!({"source_root": root, "set": "session", "mods": []}),
            SetUse::Client,
        )
        .unwrap();

        assert_eq!(filled["mods"], json!([a]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_set_keeps_the_saved_test_mods_out() {
        let root = temp("set-saved");
        let folder = root.join("mods/@FolderA");
        std::fs::create_dir_all(&folder).unwrap();
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [{"folder": folder}]}}}).to_string(),
        );
        let db = root.join("agents.db");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Workspace(project_id(&root)),
                PROJECT_KEY,
                &json!({"launch.mods.test": {"Mods": [{
                    "id": "m1", "name": "Saved", "role": "Both", "enabled": true,
                    "source": {"Folder": {"path": root.join("saved")}},
                }]}}),
            )
            .unwrap();

        let (args, _) = launch_set(
            json!({"source_root": root, "set": "session", "mods": []}),
            SetUse::Client,
        )
        .unwrap();
        let (filled, used) = client(Some(&db), args).unwrap();

        assert_eq!(filled["test_mods"], json!([]));
        assert!(!used.keys.iter().any(|key| key == "test_mods"), "{used:?}");
        assert_eq!(filled["mods"], json!([folder]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_set_supplies_the_config_unless_one_is_typed() {
        let root = temp("set-config");
        let own = root.join("builds/default/@Own");
        std::fs::create_dir_all(&own).unwrap();
        write_set(
            &root,
            r#"{"sets": {"session": {"mods": [], "config": "serverDZ_expansion.cfg"}}}"#,
        );
        std::fs::write(
            root.join("server/serverDZ_expansion.cfg"),
            "hostname = \"x\";\n",
        )
        .unwrap();
        let base = json!({"source_root": root, "mod_dir": own, "set": "session"});

        let (filled, config) = launch_set(base.clone(), SetUse::Server).unwrap();
        assert_eq!(config, Some(PathBuf::from("serverDZ_expansion.cfg")));
        assert!(
            filled.get("config").is_none(),
            "a staged config is not a raw -config="
        );

        let (filled, config) = launch_set(base.clone(), SetUse::Diag).unwrap();
        assert_eq!(config, None);
        assert_eq!(filled["config"], json!("serverDZ_expansion.cfg"));

        let mut typed = base;
        typed["config"] = json!("mine.cfg");
        for usage in [SetUse::Server, SetUse::Diag] {
            let (filled, config) = launch_set(typed.clone(), usage).unwrap();
            assert_eq!(config, None);
            assert_eq!(filled["config"], json!("mine.cfg"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn set_entries_that_reach_the_runs_own_mod_through_dot_dot_are_its_own() {
        let root = temp("set-own-dotdot");
        let built = root.join("builds/default/@M");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::create_dir_all(root.join("builds/default").join(own_name(&root))).unwrap();
        let own_dir = root.file_name().unwrap().to_string_lossy().into_owned();
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [
                {"folder": root.join("builds/../builds/default/@M")},
                {"local": format!("../{own_dir}")},
            ]}}})
            .to_string(),
        );

        let (filled, _) = launch_set(
            json!({"source_root": root, "mod_dir": built, "set": "session"}),
            SetUse::Server,
        )
        .unwrap();

        assert_eq!(
            filled["test_mods"],
            json!([]),
            "both entries are the run's own"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    fn relative_to_cwd(path: &Path) -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components() {
            rel.push("..");
        }
        rel.push(path.strip_prefix("/").unwrap());
        rel
    }

    #[cfg(unix)]
    #[test]
    fn a_set_resolved_from_a_relative_root_names_its_config_file() {
        let root = temp("set-relative");
        let folder = root.join("mods/@FolderA");
        std::fs::create_dir_all(&folder).unwrap();
        write_set(
            &root,
            &json!({"sets": {"session": {"mods": [{"folder": folder}], "config": "x.cfg"}}})
                .to_string(),
        );
        std::fs::write(root.join("server/x.cfg"), "hostname = \"x\";\n").unwrap();
        let rel = relative_to_cwd(&root);

        let (_, config) = launch_set(
            json!({"source_root": rel, "set": "session", "mod_dir": folder}),
            SetUse::Server,
        )
        .unwrap();

        assert_eq!(config, Some(PathBuf::from("x.cfg")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_set_with_an_unbuilt_local_mod_is_refused_with_the_build_command() {
        let root = temp("set-unbuilt");
        write_set(
            &root,
            r#"{"sets": {"session": {"mods": [{"local": "../other"}]}}}"#,
        );

        let err = launch_set(
            json!({"source_root": root, "set": "session"}),
            SetUse::Server,
        )
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("build it first: mf build --set session"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use serde_json::json;

    fn saved(label: &str, windowed: bool) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("mf-fill-window-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("agents.db");
        let _ = std::fs::remove_file(&db);
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Workspace(project_id(&root)),
                PREFS_PROJECT_KEY,
                &json!({
                    "launch.window.windowed": { "Bool": windowed },
                    "launch.window.size": { "Size": { "width": 1600, "height": 900 } },
                    "launch.player.name": { "Text": "Tester" },
                }),
            )
            .unwrap();
        (db, root)
    }

    #[test]
    fn a_saved_server_only_target_is_named_when_it_leaves_the_client_out() {
        let cases = [
            ("ServerOnly", json!({}), Some(json!(true))),
            ("ServerOnly", json!({"client": false}), None),
            ("ServerAndClient", json!({}), None),
        ];
        for (target, given, want) in cases {
            let root = std::env::temp_dir().join(format!(
                "mf-fill-server-only-{target}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let db = root.join("agents.db");
            let _ = std::fs::remove_file(&db);
            Store::open(&db)
                .unwrap()
                .set_setting(
                    &Scope::Workspace(project_id(&root)),
                    PROJECT_KEY,
                    &json!({"launch": {"target": target, "server_dir": null, "port": 2302, "window_secs": 60}}),
                )
                .unwrap();
            let mut args = given.clone();
            args["source_root"] = json!(root);
            args["test_mods"] = json!([]);

            let (_, used) = diag(Some(&db), args).unwrap();
            let mut reply = json!({});
            used.write_into(&mut reply);

            assert_eq!(
                reply.get("saved_server_only"),
                want.as_ref(),
                "{target} {given}"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn an_explicit_window_choice_beats_the_saved_one() {
        let size = json!({"width": 800, "height": 600});
        let cases = [
            (
                "size",
                false,
                json!({"window": size}),
                None,
                Some(size.clone()),
            ),
            ("full", true, json!({"windowed": false}), Some(false), None),
        ];
        for (label, saved_windowed, given, windowed, window) in cases {
            let (db, root) = saved(label, saved_windowed);
            let mut args = given;
            args["source_root"] = json!(root);
            for fill in [client, diag] {
                let (filled, _) = fill(Some(&db), args.clone()).unwrap();
                assert_eq!(
                    filled.get("windowed"),
                    windowed.map(Value::from).as_ref(),
                    "{label}"
                );
                assert_eq!(filled.get("window"), window.as_ref(), "{label}");
            }
        }
    }

    #[test]
    fn a_windowed_flag_takes_the_saved_size_even_when_the_saved_choice_is_full_screen() {
        let size = json!({"width": 1600, "height": 900});
        let (db, root) = saved("windowed-size-full-screen", false);
        let bare = json!({ "source_root": root, "test_mods": [] });
        let mut windowed = bare.clone();
        windowed["windowed"] = json!(true);
        for fill in [client, diag] {
            let (filled, _) = fill(Some(&db), windowed.clone()).unwrap();
            assert_eq!(filled.get("windowed"), Some(&json!(true)));
            assert_eq!(filled.get("window"), Some(&size), "a --windowed run");
            let (filled, used) = fill(Some(&db), bare.clone()).unwrap();
            assert_eq!(filled.get("windowed"), Some(&json!(false)));
            assert_eq!(filled.get("window"), None, "a full-screen run");
            assert!(!used.keys.iter().any(|key| key == "window"));
        }
        let _ = std::fs::remove_dir_all(&root);

        let (db, root) = saved("windowed-size-windowed", true);
        let bare = json!({ "source_root": root, "test_mods": [] });
        for fill in [client, diag] {
            let (filled, _) = fill(Some(&db), bare.clone()).unwrap();
            assert_eq!(filled.get("window"), Some(&size), "a saved windowed run");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
