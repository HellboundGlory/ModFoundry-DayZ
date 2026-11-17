//! `imageset.build`: a folder of PNGs packed into one DayZ texture set.

use crate::{file_rel, output_dir, output_txn};
use anyhow::{anyhow, bail, ensure, Result};
use modfoundry_texture::imageset::{build_set, SetImage};
use modfoundry_texture::png::decode_png;
use modfoundry_texture::RgbaImage;
use modfoundry_vfs::ChangeSet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Arguments for `imageset.build`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesetArgs {
    /// The folder whose PNGs become the set's images; its subfolders are not read.
    pub path: PathBuf,
    /// The `.imageset` file to write; the textures are written beside it.
    pub out: PathBuf,
    /// The in-game folder the textures are packed at.
    pub game_dir: String,
}

/// Packs the folder; returns the summary line and the change-set that undoes it.
pub fn build_imageset(args: &ImagesetArgs) -> Result<(String, ChangeSet)> {
    imageset(args, true)
}

/// The same build as a dry run: the change-set it would apply.
pub fn plan_build_imageset(args: &ImagesetArgs) -> Result<ChangeSet> {
    imageset(args, false).map(|(_, change_set)| change_set)
}

/// Refuses bad arguments before any PNG is read.
pub fn check_args(args: &ImagesetArgs) -> Result<()> {
    ensure!(
        args.path.is_dir(),
        "{} is not a folder",
        args.path.display()
    );
    ensure!(
        args.out
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("imageset")),
        "{} is not an .imageset file",
        args.out.display()
    );
    ensure!(
        output_dir(&args.out).is_dir(),
        "{} is not in an existing folder",
        args.out.display()
    );
    Ok(())
}

fn imageset(args: &ImagesetArgs, recording: bool) -> Result<(String, ChangeSet)> {
    check_args(args)?;
    let images = read_images(&args.path)?;
    let count = images.len();
    let name = args
        .out
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| anyhow!("{} has no set name", args.out.display()))?;
    let built = build_set(&name, &args.game_dir, images)?;
    let (width, height) = ref_size(&built.text)?;

    let mut txn = output_txn(&args.out, recording);
    txn.write(file_rel(&args.out)?, built.text.into_bytes())?;
    txn.write(format!("{name}.edds"), built.texture)?;
    let doubles = built.double_texture.is_some();
    if let Some(texture) = built.double_texture {
        txn.write(format!("{name}@2x.edds"), texture)?;
    }
    let suffix = if doubles { ", plus @2x" } else { "" };
    let images = if count == 1 { "image" } else { "images" };
    let summary = format!(
        "packed {count} {images} into {} ({width}x{height}){suffix}",
        args.out.display()
    );
    Ok((summary, txn.finish()?))
}

fn read_images(folder: &Path) -> Result<Vec<SetImage>> {
    let pngs = png_files(folder)?;
    ensure!(!pngs.is_empty(), "no PNG files in {}", folder.display());
    // Keyed ignoring case, as image names are unique ignoring case.
    let mut doubles: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut singles: Vec<(String, PathBuf)> = Vec::new();
    for path in pngs {
        let stem = stem_of(&path);
        let (base, double) = split_double(&stem);
        if !double {
            singles.push((base.to_owned(), path));
        } else if let Some(first) = doubles.get(&base.to_ascii_lowercase()) {
            bail!(
                "{} and {} both name the 2x image of {base}",
                first.display(),
                path.display()
            );
        } else {
            doubles.insert(base.to_ascii_lowercase(), path);
        }
    }
    let has_single = |key: &String| {
        singles
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(key))
    };
    if let Some(path) = doubles
        .iter()
        .find(|(key, _)| !has_single(key))
        .map(|(_, path)| path)
    {
        bail!("{} has no 1x image beside it", path.display());
    }
    let mut images = Vec::with_capacity(singles.len());
    for (name, path) in singles {
        let double = doubles
            .remove(&name.to_ascii_lowercase())
            .map(|path| read_png(&path))
            .transpose()?;
        images.push(SetImage {
            name,
            image: read_png(&path)?,
            double,
        });
    }
    Ok(images)
}

/// The stem and whether it names the double of the stem before `@2x`.
fn split_double(stem: &str) -> (&str, bool) {
    let start = stem.len().saturating_sub(3);
    match stem.get(start..) {
        Some(suffix) if suffix.eq_ignore_ascii_case("@2x") => (&stem[..start], true),
        _ => (stem, false),
    }
}

/// The PNG files directly in `folder`, by file name; links are followed, AppleDouble sidecars skipped.
fn png_files(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(folder)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !is_appledouble(path))
        .filter(|path| is_png(path))
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(files)
}

/// macOS writes a `._` metadata sidecar beside each copied file; it is never an image.
fn is_appledouble(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("._"))
}

fn is_png(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_png(path: &Path) -> Result<RgbaImage> {
    let bytes = fs::read(path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    decode_png(&bytes).map_err(|e| anyhow!("{}: {e}", path.display()))
}

/// The atlas size `build_set` wrote into the text as its `RefSize`.
fn ref_size(text: &str) -> Result<(u32, u32)> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("RefSize "))
        .ok_or_else(|| anyhow!("the built set has no texture size"))?;
    let (width, height) = value
        .split_once(' ')
        .ok_or_else(|| anyhow!("the built set's texture size is malformed"))?;
    Ok((width.parse()?, height.parse()?))
}
