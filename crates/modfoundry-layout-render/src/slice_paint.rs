//! Draws a style slot's image into a box: stretched, or repeated along its tiled axes.

use modfoundry_layout::geometry::Rect;
use modfoundry_layout::imageset::Tile;

use crate::canvas::{BlitOptions, Fit};
use crate::context::DrawContext;
use crate::image_paint::SlotPicture;

const MIN_REPEAT: f32 = 0.5;
const MAX_REPEATS: f32 = 4096.0;

/// Draws `slot` into `rect`: stretched, or repeated at its natural size times `scale` along the tiled axis or axes, the last repeat cut off.
pub(crate) fn draw_slot(
    ctx: &mut DrawContext<'_>,
    rect: Rect,
    clip: Option<Rect>,
    slot: &SlotPicture,
    scale: f32,
    tint: [f32; 4],
) -> bool {
    let tiled_x = matches!(slot.tile, Tile::Horizontal | Tile::Both);
    let tiled_y = matches!(slot.tile, Tile::Vertical | Tile::Both);
    let [crop_x0, crop_y0, crop_x1, crop_y1] = slot.crop;
    let columns = spans(
        rect.x,
        rect.w,
        slot.size[0] * scale,
        (crop_x0, crop_x1),
        tiled_x,
    );
    let rows = spans(
        rect.y,
        rect.h,
        slot.size[1] * scale,
        (crop_y0, crop_y1),
        tiled_y,
    );
    let mut painted = false;
    for row in &rows {
        for column in &columns {
            let piece = Rect {
                x: column.at,
                y: row.at,
                w: column.len,
                h: row.len,
            };
            let crop = [column.texels.0, row.texels.0, column.texels.1, row.texels.1];
            let options = BlitOptions {
                tint,
                use_alpha: true,
                flip_u: false,
                flip_v: false,
                smooth: true,
                fit: Fit::Stretch,
            };
            painted |= ctx.canvas.blit(piece, clip, &slot.pixels, crop, options);
        }
    }
    painted
}

pub(crate) fn place(rect: Rect, size: impl Fn(&str) -> [f32; 2]) -> Vec<(&'static str, Rect)> {
    let [left, _] = size("Left");
    let [right, _] = size("Right");
    let [_, top] = size("Top");
    let [_, bottom] = size("Bottom");
    let [lt_w, lt_h] = size("LeftTop");
    let [rt_w, rt_h] = size("RightTop");
    let [lb_w, lb_h] = size("LeftBottom");
    let [rb_w, rb_h] = size("RightBottom");
    let x0 = rect.x;
    let x1 = rect.x + rect.w;
    let y0 = rect.y;
    let y1 = rect.y + rect.h;
    let piece = |a: f32, b: f32, c: f32, d: f32| {
        piece(
            a.clamp(x0, x1),
            b.clamp(y0, y1),
            c.clamp(x0, x1),
            d.clamp(y0, y1),
        )
    };
    let left_x = x0 + left;
    let right_x = x1 - right;
    let top_y = y0 + top;
    let bottom_y = y1 - bottom;
    let top_x0 = x0 + lt_w;
    let top_x1 = x1 - rt_w;
    let bottom_x0 = x0 + lb_w;
    let bottom_x1 = x1 - rb_w;
    let left_y0 = y0 + lt_h;
    let left_y1 = y1 - lb_h;
    let right_y0 = y0 + rt_h;
    let right_y1 = y1 - rb_h;
    vec![
        ("Center", piece(left_x, top_y, right_x, bottom_y)),
        ("Left", piece(x0, left_y0, left_x, left_y1)),
        ("Right", piece(right_x, right_y0, x1, right_y1)),
        ("Top", piece(top_x0, y0, top_x1, top_y)),
        ("Bottom", piece(bottom_x0, bottom_y, bottom_x1, y1)),
        ("LeftTop", piece(x0, y0, top_x0, left_y0)),
        ("RightTop", piece(top_x1, y0, x1, right_y0)),
        ("LeftBottom", piece(x0, left_y1, bottom_x0, y1)),
        ("RightBottom", piece(bottom_x1, right_y1, x1, y1)),
    ]
}

fn piece(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

/// One run of the box along an axis: where it starts, how long it is and the texels it shows.
struct Span {
    at: f32,
    len: f32,
    texels: (u32, u32),
}

fn spans(start: f32, length: f32, repeat: f32, texels: (u32, u32), tiled: bool) -> Vec<Span> {
    if length.is_nan() || length <= 0.0 {
        return Vec::new();
    }
    if !tiled || repeat < MIN_REPEAT || length / repeat > MAX_REPEATS {
        return vec![Span {
            at: start,
            len: length,
            texels,
        }];
    }
    // Each repeat's end is the next one's start, snapped to a pixel, so neighbours meet exactly.
    let whole = (length / repeat).floor() as u32;
    let edge = |index: u32| (start + index as f32 * repeat).round();
    let mut spans: Vec<Span> = (0..whole)
        .map(|index| Span {
            at: edge(index),
            len: edge(index + 1) - edge(index),
            texels,
        })
        .collect();
    let left = length - whole as f32 * repeat;
    if left > 0.0 {
        let width = texels.1 - texels.0;
        let cut = ((width as f32 * left / repeat).round() as u32)
            .max(1)
            .min(width);
        let at = edge(whole);
        spans.push(Span {
            at,
            len: (start + length).round() - at,
            texels: (texels.0, texels.0 + cut),
        });
    }
    spans
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use modfoundry_layout::geometry::Rect;
    use modfoundry_layout::imageset::Tile;
    use modfoundry_texture::RgbaImage;

    use crate::canvas::Canvas;
    use crate::context::DrawContext;
    use crate::image_paint::SlotPicture;
    use crate::resources::NoResources;
    use crate::slice_paint::draw_slot;
    use crate::textures::Textures;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CANVAS: u32 = 12;

    fn row(colours: &[[u8; 4]]) -> RgbaImage {
        RgbaImage {
            width: colours.len() as u32,
            height: 1,
            pixels: colours.concat(),
        }
    }

    fn slot(texels: RgbaImage, size: [f32; 2], tile: Tile) -> SlotPicture {
        SlotPicture {
            crop: [0, 0, texels.width, texels.height],
            pixels: Rc::new(texels),
            size,
            tile,
        }
    }

    fn draw(rect: Rect, slot: &SlotPicture, scale: f32) -> RgbaImage {
        let mut canvas = Canvas::new(CANVAS, CANVAS, [0, 0, 0, 255]);
        let mut textures = Textures::default();
        let mut ctx = DrawContext {
            canvas: &mut canvas,
            resources: &NoResources,
            textures: &mut textures,
            screen_height: 1080,
        };
        draw_slot(&mut ctx, rect, None, slot, scale, [1.0; 4]);
        canvas.into_image()
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn tiled_pieces_meet_without_a_gap() {
        let pieces = super::spans(37.3, 12.3, 0.6, (0, 4), true);
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].at + pair[0].len, pair[1].at);
        }
    }

    #[test]
    fn a_box_with_no_length_has_no_pieces() {
        assert!(super::spans(0.0, -1.0, 4.0, (0, 4), true).is_empty());
        assert!(super::spans(0.0, 0.0, 4.0, (0, 4), false).is_empty());
    }

    #[test]
    fn a_slot_without_tiling_is_stretched_over_the_box() {
        let texels = row(&[RED, RED, BLUE, BLUE]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 8.0,
            h: 1.0,
        };
        let image = draw(rect, &slot(texels, [4.0, 1.0], Tile::None), 1.0);
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 7, 0), BLUE);
    }

    #[test]
    fn a_horizontally_tiled_slot_repeats_and_cuts_the_last_repeat() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 1.0,
        };
        let image = draw(
            rect,
            &slot(row(&[RED, RED, BLUE, BLUE]), [4.0, 1.0], Tile::Horizontal),
            1.0,
        );
        let repeats = [0, 3, 4, 7].map(|x| pixel(&image, x, 0));
        assert_eq!(repeats, [RED, BLUE, RED, BLUE]);
        assert_eq!([pixel(&image, 8, 0), pixel(&image, 9, 0)], [RED, RED]);
        assert_eq!(pixel(&image, 10, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn the_scale_shortens_the_repeat() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        let image = draw(
            rect,
            &slot(row(&[RED, RED, BLUE, BLUE]), [4.0, 1.0], Tile::Horizontal),
            0.5,
        );
        assert_eq!(pixel(&image, 2, 0), RED);
    }

    #[test]
    fn a_vertically_tiled_slot_repeats_down_the_box() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 10.0,
        };
        let texels = RgbaImage {
            width: 1,
            height: 4,
            pixels: [RED, RED, BLUE, BLUE].concat(),
        };
        let image = draw(rect, &slot(texels, [1.0, 4.0], Tile::Vertical), 1.0);
        let repeats = [0, 3, 4, 7].map(|y| pixel(&image, 0, y));
        assert_eq!(repeats, [RED, BLUE, RED, BLUE]);
        assert_eq!([pixel(&image, 0, 8), pixel(&image, 0, 9)], [RED, RED]);
    }

    #[test]
    fn a_repeat_under_half_a_pixel_is_stretched() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 8.0,
            h: 1.0,
        };
        let image = draw(
            rect,
            &slot(row(&[RED, RED, BLUE, BLUE]), [0.1, 1.0], Tile::Horizontal),
            1.0,
        );
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 7, 0), BLUE);
    }

    #[test]
    fn a_box_needing_more_than_4096_repeats_is_stretched() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 5000.0,
            h: 1.0,
        };
        let image = draw(
            rect,
            &slot(row(&[RED, RED, BLUE, BLUE]), [1.0, 1.0], Tile::Horizontal),
            1.0,
        );
        assert!((0..CANVAS).all(|x| pixel(&image, x, 0) == RED));
    }
}
