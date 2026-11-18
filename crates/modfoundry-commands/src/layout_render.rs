//! `layout.render`: draws a `.layout` from the game's files, its dependencies and the mod's own files.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Result;
use modfoundry_layout::geometry::{resolve_with, Screen, WidgetBox};
use modfoundry_layout::imageset::{parse_imageset, Imageset};
use modfoundry_layout::parse::parse;
use modfoundry_layout::stringtable::{parse_stringtable, Stringtable};
use modfoundry_layout::styles::{style_colors, style_fonts, style_images, StyleImages};
use modfoundry_layout_render::{render, GameText, RenderOptions, Resources};
use modfoundry_pbo::PboArchive;
use modfoundry_texture::png::encode_png;
use modfoundry_texture::RgbaImage;
use modfoundry_vfs::{root_name, ChangeSet, WriteTxn};
use serde::Serialize;

use crate::layout_check::mod_root;
use crate::paths::{ends_with_path, extension, norm};
use crate::prefix_marker::Markers;
use crate::registry::FindingReport;
use crate::search_index::{addon_pbos, game_pbos, pbo_prefix};
use crate::sources::{has_ext, open_pbo};
use crate::walk::{entry_path, rel_path, walk_layout_tree};
use crate::{file_rel, output_dir};

/// What a render draws and from where.
pub struct RenderRequest {
    pub path: PathBuf,
    pub root: Option<PathBuf>,
    pub game: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub size: (u32, u32),
    pub background: [u8; 4],
    pub show_hidden: bool,
    pub language: String,
    pub boxes: bool,
}

/// The parsed arguments of a render; a size, backdrop or language left out takes its default in `RenderRequest::new`.
pub struct RenderArgs {
    pub path: PathBuf,
    pub root: Option<PathBuf>,
    pub dependencies: Vec<PathBuf>,
    pub size: Option<(u32, u32)>,
    pub background: Option<[u8; 4]>,
    pub show_hidden: bool,
    pub language: Option<String>,
    pub boxes: bool,
}

impl RenderRequest {
    /// The request for `args` drawn from `game`, with the defaults filled in.
    pub fn new(args: RenderArgs, game: PathBuf) -> Self {
        Self {
            path: args.path,
            root: args.root,
            game,
            dependencies: args.dependencies,
            size: args.size.unwrap_or(DEFAULT_SIZE),
            background: args
                .background
                .unwrap_or(RenderOptions::default().background),
            show_hidden: args.show_hidden,
            language: args.language.unwrap_or_else(|| "english".to_owned()),
            boxes: args.boxes,
        }
    }
}

/// The picture and the report of one render.
pub struct RenderOutput {
    pub image: RgbaImage,
    pub report: RenderReport,
}

/// What a render drew, what it could not draw, and the boxes when asked for.
#[derive(Debug, Serialize)]
pub struct RenderReport {
    pub layout: PathBuf,
    pub width: u32,
    pub height: u32,
    pub widgets: usize,
    pub drawn: usize,
    pub hidden_skipped: usize,
    pub findings: Vec<FindingReport>,
    pub warnings: Vec<String>,
    pub boxes: Option<Vec<BoxReport>>,
}

/// One widget's box as the geometry placed it.
#[derive(Debug, Serialize)]
pub struct BoxReport {
    pub name: Option<String>,
    pub class: String,
    pub line: u32,
    pub column: u32,
    pub depth: u32,
    pub draw_order: u32,
    pub rect: [f32; 4],
    pub clip: Option<[f32; 4]>,
    pub visible: bool,
}

/// Draws the `.layout` at `req.path`; what is not drawn as the game draws it comes back as findings.
pub fn render_layout(req: &RenderRequest) -> Result<RenderOutput, String> {
    if !req.path.is_file() {
        return Err(format!("{} is not a file", req.path.display()));
    }
    let bytes = fs::read(&req.path).map_err(|err| format!("{}: {err}", req.path.display()))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let widgets = parse(&text).map_err(|err| format!("{}: {err}", req.path.display()))?;
    let root = mod_root(&req.path, req.root.as_deref());
    let index = FileIndex::build(&req.game, &req.dependencies, &root, &req.language);
    match index.text_table() {
        Some(table) => {
            let languages = table.languages();
            if !languages
                .iter()
                .any(|language| language.eq_ignore_ascii_case(&req.language))
            {
                return Err(format!(
                    "language {} is not in the stringtable; it has {}",
                    req.language,
                    languages.join(", ")
                ));
            }
        }
        None => index.warn("no stringtable was found; text keys are drawn as written".to_owned()),
    }
    let layout = resolve_with(
        &widgets,
        &text,
        Screen {
            width: req.size.0,
            height: req.size.1,
        },
        &GameText::new(&index),
    );
    let options = RenderOptions {
        background: req.background,
        show_hidden: req.show_hidden,
    };
    let rendered = render(&layout, &widgets, &text, &options, &index);
    let widget_count = layout.boxes.len();
    let boxes = req
        .boxes
        .then(|| layout.boxes.iter().map(box_report).collect());
    let findings = layout
        .findings
        .into_iter()
        .chain(rendered.findings)
        .map(|diagnostic| FindingReport {
            diagnostic,
            path: req.path.clone(),
        })
        .collect();
    Ok(RenderOutput {
        image: rendered.image,
        report: RenderReport {
            layout: req.path.clone(),
            width: req.size.0,
            height: req.size.1,
            widgets: widget_count,
            drawn: rendered.drawn,
            hidden_skipped: rendered.hidden_skipped,
            findings,
            warnings: index.into_warnings(),
            boxes,
        },
    })
}

fn box_report(item: &WidgetBox) -> BoxReport {
    BoxReport {
        name: item.name.clone(),
        class: item.class.clone(),
        line: item.line,
        column: item.column,
        depth: item.depth,
        draw_order: item.draw_order,
        rect: [item.rect.x, item.rect.y, item.rect.w, item.rect.h],
        clip: item.clip.map(|clip| [clip.x, clip.y, clip.w, clip.h]),
        visible: item.visible,
    }
}

/// The change-set of writing `image` as a PNG to `out`; nothing is written unless `write` is set.
pub fn picture_change(out: &Path, image: &RgbaImage, write: bool) -> Result<ChangeSet> {
    let bytes = encode_png(image)?;
    let dir = std::path::absolute(output_dir(out))?;
    let root = root_name("mod", &dir);
    let mut txn = if write {
        WriteTxn::unrecorded(root, dir)
    } else {
        WriteTxn::dry_run(root, dir)
    };
    txn.write(file_rel(out)?, bytes)?;
    Ok(txn.finish()?)
}

/// One file a render can read: its normalised virtual path and where its bytes come from.
struct Item {
    path: String,
    body: Body,
}

enum Body {
    Pbo { archive: usize, entry: usize },
    Loose(PathBuf),
}

/// The game's PBOs, the dependencies and the mod's loose files, read through the render's `Resources` port.
/// Later sources win for the same virtual path.
struct FileIndex {
    language: String,
    archives: Vec<PboArchive>,
    items: Vec<Item>,
    by_path: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
    warned: RefCell<HashSet<String>>,
    warnings: RefCell<Vec<String>>,
    image_sets: OnceCell<HashMap<String, Rc<Imageset>>>,
    styles: OnceCell<StyleTables>,
    text: OnceCell<Option<Stringtable>>,
}

impl FileIndex {
    fn build(game: &Path, dependencies: &[PathBuf], root: &Path, language: &str) -> Self {
        let mut index = Self {
            language: language.to_owned(),
            archives: Vec::new(),
            items: Vec::new(),
            by_path: HashMap::new(),
            by_name: HashMap::new(),
            warned: RefCell::new(HashSet::new()),
            warnings: RefCell::new(Vec::new()),
            image_sets: OnceCell::new(),
            styles: OnceCell::new(),
            text: OnceCell::new(),
        };
        for pbo in game_pbos(game) {
            index.add_pbo(&pbo);
        }
        for dep in dependencies {
            index.add_dependency(dep);
        }
        index.add_mod_root(root);
        index
    }

    fn into_warnings(self) -> Vec<String> {
        self.warnings.into_inner()
    }

    fn warn(&self, message: String) {
        self.warnings.borrow_mut().push(message);
    }

    fn collect<T>(&self, learn: impl FnOnce(&mut Vec<String>) -> T) -> T {
        let mut found = Vec::new();
        let value = learn(&mut found);
        self.warnings.borrow_mut().extend(found);
        value
    }

    fn add_dependency(&mut self, dep: &Path) {
        if !dep.exists() {
            self.warn(format!("dependency {} not found", dep.display()));
        } else if dep.is_file() {
            if has_ext(dep, "pbo") {
                self.add_pbo(dep);
            } else {
                self.warn(format!("{} is not a .pbo or a folder", dep.display()));
            }
        } else if let Some(pbos) = addon_pbos(dep) {
            for pbo in pbos {
                self.add_pbo(&pbo);
            }
        } else {
            self.add_loose_folder(dep);
        }
    }

    fn add_pbo(&mut self, pbo: &Path) {
        let Some(archive) = self.collect(|found| open_pbo(pbo, found)) else {
            return;
        };
        let prefix = pbo_prefix(&archive);
        let archive_index = self.archives.len();
        for (entry, info) in archive.entries().iter().enumerate() {
            let path = norm(&entry_path(&prefix, &info.name_str()));
            self.by_path.insert(path.clone(), self.items.len());
            self.items.push(Item {
                path,
                body: Body::Pbo {
                    archive: archive_index,
                    entry,
                },
            });
        }
        self.archives.push(archive);
    }

    fn add_loose_folder(&mut self, root: &Path) {
        let mut markers = Markers::new(root);
        for path in self.walk(root) {
            let rel = markers.virtual_rel(&path);
            self.add_loose(&rel, path);
        }
    }

    fn add_mod_root(&mut self, root: &Path) {
        if !root.is_dir() {
            self.warn(format!("mod root {} is not a folder", root.display()));
            return;
        }
        for path in self.walk(root) {
            let rel = rel_path(root, &path);
            self.add_loose(&rel, path);
        }
    }

    fn walk(&self, root: &Path) -> Vec<PathBuf> {
        self.collect(|found| walk_layout_tree(root, |_, _| true, found))
    }

    fn add_loose(&mut self, rel: &str, path: PathBuf) {
        let rel = norm(rel);
        let name = rel.rsplit('/').next().unwrap_or_default().to_owned();
        let index = self.items.len();
        self.items.push(Item {
            path: rel,
            body: Body::Loose(path),
        });
        self.by_name.entry(name).or_default().push(index);
    }

    fn read(&self, index: usize) -> Result<Vec<u8>, String> {
        let item = &self.items[index];
        let result = match &item.body {
            Body::Pbo { archive, entry } => self.archives[*archive]
                .read_entry(*entry)
                .map_err(|err| err.to_string()),
            Body::Loose(path) => fs::read(path).map_err(|err| err.to_string()),
        };
        if let Err(err) = &result {
            if self.warned.borrow_mut().insert(item.path.clone()) {
                self.warn(format!("{}: {err}", item.path));
            }
        }
        result
    }

    fn text_of(&self, index: usize) -> Option<String> {
        self.read(index)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn indices_where(&self, keep: impl Fn(&str) -> bool) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&index| keep(&self.items[index].path))
            .collect()
    }

    fn image_sets(&self) -> &HashMap<String, Rc<Imageset>> {
        self.image_sets.get_or_init(|| {
            let mut sets = HashMap::new();
            for index in self.indices_where(|path| extension(path) == "imageset") {
                let Some(text) = self.text_of(index) else {
                    continue;
                };
                match parse_imageset(&text) {
                    Ok(set) => {
                        sets.insert(set.name.to_ascii_lowercase(), Rc::new(set));
                    }
                    Err(err) => self.warn(format!("{}: {err}", self.items[index].path)),
                }
            }
            sets
        })
    }

    fn style_tables(&self) -> &StyleTables {
        self.styles.get_or_init(|| {
            let mut tables = StyleTables::default();
            for index in self.indices_where(|path| extension(path) == "styles") {
                let Some(text) = self.text_of(index) else {
                    continue;
                };
                for (widget, style, font) in style_fonts(&text) {
                    tables.fonts.insert(style_key(&widget, &style), font);
                }
                for (widget, style, colour) in style_colors(&text) {
                    tables.colours.insert(style_key(&widget, &style), colour);
                }
                for (widget, style, images) in style_images(&text) {
                    tables.images.insert(style_key(&widget, &style), images);
                }
            }
            tables
        })
    }

    fn text_table(&self) -> Option<&Stringtable> {
        self.text
            .get_or_init(|| {
                let mut table: Option<Stringtable> = None;
                let indices =
                    self.indices_where(|path| path.rsplit('/').next() == Some("stringtable.csv"));
                for index in indices {
                    let Some(text) = self.text_of(index) else {
                        continue;
                    };
                    match parse_stringtable(&text) {
                        Ok(next) => match table.as_mut() {
                            Some(merged) => merged.merge(next),
                            None => table = Some(next),
                        },
                        Err(err) => self.warn(format!("{}: {err}", self.items[index].path)),
                    }
                }
                table
            })
            .as_ref()
    }
}

impl Resources for FileIndex {
    fn imageset(&self, name: &str) -> Option<Rc<Imageset>> {
        self.image_sets().get(&name.to_ascii_lowercase()).cloned()
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        let wanted = norm(path);
        let name = wanted.rsplit('/').next().unwrap_or_default();
        let exact = self.by_path.get(&wanted).copied();
        let loose = self
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&index| ends_with_path(&wanted, &self.items[index].path));
        let winner = exact.into_iter().chain(loose).max_by_key(|&index| {
            let path = &self.items[index].path;
            (*path == wanted, path.len(), index)
        })?;
        self.read(winner).ok()
    }

    fn text(&self, key: &str) -> Option<String> {
        self.text_table()?
            .lookup(key, &self.language)
            .map(str::to_owned)
    }

    fn font_style(&self, class: &str, style: &str) -> Option<String> {
        self.style_tables()
            .fonts
            .get(&style_key(class, style))
            .cloned()
    }

    fn style_color(&self, class: &str, style: &str) -> Option<u32> {
        self.style_tables()
            .colours
            .get(&style_key(class, style))
            .copied()
    }

    fn style_images(&self, class: &str, style: &str) -> Option<StyleImages> {
        self.style_tables()
            .images
            .get(&style_key(class, style))
            .cloned()
    }
}

#[derive(Default)]
struct StyleTables {
    fonts: HashMap<(String, String), String>,
    colours: HashMap<(String, String), u32>,
    images: HashMap<(String, String), StyleImages>,
}

fn style_key(widget: &str, style: &str) -> (String, String) {
    (widget.to_ascii_lowercase(), style.to_ascii_lowercase())
}

/// `WIDTHxHEIGHT` from the `size` argument, within the largest picture the game takes.
pub fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text
        .split_once(['x', 'X'])
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .ok_or_else(|| format!("size {text} must be WIDTHxHEIGHT"))?;
    if width == 0 || height == 0 {
        return Err("size must be at least 1x1".to_owned());
    }
    if width > MAX_WIDTH || height > MAX_HEIGHT {
        return Err(format!(
            "size {width}x{height} is over the {MAX_WIDTH}x{MAX_HEIGHT} limit"
        ));
    }
    Ok((width, height))
}

/// The `out` argument names a PNG file.
pub fn check_out(out: &Path) -> Result<(), String> {
    if out.is_dir() {
        return Err(format!("out {} is a folder, not a file", out.display()));
    }
    if out
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        Ok(())
    } else {
        Err("out must be a .png file".to_owned())
    }
}

/// `#rrggbb` or `#rrggbbaa` from the `background` argument; without alpha the backdrop is opaque.
pub fn parse_background(text: &str) -> Result<[u8; 4], String> {
    let refused = || format!("background {text} must be #rrggbb or #rrggbbaa");
    let hex = text.strip_prefix('#').ok_or_else(refused)?;
    let digits_ok = matches!(hex.len(), 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !digits_ok {
        return Err(refused());
    }
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| refused());
    let alpha = if hex.len() == 8 { byte(6)? } else { 255 };
    Ok([byte(0)?, byte(2)?, byte(4)?, alpha])
}

const MAX_WIDTH: u32 = 7680;
const MAX_HEIGHT: u32 = 4320;
const DEFAULT_SIZE: (u32, u32) = (1920, 1080);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_file_is_warned_about_once() {
        let mut index = FileIndex::build(
            Path::new("no-such-game"),
            &[],
            Path::new("no-such-root"),
            "english",
        );
        index.add_loose("gui/gone.edds", PathBuf::from("no-such-root/gone.edds"));

        assert!(index.file("gui/gone.edds").is_none());
        assert!(index.file("gui/gone.edds").is_none());

        let warnings = index.into_warnings();
        assert_eq!(
            warnings.iter().filter(|w| w.contains("gone.edds")).count(),
            1,
            "{warnings:?}"
        );
    }

    #[test]
    fn a_style_colour_is_read_from_a_styles_file_ignoring_case() {
        let dir = std::env::temp_dir().join(format!("mf-style-colour-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dayzwidgets.styles");
        fs::write(
            &path,
            r#"<Widgets>
  <Widget Name="WrapSpacerWidget">
    <Style Name="Colorable" Font="gui/fonts/X" Color="0"/>
  </Widget>
</Widgets>"#,
        )
        .unwrap();
        let mut index = FileIndex::build(
            Path::new("no-such-game"),
            &[],
            Path::new("no-such-root"),
            "english",
        );
        index.add_loose("looknfeel/dayzwidgets.styles", path);

        assert_eq!(index.style_color("WrapSpacerWidget", "Colorable"), Some(0));
        assert_eq!(index.style_color("wrapspacerwidget", "COLORABLE"), Some(0));
        assert_eq!(index.style_color("WrapSpacerWidget", "Plain"), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_style_images_row_is_read_from_a_styles_file_ignoring_case() {
        let dir = std::env::temp_dir().join(format!("mf-style-images-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dayzwidgets.styles");
        fs::write(
            &path,
            r#"<Widgets>
  <Widget Name="PanelWidget">
    <Style Name="DayZDefaultPanel" ImageSet="dayz_gui">
      <State Name="Normal">
        <Item Name="Center" Image="DayZDefaultPanelCorner" />
      </State>
    </Style>
    <Style Name="Plain" ImageSet="dayz_gui"/>
  </Widget>
</Widgets>"#,
        )
        .unwrap();
        let mut index = FileIndex::build(
            Path::new("no-such-game"),
            &[],
            Path::new("no-such-root"),
            "english",
        );
        index.add_loose("looknfeel/dayzwidgets.styles", path);

        let images = index
            .style_images("PanelWidget", "DayZDefaultPanel")
            .expect("the style has a row");
        assert_eq!(images.imageset, "dayz_gui");
        assert_eq!(
            images.slots,
            [("Center".to_owned(), "DayZDefaultPanelCorner".to_owned())]
        );
        assert!(index
            .style_images("panelwidget", "dayzdefaultpanel")
            .is_some());
        assert_eq!(index.style_images("PanelWidget", "Unknown"), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
