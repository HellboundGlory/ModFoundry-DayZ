//! Deterministic `mf build`: snapshot a content tree, pack it into a PBO, stage
//! the `@<name>/{addons,keys,mod.cpp}` layout, and (optionally) sign it.
//!
//! The staged artifacts pass `DSCheckSignatures`: signing is native by default
//! (`modfoundry-sign::native`) with the DsUtils adapter as an opt-in (`Signing::tools_dir`).
//! The plaintext PBO (`config.cpp` + `.c` scripts) is directly engine-loadable as packed;
//! `BuildOptions::binarize_config` replaces the packed `config.cpp` with a RaP
//! `config.bin` (release parity, `modfoundry-config::rap`). `pack_addon` also
//! enforces that declared `CfgMods.files[]` repeat the effective PBO prefix —
//! otherwise the engine binds nothing.

use crate::image_rules;
use crate::manifest::BuildReport;
use crate::pack_rules;
use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
use modfoundry_sign::{bisign_name, public_key_name, DsUtils, SignVersion};
use modfoundry_texture::convert::{convert_png, Target};
use modfoundry_vfs::write::WriteTxn;
use modfoundry_vfs::{root_name, ChangeSet};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// `mod.cpp` presentation metadata (launcher/main-menu only; not load-bearing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModMeta {
    pub name: String,
    pub author: String,
    pub version: String,
}

/// Signing inputs. The private + public key are produced once by `mf keygen`
/// and reused across builds; only the public `.bikey` is staged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signing {
    pub authority: String,
    pub private_key: PathBuf,
    pub public_key: PathBuf,
    /// When set, drive the official `DSSignFile` from this DsUtils dir
    /// (Wine/native Windows) instead of the pure-Rust signer.
    pub tools_dir: Option<PathBuf>,
}

/// Everything `build_mod` needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOptions {
    /// Mod name; becomes `@<name>`, the PBO stem, and the default prefix.
    pub name: String,
    /// Directory whose tree becomes the addon's PBO content.
    pub source_root: PathBuf,
    /// Staged output dir (`builds/<profile>/@<name>`).
    pub output_dir: PathBuf,
    /// Optional `prefix` PBO property override (defaults to `name`).
    pub prefix: Option<String>,
    /// Fixed unix timestamp for every PBO entry (determinism).
    pub timestamp: u32,
    pub mod_cpp: ModMeta,
    pub sign: Option<Signing>,
    /// Replace a packed `config.cpp` with its RaP `config.bin` (release parity).
    pub binarize_config: bool,
    /// Remove `//` and `/* */` comments from the packed `.c` files.
    pub strip_comments: bool,
}

/// Root-level dirs never packed; the same names below the root are content.
const EXCLUDED: &[&str] = &["keys", "builds", "target", "workshop"];

/// The launch setup ModFoundry keeps in the project's root `server/` folder; the
/// folder's other files are the mod's own content and still pack.
fn is_server_setup(root: &Path, dir: &Path, name: &str) -> bool {
    dir.parent() == Some(root)
        && dir
            .file_name()
            .is_some_and(|folder| folder.eq_ignore_ascii_case("server"))
        && (name.eq_ignore_ascii_case("serverDZ.cfg") || name.eq_ignore_ascii_case("profiles"))
}

const PRIVATE_KEY_EXT: &str = "biprivatekey";

fn is_excluded(name: &str, at_root: bool) -> bool {
    // A private key never leaves the machine; dotfiles are VCS or editor state.
    // The remaining names belong to the `@Mod` root and are ordinary content below it.
    name.starts_with('.')
        || has_extension(name, &[PRIVATE_KEY_EXT])
        || (at_root
            && (name.eq_ignore_ascii_case("mod.cpp")
                || name.eq_ignore_ascii_case("meta.cpp")
                || EXCLUDED
                    .iter()
                    .any(|excluded| name.eq_ignore_ascii_case(excluded))))
}

/// A link with an ordinary name can still point at a private key.
fn links_to_a_private_key(path: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|_| {
        path.canonicalize().is_ok_and(|target| {
            target
                .file_name()
                .is_some_and(|name| has_extension(&name.to_string_lossy(), &[PRIVATE_KEY_EXT]))
        })
    })
}

pub(crate) fn has_extension(name: &str, exts: &[&str]) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| exts.iter().any(|want| ext.eq_ignore_ascii_case(want)))
}

/// The `addons` folder's real name in `output_dir`, matched in any case.
fn addons_dir(output_dir: &Path) -> Option<String> {
    std::fs::read_dir(output_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|ent| ent.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|ent| ent.file_name().to_string_lossy().into_owned())
        .find(|name| name.eq_ignore_ascii_case("addons"))
}

/// Removes the signatures of `pbo_name` an earlier build left, except the one
/// this build signs natively: a stale `.bisign` beside a new PBO gets a client
/// kicked. Other PBOs and keys in the folder are left alone; it may be shared.
fn remove_stale_signatures(
    opts: &BuildOptions,
    txn: &mut WriteTxn,
    pbo_name: &str,
    native_bisign: bool,
) -> anyhow::Result<()> {
    let Some(dir) = addons_dir(&opts.output_dir) else {
        return Ok(());
    };
    let kept = opts
        .sign
        .as_ref()
        .filter(|_| native_bisign)
        .map(|signing| bisign_name(pbo_name, &signing.authority));
    let prefix = format!("{}.", pbo_name.to_ascii_lowercase());
    for ent in std::fs::read_dir(opts.output_dir.join(&dir))? {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let stale = ent.file_type()?.is_file()
            && lower.starts_with(&prefix)
            && has_extension(&name, &["bisign"])
            && kept
                .as_deref()
                .is_none_or(|kept| !kept.eq_ignore_ascii_case(&name));
        if stale {
            txn.remove(format!("{dir}/{name}"))?;
        }
    }
    Ok(())
}

/// Removes the PBOs and signatures in `addons/` this build did not write, and
/// returns them as `addons/<name>`. Publishing sends the whole folder, so
/// anything else left there would ship.
pub fn remove_unbuilt_addons(report: &BuildReport) -> anyhow::Result<Vec<String>> {
    let Some(dir) = addons_dir(&report.output_dir) else {
        return Ok(Vec::new());
    };
    let mut txn = WriteTxn::unrecorded(root_name("stage", &report.output_dir), &report.output_dir);
    let mut removed = Vec::new();
    for ent in std::fs::read_dir(report.output_dir.join(&dir))? {
        let ent = ent?;
        // A linked PBO ships like a real one, so only folders are skipped.
        if ent.file_type()?.is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        let unbuilt = (has_extension(&name, &["pbo"]) && !is_built_pbo(&name, &report.addons))
            || (has_extension(&name, &["bisign"])
                && !belongs_to_a_built_pbo(&name, &report.addons));
        if unbuilt {
            let path = format!("{dir}/{name}");
            txn.remove(&path)?;
            removed.push(path);
        }
    }
    txn.finish()?;
    removed.sort();
    Ok(removed)
}

fn is_built_pbo(pbo: &str, addons: &[String]) -> bool {
    addons.iter().any(|built| built.eq_ignore_ascii_case(pbo))
}

fn belongs_to_a_built_pbo(bisign: &str, addons: &[String]) -> bool {
    addons.iter().any(|pbo| {
        bisign
            .get(..pbo.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(pbo))
            && bisign[pbo.len()..].starts_with('.')
    })
}

/// A file under a source tree, with its `/`-separated path and its pack decision.
#[derive(Debug, Clone, Serialize)]
pub struct ContentFile {
    pub path: String,
    pub size: u64,
    pub decision: pack_rules::Decision,
    pub why: String,
}

/// The files a build of a source tree considers, and warnings about what it would pack.
#[derive(Debug, Clone, Serialize)]
pub struct Contents {
    pub files: Vec<ContentFile>,
    pub warnings: Vec<String>,
}

/// Every file under `root` in sorted order with its pack decision: never [`EXCLUDED`],
/// private keys, the server setup or `output_dir`. With `binarize_config` the root
/// `config.cpp` is listed as the `config.bin` the build packs.
pub fn contents(root: &Path, output_dir: &Path, binarize_config: bool) -> anyhow::Result<Contents> {
    let src = root.canonicalize()?;
    let skip = output_dir
        .canonicalize()
        .ok()
        .filter(|out| out.starts_with(&src));
    if skip.as_ref() == Some(&src) {
        anyhow::bail!(
            "the output folder {} is the source folder; build into another folder",
            output_dir.display()
        );
    }
    let mut names = BTreeSet::new();
    collect_rec(root, root, skip.as_deref(), &mut Vec::new(), &mut names)?;
    let rules = pack_rules::load(root)?;
    let images = image_rules::load(root)?;
    let files = names
        .into_iter()
        .map(|rel| -> anyhow::Result<ContentFile> {
            let path = rel.replace('\\', "/");
            let rule = rules.decide(&path);
            let rule_text = match &rule.reason {
                pack_rules::Reason::Rule { .. } => Some(rule.reason.to_string()),
                _ => None,
            };
            let decision = image_decision(rule, &images, &path);
            let decision = binarized(decision, &path, binarize_config);
            let why = match (rule_text, &decision.reason) {
                (Some(rule), pack_rules::Reason::Converted { .. }) => {
                    format!("{}; {rule}", decision.reason)
                }
                _ => decision.reason.to_string(),
            };
            let size = std::fs::metadata(root.join(&path))?.len();
            Ok(ContentFile {
                why,
                path,
                size,
                decision,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let warnings = content_warnings(&files);
    Ok(Contents { files, warnings })
}

fn image_decision(
    decision: pack_rules::Decision,
    images: &image_rules::ImageRules,
    path: &str,
) -> pack_rules::Decision {
    match images.target(path) {
        Some((line, target)) if decision.included => pack_rules::Decision {
            included: true,
            reason: pack_rules::Reason::Converted {
                line,
                to: Path::new(path)
                    .with_extension(target.extension())
                    .to_string_lossy()
                    .into_owned(),
            },
        },
        _ => decision,
    }
}

fn is_root_config(path: &str) -> bool {
    !path.contains(['/', '\\']) && path.eq_ignore_ascii_case("config.cpp")
}

/// The root `config.cpp` a binarized build renames to `config.bin`.
fn binarized(
    decision: pack_rules::Decision,
    path: &str,
    binarize_config: bool,
) -> pack_rules::Decision {
    if binarize_config && is_root_config(path) && decision.included {
        return pack_rules::Decision {
            included: true,
            reason: pack_rules::Reason::Binarized,
        };
    }
    decision
}

fn content_warnings(files: &[ContentFile]) -> Vec<String> {
    let mut warnings = Vec::new();
    let configs: Vec<&str> = files
        .iter()
        .filter(|file| file.decision.included && is_config_file(&file.path))
        .map(|file| file.path.as_str())
        .collect();
    if configs.len() > 1 && !binarized_config_clash(files) {
        warnings.push(format!(
            "{} config files will be packed and the game loads every one: {}",
            configs.len(),
            configs.join(", ")
        ));
    }
    for file in files.iter().filter(|file| file.decision.included) {
        let pack_rules::Reason::Rule { line, .. } = &file.decision.reason else {
            continue;
        };
        if has_extension(&file.path, pack_rules::GAME_EXTENSIONS) {
            continue;
        }
        let kind = match Path::new(&file.path).extension() {
            Some(ext) => format!(".{} files", ext.to_string_lossy()),
            None => "files without an extension".to_owned(),
        };
        warnings.push(format!(
            "{} is packed by .pboignore line {line} but the game does not load {kind}",
            file.path
        ));
    }
    warnings.extend(name_clashes(files));
    warnings
}

fn is_config_file(path: &str) -> bool {
    Path::new(path).file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case("config.cpp") || name.eq_ignore_ascii_case("config.bin")
    })
}

/// A binarized root `config.cpp` beside a root `config.bin` packs nothing: the
/// clash is the only thing to report.
fn binarized_config_clash(files: &[ContentFile]) -> bool {
    let included = || files.iter().filter(|file| file.decision.included);
    let binarized = included().any(|file| {
        matches!(file.decision.reason, pack_rules::Reason::Binarized) && is_root_config(&file.path)
    });
    binarized
        && included().any(|file| {
            !file.path.contains(['/', '\\']) && file.path.eq_ignore_ascii_case("config.bin")
        })
}

/// The entries a build packs, with the content warnings its file listing gave.
struct Content {
    entries: Vec<(String, Vec<u8>)>,
    warnings: Vec<String>,
}

fn collect_content(
    root: &Path,
    output_dir: &Path,
    binarize_config: bool,
) -> anyhow::Result<Content> {
    let listing = contents(root, output_dir, binarize_config)?;
    if let Some(clash) = name_clashes(&listing.files).into_iter().next() {
        anyhow::bail!("{clash}");
    }
    let entries = listing
        .files
        .into_iter()
        .filter(|file| file.decision.included)
        .map(|file| -> anyhow::Result<(String, Vec<u8>)> {
            let bytes = std::fs::read(root.join(&file.path))?;
            Ok(match &file.decision.reason {
                pack_rules::Reason::Converted { to, .. } => (
                    to.replace('/', "\\"),
                    convert_png(&bytes, target_of(to))
                        .map_err(|err| anyhow::anyhow!("{}: {err}", file.path))?,
                ),
                _ => (file.path.replace('/', "\\"), bytes),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Content {
        entries,
        warnings: listing.warnings,
    })
}

/// The conversion a packed name implies, so packing follows the decision the listing showed.
fn target_of(packed: &str) -> Target {
    if has_extension(packed, &["paa"]) {
        Target::Paa(None)
    } else {
        Target::Edds
    }
}

fn packed_path(file: &ContentFile) -> &str {
    match &file.decision.reason {
        pack_rules::Reason::Converted { to, .. } => to,
        pack_rules::Reason::Binarized => "config.bin",
        _ => &file.path,
    }
}

fn name_clashes(files: &[ContentFile]) -> Vec<String> {
    let mut seen: HashMap<String, &ContentFile> = HashMap::new();
    let mut clashes = Vec::new();
    for file in files.iter().filter(|file| file.decision.included) {
        let key = packed_path(file).to_ascii_lowercase();
        if let Some(first) = seen.get(&key) {
            if is_renamed(first) || is_renamed(file) {
                clashes.push(clash_warning(first, file));
            }
        } else {
            seen.insert(key, file);
        }
    }
    clashes
}

fn is_renamed(file: &ContentFile) -> bool {
    matches!(
        file.decision.reason,
        pack_rules::Reason::Converted { .. } | pack_rules::Reason::Binarized
    )
}

fn clash_warning(first: &ContentFile, later: &ContentFile) -> String {
    let (renamed, other) = if is_renamed(later) {
        (later, first)
    } else {
        (first, later)
    };
    format!(
        "{} and {} would both be packed as {}",
        renamed.path,
        other.path,
        packed_path(renamed)
    )
}

fn collect_rec(
    root: &Path,
    dir: &Path,
    skip: Option<&Path>,
    ancestors: &mut Vec<PathBuf>,
    out: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    let here = dir.canonicalize()?;
    if skip == Some(here.as_path()) || ancestors.contains(&here) {
        return Ok(());
    }
    ancestors.push(here);
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let path = ent.path();
        if is_excluded(&name, dir == root) || is_server_setup(root, dir, &name) {
            continue;
        }
        if path.is_dir() {
            collect_rec(root, &path, skip, ancestors, out)?;
        } else if path.is_file() {
            if links_to_a_private_key(&path) {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map_err(|_| anyhow::anyhow!("path outside root"))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\\");
            out.insert(rel);
        }
    }
    ancestors.pop();
    Ok(())
}

/// The `@Mod`-root file under `root`, by exact name and then by any case: mod
/// trees are authored on case-insensitive systems too.
pub(crate) fn source_root_file(root: &Path, name: &str) -> Option<PathBuf> {
    let exact = root.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|ent| ent.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
}

/// Render `@<name>/mod.cpp` for a tree that has none of its own.
fn render_mod_cpp(meta: &ModMeta) -> String {
    fn esc(s: &str) -> String {
        s.replace('\\', "").replace('"', "\\\"")
    }
    format!(
        "name = \"{}\";\nauthor = \"{}\";\nversion = \"{}\";\n",
        esc(&meta.name),
        esc(&meta.author),
        esc(&meta.version)
    )
}

/// Pack the source tree into one plaintext PBO. Returns the PBO bytes and the listing's warnings.
fn pack_addon(opts: &BuildOptions, prefix: &str) -> anyhow::Result<(Vec<u8>, Vec<String>)> {
    let content = collect_content(&opts.source_root, &opts.output_dir, opts.binarize_config)?;
    let files = content.entries;
    check_files_bind_prefix(&files, prefix)?;
    let files = if opts.strip_comments {
        strip_comment_entries(files)?
    } else {
        files
    };
    let files = if opts.binarize_config {
        binarize_config_entries(files)?
    } else {
        files
    };
    let props = vec![
        ("product".to_owned(), "dayz".to_owned()),
        ("prefix".to_owned(), prefix.to_owned()),
        ("version".to_owned(), "1".to_owned()),
    ];
    let entries: Vec<PboFileToWrite> = files
        .into_iter()
        .map(|(name, data)| PboFileToWrite {
            name,
            data,
            timestamp: opts.timestamp,
        })
        .collect();
    let pbo = pack_pbo(&props, &entries, &PackOptions::default())?;
    Ok((pbo, content.warnings))
}

/// `mf build` writes `prefix = <name>` and the engine addresses packed entries
/// as `<prefix>/<entry>`, so a `CfgMods.defs.*.files[]` entry that does not
/// repeat the prefix binds nothing at all (verified on
/// the real native server). Catch it at build time rather than shipping a mod
/// that silently never loads its scripts.
fn check_files_bind_prefix(files: &[(String, Vec<u8>)], prefix: &str) -> anyhow::Result<()> {
    let Some((_, data)) = files
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("config.cpp"))
    else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(data);
    let parsed = modfoundry_config::parser::parse(&text);
    let dz = modfoundry_config::DayZConfig::from_file(&parsed.file);
    let wanted = format!("{}/", prefix.replace('\\', "/").to_lowercase());

    for m in &dz.mods {
        for sm in &m.script_modules {
            for entry in &sm.files {
                let normalized = entry.replace('\\', "/");
                let trimmed = normalized
                    .trim_start_matches('/')
                    .trim_start_matches("./")
                    .to_lowercase();
                if !trimmed.starts_with(&wanted) {
                    anyhow::bail!(
                        "config.cpp binds `{}` with files[] = \"{entry}\", but this build's PBO \
                         prefix is \"{prefix}\": the engine would look for \"{prefix}/{entry}\" and \
                         bind nothing. Prefix the entry with \"{prefix}/\" or build with \
                         `--name <the prefix the config expects>`.",
                        sm.defs_name
                    );
                }
            }
        }
    }
    Ok(())
}

/// Replace the root `config.cpp` with a binarized `config.bin`.
fn binarize_config_entries(
    files: Vec<(String, Vec<u8>)>,
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    files
        .into_iter()
        .map(|(name, data)| {
            if !is_root_config(&name) {
                return Ok((name, data));
            }
            let text = String::from_utf8(data)
                .map_err(|_| anyhow::anyhow!("config.cpp is not valid UTF-8"))?;
            let parsed = modfoundry_config::parser::parse(&text);
            if let Some(first) = parsed.issues.first() {
                anyhow::bail!(
                    "config.cpp has {} parse issue(s) (first: {}); fix with `mf validate` or drop --binarize",
                    parsed.issues.len(),
                    first.message
                );
            }
            Ok(("config.bin".to_owned(), modfoundry_config::write_rap(&parsed.file)?))
        })
        .collect()
}

/// Remove comments from the packed `.c` files, the ones the game compiles as scripts.
fn strip_comment_entries(files: Vec<(String, Vec<u8>)>) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    files
        .into_iter()
        .map(|(name, data)| {
            if !has_extension(&name, &["c"]) {
                return Ok((name, data));
            }
            let stripped = crate::strip_comments::strip_comments(&data).map_err(|err| {
                anyhow::anyhow!(
                    "{name}:{}: this block comment is never closed; close it or build without \
                     --strip-comments",
                    err.line
                )
            })?;
            Ok((name, stripped))
        })
        .collect()
}

/// Deterministically stage and (optionally) sign a mod into `output_dir`.
///
/// Everything ModFoundry writes goes through the VFS write API (atomic
/// temp+rename, case preflight). An external `DSSignFile` run is the one
/// exception: it writes the `.bisign` itself, after the PBO has landed.
pub fn build_mod(opts: &BuildOptions) -> anyhow::Result<BuildReport> {
    let mut txn = WriteTxn::unrecorded(root_name("stage", &opts.output_dir), &opts.output_dir);
    let report = stage(opts, &mut txn)?;
    txn.finish()?;

    if let Some(signing) = &opts.sign {
        if let Some(tools) = &signing.tools_dir {
            let pbo = opts
                .output_dir
                .join("addons")
                .join(format!("{}.pbo", opts.name));
            DsUtils::new(tools.clone()).sign(&signing.private_key, &pbo, SignVersion::V3)?;
        }
    }
    Ok(report)
}

/// What a build would write, without writing it: the change-set `mf build
/// --preview` returns. A build's output is regenerable, so it is never recorded
/// for `workspace.undo` — this is a plan, not an undo record.
pub fn plan_build(opts: &BuildOptions) -> anyhow::Result<ChangeSet> {
    let mut txn = WriteTxn::dry_run(root_name("stage", &opts.output_dir), &opts.output_dir);
    stage(opts, &mut txn)?;
    Ok(txn.finish()?)
}

/// The build body: pack, then stage `addons/`, `mod.cpp`/`meta.cpp` and (when
/// signing) the public key and — natively signed — the `.bisign`.
fn stage(opts: &BuildOptions, txn: &mut WriteTxn) -> anyhow::Result<BuildReport> {
    if !opts.source_root.is_dir() {
        anyhow::bail!("source root not found: {}", opts.source_root.display());
    }
    let prefix = opts.prefix.as_deref().unwrap_or(&opts.name);
    let pbo_name = format!("{}.pbo", opts.name);
    let (pbo_bytes, warnings) = pack_addon(opts, prefix)?;

    // Both signing inputs up front, so an incomplete keypair fails the build
    // before any of it is staged — including on the external-signer path, where
    // the failure would otherwise surface after the PBO had been written.
    if let Some(signing) = &opts.sign {
        if !signing.public_key.is_file() {
            anyhow::bail!("public key not found: {}", signing.public_key.display());
        }
        if !signing.private_key.is_file() {
            anyhow::bail!("private key not found: {}", signing.private_key.display());
        }
    }

    let pbo_sha256 = modfoundry_vfs::sha256_hex(&pbo_bytes);
    let bisign = match &opts.sign {
        Some(signing) if signing.tools_dir.is_none() => {
            let key = std::fs::read(&signing.private_key)?;
            Some(modfoundry_sign::sign_pbo_bytes(
                &key,
                &pbo_bytes,
                SignVersion::V3,
            )?)
        }
        _ => None,
    };

    let mut note = if opts.binarize_config {
        "Packed `config.bin` (RaP) written by the native writer, byte-parity with CfgConvert -bin; \
         the source `config.cpp` is not packed."
            .to_owned()
    } else {
        "Plaintext PBO (config.cpp + .c) is engine-loadable as packed; pass --binarize to ship a \
         RaP config.bin instead."
            .to_owned()
    };
    if opts.strip_comments {
        note.push_str(" Comments were removed from packed .c files.");
    }

    let mut report = BuildReport {
        name: opts.name.clone(),
        output_dir: opts.output_dir.clone(),
        addons: vec![pbo_name.clone()],
        signed: false,
        timestamp: opts.timestamp,
        pbo_sha256: vec![pbo_sha256],
        note: Some(note),
        warnings,
    };

    remove_stale_signatures(opts, txn, &pbo_name, bisign.is_some())?;

    txn.write(format!("addons/{pbo_name}"), pbo_bytes)?;
    match source_root_file(&opts.source_root, "mod.cpp") {
        Some(path) => txn.write("mod.cpp", std::fs::read(path)?)?,
        None => txn.write("mod.cpp", render_mod_cpp(&opts.mod_cpp))?,
    }
    if let Some(path) = source_root_file(&opts.source_root, "meta.cpp") {
        txn.write("meta.cpp", std::fs::read(path)?)?;
    }

    if let Some(signing) = &opts.sign {
        // Only the public key is distributed with a build.
        txn.write(
            format!("keys/{}", public_key_name(&signing.authority)),
            std::fs::read(&signing.public_key)?,
        )?;
        if let Some(bisign) = bisign {
            txn.write(
                format!("addons/{}", bisign_name(&pbo_name, &signing.authority)),
                bisign,
            )?;
        }
        report.signed = true;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_vfs::ChangeOp;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mfbuild-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn packed_names(out: &Path) -> Vec<String> {
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        hdr.entries
            .iter()
            .map(|e| e.name_str().into_owned())
            .collect()
    }

    fn packed_file(out: &Path, name: &str) -> Vec<u8> {
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, data) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        let index = hdr
            .entries
            .iter()
            .position(|entry| entry.name_str() == name)
            .unwrap_or_else(|| panic!("no {name} packed"));
        data[index].clone()
    }

    fn options(source: PathBuf, output: PathBuf) -> BuildOptions {
        BuildOptions {
            name: "MyMod".into(),
            source_root: source,
            output_dir: output,
            prefix: None,
            timestamp: 1700000000,
            mod_cpp: ModMeta {
                name: "MyMod".into(),
                author: "Test".into(),
                version: "1.0".into(),
            },
            sign: None,
            binarize_config: false,
            strip_comments: false,
        }
    }

    #[test]
    fn contents_lists_the_root_config_cpp_as_binarized_when_binarizing() {
        let src = tree(
            "contents-binarized",
            &[
                ("config.cpp", b"class CfgPatches {}".to_vec()),
                ("sub/config.cpp", b"class CfgPatches {}".to_vec()),
            ],
        );
        let out = tmp("contents-binarized-out");
        let row = |binarize: bool, path: &str| {
            contents(&src, &out, binarize)
                .unwrap()
                .files
                .into_iter()
                .find(|file| file.path == path)
                .map(|file| (file.decision.included, file.decision.reason, file.why))
        };
        let game = pack_rules::Reason::GameFile;

        assert_eq!(
            row(true, "config.cpp"),
            Some((
                true,
                pack_rules::Reason::Binarized,
                "packed as config.bin (--binarize)".to_owned()
            ))
        );
        assert_eq!(
            row(true, "sub/config.cpp"),
            Some((true, game.clone(), "game file".to_owned()))
        );
        assert_eq!(
            row(false, "config.cpp"),
            Some((true, game.clone(), "game file".to_owned()))
        );
        assert_eq!(
            row(false, "sub/config.cpp"),
            Some((true, game, "game file".to_owned()))
        );
    }

    #[test]
    fn a_binarized_build_packs_config_bin_for_the_file_the_listing_marks() {
        let config = b"class CfgPatches {};".to_vec();
        for (root_name, nested) in [
            ("config.cpp", "sub/config.cpp"),
            ("Config.cpp", "sub/config.cpp"),
            ("config.cpp", "mods/config.cpp"),
        ] {
            let src = tree(
                "binarized-matches-listing",
                &[(root_name, config.clone()), (nested, config.clone())],
            );
            let out = tmp("binarized-matches-listing-out");
            let marked: Vec<String> = contents(&src, &out, true)
                .unwrap()
                .files
                .into_iter()
                .filter(|file| file.decision.reason == pack_rules::Reason::Binarized)
                .map(|file| file.path)
                .collect();

            build_mod(&BuildOptions {
                binarize_config: true,
                ..options(src, out.clone())
            })
            .unwrap();

            assert_eq!(marked, [root_name], "{root_name} with {nested}");
            let mut names = packed_names(&out);
            names.sort();
            assert_eq!(
                names,
                ["config.bin", &nested.replace('/', "\\")],
                "{root_name} with {nested}"
            );
        }
    }

    #[test]
    fn a_build_report_carries_the_content_warnings() {
        let src = tree(
            "report-warnings",
            &[
                ("config.cpp", b"class CfgPatches {}".to_vec()),
                ("sub/config.cpp", b"class CfgPatches {}".to_vec()),
            ],
        );
        let out = tmp("report-warnings-out");
        let listing = contents(&src, &out, false).unwrap();

        let report = build_mod(&options(src, out)).unwrap();

        assert_eq!(report.warnings, listing.warnings);
        assert_eq!(
            report.warnings,
            ["2 config files will be packed and the game loads every one: config.cpp, sub/config.cpp"]
        );
    }

    #[test]
    fn packs_and_stages_layout() {
        let src = tmp("src");
        std::fs::create_dir_all(src.join("Scripts/1_Core")).unwrap();
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("Scripts/1_Core/a.c"), b"class A {}").unwrap();
        let out = tmp("out");
        let report = build_mod(&options(src, out.clone())).unwrap();
        assert!(out.join("addons/MyMod.pbo").is_file());
        assert_eq!(
            std::fs::read_to_string(out.join("mod.cpp")).unwrap(),
            "name = \"MyMod\";\nauthor = \"Test\";\nversion = \"1.0\";\n"
        );
        assert_eq!(report.addons, vec!["MyMod.pbo"]);
        assert!(!report.signed);
        // prefix defaulted to the mod name
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        assert_eq!(hdr.property("prefix"), Some("MyMod"));
        assert_eq!(hdr.entries.len(), 2);
    }

    #[test]
    fn stages_the_trees_own_mod_cpp_bytes() {
        let own = b"name = \"Field Kit\";\nauthor = \"Joy\";\nversion = \"2.1\";\n";
        for name in ["mod.cpp", "Mod.cpp"] {
            let src = tmp(&format!("src-own-{name}"));
            std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
            std::fs::write(src.join(name), own).unwrap();
            let out = tmp(&format!("out-own-{name}"));
            build_mod(&options(src, out.clone())).unwrap();

            assert_eq!(std::fs::read(out.join("mod.cpp")).unwrap(), own, "{name}");
            let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
            let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
            assert!(
                hdr.entries
                    .iter()
                    .all(|e| !e.name_str().eq_ignore_ascii_case(name)),
                "{name} must not be packed: {:?}",
                hdr.entries.iter().map(|e| e.name_str()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn stages_the_trees_own_meta_cpp_only_when_it_has_one() {
        let own = b"name = \"Field Kit\";\nversion = \"2.1\";\n";

        let with = tmp("src-meta");
        std::fs::write(with.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(with.join("meta.cpp"), own).unwrap();
        let out = tmp("out-meta");
        build_mod(&options(with, out.clone())).unwrap();
        assert_eq!(std::fs::read(out.join("meta.cpp")).unwrap(), own);

        let without = tmp("src-nometa");
        std::fs::write(without.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-nometa");
        build_mod(&options(without, out.clone())).unwrap();
        assert!(!out.join("meta.cpp").exists());
    }

    #[test]
    fn build_is_byte_deterministic() {
        let src = tmp("src2");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out1 = tmp("o1");
        let out2 = tmp("o2");
        build_mod(&options(src.clone(), out1.clone())).unwrap();
        build_mod(&options(src, out2.clone())).unwrap();
        let a = std::fs::read(out1.join("addons/MyMod.pbo")).unwrap();
        let b = std::fs::read(out2.join("addons/MyMod.pbo")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn signs_natively_and_stages_only_the_public_key() {
        let src = tmp("src-sign");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("a.c"), b"void Foo() {}").unwrap();
        let keys = src.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        let (private, public) = modfoundry_sign::generate_keypair("MF", 1024).unwrap();
        std::fs::write(keys.join("MF.biprivatekey"), private.to_bytes()).unwrap();
        std::fs::write(keys.join("MF.bikey"), public.to_bytes()).unwrap();

        let out = tmp("out-sign");
        let mut opts = options(src.clone(), out.clone());
        opts.sign = Some(Signing {
            authority: "MF".into(),
            private_key: keys.join("MF.biprivatekey"),
            public_key: keys.join("MF.bikey"),
            tools_dir: None,
        });
        let report = build_mod(&opts).unwrap();
        assert!(report.signed);

        let bisign = out.join("addons/MyMod.pbo.MF.bisign");
        assert!(bisign.is_file());
        assert!(out.join("keys/MF.bikey").is_file());
        assert!(!out.join("keys/MF.biprivatekey").exists());

        let digest = modfoundry_sign::PboDigest::from_pbo_bytes(
            &std::fs::read(out.join("addons/MyMod.pbo")).unwrap(),
        )
        .unwrap();
        let parsed = modfoundry_sign::Bisign::parse(&std::fs::read(bisign).unwrap()).unwrap();
        modfoundry_sign::verify(&public, &digest, &parsed).unwrap();
    }

    #[test]
    fn binarize_replaces_config_cpp_with_config_bin() {
        let src = tmp("src-bin");
        std::fs::write(
            src.join("config.cpp"),
            b"class CfgPatches\n{\n\tclass MyMod\n\t{\n\t\tunits[]={};\n\t\trequiredVersion=0.1;\n\t};\n};\n",
        )
        .unwrap();
        let out = tmp("out-bin");
        let mut opts = options(src, out.clone());
        opts.binarize_config = true;
        build_mod(&opts).unwrap();

        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, payloads) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        let names: Vec<std::borrow::Cow<str>> = hdr.entries.iter().map(|e| e.name_str()).collect();
        assert_eq!(names, vec!["config.bin"]);
        let file = modfoundry_config::read_rap(&payloads[0]).unwrap();
        assert!(modfoundry_config::to_text(&file).contains("class CfgPatches"));
    }

    #[test]
    fn binarize_refuses_a_config_with_parse_issues() {
        let src = tmp("src-badbin");
        std::fs::write(src.join("config.cpp"), b"class Broken { units[] = ").unwrap();
        let out = tmp("out-badbin");
        let mut opts = options(src, out);
        opts.binarize_config = true;
        assert!(build_mod(&opts).is_err());
    }

    #[test]
    fn strip_comments_packs_scripts_without_comments_and_leaves_the_source() {
        let src = tmp("src-strip");
        std::fs::create_dir_all(src.join("Scripts")).unwrap();
        let script = b"class A\n{\n    // note\n    int x; /* inline */\n};\n";
        let config = b"class CfgPatches\n{\n    // keep me\n    class MyMod {};\n};\n";
        std::fs::write(src.join("Scripts/a.c"), script).unwrap();
        std::fs::write(src.join("config.cpp"), config).unwrap();

        let out = tmp("out-strip");
        let mut opts = options(src.clone(), out.clone());
        opts.strip_comments = true;
        build_mod(&opts).unwrap();

        let packed = packed_file(&out, "Scripts\\a.c");
        assert_eq!(packed, b"class A\n{\n\n    int x;\n};\n");
        let breaks = |bytes: &[u8]| bytes.iter().filter(|byte| **byte == b'\n').count();
        assert_eq!(
            breaks(&packed),
            breaks(script),
            "line numbers must not move"
        );
        assert_eq!(packed_file(&out, "config.cpp"), config);
        assert_eq!(std::fs::read(src.join("Scripts/a.c")).unwrap(), script);
        assert_eq!(std::fs::read(src.join("config.cpp")).unwrap(), config);

        let again = tmp("out-strip-again");
        let mut second = options(src.clone(), again.clone());
        second.strip_comments = true;
        build_mod(&second).unwrap();
        let pbo = |dir: &Path| std::fs::read(dir.join("addons/MyMod.pbo")).unwrap();
        assert_eq!(pbo(&out), pbo(&again), "stripping must stay deterministic");

        let plain = tmp("out-strip-off");
        build_mod(&options(src, plain.clone())).unwrap();
        assert_eq!(
            packed_file(&plain, "Scripts\\a.c"),
            script,
            "without the option the comment packs"
        );
    }

    #[test]
    fn an_unclosed_block_comment_fails_the_build_naming_the_file_and_line() {
        let src = tmp("src-unclosed");
        std::fs::create_dir_all(src.join("Scripts")).unwrap();
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("Scripts/a.c"), b"class A {};\n/* open\n").unwrap();
        let out = tmp("out-unclosed");
        let mut opts = options(src, out.clone());
        opts.strip_comments = true;

        let err = build_mod(&opts).expect_err("an unclosed block comment must fail the build");

        assert_eq!(
            err.to_string(),
            "Scripts\\a.c:2: this block comment is never closed; close it or build without \
             --strip-comments"
        );
        assert!(!out.join("addons").exists(), "nothing is staged on failure");
    }

    #[test]
    fn refuses_to_build_when_files_do_not_match_the_pbo_prefix() {
        let src = tmp("src-prefix");
        std::fs::write(
            src.join("config.cpp"),
            b"class CfgPatches { class MyMod_Scripts { requiredAddons[] = {\"DZ_Data\"}; }; };\n\
              class CfgMods { class MyMod { type = \"mod\"; class defs {\n\
                  class gameScriptModule { files[] = { \"MyMod/Scripts/3_Game\" }; };\n\
              }; }; };\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("Scripts/3_Game")).unwrap();

        let mut renamed = options(src.clone(), tmp("out-prefix-bad"));
        renamed.name = "Renamed".into();
        let err = build_mod(&renamed).expect_err("prefix mismatch must fail the build");
        assert!(
            err.to_string().contains("files[]") && err.to_string().contains("Renamed"),
            "error must explain the mismatch, got: {err}"
        );

        let mut matching = options(src, tmp("out-prefix-ok"));
        matching.name = "MyMod".into();
        build_mod(&matching).expect("a matching prefix builds");
    }

    #[test]
    fn the_prefix_guard_reads_a_lowercase_cfgmods() {
        let src = tmp("src-prefix-lower");
        std::fs::write(
            src.join("config.cpp"),
            b"class cfgpatches { class MyMod_Scripts { requiredAddons[] = {\"DZ_Data\"}; }; };\n\
              class cfgmods { class MyMod { type = \"mod\"; class defs {\n\
                  class gamescriptmodule { files[] = { \"Scripts/3_Game\" }; };\n\
              }; }; };\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("Scripts/3_Game")).unwrap();

        let opts = options(src, tmp("out-prefix-lower"));
        let err = build_mod(&opts).expect_err("a files[] entry without the prefix must fail");
        assert!(err.to_string().contains("files[]"), "got: {err}");
    }

    #[test]
    fn prefix_guard_ignores_mods_without_script_bindings() {
        let src = tmp("src-nobind");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let mut opts = options(src, tmp("out-nobind"));
        opts.name = "Anything".into();
        build_mod(&opts).expect("a config with no CfgMods binds nothing to check");
    }

    #[test]
    fn plan_build_lists_what_a_build_stages_without_staging_it() {
        let src = tmp("src-plan");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("meta.cpp"), b"name = \"MyMod\";\n").unwrap();
        let out = tmp("out-plan");
        let _ = std::fs::remove_dir_all(&out);
        let opts = options(src, out.clone());

        let plan = plan_build(&opts).unwrap();
        let paths: Vec<&str> = plan.files().iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.iter().any(|p| p.ends_with(":addons/MyMod.pbo")),
            "{paths:?}"
        );
        assert!(paths.iter().any(|p| p.ends_with(":mod.cpp")), "{paths:?}");
        assert!(paths.iter().any(|p| p.ends_with(":meta.cpp")), "{paths:?}");
        assert!(!out.exists(), "a preview stages nothing");

        build_mod(&opts).unwrap();
        assert!(out.join("addons/MyMod.pbo").is_file());
        assert!(
            std::fs::read_dir(out.join("addons")).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".mf-")),
            "atomic writes leave no staging file behind"
        );
    }

    #[test]
    fn a_private_key_is_never_packed_wherever_it_sits() {
        let src = tmp("src-key-anywhere");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::create_dir_all(src.join("Keys")).unwrap();
        std::fs::write(src.join("Keys/MF.biprivatekey"), b"x").unwrap();
        std::fs::write(src.join("MF.BIPRIVATEKEY"), b"x").unwrap();
        std::fs::create_dir_all(src.join("Scripts")).unwrap();
        std::fs::write(src.join("Scripts/MF.biprivatekey"), b"x").unwrap();
        std::fs::write(src.join("Scripts/x.c"), b"void Foo() {}").unwrap();

        let out = tmp("out-key-anywhere");
        build_mod(&options(src, out.clone())).unwrap();

        let names = packed_names(&out);
        assert_eq!(names, vec!["Scripts\\x.c", "config.cpp"]);
    }

    #[test]
    fn root_only_names_are_packed_below_the_root() {
        let src = tmp("src-nested-names");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::create_dir_all(src.join("data/keys")).unwrap();
        std::fs::write(src.join("data/keys/item.p3d"), b"p3d").unwrap();
        std::fs::create_dir_all(src.join("data/target")).unwrap();
        std::fs::write(src.join("data/target/a.paa"), b"paa").unwrap();
        std::fs::write(src.join("data/mod.cpp"), b"// content").unwrap();
        std::fs::create_dir_all(src.join("Builds")).unwrap();
        std::fs::write(src.join("Builds/stale.c"), b"stale").unwrap();

        let out = tmp("out-nested-names");
        build_mod(&options(src, out.clone())).unwrap();

        let names = packed_names(&out);
        assert_eq!(
            names,
            vec![
                "config.cpp",
                "data\\keys\\item.p3d",
                "data\\mod.cpp",
                "data\\target\\a.paa"
            ]
        );
    }

    #[test]
    fn an_output_folder_inside_the_source_root_is_not_packed() {
        let src = tmp("src-out-inside");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = src.join("out/@MyMod");
        let opts = options(src, out.clone());

        build_mod(&opts).unwrap();
        build_mod(&opts).unwrap();

        let names = packed_names(&out);
        assert_eq!(names, vec!["config.cpp"]);
    }

    #[test]
    fn a_build_into_its_own_source_folder_is_refused() {
        let src = tmp("src-out-same");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();

        let err = build_mod(&options(src.clone(), src.clone())).expect_err("output is the source");

        assert!(err.to_string().contains("is the source folder"), "{err}");
        assert!(!src.join("addons").exists());
    }

    #[test]
    fn excludes_keys_workshop_server_and_build_dirs() {
        let src = tmp("src3");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::create_dir_all(src.join("keys")).unwrap();
        std::fs::write(src.join("keys/secret.biprivatekey"), b"x").unwrap();
        std::fs::create_dir_all(src.join("workshop")).unwrap();
        std::fs::write(src.join("workshop/item.json"), b"{}").unwrap();
        std::fs::create_dir_all(src.join("server/profiles")).unwrap();
        std::fs::write(src.join("server/profiles/x.json"), b"{}").unwrap();
        let out = tmp("o3");
        let report = build_mod(&options(src, out.clone())).unwrap();
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        assert_eq!(hdr.entries.len(), 1);
        assert_eq!(hdr.entries[0].name_str(), "config.cpp");
        assert_eq!(report.pbo_sha256.len(), 1);
    }

    #[test]
    fn only_game_files_pack_by_default() {
        let src = tmp("src-game-only");
        std::fs::create_dir_all(src.join("Scripts/3_Game")).unwrap();
        std::fs::create_dir_all(src.join("docs")).unwrap();
        std::fs::create_dir_all(src.join("tests/selftest")).unwrap();
        std::fs::create_dir_all(src.join("tools")).unwrap();
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("Scripts/3_Game/a.c"), b"class A {}").unwrap();
        std::fs::write(src.join("AGENTS.md"), b"# notes").unwrap();
        std::fs::write(src.join("docs/notes.c"), b"class N {}").unwrap();
        std::fs::write(
            src.join("tests/selftest/config.cpp"),
            b"class CfgPatches {}",
        )
        .unwrap();
        std::fs::write(src.join("tools/run.sh"), b"#!/bin/sh").unwrap();
        let out = tmp("out-game-only");
        build_mod(&options(src, out.clone())).unwrap();

        assert_eq!(
            packed_names(&out),
            vec!["Scripts\\3_Game\\a.c", "config.cpp"]
        );
    }

    #[test]
    fn a_pboignore_rule_can_add_and_drop_files_but_never_a_private_key() {
        let src = tmp("src-pboignore");
        std::fs::create_dir_all(src.join("Scripts/3_Game")).unwrap();
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::write(src.join("AGENTS.md"), b"# notes").unwrap();
        std::fs::write(src.join("Scripts/3_Game/skip.c"), b"class S {}").unwrap();
        std::fs::write(src.join("secret.biprivatekey"), b"x").unwrap();
        std::fs::write(
            src.join(".pboignore"),
            "!*.md\nScripts/3_Game/skip.c\n!*.biprivatekey\n",
        )
        .unwrap();
        let out = tmp("out-pboignore");
        build_mod(&options(src, out.clone())).unwrap();

        assert_eq!(packed_names(&out), vec!["AGENTS.md", "config.cpp"]);
    }

    #[test]
    fn a_tools_signing_build_refuses_a_missing_private_key_before_staging_anything() {
        let src = tmp("src-tools-nokey");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let keys = tmp("keys-tools-nokey");
        std::fs::create_dir_all(&keys).unwrap();
        let (_private, public) = modfoundry_sign::generate_keypair("MF", 1024).unwrap();
        std::fs::write(keys.join("MF.bikey"), public.to_bytes()).unwrap();

        let out = tmp("out-tools-nokey");
        let _ = std::fs::remove_dir_all(&out);
        let mut opts = options(src, out.clone());
        opts.sign = Some(Signing {
            authority: "MF".into(),
            private_key: keys.join("MF.biprivatekey"),
            public_key: keys.join("MF.bikey"),
            tools_dir: Some(tmp("dsutils-empty")),
        });

        let err = build_mod(&opts).expect_err("a missing private key must fail the build");
        assert!(err.to_string().contains("private key"), "{err}");
        assert!(
            !out.exists(),
            "the signing inputs are checked before anything is staged"
        );
    }

    fn signing_in(dir: &Path, authority: &str) -> Signing {
        let (private, public) = modfoundry_sign::generate_keypair(authority, 1024).unwrap();
        let keys = dir.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        let private_key = keys.join(format!("{authority}.biprivatekey"));
        let public_key = keys.join(format!("{authority}.bikey"));
        std::fs::write(&private_key, private.to_bytes()).unwrap();
        std::fs::write(&public_key, public.to_bytes()).unwrap();
        Signing {
            authority: authority.into(),
            private_key,
            public_key,
            tools_dir: None,
        }
    }

    fn deleted_paths(plan: &ChangeSet) -> Vec<&str> {
        plan.files()
            .iter()
            .filter(|f| f.op == ChangeOp::Delete)
            .map(|f| f.path.as_str())
            .collect()
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_build_removes_the_signatures_an_earlier_build_left() {
        let src = tmp("src-stale");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-stale");

        let mut signed = options(src.clone(), out.clone());
        signed.sign = Some(signing_in(&src, "Old"));
        build_mod(&signed).unwrap();
        assert!(out.join("addons/MyMod.pbo.Old.bisign").is_file());

        std::fs::write(out.join("notes.txt"), b"kept").unwrap();
        std::fs::write(out.join("addons/notes.txt"), b"kept").unwrap();
        build_mod(&options(src, out.clone())).unwrap();

        assert_eq!(file_names(&out.join("addons")), ["MyMod.pbo", "notes.txt"]);
        assert_eq!(file_names(&out.join("keys")), ["Old.bikey"]);
        assert!(out.join("notes.txt").is_file());
    }

    #[test]
    fn a_signed_rebuild_keeps_only_its_own_signature() {
        let src = tmp("src-resign");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-resign");

        let mut old = options(src.clone(), out.clone());
        old.sign = Some(signing_in(&src, "Old"));
        build_mod(&old).unwrap();

        let mut new = options(src.clone(), out.clone());
        new.sign = Some(signing_in(&src, "New"));
        build_mod(&new).unwrap();

        assert_eq!(
            file_names(&out.join("addons")),
            ["MyMod.pbo", "MyMod.pbo.New.bisign"]
        );
        assert_eq!(file_names(&out.join("keys")), ["New.bikey", "Old.bikey"]);
    }

    #[test]
    fn removing_unbuilt_addons_keeps_only_this_builds_pbo_and_its_signatures() {
        let src = tmp("src-unbuilt");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-unbuilt");
        let mut signed = options(src.clone(), out.clone());
        signed.sign = Some(signing_in(&src, "MF"));
        let report = build_mod(&signed).unwrap();

        let addons = out.join("addons");
        for name in [
            "Old.pbo",
            "Old.pbo.MF.bisign",
            "Spare.PBO",
            "MYMOD.PBO.Other.bisign",
            "notes.txt",
        ] {
            std::fs::write(addons.join(name), b"old").unwrap();
        }
        std::fs::write(out.join("keys/Other.bikey"), b"old").unwrap();
        std::fs::write(addons.join("MyMod.pbo2.MF.bisign"), b"old").unwrap();
        std::fs::create_dir(addons.join("Folder.pbo")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(addons.join("notes.txt"), addons.join("Linked.pbo")).unwrap();

        let removed = remove_unbuilt_addons(&report).unwrap();

        let mut wanted = vec![
            "addons/MyMod.pbo2.MF.bisign",
            "addons/Old.pbo",
            "addons/Old.pbo.MF.bisign",
            "addons/Spare.PBO",
        ];
        if cfg!(unix) {
            wanted.insert(0, "addons/Linked.pbo");
        }
        assert_eq!(removed, wanted);
        assert!(addons.join("Folder.pbo").is_dir());
        assert_eq!(
            file_names(&addons),
            [
                "Folder.pbo",
                "MYMOD.PBO.Other.bisign",
                "MyMod.pbo",
                "MyMod.pbo.MF.bisign",
                "notes.txt"
            ]
        );
        assert!(out.join("keys/Other.bikey").is_file());
    }

    #[test]
    fn a_plan_for_a_resign_with_the_same_authority_deletes_nothing() {
        let src = tmp("src-plan-same");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-plan-same");
        let mut opts = options(src.clone(), out.clone());
        opts.sign = Some(signing_in(&src, "Old"));
        build_mod(&opts).unwrap();

        let plan = plan_build(&opts).unwrap();
        let deletes = deleted_paths(&plan);
        assert!(deletes.is_empty(), "{deletes:?}");
    }

    #[test]
    fn a_plan_lists_only_this_pbos_stale_signatures_and_leaves_them_on_disk() {
        let src = tmp("src-plan-stale");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        let out = tmp("out-plan-stale");
        std::fs::create_dir_all(out.join("addons")).unwrap();
        for name in ["MyMod.pbo.Gone.bisign", "Other.pbo", "Other.pbo.X.bisign"] {
            std::fs::write(out.join("addons").join(name), b"old").unwrap();
        }

        let plan = plan_build(&options(src, out.clone())).unwrap();
        let deletes: Vec<&str> = deleted_paths(&plan)
            .into_iter()
            .map(|p| p.rsplit_once(':').map_or(p, |(_, rel)| rel))
            .collect();
        assert_eq!(deletes, ["addons/MyMod.pbo.Gone.bisign"]);
        assert!(out.join("addons/MyMod.pbo.Gone.bisign").is_file());
    }

    #[test]
    fn the_server_setup_is_left_out_and_a_mods_own_server_files_still_pack() {
        let src = tmp("src-server");
        std::fs::write(src.join("config.cpp"), b"class CfgPatches {}").unwrap();
        std::fs::create_dir_all(src.join("Server/profiles")).unwrap();
        std::fs::write(src.join("Server/serverDZ.cfg"), b"passwordAdmin = \"x\";").unwrap();
        std::fs::write(src.join("Server/profiles/admins.json"), b"{}").unwrap();
        std::fs::write(src.join("Server/init.c"), b"void main() {}").unwrap();
        let out = tmp("out-server");
        build_mod(&options(src, out.clone())).unwrap();
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        let names: Vec<String> = hdr
            .entries
            .iter()
            .map(|e| e.name_str().into_owned())
            .collect();
        assert_eq!(names, ["Server\\init.c", "config.cpp"]);
    }

    fn mixed_tree(label: &str) -> PathBuf {
        let src = tmp(label);
        for (rel, text) in [
            ("config.cpp", "class CfgPatches {}"),
            ("Scripts/4_World/a.c", "class A {}"),
            ("docs/readme.md", "# docs"),
            ("tests/selftest/config.cpp", "class CfgPatches {}"),
            ("notes.txt", "notes"),
            ("LICENSE", "MIT"),
            (".pboignore", "!notes.txt\n!tests/\n!LICENSE\n"),
        ] {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        src
    }

    #[test]
    fn contents_gives_each_file_its_decision_and_reason() {
        let src = mixed_tree("contents-decisions");
        let listing = contents(&src, &tmp("contents-decisions-out"), false).unwrap();
        let rule = |line, pattern: &str| pack_rules::Reason::Rule {
            line,
            pattern: pattern.into(),
        };
        let cases = [
            ("LICENSE", true, rule(3, "LICENSE")),
            ("Scripts/4_World/a.c", true, pack_rules::Reason::GameFile),
            ("config.cpp", true, pack_rules::Reason::GameFile),
            ("docs/readme.md", false, pack_rules::Reason::SkippedFolder),
            ("notes.txt", true, rule(1, "notes.txt")),
            ("tests/selftest/config.cpp", true, rule(2, "tests/")),
        ];
        let actual: Vec<_> = listing
            .files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.decision.included,
                    file.decision.reason.clone(),
                )
            })
            .collect();
        assert_eq!(actual, cases);
        let docs = listing
            .files
            .iter()
            .find(|file| file.path == "docs/readme.md");
        assert_eq!(docs.map(|file| file.why.as_str()), Some("skipped folder"));
    }

    #[test]
    fn contents_warns_when_several_config_files_are_packed() {
        let src = mixed_tree("contents-configs");
        let listing = contents(&src, &tmp("contents-configs-out"), false).unwrap();
        let wanted = "2 config files will be packed and the game loads every one: config.cpp, tests/selftest/config.cpp";
        assert!(
            listing.warnings.iter().any(|warning| warning == wanted),
            "{:?}",
            listing.warnings
        );
    }

    #[test]
    fn contents_warns_when_a_pboignore_rule_packs_a_file_the_game_does_not_load() {
        let src = mixed_tree("contents-rule-warning");
        let listing = contents(&src, &tmp("contents-rule-warning-out"), false).unwrap();
        for wanted in [
            "notes.txt is packed by .pboignore line 1 but the game does not load .txt files",
            "LICENSE is packed by .pboignore line 3 but the game does not load files without an extension",
        ] {
            assert!(
                listing.warnings.iter().any(|warning| warning == wanted),
                "{wanted}: {:?}",
                listing.warnings
            );
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([x as u8, y as u8, 0, 255])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn tree(label: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
        let src = tmp(label);
        for (rel, bytes) in files {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        src
    }

    #[test]
    fn a_converted_png_keeps_its_pboignore_rule_in_its_reason() {
        let src = tree(
            "contents-images-rule",
            &[
                (".pboignore", b"!tests/\n".to_vec()),
                (".pboimages", b"tests/art edds\ngui edds\n".to_vec()),
                ("tests/art/frame.png", png(16, 16)),
                ("gui/x.png", png(16, 16)),
                ("config.cpp", b"class CfgPatches {}".to_vec()),
            ],
        );
        let listing = contents(&src, &tmp("contents-images-rule-out"), false).unwrap();
        let row = |path: &str| {
            listing
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| (file.decision.included, file.why.clone()))
        };

        assert_eq!(
            row("tests/art/frame.png"),
            Some((
                true,
                "packed as frame.edds (.pboimages line 1); .pboignore line 1: tests/".to_owned()
            ))
        );
        assert_eq!(
            row("gui/x.png"),
            Some((true, "packed as x.edds (.pboimages line 2)".to_owned()))
        );
    }

    #[test]
    fn contents_gives_a_pboimages_png_its_converted_name_as_its_reason() {
        let src = tree(
            "contents-images",
            &[
                (".pboimages", b"gui/images edds\n".to_vec()),
                ("gui/images/frame.png", png(16, 16)),
                ("gui/other/frame.png", png(16, 16)),
            ],
        );
        let listing = contents(&src, &tmp("contents-images-out"), false).unwrap();
        let decision_of = |path: &str| {
            listing
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| (file.decision.included, file.why.clone()))
        };
        assert_eq!(
            decision_of("gui/images/frame.png"),
            Some((true, "packed as frame.edds (.pboimages line 1)".to_owned()))
        );
        assert_eq!(
            decision_of("gui/other/frame.png"),
            Some((true, "game file".to_owned()))
        );
    }

    #[test]
    fn build_packs_pboimages_pngs_as_their_converted_textures() {
        let src = tree(
            "build-images",
            &[
                ("config.cpp", b"class CfgPatches {}".to_vec()),
                (".pboimages", b"gui/images edds\ndata paa\n".to_vec()),
                ("gui/images/frame.png", png(16, 16)),
                ("data/logo.png", png(16, 16)),
            ],
        );
        let out = tmp("build-images-out");
        build_mod(&options(src, out.clone())).unwrap();

        assert_eq!(
            packed_names(&out),
            ["config.cpp", r"data\logo.paa", r"gui\images\frame.edds"]
        );
        let bytes = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
        let (hdr, data) = modfoundry_pbo::read_pbo(&bytes).unwrap();
        let entry = |name: &str| {
            let index = hdr
                .entries
                .iter()
                .position(|e| e.name_str() == name)
                .unwrap();
            &data[index]
        };
        let frame = modfoundry_texture::edds::decode_edds(entry(r"gui\images\frame.edds")).unwrap();
        assert_eq!((frame[0].width, frame[0].height), (16, 16));
        let (format, logo) = modfoundry_texture::paa::decode_paa(entry(r"data\logo.paa")).unwrap();
        assert_eq!(format, modfoundry_texture::paa::PaaFormat::Dxt1);
        assert_eq!(logo[0].width, 16);
    }

    #[test]
    fn build_refuses_a_png_that_paa_cannot_hold() {
        let src = tree(
            "build-paa-size",
            &[
                (".pboimages", b"data paa\n".to_vec()),
                ("data/logo.png", png(48, 48)),
            ],
        );

        let error = build_mod(&options(src, tmp("build-paa-size-out"))).unwrap_err();

        assert_eq!(
            error.to_string(),
            "data/logo.png: 48x48 is not a power-of-two size, which .paa needs; resize the PNG to 32x32 or 64x64"
        );
    }

    #[test]
    fn files_differing_only_in_case_still_build_when_nothing_is_converted() {
        let src = tree(
            "build-case-only",
            &[
                ("Data/a.c", b"class A {}".to_vec()),
                ("data/b/a.c", b"class B {}".to_vec()),
                ("data/a.c", b"class C {}".to_vec()),
            ],
        );

        let listing = contents(&src, &tmp("build-case-only-listing"), false).unwrap();

        assert!(
            listing
                .warnings
                .iter()
                .all(|w| !w.contains("would both be packed")),
            "{:?}",
            listing.warnings
        );
    }

    #[test]
    fn a_binarized_build_refuses_a_config_cpp_and_a_config_bin_that_would_both_pack_as_config_bin()
    {
        let src = tree(
            "build-config-clash",
            &[
                ("config.cpp", b"class CfgPatches {}".to_vec()),
                ("config.bin", b"rap bytes".to_vec()),
            ],
        );
        let wanted = "config.cpp and config.bin would both be packed as config.bin";

        let listing = contents(&src, &tmp("build-config-clash-listing"), true).unwrap();
        assert!(
            listing.warnings.iter().any(|warning| warning == wanted),
            "{:?}",
            listing.warnings
        );
        assert!(
            listing
                .warnings
                .iter()
                .all(|warning| !warning.contains("config files will be packed")),
            "{:?}",
            listing.warnings
        );
        let error = build_mod(&BuildOptions {
            binarize_config: true,
            ..options(src.clone(), tmp("build-config-clash-out"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), wanted);

        let out = tmp("build-config-clash-plain");
        build_mod(&options(src.clone(), out.clone())).unwrap();
        assert_eq!(packed_names(&out), ["config.bin", "config.cpp"]);
        let plain = contents(&src, &tmp("build-config-clash-plain-listing"), false).unwrap();
        assert!(
            plain
                .warnings
                .iter()
                .all(|warning| !warning.contains("would both be packed")),
            "{:?}",
            plain.warnings
        );
        assert!(
            plain.warnings.iter().any(|warning| warning
                == "2 config files will be packed and the game loads every one: config.bin, config.cpp"),
            "{:?}",
            plain.warnings
        );
    }

    #[test]
    fn a_binarized_build_ignores_a_config_bin_that_pboignore_leaves_out() {
        let src = tree(
            "build-config-clash-ignored",
            &[
                (".pboignore", b"config.bin\n".to_vec()),
                ("config.cpp", b"class CfgPatches {}".to_vec()),
                ("config.bin", b"rap bytes".to_vec()),
            ],
        );
        let out = tmp("build-config-clash-ignored-out");

        build_mod(&BuildOptions {
            binarize_config: true,
            ..options(src, out.clone())
        })
        .unwrap();

        assert_eq!(packed_names(&out), ["config.bin"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_to_a_parent_folder_is_skipped_not_repeated() {
        let src = tree(
            "links-loop",
            &[("Scripts/4_World/a.c", b"class A {}".to_vec())],
        );
        std::os::unix::fs::symlink(&src, src.join("Scripts/loop")).unwrap();
        std::os::unix::fs::symlink(src.join("Scripts"), src.join("Scripts/4_World/up")).unwrap();

        let listing = contents(&src, &tmp("links-loop-out"), false).unwrap();
        let paths: Vec<&str> = listing
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert_eq!(paths, ["Scripts/4_World/a.c"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_private_key_is_never_listed_whatever_its_name() {
        let keys = tree("links-key", &[("Mod.biprivatekey", b"secret".to_vec())]);
        let src = tree("links-key-src", &[("Scripts/a.c", b"class A {}".to_vec())]);
        std::os::unix::fs::symlink(keys.join("Mod.biprivatekey"), src.join("Scripts/x.c")).unwrap();

        let listing = contents(&src, &tmp("links-key-out"), false).unwrap();
        let paths: Vec<&str> = listing
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();

        assert_eq!(paths, ["Scripts/a.c"]);
        let _ = std::fs::remove_dir_all(&keys);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_is_packed_under_every_name_it_is_reached_by() {
        use std::os::unix::fs::symlink;

        let outside = tree("links-outside", &[("shared/s.c", b"class S {}".to_vec())]);
        let linked_outside = tree(
            "links-src-outside",
            &[("config.cpp", b"class CfgPatches {}".to_vec())],
        );
        std::fs::create_dir_all(linked_outside.join("Scripts")).unwrap();
        symlink(
            outside.join("shared"),
            linked_outside.join("Scripts/shared"),
        )
        .unwrap();

        let linked_sibling = tree(
            "links-src-sibling",
            &[("common/c.c", b"class C {}".to_vec())],
        );
        std::fs::create_dir_all(linked_sibling.join("Scripts")).unwrap();
        symlink(
            linked_sibling.join("common"),
            linked_sibling.join("Scripts/common"),
        )
        .unwrap();

        let linked_dead = tree("links-src-dead", &[("Scripts/a.c", b"class A {}".to_vec())]);
        symlink(
            linked_dead.join("missing"),
            linked_dead.join("Scripts/dead"),
        )
        .unwrap();

        let cases: [(&Path, &[&str]); 3] = [
            (&linked_outside, &["Scripts/shared/s.c", "config.cpp"]),
            (&linked_sibling, &["Scripts/common/c.c", "common/c.c"]),
            (&linked_dead, &["Scripts/a.c"]),
        ];
        for (src, wanted) in cases {
            let listing = contents(src, &tmp("links-out"), false).unwrap();
            let paths: Vec<&str> = listing
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect();
            assert_eq!(paths, wanted, "{}", src.display());
        }

        let out = tmp("links-outside-build");
        build_mod(&options(linked_outside, out.clone())).unwrap();
        assert!(
            packed_names(&out).contains(&"Scripts\\shared\\s.c".to_owned()),
            "{:?}",
            packed_names(&out)
        );
    }

    #[test]
    fn a_png_and_a_texture_with_the_same_packed_name_refuse_the_build() {
        let src = tree(
            "build-clash",
            &[
                (".pboimages", b"gui/images edds\n".to_vec()),
                ("gui/images/frame.png", png(16, 16)),
                ("gui/images/frame.edds", b"not a texture".to_vec()),
            ],
        );
        let wanted = "gui/images/frame.png and gui/images/frame.edds would both be packed as gui/images/frame.edds";

        let listing = contents(&src, &tmp("build-clash-listing"), false).unwrap();
        assert!(
            listing.warnings.iter().any(|warning| warning == wanted),
            "{:?}",
            listing.warnings
        );
        let error = build_mod(&options(src, tmp("build-clash-out"))).unwrap_err();
        assert_eq!(error.to_string(), wanted);
    }
}
