//! Draws an ImageWidget's picture from an imageset or a plain texture path.

use std::rc::Rc;

use modfoundry_layout::geometry::WidgetBox;
use modfoundry_layout::imageset::{Imageset, ImagesetImage, ImagesetTexture, Tile};
use modfoundry_layout::parse::Widget;
use modfoundry_texture::RgbaImage;

use crate::canvas::{BlitOptions, Fit};
use crate::context::DrawContext;
use crate::paint::first_text;
use crate::resources::normalize_path;
use crate::textures::FILE_NOT_FOUND;

const IMAGE_CLASS: &str = "ImageWidgetClass";

pub(crate) struct PictureOutcome {
    pub(crate) painted: bool,
    pub(crate) findings: Vec<(&'static str, String)>,
}

struct Picture {
    pixels: Rc<RgbaImage>,
    crop: [u32; 4],
}

/// A style slot's image: its texels, the crop in them, its size in reference units and how it tiles.
pub(crate) struct SlotPicture {
    pub(crate) pixels: Rc<RgbaImage>,
    pub(crate) crop: [u32; 4],
    pub(crate) size: [f32; 2],
    pub(crate) tile: Tile,
}

/// The picture of an ImageWidget, or `None` when it has no picture and keeps its style.
pub(crate) fn paint(
    ctx: &mut DrawContext<'_>,
    item: &WidgetBox,
    widget: &Widget,
    tint: [f32; 4],
) -> Option<PictureOutcome> {
    if widget.class != IMAGE_CLASS {
        return None;
    }
    let reference = non_empty(widget, "image0").or_else(|| non_empty(widget, "imageTexture"))?;
    let label = widget.label();
    let mut findings = Vec::new();
    let picture = match load(ctx, reference) {
        Ok(picture) => picture,
        Err(reason) => {
            findings.push((
                "MF-L214",
                format!("image {reference} is drawn as a placeholder: {reason}: {label}"),
            ));
            let painted = ctx.canvas.placeholder(item.rect, item.clip, tint);
            return Some(PictureOutcome { painted, findings });
        }
    };
    let use_alpha = first_text(widget, "src alpha") == Some("1");
    let options = BlitOptions {
        tint,
        use_alpha,
        flip_u: is_set(widget, "flip u"),
        flip_v: is_set(widget, "flip v"),
        smooth: first_text(widget, "filter") != Some("0"),
        fit: match widget.word("stretch mode") {
            Some("none") => Fit::Natural,
            _ => Fit::Stretch,
        },
    };
    let painted = ctx
        .canvas
        .blit(item.rect, item.clip, &picture.pixels, picture.crop, options);
    if !use_alpha && has_partial_alpha(&picture) {
        findings.push((
            "MF-L215",
            format!("picture alpha is not used without \"src alpha\" 1: {label}"),
        ));
    }
    findings.extend(option_findings(widget, label));
    Some(PictureOutcome { painted, findings })
}

fn non_empty<'a>(widget: &'a Widget, key: &str) -> Option<&'a str> {
    widget.word(key).filter(|text| !text.is_empty())
}

fn is_set(widget: &Widget, key: &str) -> bool {
    first_text(widget, key).is_some_and(|value| value != "0")
}

fn load(ctx: &mut DrawContext<'_>, reference: &str) -> Result<Picture, String> {
    match reference.strip_prefix("set:") {
        Some(rest) => load_from_set(ctx, rest),
        None => load_texture(ctx, reference),
    }
}

fn load_texture(ctx: &mut DrawContext<'_>, path: &str) -> Result<Picture, String> {
    let pixels = ctx
        .textures
        .get(ctx.resources, path)
        .map_err(|error| texture_reason(path, &error))?;
    let crop = [0, 0, pixels.width, pixels.height];
    Ok(Picture { pixels, crop })
}

/// `rest` is the reference after `set:`: `<set> image:<name>`.
fn load_from_set(ctx: &mut DrawContext<'_>, rest: &str) -> Result<Picture, String> {
    let mut words = rest.split_whitespace();
    let set = words.next().unwrap_or_default();
    let name = words.next().unwrap_or_default();
    let name = name.strip_prefix("image:").unwrap_or(name);
    let slot = load_slot(ctx, set, name)?;
    Ok(Picture {
        pixels: slot.pixels,
        crop: slot.crop,
    })
}

/// The image `name` of imageset `set`, cut from the best texture that can be read.
pub(crate) fn load_slot(
    ctx: &mut DrawContext<'_>,
    set: &str,
    name: &str,
) -> Result<SlotPicture, String> {
    let imageset = ctx
        .resources
        .imageset(set)
        .ok_or_else(|| format!("imageset {set} not found"))?;
    let missing = format!("image {name} is not in imageset {set}");
    let Some(image) = find_image(&imageset, name) else {
        return Err(missing);
    };
    let mut textures: Vec<&ImagesetTexture> = imageset.textures.iter().collect();
    textures.sort_by_key(|texture| std::cmp::Reverse(texture.mpix));
    let mut reason = format!("imageset {set} has no textures");
    for texture in textures {
        match ctx.textures.get(ctx.resources, &texture.path) {
            Ok(pixels) => {
                let scale = imageset.texel_scale(pixels.width, pixels.height);
                let crop = crop_texels(image, scale, pixels.width, pixels.height).ok_or(missing)?;
                return Ok(SlotPicture {
                    pixels,
                    crop,
                    size: [image.w as f32, image.h as f32],
                    tile: image.tile(),
                });
            }
            Err(error) => reason = texture_reason(&texture.path, &error),
        }
    }
    Err(reason)
}

fn find_image<'a>(imageset: &'a Imageset, name: &str) -> Option<&'a ImagesetImage> {
    imageset.image(name).or_else(|| {
        imageset
            .images
            .iter()
            .find(|image| image.name.eq_ignore_ascii_case(name))
    })
}

fn crop_texels(
    image: &ImagesetImage,
    (scale_x, scale_y): (f32, f32),
    width: u32,
    height: u32,
) -> Option<[u32; 4]> {
    let cut = |at: f32, limit: u32| (at.round().min(limit as f32)) as u32;
    let x0 = cut(image.x as f32 * scale_x, width);
    let y0 = cut(image.y as f32 * scale_y, height);
    let x1 = cut((image.x as f32 + image.w as f32) * scale_x, width);
    let y1 = cut((image.y as f32 + image.h as f32) * scale_y, height);
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

fn texture_reason(path: &str, error: &str) -> String {
    let path = normalize_path(path);
    if error == FILE_NOT_FOUND {
        format!("texture {path} not found")
    } else {
        format!("texture {path} cannot be read ({error})")
    }
}

fn has_partial_alpha(picture: &Picture) -> bool {
    let [x0, y0, x1, y1] = picture.crop;
    let width = picture.pixels.width as usize;
    (y0..y1).any(|y| {
        (x0..x1).any(|x| picture.pixels.pixels[(y as usize * width + x as usize) * 4 + 3] < 255)
    })
}

fn option_findings(widget: &Widget, label: &str) -> Vec<(&'static str, String)> {
    type Unsupported = fn(&str) -> bool;
    let unsupported: [(&str, Unsupported); 5] = [
        ("mode", |mode| mode == "additive"),
        ("stretch mode", |mode| {
            mode != "stretch_w_h" && mode != "none"
        }),
        ("Mask", |value| value != "0"),
        ("Progress", |value| value != "0"),
        ("inheritalpha", |value| value != "0"),
    ];
    unsupported
        .into_iter()
        .filter_map(|(key, is_unsupported)| {
            let value = first_text(widget, key)?;
            is_unsupported(value).then(|| {
                (
                    "MF-L216",
                    format!("image option not drawn: {key} {value}: {label}"),
                )
            })
        })
        .collect()
}
