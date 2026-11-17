//! Diag launch staging: the mod source link, the packed `@Mod` link, the
//! server `mpmissions` link, a patched copy of the server config and a
//! relative `-profiles` link, all inside the client install, planned by
//! `plan_diag_stage` and removed again by the run's `Staged` guard.

use crate::launch_plan::StageStep;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[cfg(unix)]
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
use crate::run_ops::SERVER_CONFIG;
#[cfg(unix)]
use crate::run_ops::{absent, at_link_name, in_place};

/// The file name of the patched server config staged into the client install.
pub const DIAG_CONFIG_NAME: &str = "serverDZ_modfoundry.cfg";

/// The relative `-profiles` dir staged into the client install.
pub const DIAG_PROFILES_LINK: &str = "mf-diag-profiles";

/// A mod loaded only for a diag run, never a project dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestModArg {
    /// The mod dir staged as the `@<link_name>` link.
    pub path: PathBuf,
    /// The bare link name; the staged link is `@<link_name>`.
    pub link_name: String,
    pub role: TestModRole,
}

/// Which halves of a diag run load a test mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestModRole {
    /// Loaded by the server and the client via `-mod=`.
    Both,
    /// Loaded by the server only via `-serverMod=`.
    ServerOnly,
}

/// Inputs for [`plan_diag_stage`].
#[derive(Debug, Clone)]
pub struct DiagStageArgs {
    /// The DayZ client install the diag server runs from.
    pub game_dir: PathBuf,
    /// The retail server install the `mpmissions`/config come from.
    pub server_dir: PathBuf,
    /// The config template under the project's server folder (default `serverDZ.cfg`).
    pub config: Option<PathBuf>,
    /// The built `@Mod` dir loaded via `-mod=`.
    pub built_mod: PathBuf,
    /// The loose mod source root (wins over the PBO under `-filePatching`).
    pub source_root: PathBuf,
    /// The PBO prefix: the link name the loose source is staged as.
    pub mod_prefix: String,
    /// The real profiles dir the relative `mf-diag-profiles` link points at.
    pub profiles_dir: PathBuf,
    /// Extra mods staged as `@<link_name>` links for this run only.
    pub test_mods: Vec<TestModArg>,
}

/// The staged artefacts; the guard that owns them removes them when the run ends.
#[derive(Debug)]
pub struct DiagStage {
    config_name: String,
    profiles_arg: String,
    mod_args: Vec<String>,
    server_mod_args: Vec<String>,
}

impl DiagStage {
    /// The `-config=` name to pass (relative to the client install).
    pub fn config_name(&self) -> &str {
        &self.config_name
    }

    /// The `-profiles=` argument to pass (relative to the client install).
    pub fn profiles_arg(&self) -> &str {
        &self.profiles_arg
    }

    /// The ordered `-mod=` entries: the Both test mods, then the project mod.
    pub fn mod_args_both(&self) -> Vec<String> {
        self.mod_args.clone()
    }

    /// The ordered `-serverMod=` entries: the ServerOnly test mods.
    pub fn mod_args_server(&self) -> Vec<String> {
        self.server_mod_args.clone()
    }
}

/// Patch `BattlEye`/`allowFilePatching`/`verifySignatures` into `original`:
/// each existing assignment keeps its position, casing, spacing and trailing
/// comment with only the value swapped; a missing key is appended. Everything
/// else in the text — comments, order, CRLF endings — is preserved.
pub fn patched_server_config(original: &str) -> String {
    const KEYS: [(&str, &str); 3] = [
        ("BattlEye", "0"),
        ("allowFilePatching", "1"),
        ("verifySignatures", "0"),
    ];
    let mut found = [false; KEYS.len()];
    let mut out = String::with_capacity(original.len());
    for chunk in original.split_inclusive('\n') {
        let (body, ending) = match chunk.strip_suffix('\n') {
            Some(rest) => match rest.strip_suffix('\r') {
                Some(body) => (body, "\r\n"),
                None => (rest, "\n"),
            },
            None => (chunk, ""),
        };
        let mut line = body.to_string();
        for (i, (key, value)) in KEYS.iter().enumerate() {
            if found[i] {
                continue;
            }
            if let Some(patched) = patch_line(body, key, value) {
                line = patched;
                found[i] = true;
                break;
            }
        }
        out.push_str(&line);
        out.push_str(ending);
    }
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    for (i, (key, value)) in KEYS.iter().enumerate() {
        if !found[i] {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&format!("{key} = {value};"));
            out.push_str(eol);
        }
    }
    out
}

/// `Some(line)` when `line` assigns `key` (case-insensitively, `=` right after
/// it) with the value swapped for `value`; a key-like prefix such as
/// `verifySignaturesX` is not an assignment.
fn patch_line(line: &str, key: &str, value: &str) -> Option<String> {
    let indent = &line[..line.len() - line.trim_start().len()];
    let body = &line[indent.len()..];
    let written = body.get(..key.len())?;
    if !written.eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = &body[key.len()..];
    let eq = rest.find('=')?;
    if !rest[..eq].trim().is_empty() {
        return None;
    }
    let after = &rest[eq + 1..];
    let semi = after.find(';')?;
    let val = &after[..semi];
    let tail = &after[semi..];
    let replaced = if val.trim().is_empty() {
        value.to_string()
    } else {
        let lead = &val[..val.len() - val.trim_start().len()];
        let trail = &val[val.trim_end().len()..];
        format!("{lead}{value}{trail}")
    };
    Some(format!("{indent}{written}{}={replaced}{tail}", &rest[..eq]))
}

/// The diag staging a run makes, worked out without writing anything and validating every path
/// first, so a conflict leaves the install untouched: the steps that make each artefact, and the
/// stale paths they clear. Paths in `gone` are treated as absent.
#[cfg(unix)]
pub(crate) fn plan_diag_stage(
    args: &DiagStageArgs,
    gone: &[PathBuf],
    project: &mut Vec<StageStep>,
) -> anyhow::Result<(DiagStage, Vec<StageStep>, Vec<PathBuf>)> {
    let args = &absolute_targets(args)?;
    let game = &args.game_dir;
    let prefix = relative_prefix(&args.mod_prefix)?;
    let config = game.join(DIAG_CONFIG_NAME);
    let mpmissions = game.join("mpmissions");
    let source_link = game.join(&prefix);
    let mod_name = match args.built_mod.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => anyhow::bail!("built mod {} has no name", args.built_mod.display()),
    };
    let mod_arg = at_link_name(mod_name);
    let mod_link = game.join(&mod_arg);
    let profiles_link = game.join(DIAG_PROFILES_LINK);

    check_test_mods(&args.test_mods)?;
    if !args.profiles_dir.is_dir() {
        project.push(StageStep::CreateDir {
            path: args.profiles_dir.clone(),
        });
    }
    let mut steps = Vec::new();
    remove_stale(&config, gone, &mut steps)?;
    if mpmissions.symlink_metadata().is_err() || absent(gone, &mpmissions) {
        steps.push(StageStep::Link {
            link: mpmissions,
            target: args.server_dir.join("mpmissions"),
        });
    }
    if plan_link(&source_link, &args.source_root, gone)? {
        steps.extend(missing_parents(game, &prefix, gone));
        steps.push(StageStep::Link {
            link: source_link,
            target: args.source_root.clone(),
        });
    }
    if plan_link(&mod_link, &args.built_mod, gone)? {
        steps.push(StageStep::Link {
            link: mod_link,
            target: args.built_mod.clone(),
        });
    }
    for test_mod in &args.test_mods {
        let link = game.join(at_link_name(&test_mod.link_name));
        if plan_link(&link, &test_mod.path, gone)? {
            steps.push(StageStep::Link {
                link,
                target: test_mod.path.clone(),
            });
        }
    }
    let contents = patched_server_config(&project_config_text(args, project)?).into_bytes();
    steps.push(StageStep::Write {
        path: config,
        bytes: contents.len(),
        contents,
    });
    profiles_link_steps(&profiles_link, &args.profiles_dir, gone, &mut steps)?;
    refuse_shared_links(&steps)?;

    let stale = steps
        .iter()
        .filter_map(|step| match step {
            StageStep::Remove { path } => Some(path.clone()),
            StageStep::Relink { link, .. } => Some(link.clone()),
            _ => None,
        })
        .collect();
    let mut mod_args: Vec<String> = args
        .test_mods
        .iter()
        .filter(|m| m.role == TestModRole::Both)
        .map(|m| at_link_name(&m.link_name))
        .collect();
    mod_args.push(mod_arg);
    let server_mod_args: Vec<String> = args
        .test_mods
        .iter()
        .filter(|m| m.role == TestModRole::ServerOnly)
        .map(|m| at_link_name(&m.link_name))
        .collect();
    let stage = DiagStage {
        config_name: DIAG_CONFIG_NAME.to_string(),
        profiles_arg: DIAG_PROFILES_LINK.to_string(),
        mod_args,
        server_mod_args,
    };
    Ok((stage, steps, stale))
}

/// Refuses two links at one path, such as a test mod named like the built mod.
#[cfg(unix)]
fn refuse_shared_links(steps: &[StageStep]) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for step in steps {
        if let StageStep::Link { link, .. } | StageStep::Relink { link, .. } = step {
            if !seen.insert(link) {
                anyhow::bail!("two artefacts would be staged as {}", link.display());
            }
        }
    }
    Ok(())
}

/// Pushes the step that removes a stale file or link at `path`; a folder there is refused.
#[cfg(unix)]
fn remove_stale(path: &Path, gone: &[PathBuf], steps: &mut Vec<StageStep>) -> anyhow::Result<()> {
    if absent(gone, path) {
        return Ok(());
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => anyhow::bail!(
            "{} is a directory; remove it to stage the diag run",
            path.display()
        ),
        Ok(_) => steps.push(StageStep::Remove {
            path: path.to_path_buf(),
        }),
        Err(_) => {}
    }
    Ok(())
}

/// Pushes the steps that point the profiles link at `target`: a stale link is re-pointed, a stale
/// file is removed first, and a folder there is refused.
#[cfg(unix)]
fn profiles_link_steps(
    link: &Path,
    target: &Path,
    gone: &[PathBuf],
    steps: &mut Vec<StageStep>,
) -> anyhow::Result<()> {
    if !absent(gone, link) {
        match std::fs::symlink_metadata(link) {
            Ok(meta) if meta.is_dir() => anyhow::bail!(
                "{} is a directory; remove it to stage the diag run",
                link.display()
            ),
            Ok(meta) if meta.file_type().is_symlink() => {
                steps.push(StageStep::Relink {
                    link: link.to_path_buf(),
                    target: target.to_path_buf(),
                });
                return Ok(());
            }
            Ok(_) => steps.push(StageStep::Remove {
                path: link.to_path_buf(),
            }),
            Err(_) => {}
        }
    }
    steps.push(StageStep::Link {
        link: link.to_path_buf(),
        target: target.to_path_buf(),
    });
    Ok(())
}

/// The folders a nested prefix link needs, shallowest first, when they are not there.
#[cfg(unix)]
fn missing_parents(game: &Path, prefix: &Path, gone: &[PathBuf]) -> Vec<StageStep> {
    let mut dir = game.to_path_buf();
    let mut steps = Vec::new();
    for part in prefix.parent().into_iter().flat_map(Path::components) {
        dir.push(part.as_os_str());
        if dir.is_dir() && !absent(gone, &dir) {
            continue;
        }
        steps.push(StageStep::CreateDir { path: dir.clone() });
    }
    steps
}

/// A `mod_prefix` as a relative path of plain folder names.
#[cfg(unix)]
fn relative_prefix(prefix: &str) -> anyhow::Result<PathBuf> {
    let normalized = prefix.replace('\\', "/");
    let path = PathBuf::from(&normalized);
    if normalized.is_empty()
        || path.is_absolute()
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        anyhow::bail!("{prefix} is not a relative PBO prefix");
    }
    Ok(path)
}

/// The folder name a link is for: `name` with one leading `@` removed.
fn bare_link_name(name: &str) -> &str {
    name.strip_prefix('@').unwrap_or(name)
}

/// A test mod's link name must survive `-mod=` and stay inside the install.
fn check_link_name(name: &str) -> anyhow::Result<()> {
    let bare = bare_link_name(name);
    let bad = bare.is_empty()
        || bare.starts_with('@')
        || bare == "."
        || bare == ".."
        || bare.contains(['/', '\\', ';']);
    if bad {
        anyhow::bail!("test mod link name {name} must be a bare folder name");
    }
    Ok(())
}

/// Refuses test mods whose link name is bad or repeated, or whose folder is not there.
pub(crate) fn check_test_mods(test_mods: &[TestModArg]) -> anyhow::Result<()> {
    let mut seen_names = HashSet::new();
    for test_mod in test_mods {
        check_link_name(&test_mod.link_name)?;
        let bare = bare_link_name(&test_mod.link_name);
        if !seen_names.insert(bare) {
            anyhow::bail!("duplicate test-mod link name @{bare}");
        }
        if !test_mod.path.is_dir() {
            anyhow::bail!("test mod {} is not a directory", test_mod.path.display());
        }
    }
    Ok(())
}

/// `args` with every path that becomes a link target made absolute: a relative
/// target resolves against the install the link sits in, not the caller's cwd.
#[cfg(unix)]
fn absolute_targets(args: &DiagStageArgs) -> anyhow::Result<DiagStageArgs> {
    let mut args = args.clone();
    for path in [
        &mut args.server_dir,
        &mut args.built_mod,
        &mut args.source_root,
        &mut args.profiles_dir,
    ] {
        *path = std::path::absolute(&*path)?;
    }
    for test_mod in &mut args.test_mods {
        test_mod.path = std::path::absolute(&test_mod.path)?;
    }
    Ok(args)
}

/// The project's server config file `name` in `dir`, found by file name in any ASCII case.
pub(crate) fn project_config_path(dir: &Path, name: &Path) -> PathBuf {
    let exact = dir.join(name);
    if exact.is_file() {
        return exact;
    }
    let Some(wanted) = name.file_name() else {
        return exact;
    };
    let folder = exact.parent().unwrap_or(dir);
    let mut found: Vec<std::ffi::OsString> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name())
                .filter(|file_name| file_name.eq_ignore_ascii_case(wanted))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    match found.into_iter().next() {
        Some(file_name) => folder.join(file_name),
        None => exact,
    }
}

/// The project's copy of the server config, with a step that makes it from the install's the
/// first time; a named template replaces the default `serverDZ.cfg`.
#[cfg(unix)]
fn project_config_text(
    args: &DiagStageArgs,
    project: &mut Vec<StageStep>,
) -> anyhow::Result<String> {
    let name = args.config.as_deref().unwrap_or(Path::new(SERVER_CONFIG));
    let project_cfg = project_config_path(&args.source_root.join("server"), name);
    if project_cfg.is_file() {
        return std::fs::read_to_string(&project_cfg)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", project_cfg.display()));
    }
    if args.config.is_some() {
        anyhow::bail!("server config {} is missing", project_cfg.display());
    }
    let install_cfg = args.server_dir.join(SERVER_CONFIG);
    let retail = std::fs::read(&install_cfg)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", install_cfg.display()))?;
    let text = String::from_utf8(retail.clone())
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", install_cfg.display()))?;
    project.push(StageStep::Write {
        path: project_cfg,
        bytes: retail.len(),
        contents: retail,
    });
    Ok(text)
}

/// Whether `link -> target` must be created: an identical link already there
/// or the target folder itself is kept (not ours to remove), any other occupant fails.
#[cfg(unix)]
fn plan_link(link: &Path, target: &Path, gone: &[PathBuf]) -> anyhow::Result<bool> {
    if absent(gone, link) {
        return Ok(true);
    }
    match std::fs::symlink_metadata(link) {
        Ok(md) if md.file_type().is_symlink() => {
            if std::fs::read_link(link).ok().as_deref() == Some(target) {
                Ok(false)
            } else {
                anyhow::bail!(
                    "{} already exists and does not point at {}; remove it to stage the diag run",
                    link.display(),
                    target.display()
                )
            }
        }
        Ok(_) if in_place(link, target) => Ok(false),
        Ok(_) => anyhow::bail!(
            "{} already exists and is not a symlink; remove it to stage the diag run",
            link.display()
        ),
        Err(_) => Ok(true),
    }
}

#[cfg(not(unix))]
pub(crate) fn plan_diag_stage(
    _args: &DiagStageArgs,
    _gone: &[PathBuf],
    _project: &mut Vec<StageStep>,
) -> anyhow::Result<(DiagStage, Vec<StageStep>, Vec<PathBuf>)> {
    anyhow::bail!("diag staging is only supported on unix hosts")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-diagstage-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_already_at_the_link_path_is_used_in_place_and_another_is_refused() {
        let game = tmp("in-place");
        let mod_dir = game.join("@A");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(mod_dir.join("marker.txt"), b"here").unwrap();
        assert!(!plan_link(&mod_dir, &mod_dir, &[]).unwrap());
        assert!(!plan_link(&mod_dir, &game.join(".").join("@A"), &[]).unwrap());

        let other = tmp("in-place-other");
        let err = plan_link(&mod_dir, &other, &[]).unwrap_err().to_string();
        assert!(err.contains("is not a symlink"), "{err}");
        assert!(mod_dir.join("marker.txt").is_file());
        let _ = std::fs::remove_dir_all(&game);
        let _ = std::fs::remove_dir_all(&other);
    }

    struct Fx {
        root: PathBuf,
        game: PathBuf,
        server: PathBuf,
        project_server: PathBuf,
        source: PathBuf,
        profiles: PathBuf,
        built_mod: PathBuf,
    }

    fn fixture(label: &str) -> Fx {
        let root = tmp(label);
        let game = root.join("game");
        let server = root.join("server");
        let source = root.join("source");
        let project_server = source.join("server");
        let profiles = root.join("profiles");
        let built_mod = root.join("build/@FieldKit");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(server.join("mpmissions")).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::create_dir_all(&built_mod).unwrap();
        std::fs::write(
            server.join("serverDZ.cfg"),
            "// head\nBattlEye = 1; // on\nallowfilepatching = 0;\nverifySignatures = 2;\n",
        )
        .unwrap();
        Fx {
            root,
            game,
            server,
            project_server,
            source,
            profiles,
            built_mod,
        }
    }

    fn args(fx: &Fx) -> DiagStageArgs {
        DiagStageArgs {
            game_dir: fx.game.clone(),
            server_dir: fx.server.clone(),
            config: None,
            built_mod: fx.built_mod.clone(),
            source_root: fx.source.clone(),
            mod_prefix: "FieldKit".into(),
            profiles_dir: fx.profiles.clone(),
            test_mods: Vec::new(),
        }
    }

    fn test_mod(root: &Path, name: &str, role: TestModRole) -> TestModArg {
        let path = root.join("testmods").join(name);
        std::fs::create_dir_all(&path).unwrap();
        TestModArg {
            path,
            link_name: name.to_string(),
            role,
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn relative(path: &Path) -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components() {
            rel.push("..");
        }
        rel.push(path.strip_prefix("/").unwrap());
        rel
    }

    /// A staged run: the stage's argv getters, and the guard that removes the staging on drop.
    #[derive(Debug)]
    struct Guarded {
        stage: DiagStage,
        _staged: modfoundry_platform::stage::Staged,
    }

    impl std::ops::Deref for Guarded {
        type Target = DiagStage;

        fn deref(&self) -> &DiagStage {
            &self.stage
        }
    }

    /// Stages through the same steps a run applies.
    fn stage_diag(args: &DiagStageArgs) -> anyhow::Result<Guarded> {
        let mut project = Vec::new();
        let (stage, steps, _) = plan_diag_stage(args, &[], &mut project)?;
        let staged = crate::diag_run::stage_steps(&args.game_dir, &project, &steps)?;
        Ok(Guarded {
            stage,
            _staged: staged,
        })
    }

    #[test]
    fn the_project_server_config_is_copied_once_then_used() {
        let fx = fixture("projectcfg");
        std::fs::write(fx.server.join("serverDZ.cfg"), "hostname = \"A\";\n").unwrap();
        let a = args(&fx);
        let project_cfg = fx.project_server.join("serverDZ.cfg");

        let stage = stage_diag(&a).unwrap();
        assert_eq!(
            std::fs::read_to_string(&project_cfg).unwrap(),
            "hostname = \"A\";\n"
        );
        let staged = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert!(staged.contains("hostname = \"A\";"), "{staged}");
        drop(stage);

        std::fs::write(&project_cfg, "hostname = \"B\";\n").unwrap();
        let stage = stage_diag(&a).unwrap();
        let staged = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert!(staged.contains("hostname = \"B\";"), "{staged}");
        assert_eq!(
            std::fs::read_to_string(fx.server.join("serverDZ.cfg")).unwrap(),
            "hostname = \"A\";\n"
        );
        drop(stage);
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_project_config_in_another_case_is_read_without_writing_one() {
        let fx = fixture("lowercfg");
        std::fs::create_dir_all(&fx.project_server).unwrap();
        std::fs::write(
            fx.project_server.join("serverdz.cfg"),
            "hostname = \"lower\";\n",
        )
        .unwrap();

        let mut project = Vec::new();
        let (_stage, _steps, _stale) = plan_diag_stage(&args(&fx), &[], &mut project).unwrap();

        assert!(
            project.is_empty(),
            "the config is already there, nothing to write: {project:?}"
        );
        let stage = stage_diag(&args(&fx)).unwrap();
        let staged = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert!(staged.contains("hostname = \"lower\";"), "{staged}");
        drop(stage);
        assert!(
            !fx.project_server.join("serverDZ.cfg").exists(),
            "the install's config must not be copied beside a different-case file"
        );
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_folder_named_like_the_config_is_passed_over_for_the_file() {
        let fx = fixture("cfgdir");
        std::fs::create_dir_all(fx.project_server.join("ServerDZ.cfg")).unwrap();
        std::fs::write(
            fx.project_server.join("serverdz.cfg"),
            "hostname = \"file\";\n",
        )
        .unwrap();

        let found = project_config_path(&fx.project_server, Path::new(SERVER_CONFIG));

        assert_eq!(found, fx.project_server.join("serverdz.cfg"));
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_named_config_in_another_case_is_used() {
        struct Case {
            label: &'static str,
            files: &'static [(&'static str, &'static str)],
            expected: &'static str,
        }
        let cases = [
            Case {
                label: "one variant",
                files: &[("serverdz_expansion.cfg", "hostname = \"only\";\n")],
                expected: "hostname = \"only\";",
            },
            Case {
                label: "two variants",
                files: &[
                    ("serverDZ_expansion.cfg", "hostname = \"first\";\n"),
                    ("serverdz_expansion.cfg", "hostname = \"second\";\n"),
                ],
                expected: "hostname = \"first\";",
            },
        ];
        for Case {
            label,
            files,
            expected,
        } in cases
        {
            let fx = fixture("namedcase");
            std::fs::create_dir_all(&fx.project_server).unwrap();
            for (file, text) in files {
                std::fs::write(fx.project_server.join(file), text).unwrap();
            }
            let mut a = args(&fx);
            a.config = Some(PathBuf::from("serverDZ_Expansion.cfg"));

            let stage = stage_diag(&a).unwrap();

            let staged = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
            assert!(staged.contains(expected), "{label}: {staged}");
            drop(stage);
            let _ = std::fs::remove_dir_all(&fx.root);
        }
    }

    #[test]
    fn a_named_config_is_the_template_the_patch_starts_from() {
        let fx = fixture("namedcfg");
        std::fs::create_dir_all(&fx.project_server).unwrap();
        std::fs::write(
            fx.project_server.join("serverDZ_expansion.cfg"),
            "hostname = \"expansion\";\n",
        )
        .unwrap();
        let mut a = args(&fx);
        a.config = Some(PathBuf::from("serverDZ_expansion.cfg"));

        let stage = stage_diag(&a).unwrap();

        let staged = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert!(staged.contains("hostname = \"expansion\";"), "{staged}");
        assert!(staged.contains("BattlEye = 0;"), "{staged}");
        assert!(
            !fx.project_server.join(SERVER_CONFIG).exists(),
            "the default template is not made"
        );
        drop(stage);

        a.config = Some(PathBuf::from("serverDZ_typo.cfg"));
        let Err(err) = stage_diag(&a) else {
            panic!("a missing named config is refused");
        };
        assert!(
            err.to_string().contains("serverDZ_typo.cfg is missing"),
            "{err}"
        );
        assert!(!fx.project_server.join("serverDZ_typo.cfg").exists());
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn relative_paths_are_staged_as_absolute_links() {
        let fx = fixture("relative");
        let mut a = args(&fx);
        a.built_mod = relative(&a.built_mod);
        a.source_root = relative(&a.source_root);
        a.profiles_dir = relative(&a.profiles_dir);

        let stage = stage_diag(&a).unwrap();

        for link in ["@FieldKit", "FieldKit", DIAG_PROFILES_LINK] {
            let target = std::fs::read_link(fx.game.join(link)).unwrap();
            assert!(target.is_absolute(), "{link}: relative target {target:?}");
            assert!(
                target.exists(),
                "{link}: target {target:?} does not resolve"
            );
        }

        drop(stage);
        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn patched_config_replaces_values_in_place_and_keeps_comments() {
        let src = "// head\nBattlEye = 1; // on\nallowfilepatching = 0;\nverifySignatures = 2;\n";
        let out = patched_server_config(src);
        assert_eq!(
            out,
            "// head\nBattlEye = 0; // on\nallowfilepatching = 1;\nverifySignatures = 0;\n"
        );
    }

    #[test]
    fn patched_config_appends_missing_keys() {
        let out = patched_server_config("hostname = \"x\";\n");
        assert_eq!(
            out,
            "hostname = \"x\";\nBattlEye = 0;\nallowFilePatching = 1;\nverifySignatures = 0;\n"
        );
    }

    #[test]
    fn patched_config_is_idempotent() {
        let once = patched_server_config("BattlEye = 1;\n");
        assert_eq!(patched_server_config(&once), once);
    }

    #[test]
    fn patched_config_preserves_crlf() {
        let out = patched_server_config("BattlEye = 1;\r\nhostname = \"x\";\r\n");
        assert_eq!(
            out,
            "BattlEye = 0;\r\nhostname = \"x\";\r\nallowFilePatching = 1;\r\nverifySignatures = 0;\r\n"
        );
        assert!(!out.contains("\n\n"), "no LF-only line ends: {out:?}");
    }

    #[test]
    fn patched_config_leaves_lookalike_keys_alone() {
        let out = patched_server_config("verifySignaturesX = 5;\nBattlEyeX = 3;\n");
        assert!(
            out.starts_with("verifySignaturesX = 5;\nBattlEyeX = 3;\n"),
            "{out:?}"
        );
        assert!(out.contains("BattlEyeX = 3;") && out.contains("verifySignaturesX = 5;"));
        assert_eq!(out.matches("BattlEye = 0;").count(), 1);
    }

    #[test]
    fn a_stale_config_and_profiles_link_from_a_crashed_run_are_replaced() {
        let fx = fixture("stale");
        std::fs::write(fx.game.join(DIAG_CONFIG_NAME), "stale").unwrap();
        let elsewhere = fx.root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, fx.game.join(DIAG_PROFILES_LINK)).unwrap();

        let stage = stage_diag(&args(&fx)).unwrap();

        let cfg = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert!(
            cfg.to_ascii_lowercase().contains("allowfilepatching = 1;"),
            "{cfg}"
        );
        assert_eq!(
            std::fs::read_link(fx.game.join(DIAG_PROFILES_LINK)).unwrap(),
            fx.profiles
        );
        drop(stage);
        assert!(!fx.game.join(DIAG_CONFIG_NAME).exists());
        assert!(fx.game.join(DIAG_PROFILES_LINK).symlink_metadata().is_err());
    }

    #[test]
    fn a_missing_profiles_dir_is_made_before_the_link() {
        let fx = fixture("noprofiles");
        let mut a = args(&fx);
        a.profiles_dir = fx.root.join("absent");

        let stage = stage_diag(&a).unwrap();

        assert!(
            a.profiles_dir.is_dir(),
            "the profiles folder must exist before the link points at it"
        );
        assert_eq!(
            std::fs::read_link(fx.game.join(DIAG_PROFILES_LINK))
                .unwrap()
                .canonicalize()
                .unwrap(),
            a.profiles_dir.canonicalize().unwrap()
        );
        drop(stage);
        assert!(
            a.profiles_dir.is_dir(),
            "the profiles folder outlives the run"
        );

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_real_directory_named_like_our_profiles_link_is_never_replaced() {
        let fx = fixture("realdir");
        std::fs::create_dir_all(fx.game.join(DIAG_PROFILES_LINK)).unwrap();
        assert!(stage_diag(&args(&fx)).is_err());
        assert!(fx.game.join(DIAG_PROFILES_LINK).is_dir());
    }

    #[test]
    fn two_artefacts_at_one_link_are_refused_before_anything_is_staged() {
        let fx = fixture("one-link-twice");
        let before = entries(&fx.game);
        let mut a = args(&fx);
        a.test_mods.push(TestModArg {
            path: fx.source.clone(),
            link_name: "FieldKit".into(),
            role: TestModRole::Both,
        });

        let err = plan_diag_stage(&a, &[], &mut Vec::new())
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("@FieldKit"), "{err}");
        assert_eq!(entries(&fx.game), before);
    }

    #[test]
    fn stage_creates_every_artefact_and_drop_removes_them() {
        let fx = fixture("stage");
        let before = entries(&fx.game);

        let stage = stage_diag(&args(&fx)).unwrap();
        assert_eq!(stage.config_name(), DIAG_CONFIG_NAME);
        assert_eq!(stage.profiles_arg(), DIAG_PROFILES_LINK);
        assert_eq!(stage.mod_args_both(), vec!["@FieldKit"]);
        assert!(stage.mod_args_server().is_empty());

        assert_eq!(
            std::fs::read_link(fx.game.join("mpmissions")).unwrap(),
            fx.server.join("mpmissions")
        );
        assert_eq!(
            std::fs::read_link(fx.game.join("FieldKit")).unwrap(),
            fx.source
        );
        assert_eq!(
            std::fs::read_link(fx.game.join("@FieldKit")).unwrap(),
            fx.built_mod
        );
        assert_eq!(
            std::fs::read_link(fx.game.join(DIAG_PROFILES_LINK)).unwrap(),
            fx.profiles
        );
        let cfg = std::fs::read_to_string(fx.game.join(DIAG_CONFIG_NAME)).unwrap();
        assert_eq!(
            cfg,
            "// head\nBattlEye = 0; // on\nallowfilepatching = 1;\nverifySignatures = 0;\n"
        );
        assert!(
            fx.server.join("serverDZ.cfg").is_file(),
            "the user's serverDZ.cfg must be untouched"
        );

        drop(stage);
        assert_eq!(
            entries(&fx.game),
            before,
            "the guard must remove every artefact"
        );
        assert!(fx.source.is_dir() && fx.profiles.is_dir());

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_user_mpmissions_dir_survives() {
        let fx = fixture("usermpp");
        std::fs::create_dir(fx.game.join("mpmissions")).unwrap();

        let stage = stage_diag(&args(&fx)).unwrap();
        assert!(
            !std::fs::symlink_metadata(fx.game.join("mpmissions"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "a pre-existing mpmissions is not staged"
        );

        drop(stage);
        assert!(fx.game.join("mpmissions").is_dir());
        assert_eq!(entries(&fx.game), vec!["mpmissions".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_conflicting_mod_prefix_dir_fails_and_creates_nothing() {
        let fx = fixture("conflict-dir");
        std::fs::create_dir(fx.game.join("FieldKit")).unwrap();

        let err = stage_diag(&args(&fx)).unwrap_err().to_string();
        assert!(err.contains("FieldKit"), "unhelpful error: {err}");
        assert_eq!(entries(&fx.game), vec!["FieldKit".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn bad_prefixes_and_link_names_are_refused_before_staging() {
        let fx = fixture("bad-paths");
        let before = entries(&fx.game);
        for prefix in ["/abs", "../up", "a/../b", ""] {
            let mut a = args(&fx);
            a.mod_prefix = prefix.into();
            let err = stage_diag(&a).unwrap_err().to_string();
            assert!(err.contains("relative PBO prefix"), "{prefix:?}: {err}");
            assert_eq!(entries(&fx.game), before, "{prefix:?} staged something");
        }
        for name in ["", "@", "@@x", "a/b", "a\\b", "a;@b", ".."] {
            let mut a = args(&fx);
            a.test_mods = vec![TestModArg {
                path: fx.source.clone(),
                link_name: name.into(),
                role: TestModRole::Both,
            }];
            let err = stage_diag(&a).unwrap_err().to_string();
            assert!(err.contains("bare folder name"), "{name:?}: {err}");
            assert_eq!(entries(&fx.game), before, "{name:?} staged something");
        }

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_nested_prefix_is_staged_and_removed() {
        let fx = fixture("nested");
        let mut a = args(&fx);
        a.mod_prefix = "JM/CF".into();

        let stage = stage_diag(&a).unwrap();
        assert_eq!(
            std::fs::read_link(fx.game.join("JM/CF")).unwrap(),
            fx.source
        );
        drop(stage);
        assert!(fx.game.join("JM/CF").symlink_metadata().is_err());
        assert!(
            !fx.game.join("JM").exists(),
            "the made parent folder stayed"
        );

        std::fs::create_dir(fx.game.join("JM")).unwrap();
        let stage = stage_diag(&a).unwrap();
        assert_eq!(
            std::fs::read_link(fx.game.join("JM/CF")).unwrap(),
            fx.source
        );
        drop(stage);
        assert!(fx.game.join("JM/CF").symlink_metadata().is_err());
        assert!(fx.game.join("JM").is_dir(), "a user folder was removed");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_mod_prefix_link_to_somewhere_else_fails_and_creates_nothing() {
        let fx = fixture("conflict-link");
        let other = tmp("other");
        std::os::unix::fs::symlink(&other, fx.game.join("FieldKit")).unwrap();

        let err = stage_diag(&args(&fx)).unwrap_err().to_string();
        assert!(err.contains("FieldKit"), "unhelpful error: {err}");
        assert_eq!(std::fs::read_link(fx.game.join("FieldKit")).unwrap(), other);
        assert_eq!(entries(&fx.game), vec!["FieldKit".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn an_existing_identical_mod_link_is_kept_after_drop() {
        let fx = fixture("reuse");
        let link = fx.game.join("@FieldKit");
        std::os::unix::fs::symlink(&fx.built_mod, &link).unwrap();

        let stage = stage_diag(&args(&fx)).unwrap();
        drop(stage);

        assert_eq!(std::fs::read_link(&link).unwrap(), fx.built_mod);
        assert_eq!(entries(&fx.game), vec!["@FieldKit".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn test_mod_links_are_staged_and_removed_on_drop() {
        let fx = fixture("testmods");
        let before = entries(&fx.game);
        let a = test_mod(&fx.root, "a", TestModRole::Both);
        let b = test_mod(&fx.root, "b", TestModRole::Both);
        let s = test_mod(&fx.root, "s", TestModRole::ServerOnly);
        let at = test_mod(&fx.root, "@at", TestModRole::Both);
        let mut args = args(&fx);
        args.test_mods = vec![a.clone(), b.clone(), s.clone(), at.clone()];

        let stage = stage_diag(&args).unwrap();

        assert_eq!(std::fs::read_link(fx.game.join("@a")).unwrap(), a.path);
        assert_eq!(std::fs::read_link(fx.game.join("@b")).unwrap(), b.path);
        assert_eq!(std::fs::read_link(fx.game.join("@s")).unwrap(), s.path);
        assert_eq!(std::fs::read_link(fx.game.join("@at")).unwrap(), at.path);
        assert_eq!(stage.mod_args_both(), vec!["@a", "@b", "@at", "@FieldKit"]);
        assert_eq!(stage.mod_args_server(), vec!["@s"]);

        drop(stage);
        assert_eq!(entries(&fx.game), before, "test-mod links must be removed");

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_conflicting_test_mod_link_fails_and_creates_nothing() {
        let fx = fixture("testmod-conflict");
        let a = test_mod(&fx.root, "a", TestModRole::Both);
        std::fs::create_dir(fx.game.join("@a")).unwrap();
        let mut args = args(&fx);
        args.test_mods = vec![a];

        let err = stage_diag(&args).unwrap_err().to_string();
        assert!(err.contains("@a"), "unhelpful error: {err}");
        assert_eq!(entries(&fx.game), vec!["@a".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn a_missing_test_mod_dir_fails_and_creates_nothing() {
        let fx = fixture("testmod-missing");
        let missing = fx.root.join("testmods/absent");
        let mut args = args(&fx);
        args.test_mods = vec![TestModArg {
            path: missing.clone(),
            link_name: "absent".into(),
            role: TestModRole::Both,
        }];

        let err = stage_diag(&args).unwrap_err().to_string();
        assert!(err.contains(&missing.display().to_string()), "{err}");
        assert!(entries(&fx.game).is_empty());

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn duplicate_test_mod_link_names_fail_and_create_nothing() {
        let fx = fixture("testmod-dup");
        let a = test_mod(&fx.root, "Foo", TestModRole::Both);
        let other = fx.root.join("testmods/other");
        std::fs::create_dir_all(&other).unwrap();
        let mut args = args(&fx);
        args.test_mods = vec![
            a,
            TestModArg {
                path: other,
                link_name: "@Foo".into(),
                role: TestModRole::ServerOnly,
            },
        ];

        let err = stage_diag(&args).unwrap_err().to_string();
        assert!(err.contains("duplicate"), "unhelpful error: {err}");
        assert!(err.contains("@Foo"), "unhelpful error: {err}");
        assert!(entries(&fx.game).is_empty());

        let _ = std::fs::remove_dir_all(&fx.root);
    }

    #[test]
    fn an_existing_identical_test_mod_link_is_kept() {
        let fx = fixture("testmod-reuse");
        let a = test_mod(&fx.root, "a", TestModRole::Both);
        let path = a.path.clone();
        std::os::unix::fs::symlink(&path, fx.game.join("@a")).unwrap();
        let mut args = args(&fx);
        args.test_mods = vec![a];

        let stage = stage_diag(&args).unwrap();
        drop(stage);

        assert_eq!(std::fs::read_link(fx.game.join("@a")).unwrap(), path);
        assert_eq!(entries(&fx.game), vec!["@a".to_string()]);

        let _ = std::fs::remove_dir_all(&fx.root);
    }
}
