//! `image.convert`: a PNG, or every PNG in a folder, into a DayZ texture.

use crate::walk::walk_files;
use crate::{dir_txn, file_rel, output_txn};
use anyhow::{anyhow, bail, ensure, Result};
use modfoundry_texture::convert::{convert_png, Target};
use modfoundry_texture::paa::PaaFormat;
use modfoundry_vfs::ChangeSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Arguments for `image.convert`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageArgs {
    /// A PNG file, or a folder whose PNGs (up to six folders deep) are all converted.
    pub path: PathBuf,
    /// "edds" or "paa".
    pub to: String,
    /// For a file, the output file; for a folder, the folder the outputs mirror into. Default: beside each PNG.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// "dxt1" or "dxt5", only with "paa".
    #[serde(default)]
    pub format: Option<String>,
}

/// Converts and writes; returns the summary line and the change-set that undoes it.
pub fn convert_images(args: &ImageArgs) -> Result<(String, ChangeSet)> {
    convert(args, true)
}

/// The same as a dry run.
pub fn plan_convert_images(args: &ImageArgs) -> Result<ChangeSet> {
    convert(args, false).map(|(_, change_set)| change_set)
}

fn convert(args: &ImageArgs, recording: bool) -> Result<(String, ChangeSet)> {
    let target = target_of(args)?;
    let ext = target.extension();
    if args.path.is_file() {
        let output = args
            .out
            .clone()
            .unwrap_or_else(|| args.path.with_extension(ext));
        // The output is named by its file name alone, so only that has to survive as text.
        texture_path(Path::new(output.file_name().unwrap_or_default()))?;
        let texture = convert_file(&args.path, target)?;
        let summary = format!(
            "converted {} -> {} ({} bytes)",
            args.path.display(),
            output.display(),
            texture.len()
        );
        let mut txn = output_txn(&output, recording);
        txn.write(file_rel(&output)?, texture)?;
        return Ok((summary, txn.finish()?));
    }

    let pngs = walk_files(&args.path, false, |_, name| is_png(name));
    ensure!(
        !pngs.is_empty(),
        "no PNG files under {}",
        args.path.display()
    );
    let root = args.out.clone().unwrap_or_else(|| args.path.clone());
    let mut names = Vec::with_capacity(pngs.len());
    let mut seen: HashMap<String, (String, String)> = HashMap::new();
    for png in &pngs {
        let rel = png.strip_prefix(&args.path)?;
        let source = texture_path(rel)?;
        let texture = texture_path(&rel.with_extension(ext))?;
        let key = texture.to_ascii_lowercase();
        if let Some((first_source, first_texture)) = seen.get(&key) {
            bail!("{first_source} and {source} would both be written as {first_texture}");
        }
        seen.insert(key, (source, texture.clone()));
        names.push((png, texture));
    }
    let mut txn = dir_txn(&root, recording);
    for (png, name) in names {
        let texture = convert_file(png, target)?;
        txn.write(name, texture)?;
    }
    let count = match pngs.len() {
        1 => "1 PNG".to_owned(),
        n => format!("{n} PNGs"),
    };
    let summary = format!("converted {count} to .{ext} under {}", root.display());
    Ok((summary, txn.finish()?))
}

/// The texture `args` asks for; a bad argument is named in the error.
pub fn target_of(args: &ImageArgs) -> Result<Target> {
    ensure!(args.path.exists(), "{} does not exist", args.path.display());
    if args.path.is_file() && !is_png(&args.path) {
        bail!("{} is not a PNG file", args.path.display());
    }
    let overwrites_input = |out: &PathBuf| out.canonicalize().ok() == args.path.canonicalize().ok();
    if args.path.is_file() && args.out.as_ref().is_some_and(overwrites_input) {
        bail!("--out is the PNG itself; choose another file");
    }
    match (args.to.as_str(), args.format.as_deref()) {
        ("edds", None) => Ok(Target::Edds),
        ("edds", Some(format)) => {
            bail!("format {format} only applies to paa; leave it out for edds")
        }
        ("paa", None) => Ok(Target::Paa(None)),
        ("paa", Some("dxt1")) => Ok(Target::Paa(Some(PaaFormat::Dxt1))),
        ("paa", Some("dxt5")) => Ok(Target::Paa(Some(PaaFormat::Dxt5))),
        ("paa", Some(format)) => bail!("unknown format {format}; use dxt1 or dxt5"),
        (to, _) => bail!("unknown output {to}; use edds or paa"),
    }
}

fn convert_file(png: &Path, target: Target) -> Result<Vec<u8>> {
    let bytes = std::fs::read(png).map_err(|e| anyhow!("{}: {e}", png.display()))?;
    convert_png(&bytes, target).map_err(|e| anyhow!("{}: {e}", png.display()))
}

fn is_png(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

fn texture_path(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|component| {
            component.as_os_str().to_str().ok_or_else(|| {
                anyhow!(
                    "{} is not valid UTF-8; rename it before converting",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}
