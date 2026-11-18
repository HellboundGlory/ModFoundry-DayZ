//! What a painting step gets: the picture, the port, the decode cache and the screen height.

use crate::canvas::Canvas;
use crate::resources::Resources;
use crate::textures::Textures;

pub(crate) struct DrawContext<'a> {
    pub(crate) canvas: &'a mut Canvas,
    pub(crate) resources: &'a dyn Resources,
    pub(crate) textures: &'a mut Textures,
    pub(crate) screen_height: u32,
}
