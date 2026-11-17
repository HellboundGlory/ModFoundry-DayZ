//! Publishing a build to the Steam Workshop.

pub mod checks;
pub mod details;
pub mod facts;
pub mod hash;
pub mod meta;
pub mod preview;
pub mod tags;

/// Preview image extensions, in the order a mod's `workshop/` folder is searched.
pub const PREVIEW_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "bmp"];
