use crate::edds::encode_edds;
use crate::mips::{edds_level_count, mip_chain, paa_level_count};
use crate::paa::{encode_paa, PaaFormat};
use crate::png::decode_png;
use crate::RgbaImage;
use anyhow::{bail, Result};

/// What a PNG becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Edds,
    /// `None` picks DXT5 for an image with any see-through pixel, else DXT1.
    Paa(Option<PaaFormat>),
}

impl Target {
    /// The file extension it writes, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Target::Edds => "edds",
            Target::Paa(_) => "paa",
        }
    }
}

/// Converts PNG bytes into a finished texture file.
pub fn convert_png(png: &[u8], target: Target) -> Result<Vec<u8>> {
    let image = decode_png(png)?;
    match target {
        Target::Edds => {
            let count = edds_level_count(image.width, image.height);
            let levels = mip_chain(image, count);
            encode_edds(&levels)
        }
        Target::Paa(format) => {
            ensure_power_of_two_sides(&image)?;
            let format = format.unwrap_or_else(|| PaaFormat::for_image(&image));
            let count = paa_level_count(image.width, image.height);
            let levels = mip_chain(image, count);
            encode_paa(&levels, format)
        }
    }
}

fn ensure_power_of_two_sides(image: &RgbaImage) -> Result<()> {
    let (width, height) = (image.width, image.height);
    if width.is_power_of_two() && height.is_power_of_two() {
        return Ok(());
    }
    let below = |side: u32| 1u32 << (31 - side.leading_zeros());
    let above = |side: u32| side.next_power_of_two();
    bail!(
        "{width}x{height} is not a power-of-two size, which .paa needs; resize the PNG to {}x{} or {}x{}",
        below(width),
        below(height),
        above(width),
        above(height)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edds::decode_edds;
    use crate::paa::decode_paa;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let buffer = ImageBuffer::from_fn(width, height, |x, y| Rgba(pixel(x, y)));
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn gradient(x: u32, y: u32) -> [u8; 4] {
        [x as u8, y as u8, (x + y) as u8, 255]
    }

    #[test]
    fn edds_writes_all_eight_mip_levels_with_the_base_image_first() {
        let bytes = png(136, 112, gradient);

        let levels = decode_edds(&convert_png(&bytes, Target::Edds).unwrap()).unwrap();

        assert_eq!(levels.len(), 8);
        assert_eq!(levels[0], decode_png(&bytes).unwrap());
    }

    #[test]
    fn paa_without_a_format_is_dxt1_for_an_opaque_image_with_four_levels() {
        let bytes = png(64, 32, gradient);

        let (format, levels) =
            decode_paa(&convert_png(&bytes, Target::Paa(None)).unwrap()).unwrap();

        assert_eq!(format, PaaFormat::Dxt1);
        assert_eq!(levels.len(), 4);
    }

    #[test]
    fn paa_without_a_format_is_dxt5_when_a_pixel_is_see_through() {
        let bytes = png(64, 32, |x, y| {
            if (x, y) == (5, 5) {
                [0, 0, 0, 254]
            } else {
                gradient(x, y)
            }
        });

        let (format, _) = decode_paa(&convert_png(&bytes, Target::Paa(None)).unwrap()).unwrap();

        assert_eq!(format, PaaFormat::Dxt5);
    }

    #[test]
    fn paa_with_a_given_format_overrides_the_pick() {
        let bytes = png(64, 32, gradient);

        let (format, _) =
            decode_paa(&convert_png(&bytes, Target::Paa(Some(PaaFormat::Dxt5))).unwrap()).unwrap();

        assert_eq!(format, PaaFormat::Dxt5);
    }

    #[test]
    fn paa_refuses_sides_that_are_not_powers_of_two_and_names_the_nearest_sizes() {
        let cases = [
            (
                48,
                48,
                "48x48 is not a power-of-two size, which .paa needs; resize the PNG to 32x32 or 64x64",
            ),
            (
                48,
                64,
                "48x64 is not a power-of-two size, which .paa needs; resize the PNG to 32x64 or 64x64",
            ),
        ];
        for (width, height, message) in cases {
            let bytes = png(width, height, gradient);

            let error = convert_png(&bytes, Target::Paa(None)).unwrap_err();

            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn edds_accepts_sides_that_are_not_powers_of_two() {
        let bytes = png(48, 48, gradient);

        assert!(convert_png(&bytes, Target::Edds).is_ok());
    }
}
