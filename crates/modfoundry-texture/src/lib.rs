//! ModFoundry DayZ textures: PNG to .edds and .paa, and back.

pub mod convert;
pub mod edds;
pub mod imageset;
pub mod mips;
pub mod paa;
pub mod png;

/// An 8-bit RGBA image, rows top to bottom, 4 bytes per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
