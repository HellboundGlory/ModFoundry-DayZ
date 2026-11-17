//! Shared build + keygen command bodies (`mf build`, `mf keygen`), the single
//! implementation CLI, MCP and automation bind. Deterministic staging
//! lives in `modfoundry-build`; signing is native by default, with the DsUtils
//! tools (Wine/native Windows) as an explicit opt-in via `tools`.

use crate::launch_sets::{check_local_names, named_set, read_sets, SetMod, SETS_FILE};
use modfoundry_vfs::write::WriteTxn;
use modfoundry_vfs::{root_name, ChangeSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Arguments for the `mf build` shared body. Frontends hand this to the
/// registry as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArgs {
    /// Mod root whose tree becomes the addon content.
    pub mod_root: PathBuf,
    /// Override mod name (default: mod_root basename, `@` stripped).
    #[serde(default)]
    pub name: Option<String>,
    /// Output directory override (default `mod_root/builds/<profile>/@<name>`).
    #[serde(default)]
    pub output: Option<PathBuf>,
    /// Build profile (directory name under `builds/`).
    #[serde(default)]
    pub profile: Option<String>,
    /// Signature authority; enables signing. Keys looked up in `keys_dir`.
    #[serde(default)]
    pub authority: Option<String>,
    /// Dir holding `<authority>.biprivatekey` + `.bikey` (default `mod_root/keys`).
    #[serde(default)]
    pub keys_dir: Option<PathBuf>,
    /// PBO header `prefix` override (defaults to the mod name). Existing mod
    /// trees declare their own prefix (e.g. `JM/CF`); this is the way to build
    /// them without rewriting their `CfgMods.files[]`.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Explicit DsUtils tools dir override.
    #[serde(default)]
    pub tools: Option<PathBuf>,
    /// Binarize `config.cpp` into `config.bin` inside the packed PBO.
    #[serde(default)]
    pub binarize_config: bool,
    /// Remove comments from the packed `.c` files.
    #[serde(default)]
    pub strip_comments: bool,
}

/// The mod name `build.run` uses when none is given: the folder's name without a leading `@`.
pub fn default_name(mod_root: &Path) -> String {
    // `file_name()` is `None` for `.`/`..`, which would silently name every such
    // build `Mod` and break the PBO-prefix the mod's `files[]` expects.
    let absolute = std::path::absolute(mod_root).unwrap_or_else(|_| mod_root.to_path_buf());
    absolute
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Mod".into())
        .trim_start_matches('@')
        .to_owned()
}

/// Entry time when the caller pins none: the wall clock would put a different
/// stamp in every entry record of two builds of the same tree.
const FIXED_ENTRY_TIME: u32 = 0;

/// Build timestamp: `SOURCE_DATE_EPOCH` when it parses, else [`FIXED_ENTRY_TIME`].
fn build_timestamp() -> u32 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(FIXED_ENTRY_TIME, |secs| secs as u32)
}

/// Deterministically stage (+ optionally sign) a mod into
/// `builds/<profile>/@<name>`. Returns the build report. Build output is
/// regenerable, so it is not recorded for `workspace.undo`.
pub fn build_project(args: &BuildArgs) -> anyhow::Result<modfoundry_build::BuildReport> {
    modfoundry_build::build_mod(&build_options(args)?)
}

/// The change-set a build would stage, without staging it (`mf build
/// --preview`).
pub fn plan_build_project(args: &BuildArgs) -> anyhow::Result<ChangeSet> {
    modfoundry_build::plan_build(&build_options(args)?)
}

/// The files a build of `args` would consider, each with whether it is packed and why.
pub fn build_contents(args: &BuildArgs) -> anyhow::Result<modfoundry_build::Contents> {
    let options = build_options(args)?;
    modfoundry_build::contents(
        &options.source_root,
        &options.output_dir,
        options.binarize_config,
    )
}

fn build_options(args: &BuildArgs) -> anyhow::Result<modfoundry_build::BuildOptions> {
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| default_name(&args.mod_root));
    let profile = args.profile.clone().unwrap_or_else(|| "default".into());
    let output_dir = args.output.clone().unwrap_or_else(|| {
        args.mod_root
            .join("builds")
            .join(&profile)
            .join(format!("@{name}"))
    });

    let author = args.authority.clone().unwrap_or_else(|| name.clone());
    Ok(modfoundry_build::BuildOptions {
        name: name.clone(),
        source_root: args.mod_root.clone(),
        output_dir,
        prefix: args.prefix.clone(),
        timestamp: build_timestamp(),
        mod_cpp: modfoundry_build::ModMeta {
            name: name.clone(),
            author,
            version: "1.0".into(),
        },
        sign: signing_for(args)?,
        binarize_config: args.binarize_config,
        strip_comments: args.strip_comments,
    })
}

/// Signing inputs for the explicit `authority`; without one the build is unsigned.
fn signing_for(args: &BuildArgs) -> anyhow::Result<Option<modfoundry_build::Signing>> {
    let Some(authority) = &args.authority else {
        return Ok(None);
    };
    check_authority(authority)?;
    let keys_dir = args
        .keys_dir
        .clone()
        .unwrap_or_else(|| args.mod_root.join("keys"));
    let (private_key, public_key) = key_paths(authority, &keys_dir);
    Ok(Some(modfoundry_build::Signing {
        authority: authority.clone(),
        private_key,
        public_key,
        tools_dir: args.tools.clone(),
    }))
}

/// Generate a `<authority>.biprivatekey` + `.bikey` pair into `keys_dir`, never
/// replacing one that exists. Native by default; `tools` forces the official
/// `DSCreateKey` from a DsUtils dir. Returns the key paths and — when ModFoundry
/// wrote the files itself — the change-set that removes them again.
pub fn keygen(
    authority: &str,
    keys_dir: &Path,
    tools: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, PathBuf, Option<ChangeSet>)> {
    check_authority(authority)?;
    let (private_path, public_path) = key_paths(authority, keys_dir);
    refuse_existing(&private_path, &public_path)?;
    match tools {
        // The official tool writes both files itself; it is an external process,
        // so there is nothing for the VFS transaction to record.
        Some(dir) => {
            keygen_with_tool(authority, keys_dir, &modfoundry_sign::DsUtils::new(dir))?;
            Ok((private_path, public_path, None))
        }
        None => {
            let mut txn = WriteTxn::new(root_name("mod", keys_dir), keys_dir);
            write_keypair(&mut txn, authority, &private_path, &public_path)?;
            let change_set = txn.finish()?;
            Ok((private_path, public_path, Some(change_set)))
        }
    }
}

fn keygen_with_tool(
    authority: &str,
    keys_dir: &Path,
    tool: &modfoundry_sign::DsUtils,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(keys_dir)?;
    let staging = keys_dir.join(format!(".mf-keygen-{authority}"));
    let _ = std::fs::remove_dir_all(&staging);
    // The tool writes the key with its own mode; only a folder nobody else can enter
    // keeps it unreadable until the mode is tightened.
    modfoundry_platform::create_dir_owner_only(&staging)?;
    let result = (|| -> anyhow::Result<()> {
        let (staged_private, staged_public) = tool.create_key(&staging, authority)?;
        modfoundry_platform::restrict_to_owner(&staged_private)?;
        let (private_path, public_path) = key_paths(authority, keys_dir);
        refuse_existing(&private_path, &public_path)?;
        std::fs::rename(&staged_private, &private_path)?;
        if let Err(e) = std::fs::rename(&staged_public, &public_path) {
            let _ = std::fs::remove_file(&private_path);
            return Err(e.into());
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// The same keygen as a dry run: the two files it would create. Generating a
/// keypair is local work with no side effect, so the preview really does
/// generate one and discards it.
pub fn plan_keygen(authority: &str, keys_dir: &Path) -> anyhow::Result<ChangeSet> {
    check_authority(authority)?;
    let (private_path, public_path) = key_paths(authority, keys_dir);
    refuse_existing(&private_path, &public_path)?;
    let mut txn = WriteTxn::dry_run(root_name("mod", keys_dir), keys_dir);
    write_keypair(&mut txn, authority, &private_path, &public_path)?;
    Ok(txn.finish()?)
}

/// A replaced private key could not be undone: it is written unretained, so a
/// change-set never holds its old bytes.
fn refuse_existing(private_path: &Path, public_path: &Path) -> anyhow::Result<()> {
    for path in [private_path, public_path] {
        if path.exists() {
            anyhow::bail!(
                "{} already exists; pick another key name or remove the old key first",
                path.display()
            );
        }
    }
    Ok(())
}

fn check_authority(authority: &str) -> anyhow::Result<()> {
    if authority.trim().is_empty() {
        anyhow::bail!("authority must not be empty");
    }
    if authority.contains(['/', '\\', '.']) {
        anyhow::bail!("authority must be a bare name (no path separators): {authority}");
    }
    Ok(())
}

fn write_keypair(
    txn: &mut WriteTxn,
    authority: &str,
    private_path: &Path,
    public_path: &Path,
) -> anyhow::Result<()> {
    let (private, public) =
        modfoundry_sign::generate_keypair(authority, modfoundry_sign::KEY_BITS)?;
    // The private key is written unretained: a change-set ends up in the
    // agent database, and key material must not.
    txn.write_unretained(&crate::file_rel(private_path)?, private.to_bytes())?;
    txn.write(&crate::file_rel(public_path)?, public.to_bytes())?;
    Ok(())
}

fn key_paths(authority: &str, keys_dir: &Path) -> (PathBuf, PathBuf) {
    (
        keys_dir.join(modfoundry_sign::private_key_name(authority)),
        keys_dir.join(modfoundry_sign::public_key_name(authority)),
    )
}

/// What a `build.run` request builds: one mod root, or every local mod of a launch set.
#[derive(Debug, Clone)]
pub(crate) enum BuildTarget {
    /// One mod root, built from the arguments as they arrived.
    One(Value),
    /// Every local mod of a named set, in order.
    Set(SetRequest),
}

/// A `mf build --set` request: the set, the project it lives in and the flags every mod shares.
#[derive(Debug, Clone)]
pub(crate) struct SetRequest {
    set: String,
    source_root: PathBuf,
    /// The folder a relative `source_root` resolved to, for the reply to name.
    pub(crate) resolved_root: Option<PathBuf>,
    flags: serde_json::Map<String, Value>,
}

/// One mod of a set, ready for the single-mod build body.
#[derive(Debug, Clone)]
pub(crate) struct SetBuild {
    /// The mod name the build and its output use.
    pub(crate) name: String,
    /// The single-mod `build.run` arguments, before saved settings fill the rest.
    pub(crate) args: Value,
}

impl BuildTarget {
    /// Splits `build.run`'s arguments into the target they name.
    pub(crate) fn parse(args: Value) -> Result<Self, String> {
        let given = |key: &str| args.get(key).is_some_and(|value| !value.is_null());
        match (given("mod_root"), given("set") || given("source_root")) {
            (true, false) => Ok(BuildTarget::One(args)),
            (false, true) => SetRequest::parse(&args).map(BuildTarget::Set),
            (true, true) => Err("a build takes a mod root or a launch set, not both".to_owned()),
            (false, false) => Err("a build needs a mod root, or a launch set".to_owned()),
        }
    }
}

impl SetRequest {
    fn parse(args: &Value) -> Result<Self, String> {
        let set = args
            .get("set")
            .and_then(Value::as_str)
            .ok_or("a set must be named by a string")?
            .to_owned();
        let resolved_root = crate::settings_fill::resolved_root(args, "source_root");
        let source_root = args
            .get("source_root")
            .and_then(Value::as_str)
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let mut flags = args
            .as_object()
            .cloned()
            .ok_or("a build takes a JSON object")?;
        flags.remove("set");
        flags.remove("source_root");
        for (key, refusal) in [
            (
                "output",
                "a set builds each mod under its own builds/<profile>, so it takes no output",
            ),
            ("name", "a set names each of its mods, so it takes no name"),
            (
                "profile",
                "a set builds under its own profile, so it takes no profile",
            ),
        ] {
            match flags.get(key) {
                Some(Value::Null) => {
                    flags.remove(key);
                }
                Some(_) => return Err(refusal.to_owned()),
                None => {}
            }
        }
        Ok(Self {
            set,
            source_root: std::path::absolute(&source_root).unwrap_or(source_root),
            resolved_root,
            flags,
        })
    }

    /// The set's local mods in build order — `mods` then `server_mods` — under the set's profile
    /// and each entry's name.
    pub(crate) fn builds(&self) -> Result<Vec<SetBuild>, String> {
        let file = self.source_root.join(SETS_FILE);
        let sets = read_sets(&self.source_root)?
            .ok_or_else(|| format!("{} is missing", file.display()))?;
        let set = named_set(&sets, &self.set)?;
        check_local_names(set)?;
        let profile = set.profile.as_deref().unwrap_or("default");
        let builds: Vec<SetBuild> = set
            .mods
            .iter()
            .chain(&set.server_mods)
            .filter_map(|entry| match entry {
                SetMod::Local { local, name } => Some((local, name)),
                SetMod::Folder { .. } | SetMod::Workshop { .. } => None,
            })
            .map(|(local, name)| {
                let root = self.source_root.join(local);
                let root = std::path::absolute(&root).unwrap_or(root);
                let name = name.clone().unwrap_or_else(|| default_name(&root));
                let mut args = self.flags.clone();
                args.insert("mod_root".into(), json!(root));
                args.insert("name".into(), json!(name));
                args.insert("profile".into(), json!(profile));
                SetBuild {
                    name,
                    args: Value::Object(args),
                }
            })
            .collect();
        if builds.is_empty() {
            return Err(format!(
                "launch set \"{}\" has no local mod to build; only entries with \"local\" are built",
                self.set
            ));
        }
        Ok(builds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_resolves_dot_to_the_directory_name() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(default_name(Path::new(".")), expected);
    }

    #[test]
    fn default_name_strips_the_mod_marker_and_keeps_real_names() {
        assert_eq!(default_name(Path::new("@MyMod")), "MyMod");
        assert_eq!(default_name(Path::new("/tmp/some/MyMod")), "MyMod");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mfdev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn args_for(root: &Path) -> BuildArgs {
        BuildArgs {
            mod_root: root.to_path_buf(),
            name: Some("FieldKit".into()),
            output: None,
            profile: None,
            authority: None,
            keys_dir: None,
            prefix: None,
            tools: None,
            binarize_config: false,
            strip_comments: false,
        }
    }

    #[test]
    fn a_build_with_an_authority_is_signed_with_that_key() {
        let root = tmp("authority-build");
        crate::scaffold(&root, &crate::ScaffoldOptions::new("FieldKit")).unwrap();
        let keys = tmp("authority-keys");
        let (private, public) =
            modfoundry_sign::generate_keypair("MF", modfoundry_sign::KEY_BITS).unwrap();
        std::fs::write(keys.join("MF.biprivatekey"), private.to_bytes()).unwrap();
        std::fs::write(keys.join("MF.bikey"), public.to_bytes()).unwrap();

        let mut args = args_for(&root);
        args.authority = Some("MF".into());
        args.keys_dir = Some(keys.clone());
        let report = build_project(&args).unwrap();
        assert!(report.signed);
        assert!(root
            .join("builds/default/@FieldKit/keys/MF.bikey")
            .is_file());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&keys);
    }

    /// Runs the tool in its working folder, recording that folder and the mode it had there.
    #[cfg(unix)]
    struct KeyTool {
        exit: i32,
        seen: std::sync::Arc<std::sync::Mutex<Option<(PathBuf, u32)>>>,
    }

    #[cfg(unix)]
    impl modfoundry_sign::tools::Runner for KeyTool {
        fn run(
            &self,
            _exe: &Path,
            args: &[String],
            cwd: &Path,
        ) -> anyhow::Result<modfoundry_sign::tools::ProcessOut> {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cwd)?.permissions().mode() & 0o777;
            *self.seen.lock().unwrap() = Some((cwd.to_path_buf(), mode));
            if self.exit == 0 {
                let authority = &args[0];
                std::fs::write(
                    cwd.join(modfoundry_sign::private_key_name(authority)),
                    b"tool private",
                )?;
                std::fs::write(
                    cwd.join(modfoundry_sign::public_key_name(authority)),
                    b"tool public",
                )?;
            }
            Ok(modfoundry_sign::tools::ProcessOut {
                code: self.exit,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_made_key_is_made_in_a_private_folder_and_moved_out() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tmp("tool-keygen");
        let tools = tmp.join("DsUtils");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join("DSCreateKey.exe"), b"").unwrap();
        let keys = tmp.join("keys");

        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let tool = modfoundry_sign::DsUtils::with_runner(
            tools.clone(),
            Box::new(KeyTool {
                exit: 0,
                seen: seen.clone(),
            }),
        );
        keygen_with_tool("MF", &keys, &tool).unwrap();

        let (cwd, mode) = seen.lock().unwrap().clone().expect("the tool did not run");
        assert_ne!(cwd, keys, "the tool ran straight into the keys folder");
        assert_eq!(mode, 0o700);

        let private = keys.join("MF.biprivatekey");
        let mode = std::fs::metadata(&private).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&private).unwrap(), b"tool private");
        assert_eq!(
            std::fs::read(keys.join("MF.bikey")).unwrap(),
            b"tool public"
        );
        let mut entries: Vec<String> = std::fs::read_dir(&keys)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(entries, ["MF.bikey", "MF.biprivatekey"]);

        let empty = tmp.join("keys-failed");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let failing =
            modfoundry_sign::DsUtils::with_runner(tools, Box::new(KeyTool { exit: 1, seen }));
        assert!(keygen_with_tool("MF", &empty, &failing).is_err());
        assert!(std::fs::read_dir(&empty).unwrap().next().is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
