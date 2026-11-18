//! Shared frontend command bodies.
//!
//! The same read/validate/lint/scaffold/build/launch operations power the CLI,
//! MCP and future automation — no duplicated implementations per interface.
//! Logic lives here; each frontend is a thin binder. [`registry::core`] is the
//! single dispatch surface: frontends enumerate it (menus, MCP tool lists) and
//! dispatch through it, so the effect gate and the shared outcome shape apply
//! to every path. Content writes go through the VFS write API
//! (`modfoundry_vfs::WriteTxn`) and report the change-set that undoes
//! them.

pub mod build_ops;
pub mod ce_classes;
pub mod ce_ops;
pub mod changelog;
pub mod config_host;
pub mod conflict_check;
pub mod conflict_data;
mod conflict_script;
pub mod diag_run;
pub mod diag_stage;
pub mod economy_host;
pub mod economy_load;
pub mod font_ops;
pub mod game_diff;
pub mod game_side;
pub mod game_snapshot;
pub mod image_ops;
pub mod imageset_ops;
pub mod known_errors;
mod latest;
pub mod launch_plan;
pub mod launch_sets;
pub mod layout_check;
pub mod layout_render;
mod par;
pub(crate) mod paths;
pub mod pbo_ops;
pub(crate) mod prefix_marker;
pub mod project_deps;
pub mod project_settings;
pub mod publish_ops;
pub mod publish_upload;
pub mod registry;
pub mod run_ops;
mod run_record;
mod script_bindings;
pub mod script_check;
pub mod script_host;
pub mod script_index;
pub mod script_lint;
pub mod script_outline;
pub mod search_index;
pub mod search_query;
mod settings_fill;
mod shared_deps;
mod sources;
pub mod unpack_cache;
pub mod update;
pub mod vfs_ops;
mod walk;
pub mod workshop_api;

pub use build_ops::{
    build_contents, build_project, default_name, keygen, plan_build_project, plan_keygen, BuildArgs,
};
pub use diag_run::{ready_timeout_message, run_diag, DiagOutcome, RunDiagArgs};
pub use modfoundry_build::BuildReport;
pub use modfoundry_config::ScaffoldOptions;
pub use registry::{
    core, register_all, ConvertArgs, ErrorArgs, FindingReport, HostReport, KeygenArgs, PathArgs,
    RunClientCommandArgs, ScaffoldArgs, SteamLibraryReport,
};
pub use run_ops::{
    errors_from_dir, resolve_game_dir, resolve_server_exe, run_client, run_profile, run_server,
    ClientRunOutcome, RunClientArgs, RunControl, RunOutcome, RunServerArgs, CLIENT_SETTLE_SECS,
};
pub use run_record::STALE_AFTER_SECS;
pub use script_host::ScriptHost;

use anyhow::Result;
use modfoundry_config::{mod_skeleton, Diagnostic};
use modfoundry_enforce::Finding;
use modfoundry_vfs::write::WriteTxn;
use modfoundry_vfs::{root_name, ChangeSet};
use std::path::{Path, PathBuf};

/// The `config.cpp` a config path names: the path itself, or the one inside it
/// when the path is a directory.
pub fn config_file(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("config.cpp")
    } else {
        path.to_path_buf()
    }
}

/// The config text at `target` plus its diagnostics. `target` may be a
/// `config.cpp` file or a directory containing one; the directory is the source
/// root for module-path existence checks. Empty diagnostics = clean.
pub fn validate_file(target: &Path) -> Result<(String, Vec<Diagnostic>)> {
    let cfg_path = config_file(target);
    if !cfg_path.is_file() {
        anyhow::bail!(
            "{} is not a config.cpp or a directory containing config.cpp",
            target.display()
        );
    }
    let text = std::fs::read_to_string(&cfg_path)?;
    let source_root = cfg_path.parent();
    let diagnostics = modfoundry_config::validate_text(&text, source_root).diagnostics;
    Ok((text, diagnostics))
}

/// Lint Enforce Script sources under `target` (a `.c` file or a directory
/// tree). Returns per-file findings; empty = clean.
pub fn lint_target(target: &Path) -> Result<Vec<Finding>> {
    Ok(script_bindings::drop_bound(
        modfoundry_enforce::lint_target(target)?,
    ))
}

/// [`lint_target`] with `extra` run on every linted file's parse.
pub fn lint_target_with(
    target: &Path,
    extra: &mut dyn FnMut(&Path, &modfoundry_enforce::Tree, &str) -> Vec<Diagnostic>,
) -> Result<Vec<Finding>> {
    Ok(modfoundry_enforce::lint_target_with(target, extra)?)
}

/// Why `project.scaffold` refuses a target folder that holds entries.
pub const SCAFFOLD_TARGET_NOT_EMPTY: &str = "already exists and is not empty";

/// How an index warning says no DayZ installation was found.
pub const NO_GAME_INSTALL: &str = "no DayZ installation found";

/// Scaffold a new mod from `options` into `dir`: the number of files written,
/// plus the change-set that undoes it. `dir` must not already exist with
/// content. The skeleton validates clean with [`validate_file`].
pub fn scaffold(dir: &Path, options: &ScaffoldOptions) -> Result<(usize, ChangeSet)> {
    let mut txn = WriteTxn::new(format!("mod:{}", options.name), dir);
    let files = scaffold_into(&mut txn, dir, options)?;
    Ok((files, txn.finish()?))
}

/// The same scaffold as a dry run: the change-set it would apply, nothing
/// written.
pub fn plan_scaffold(dir: &Path, options: &ScaffoldOptions) -> Result<ChangeSet> {
    let mut txn = WriteTxn::dry_run(format!("mod:{}", options.name), dir);
    scaffold_into(&mut txn, dir, options)?;
    Ok(txn.finish()?)
}

fn scaffold_into(txn: &mut WriteTxn, dir: &Path, options: &ScaffoldOptions) -> Result<usize> {
    let skeleton = mod_skeleton(options).map_err(anyhow::Error::msg)?;
    if dir.is_file() {
        anyhow::bail!("{} is a file, not a folder.", dir.display());
    }
    if dir.is_dir() && dir.read_dir()?.next().is_some() {
        anyhow::bail!("{} {SCAFFOLD_TARGET_NOT_EMPTY}.", dir.display());
    }
    let count = skeleton.len();
    for file in skeleton {
        txn.write(&file.path, file.contents)?;
    }
    Ok(count)
}

/// Read `path` as UTF-8 text (the MCP `vfs.read` body).
pub fn read_text(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

/// Binarize a `config.cpp` into RaP `config.bin` bytes (CfgConvert `-bin`
/// parity). Refuses to binarize a file the parser reported issues for, since a
/// half-understood config would silently become a wrong `config.bin`.
pub fn binarize_config(path: &Path) -> Result<Vec<u8>> {
    let text = std::fs::read_to_string(path)?;
    let parsed = modfoundry_config::parser::parse(&text);
    if let Some(first) = parsed.issues.first() {
        anyhow::bail!(
            "{} has {} parse issue(s); first: {} — fix with `mf validate` before binarizing",
            path.display(),
            parsed.issues.len(),
            first.message
        );
    }
    Ok(modfoundry_config::write_rap(&parsed.file)?)
}

/// Convert RaP `config.bin` bytes to CfgConvert-compatible text.
pub fn derapify_config(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let file = modfoundry_config::read_rap(&bytes)?;
    Ok(modfoundry_config::to_text(&file))
}

/// The output path of a conversion: `out`, else `config.bin` beside the input.
fn converted_path(input: &Path, out: Option<&Path>) -> std::path::PathBuf {
    out.map(Path::to_path_buf)
        .unwrap_or_else(|| input.with_file_name("config.bin"))
}

/// A transaction over the directory a conversion writes its output into.
pub(crate) fn output_txn(output: &Path, recording: bool) -> WriteTxn {
    dir_txn(output_dir(output), recording)
}

/// The folder an output file is written into.
pub(crate) fn output_dir(output: &Path) -> &Path {
    output
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// A transaction rooted at `dir`.
pub(crate) fn dir_txn(dir: &Path, recording: bool) -> WriteTxn {
    // Undo replays a change where it happened, so a relative folder must not depend on the caller's directory.
    let dir = &std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let root = root_name("mod", dir);
    if recording {
        WriteTxn::new(root, dir)
    } else {
        WriteTxn::dry_run(root, dir)
    }
}

/// The single-path write of both converters: the file's name inside its
/// directory.
pub(crate) fn file_rel(path: &Path) -> Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} has no file name to write", path.display()))
}

/// Binarize `path` and write the result to `out` (default `config.bin` beside
/// the input). Returns the file written, its size, and the change-set that
/// undoes it.
pub fn binarize_to(
    path: &Path,
    out: Option<&Path>,
) -> Result<(std::path::PathBuf, usize, ChangeSet)> {
    let output = converted_path(path, out);
    let mut txn = output_txn(&output, true);
    let size = binarize_into(&mut txn, path, &output)?;
    Ok((output, size, txn.finish()?))
}

/// The same binarize as a dry run: the change-set it would apply. The config is
/// still parsed, so a preview surfaces the parse issues an apply would refuse.
pub fn plan_binarize(path: &Path, out: Option<&Path>) -> Result<ChangeSet> {
    let output = converted_path(path, out);
    let mut txn = output_txn(&output, false);
    binarize_into(&mut txn, path, &output)?;
    Ok(txn.finish()?)
}

fn binarize_into(txn: &mut WriteTxn, path: &Path, output: &Path) -> Result<usize> {
    let bytes = binarize_config(path)?;
    let size = bytes.len();
    txn.write(&file_rel(output)?, bytes)?;
    Ok(size)
}

/// Convert a RaP `config.bin` to text and write it to `out`, returning the
/// change-set that undoes it.
pub fn derapify_to(path: &Path, out: &Path) -> Result<ChangeSet> {
    let mut txn = output_txn(out, true);
    derapify_into(&mut txn, path, out)?;
    Ok(txn.finish()?)
}

/// The same conversion as a dry run.
pub fn plan_derapify(path: &Path, out: &Path) -> Result<ChangeSet> {
    let mut txn = output_txn(out, false);
    derapify_into(&mut txn, path, out)?;
    Ok(txn.finish()?)
}

fn derapify_into(txn: &mut WriteTxn, path: &Path, output: &Path) -> Result<()> {
    let text = derapify_config(path)?;
    txn.write(&file_rel(output)?, text.into_bytes())?;
    Ok(())
}

/// Replace the text of `target` with `text`. Returns the file written, its size
/// and the change-set that undoes it. The caller resolves which file that is
/// (`text.edit`'s optional `out`), so a preview and an apply cannot disagree
/// about the destination.
///
/// `expected_sha256` is the digest of the revision the caller read: when it is
/// given the write only lands while the file still holds it, so a session that
/// planned against stale content is refused (`VfsError::Stale`) instead of
/// clobbering somebody else's edit.
pub fn edit_text_to(
    target: &Path,
    text: &str,
    expected_sha256: Option<&str>,
) -> Result<(PathBuf, usize, ChangeSet)> {
    let mut txn = output_txn(target, true);
    let size = edit_text_into(&mut txn, target, text, expected_sha256)?;
    Ok((target.to_path_buf(), size, txn.finish()?))
}

/// The same edit as a dry run: the change-set it would apply. The base-revision
/// check runs here too, so a preview of a stale edit reports the staleness an
/// apply would refuse on.
pub fn plan_edit_text(
    target: &Path,
    text: &str,
    expected_sha256: Option<&str>,
) -> Result<ChangeSet> {
    let mut txn = output_txn(target, false);
    edit_text_into(&mut txn, target, text, expected_sha256)?;
    Ok(txn.finish()?)
}

fn edit_text_into(
    txn: &mut WriteTxn,
    target: &Path,
    text: &str,
    expected_sha256: Option<&str>,
) -> Result<usize> {
    let bytes = text.as_bytes().to_vec();
    let size = bytes.len();
    let rel = file_rel(target)?;
    match expected_sha256 {
        Some(expected) => txn.write_checked(rel, bytes, expected)?,
        None => txn.write(rel, bytes)?,
    }
    Ok(size)
}
