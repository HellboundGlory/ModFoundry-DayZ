use modfoundry_layout_render::Rendered;
use modfoundry_texture::RgbaImage;

pub fn pixel(rendered: &Rendered, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * rendered.image.width + x) * 4) as usize;
    rendered.image.pixels[at..at + 4].try_into().unwrap()
}

pub fn solid_image(colour: [u8; 4]) -> RgbaImage {
    RgbaImage {
        width: 2,
        height: 2,
        pixels: colour.repeat(4),
    }
}
