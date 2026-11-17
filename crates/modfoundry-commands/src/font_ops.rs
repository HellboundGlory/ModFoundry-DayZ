//! `font.build`: a TTF or OTF into DayZ .fnt and .edds files at several sizes, or one distance-field font.

use crate::dir_txn;
use anyhow::{anyhow, ensure, Result};
use modfoundry_font::{
    build_field_font, build_font, render_field_font, render_font, CharSet, Sizing,
};
use modfoundry_vfs::ChangeSet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Arguments for `font.build`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontArgs {
    /// The TTF or OTF font to render.
    pub path: PathBuf,
    /// Pixel sizes to write.
    #[serde(default)]
    pub sizes: Vec<u32>,
    /// The folder the `.fnt`, `.edds` and `.xml` files are written into.
    pub out: PathBuf,
    /// The file name stem (default: the font file's stem, lowercased).
    #[serde(default)]
    pub name: Option<String>,
    /// `default` or `all`.
    #[serde(default)]
    pub chars: Option<String>,
    /// `line` (a size is the cell height, as vanilla's fonts) or `em` (as FontToTga).
    #[serde(default)]
    pub sizing: Option<String>,
    /// One distance-field font, drawn sharp at any size, instead of a font per size.
    #[serde(default)]
    pub msdf: bool,
}

/// Renders the font at each size; returns the summary line and the change-set that undoes it.
pub fn build_fonts(args: &FontArgs) -> Result<(String, ChangeSet)> {
    fonts(args, true)
}

/// The same build as a dry run: the change-set it would apply.
pub fn plan_build_fonts(args: &FontArgs) -> Result<ChangeSet> {
    fonts(args, false).map(|(_, change_set)| change_set)
}

/// Refuses bad arguments before the font is read.
pub fn check_args(args: &FontArgs) -> Result<()> {
    ensure!(args.path.is_file(), "{} is not a file", args.path.display());
    if args.msdf {
        ensure!(
            args.sizes.is_empty(),
            "--msdf bakes one font for every size; drop --sizes"
        );
        ensure!(
            args.sizing.is_none(),
            "--sizing is for bitmap sizes; --msdf has none"
        );
    } else {
        ensure!(!args.sizes.is_empty(), "no sizes given");
        for &size in &args.sizes {
            ensure!(
                (1..=512).contains(&size),
                "a size of {size} is outside 1 to 512 pixels"
            );
        }
        let mut unique = args.sizes.clone();
        unique.sort_unstable();
        unique.dedup();
        ensure!(unique.len() == args.sizes.len(), "a size is given twice");
    }
    ensure!(args.out.is_dir(), "{} is not a folder", args.out.display());
    if let Some(name) = &args.name {
        ensure!(!name.is_empty(), "the name is empty");
        ensure!(
            is_name(name),
            "the name `{name}` may only use a to z, digits and _"
        );
    }
    if let Some(chars) = &args.chars {
        ensure!(
            chars == "default" || chars == "all",
            "chars must be default or all, got `{chars}`"
        );
    }
    if let Some(sizing) = &args.sizing {
        ensure!(
            sizing == "line" || sizing == "em",
            "sizing must be line or em, got `{sizing}`"
        );
    }
    let stem = stem(args)?;
    // The longest file written is <stem><size>.edds (<stem>.edds with msdf), and HEAD holds the name in 254 bytes.
    let longest = if args.msdf { ".edds" } else { "512.edds" };
    ensure!(
        stem.len() + longest.len() <= 255,
        "the name `{stem}` is too long; pass a shorter --name"
    );
    Ok(())
}

fn fonts(args: &FontArgs, recording: bool) -> Result<(String, ChangeSet)> {
    check_args(args)?;
    let stem = stem(args)?;
    let bytes = fs::read(&args.path).map_err(|e| anyhow!("{}: {e}", args.path.display()))?;
    let chars = charset(args.chars.as_deref());

    if args.msdf {
        let mut txn = dir_txn(&args.out, recording);
        let in_file = |e: anyhow::Error| anyhow!("{}: {e}", args.path.display());
        let rendered = render_field_font(&bytes, chars).map_err(in_file)?;
        let built = build_field_font(&stem, &rendered).map_err(in_file)?;
        let glyphs = rendered.glyphs.len();
        txn.write(format!("{stem}.fnt"), built.fnt)?;
        txn.write(format!("{stem}.edds"), built.texture)?;
        let pairs = kerning_pairs(rendered.kerning.len());
        let summary = format!(
            "wrote {stem} as one distance-field font into {} ({glyphs} glyphs, {pairs})",
            args.out.display()
        );
        return Ok((summary, txn.finish()?));
    }

    let sizing = match args.sizing.as_deref() {
        Some("em") => Sizing::Em,
        _ => Sizing::Line,
    };
    let mut sizes = args.sizes.clone();
    sizes.sort_unstable();

    let mut txn = dir_txn(&args.out, recording);
    let mut glyphs = 0;
    let mut pairs = 0;
    for &size in &sizes {
        let at_size = |e: anyhow::Error| anyhow!("{} at {size} pixels: {e}", args.path.display());
        let rendered = render_font(&bytes, size, chars, sizing).map_err(at_size)?;
        if glyphs == 0 {
            glyphs = rendered.glyphs.len();
        }
        pairs = pairs.max(rendered.kerning.len());
        let built = build_font(&format!("{stem}{size}"), &rendered).map_err(at_size)?;
        txn.write(format!("{stem}{size}.fnt"), built.fnt)?;
        txn.write(format!("{stem}{size}.edds"), built.texture)?;
    }
    txn.write(format!("{stem}.xml"), size_list(&sizes).into_bytes())?;

    let sizes: Vec<String> = sizes.iter().map(u32::to_string).collect();
    let label = if sizes.len() == 1 { "size" } else { "sizes" };
    let summary = format!(
        "wrote {stem} at {label} {} into {} ({glyphs} glyphs, {})",
        sizes.join(", "),
        args.out.display(),
        kerning_pairs(pairs)
    );
    Ok((summary, txn.finish()?))
}

fn kerning_pairs(pairs: usize) -> String {
    let label = if pairs == 1 { "pair" } else { "pairs" };
    format!("{pairs} kerning {label}")
}

fn stem(args: &FontArgs) -> Result<String> {
    if let Some(name) = &args.name {
        return Ok(name.clone());
    }
    let stem = args
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = stem
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if is_name_char(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    ensure!(
        !cleaned.is_empty(),
        "the font file name has no usable stem; pass --name"
    );
    Ok(cleaned)
}

fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'
}

fn charset(chars: Option<&str>) -> CharSet {
    match chars {
        Some("all") => CharSet::All,
        _ => CharSet::Default,
    }
}

fn size_list(sizes: &[u32]) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<fontGenerator>\n");
    for size in sizes {
        xml.push_str(&format!("    <font size=\"{size}\"/>\n"));
    }
    xml.push_str("</fontGenerator>\n");
    xml
}
