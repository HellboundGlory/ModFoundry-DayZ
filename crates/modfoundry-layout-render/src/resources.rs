//! The outside world a render reads through: imagesets, files, stringtable text and style fonts.

use std::collections::HashMap;
use std::rc::Rc;

use modfoundry_layout::imageset::Imageset;
use modfoundry_layout::styles::StyleImages;

/// The files, imagesets, text and fonts a render reads from outside.
pub trait Resources {
    /// The imageset named `name` (the `<set>` of `set:<set>`), ASCII case ignored by the implementation. Shared,
    /// because every picture widget asks for it.
    fn imageset(&self, name: &str) -> Option<Rc<Imageset>>;
    /// The bytes of the file at a virtual path: `/` separators, no `{GUID}` prefix, no leading `/`. The
    /// implementation matches ignoring ASCII case and any folder prefix it cannot know.
    fn file(&self, path: &str) -> Option<Vec<u8>>;
    /// The stringtable text for a key written without its leading `#`, in the implementation's language.
    fn text(&self, key: &str) -> Option<String>;
    /// The font a style names for a widget class: the `Font` of `<Widget Name="<class>"><Style Name="<style>">`.
    /// `class` has no `Class` suffix (`TextWidget`); the result is the written path (`gui/fonts/MetronBook`).
    fn font_style(&self, class: &str, style: &str) -> Option<String>;
    /// The `Color` (ARGB, 32-bit decimal) a style gives a widget class: the `Color` of `<Widget Name="<class>"><Style Name="<style>">`.
    /// `class` has no `Class` suffix, as for `font_style`.
    fn style_color(&self, class: &str, style: &str) -> Option<u32>;
    /// The `Normal` images of `<Widget Name="<class>"><Style Name="<style>">`; `None` when no `.styles` file has that style.
    /// `class` has no `Class` suffix, as for `font_style`.
    fn style_images(&self, _class: &str, _style: &str) -> Option<StyleImages> {
        None
    }
}

/// A port with nothing behind it.
pub struct NoResources;

impl Resources for NoResources {
    fn imageset(&self, _name: &str) -> Option<Rc<Imageset>> {
        None
    }

    fn file(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }

    fn text(&self, _key: &str) -> Option<String> {
        None
    }

    fn font_style(&self, _class: &str, _style: &str) -> Option<String> {
        None
    }

    fn style_color(&self, _class: &str, _style: &str) -> Option<u32> {
        None
    }
}

/// A port over maps, for tests and callers that hold everything in memory. Imageset and text keys are
/// looked up lowercased, so store them lowercase; file keys are matched exactly after `normalize_path`.
#[derive(Default)]
pub struct MemoryResources {
    pub imagesets: HashMap<String, Imageset>,
    pub files: HashMap<String, Vec<u8>>,
    pub texts: HashMap<String, String>,
    pub font_styles: HashMap<(String, String), String>,
    pub style_colors: HashMap<(String, String), u32>,
    pub style_images: HashMap<(String, String), StyleImages>,
}

impl Resources for MemoryResources {
    fn imageset(&self, name: &str) -> Option<Rc<Imageset>> {
        self.imagesets
            .get(&name.to_ascii_lowercase())
            .cloned()
            .map(Rc::new)
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(&normalize_path(path)).cloned()
    }

    fn text(&self, key: &str) -> Option<String> {
        self.texts.get(&key.to_ascii_lowercase()).cloned()
    }

    fn font_style(&self, class: &str, style: &str) -> Option<String> {
        self.font_styles
            .get(&(class.to_owned(), style.to_owned()))
            .cloned()
    }

    fn style_color(&self, class: &str, style: &str) -> Option<u32> {
        self.style_colors
            .get(&(class.to_owned(), style.to_owned()))
            .copied()
    }

    fn style_images(&self, class: &str, style: &str) -> Option<StyleImages> {
        self.style_images
            .get(&(class.to_owned(), style.to_owned()))
            .cloned()
    }
}

/// `\` to `/`, a leading `{...}` GUID group removed, leading `/` removed. Case is kept.
pub fn normalize_path(path: &str) -> String {
    let forward = path.replace('\\', "/");
    let without_guid = forward
        .strip_prefix('{')
        .and_then(|rest| rest.split_once('}'))
        .map_or(forward.as_str(), |(_, after)| after);
    without_guid
        .strip_prefix('/')
        .unwrap_or(without_guid)
        .to_owned()
}
