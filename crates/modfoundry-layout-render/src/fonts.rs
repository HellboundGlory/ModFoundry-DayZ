//! The game's `.fnt` fonts by name and text size, each parsed once per cache; the painter and the measurer each hold one.

use std::collections::HashMap;
use std::rc::Rc;

use modfoundry_font::{parse_fnt, FntFont};

use crate::resources::Resources;

#[derive(Clone)]
pub(crate) enum Failure {
    NotFound,
    Unreadable(String),
}

#[derive(Default)]
pub(crate) struct FontCache {
    stems: HashMap<(String, u32), Result<String, Failure>>,
    fonts: HashMap<String, Result<Rc<FntFont>, Failure>>,
}

impl FontCache {
    /// The `.fnt` stem a font name stands for at `em_px`, with the font parsed from it.
    pub(crate) fn load(
        &mut self,
        resources: &dyn Resources,
        name: &str,
        em_px: f32,
    ) -> Result<(String, Rc<FntFont>), Failure> {
        let key = (name.to_ascii_lowercase(), em_px.to_bits());
        let stem = self
            .stems
            .entry(key)
            .or_insert_with(|| font_stem(resources, name, em_px))
            .clone()?;
        let font = self.font(resources, &stem)?;
        Ok((stem, font))
    }

    fn font(&mut self, resources: &dyn Resources, stem: &str) -> Result<Rc<FntFont>, Failure> {
        self.fonts
            .entry(stem.to_ascii_lowercase())
            .or_insert_with(|| parse_font(resources, stem))
            .clone()
    }
}

/// The `.fnt` stem a font name stands for: the name itself, or the family member nearest the text size.
fn font_stem(resources: &dyn Resources, name: &str, em_px: f32) -> Result<String, Failure> {
    if resources.file(&format!("{name}.fnt")).is_some() {
        return Ok(name.to_owned());
    }
    let xml = resources
        .file(&format!("{name}.xml"))
        .ok_or(Failure::NotFound)?;
    let size = nearest_size(&listed_sizes(&xml), em_px).ok_or(Failure::NotFound)?;
    Ok(format!("{name}{size}"))
}

fn listed_sizes(xml: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(xml)
        .split("size=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next()?.parse().ok())
        .collect()
}

fn nearest_size(sizes: &[u32], em_px: f32) -> Option<u32> {
    sizes.iter().copied().min_by(|&a, &b| {
        let distance = |size: u32| (size as f32 - em_px).abs();
        distance(a).total_cmp(&distance(b)).then(b.cmp(&a))
    })
}

fn parse_font(resources: &dyn Resources, stem: &str) -> Result<Rc<FntFont>, Failure> {
    let bytes = resources
        .file(&format!("{stem}.fnt"))
        .ok_or(Failure::NotFound)?;
    parse_fnt(&bytes)
        .map(Rc::new)
        .map_err(|error| Failure::Unreadable(format!("{error:#}")))
}
