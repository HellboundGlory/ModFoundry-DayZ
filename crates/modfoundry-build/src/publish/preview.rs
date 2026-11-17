use anyhow::{anyhow, bail};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbImage};
use std::io::Cursor;

/// Largest preview Publisher accepts, in bytes.
pub const PREVIEW_LIMIT: usize = 1_000_000;

const QUALITIES: [u8; 5] = [90, 75, 60, 45, 30];
const MIN_SIDE: u32 = 16;

/// Re-encodes a preview picture as a JPEG under `PREVIEW_LIMIT` bytes, halving it when needed.
pub fn preview_jpeg(image: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut rgb = on_white(&decode(image)?);
    loop {
        if let Some(jpeg) = fitting_jpeg(&rgb)? {
            return Ok(jpeg);
        }
        let (width, height) = (rgb.width() / 2, rgb.height() / 2);
        if width < MIN_SIDE || height < MIN_SIDE {
            bail!("the preview image cannot be made smaller than 1,000,000 bytes");
        }
        rgb = image::imageops::resize(&rgb, width, height, FilterType::Triangle);
    }
}

/// Decodes a picture and turns it upright as its EXIF says.
fn decode(image: &[u8]) -> anyhow::Result<DynamicImage> {
    let read = |e: &dyn std::fmt::Display| anyhow!("the preview image cannot be read: {e}");
    let reader = ImageReader::new(Cursor::new(image))
        .with_guessed_format()
        .map_err(|e| read(&e))?;
    let mut decoder = reader.into_decoder().map_err(|e| read(&e))?;
    // The decoder path skips the size limit `load_from_memory` applies.
    Limits::default()
        .reserve(decoder.total_bytes())
        .map_err(|e| read(&e))?;
    let orientation = decoder.orientation().map_err(|e| read(&e))?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|e| read(&e))?;
    decoded.apply_orientation(orientation);
    Ok(decoded)
}

/// Flattens a picture onto white, so a clear pixel is not turned black.
fn on_white(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0.map(u32::from);
        let over = |c: u32| ((c * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([over(r), over(g), over(b)])
    })
}

fn fitting_jpeg(rgb: &RgbImage) -> anyhow::Result<Option<Vec<u8>>> {
    for quality in QUALITIES {
        let jpeg = encode(rgb, quality)?;
        if jpeg.len() < PREVIEW_LIMIT {
            return Ok(Some(jpeg));
        }
    }
    Ok(None)
}

fn encode(rgb: &RgbImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(rgb)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, ImageFormat, Rgb, Rgba, RgbaImage};
    use std::io::Cursor;

    fn write(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, format).unwrap();
        cursor.into_inner()
    }

    fn noise(width: u32, height: u32) -> RgbImage {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        };
        RgbImage::from_fn(width, height, |_, _| Rgb([next(), next(), next()]))
    }

    #[test]
    fn small_rgba_png_becomes_a_jpeg_of_the_same_size() {
        let rgba = RgbaImage::from_pixel(64, 48, Rgba([200, 60, 30, 255]));
        let source = write(&DynamicImage::ImageRgba8(rgba), ImageFormat::Png);

        let jpeg = preview_jpeg(&source).unwrap();

        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 48));
    }

    #[test]
    fn noisy_picture_drops_quality_to_fit_the_limit() {
        let rgb = noise(1100, 1100);
        assert!(encode(&rgb, QUALITIES[0]).unwrap().len() >= PREVIEW_LIMIT);
        let source = write(&DynamicImage::ImageRgb8(rgb), ImageFormat::Png);

        let jpeg = preview_jpeg(&source).unwrap();

        assert!(jpeg.len() < PREVIEW_LIMIT);
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1100, 1100));
    }

    #[test]
    fn picture_too_large_at_every_quality_is_halved() {
        let source = write(
            &DynamicImage::ImageRgb8(noise(1400, 1400)),
            ImageFormat::Bmp,
        );

        let jpeg = preview_jpeg(&source).unwrap();

        assert!(jpeg.len() < PREVIEW_LIMIT);
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert!(decoded.width() < 1400);
    }

    #[test]
    fn a_jpeg_with_an_exif_turn_is_turned_upright() {
        let mut rgb = RgbImage::new(32, 16);
        for (x, _, pixel) in rgb.enumerate_pixels_mut() {
            *pixel = if x < 16 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            };
        }
        let exif = vec![
            b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut source = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut source, 95);
        encoder.set_exif_metadata(exif).unwrap();
        encoder.encode_image(&rgb).unwrap();

        let orientation = image::ImageReader::new(Cursor::new(&source))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap()
            .orientation()
            .unwrap();
        assert_eq!(orientation, image::metadata::Orientation::Rotate90);

        let jpeg = preview_jpeg(&source).unwrap();

        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (16, 32));
        let top = decoded.get_pixel(8, 4);
        assert!(top[0] > 200 && top[2] < 60, "{top:?}");
        let bottom = decoded.get_pixel(8, 28);
        assert!(bottom[2] > 200 && bottom[0] < 60, "{bottom:?}");
    }

    #[test]
    fn a_transparent_png_is_put_on_white() {
        let rgba = RgbaImage::from_fn(16, 16, |x, y| {
            if x < 8 && y < 8 {
                Rgba([0, 0, 0, 0])
            } else if x < 8 {
                Rgba([0, 0, 0, 128])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let source = write(&DynamicImage::ImageRgba8(rgba), ImageFormat::Png);

        let jpeg = preview_jpeg(&source).unwrap();

        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let clear = decoded.get_pixel(3, 3);
        assert!(clear.0.iter().all(|c| *c > 245), "{clear:?}");
        let half = decoded.get_pixel(3, 12);
        assert!(half.0.iter().all(|c| c.abs_diff(127) < 10), "{half:?}");
        let blue = decoded.get_pixel(12, 8);
        assert!(blue[2] > 200 && blue[0] < 60 && blue[1] < 60, "{blue:?}");
    }

    #[test]
    fn bytes_that_are_not_an_image_are_rejected() {
        let err = preview_jpeg(b"not an image").unwrap_err();

        assert!(err.to_string().contains("cannot be read"));
    }

    #[test]
    fn a_picture_too_large_to_decode_is_refused_before_it_is_read() {
        let (width, height) = (20_000u32, 20_000u32);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&height.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);

        let error = preview_jpeg(&bmp).unwrap_err().to_string();

        assert!(
            error.starts_with("the preview image cannot be read: "),
            "{error}"
        );
        assert!(error.contains("limit"), "{error}");
    }
}
