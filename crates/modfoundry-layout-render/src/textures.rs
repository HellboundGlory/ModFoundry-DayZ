//! The decode cache: each picture a render reads is decoded once, failures included.

use std::collections::HashMap;
use std::rc::Rc;

use modfoundry_texture::{edds, paa, png, RgbaImage};

use crate::resources::{normalize_path, Resources};

pub(crate) const FILE_NOT_FOUND: &str = "file not found";

#[derive(Default)]
pub(crate) struct Textures {
    decoded: HashMap<String, Result<Rc<RgbaImage>, String>>,
}

impl Textures {
    pub(crate) fn get(
        &mut self,
        resources: &dyn Resources,
        path: &str,
    ) -> Result<Rc<RgbaImage>, String> {
        let key = normalize_path(path).to_ascii_lowercase();
        self.decoded
            .entry(key)
            .or_insert_with(|| decode(resources, path))
            .clone()
    }
}

fn decode(resources: &dyn Resources, path: &str) -> Result<Rc<RgbaImage>, String> {
    let bytes = resources
        .file(&normalize_path(path))
        .ok_or_else(|| FILE_NOT_FOUND.to_owned())?;
    let extension = path
        .rsplit_once('.')
        .map_or(String::new(), |(_, extension)| {
            extension.to_ascii_lowercase()
        });
    let levels = match extension.as_str() {
        "edds" => edds::decode_edds(&bytes).map_err(|error| error.to_string())?,
        "paa" => {
            paa::decode_paa(&bytes)
                .map_err(|error| error.to_string())?
                .1
        }
        "png" => vec![png::decode_png(&bytes).map_err(|error| error.to_string())?],
        other => return Err(format!("unsupported picture type .{other}")),
    };
    levels
        .into_iter()
        .next()
        .map(Rc::new)
        .ok_or_else(|| "no picture levels".to_owned())
}
