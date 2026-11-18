//! Measures a text widget's text with the game's fonts, so size to text fits the picture the painter draws.

use std::cell::RefCell;

use modfoundry_font::{layout_text, HAlign, TextOptions};
use modfoundry_layout::geometry::{TextMeasure, TextSize};
use modfoundry_layout::parse::Widget;

use crate::fonts::FontCache;
use crate::resources::{normalize_path, Resources};
use crate::text_paint::{cut_long, font_name, stringtable_text, text_of};

/// Answers the layout crate's text measure from the game's fonts, reading them through `Resources`.
pub struct GameText<'a> {
    resources: &'a dyn Resources,
    fonts: RefCell<FontCache>,
}

impl<'a> GameText<'a> {
    /// A measurer that reads fonts and stringtable text through `resources`.
    pub fn new(resources: &'a dyn Resources) -> Self {
        Self {
            resources,
            fonts: RefCell::new(FontCache::default()),
        }
    }
}

impl TextMeasure for GameText<'_> {
    fn measure(&self, widget: &Widget, em_px: f32, wrap_width: Option<f32>) -> Option<TextSize> {
        let mut scratch: Vec<(&'static str, String)> = Vec::new();
        let raw = text_of(widget)?;
        let text = stringtable_text(self.resources, raw, widget, &mut scratch);
        let text = cut_long(text, widget, &mut scratch);
        let name =
            font_name(self.resources, widget, &mut scratch).map(|name| normalize_path(&name))?;
        let (_, font) = self
            .fonts
            .borrow_mut()
            .load(self.resources, &name, em_px)
            .ok()?;
        let options = TextOptions {
            em_px,
            wrap_width,
            halign: HAlign::Left,
        };
        let block = layout_text(&font, &text, &options);
        Some(TextSize {
            width: block.width,
            height: block.height,
        })
    }
}
