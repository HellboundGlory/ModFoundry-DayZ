use crate::RgbaImage;
use anyhow::{anyhow, Result};
use image::ImageFormat;

/// Decodes PNG bytes of any colour type and bit depth to 8-bit RGBA.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage> {
    let decoded = image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|err| anyhow!("not a readable PNG: {err}"))?
        .into_rgba8();
    Ok(RgbaImage {
        width: decoded.width(),
        height: decoded.height(),
        pixels: decoded.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, LumaA};
    use std::io::Cursor;

    fn png_bytes(pixels: Vec<u16>, width: u32) -> Vec<u8> {
        let buffer: ImageBuffer<LumaA<u16>, Vec<u16>> =
            ImageBuffer::from_raw(width, 1, pixels).unwrap();
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageLumaA16(buffer)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn sixteen_bit_grey_and_alpha_decodes_to_eight_bit_rgba() {
        let bytes = png_bytes(vec![0xFFFF, 0x0000, 0x8080, 0xFFFF], 2);

        let image = decode_png(&bytes).unwrap();

        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels, vec![255, 255, 255, 0, 128, 128, 128, 255]);
    }

    #[test]
    fn bytes_that_are_not_a_png_are_refused() {
        let error = decode_png(b"not a png at all").unwrap_err();

        assert!(error.to_string().starts_with("not a readable PNG: "));
    }
}
