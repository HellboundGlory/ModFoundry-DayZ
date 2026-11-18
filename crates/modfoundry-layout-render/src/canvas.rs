use modfoundry_layout::geometry::Rect;
use modfoundry_texture::RgbaImage;

pub(crate) struct Canvas {
    image: RgbaImage,
}

/// Pixel columns `x0..x1` and rows `y0..y1`, half-open.
#[derive(Clone, Copy)]
struct Edges {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

/// Far beyond any canvas, small enough that edge arithmetic cannot overflow.
const EDGE_LIMIT: f32 = 1.0e9;

impl Edges {
    fn of(rect: Rect) -> Self {
        let edge = |value: f32| value.round().clamp(-EDGE_LIMIT, EDGE_LIMIT) as i64;
        Self {
            x0: edge(rect.x),
            y0: edge(rect.y),
            x1: edge(rect.x + rect.w),
            y1: edge(rect.y + rect.h),
        }
    }

    fn within(self, other: Edges) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    fn contains(self, x: i64, y: i64) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    fn on_frame(self, x: i64, y: i64) -> bool {
        x == self.x0 || x == self.x1 - 1 || y == self.y0 || y == self.y1 - 1
    }

    fn pixels(self) -> impl Iterator<Item = (i64, i64)> {
        (self.y0..self.y1).flat_map(move |y| (self.x0..self.x1).map(move |x| (x, y)))
    }
}

impl Canvas {
    pub(crate) fn new(width: u32, height: u32, background: [u8; 4]) -> Self {
        let pixels = background.repeat(width as usize * height as usize);
        Self {
            image: RgbaImage {
                width,
                height,
                pixels,
            },
        }
    }

    pub(crate) fn into_image(self) -> RgbaImage {
        self.image
    }

    pub(crate) fn fill(&mut self, rect: Rect, clip: Option<Rect>, rgba: [f32; 4]) -> bool {
        let area = self.visible(Edges::of(rect), clip);
        self.plot(area, area.pixels(), rgba)
    }

    pub(crate) fn outline(&mut self, rect: Rect, clip: Option<Rect>, rgba: [f32; 4]) -> bool {
        let full = Edges::of(rect);
        let area = self.visible(full, clip);
        let frame = area.pixels().filter(|&(x, y)| full.on_frame(x, y));
        self.plot(area, frame, rgba)
    }

    pub(crate) fn cross(&mut self, rect: Rect, clip: Option<Rect>, rgba: [f32; 4]) -> bool {
        let full = Edges::of(rect);
        let area = self.visible(full, clip);
        let (right, bottom) = (full.x1 - 1, full.y1 - 1);
        let diagonals = diagonal((full.x0, full.y0), (right, bottom), area).chain(diagonal(
            (right, full.y0),
            (full.x0, bottom),
            area,
        ));
        self.plot(area, diagonals, rgba)
    }

    pub(crate) fn placeholder(&mut self, rect: Rect, clip: Option<Rect>, rgba: [f32; 4]) -> bool {
        let outlined = self.outline(rect, clip, rgba);
        let crossed = self.cross(rect, clip, rgba);
        outlined || crossed
    }

    pub(crate) fn image_mut(&mut self) -> &mut RgbaImage {
        &mut self.image
    }

    pub(crate) fn pixel_bounds(&self, rect: Rect, clip: Option<Rect>) -> Option<[i32; 4]> {
        let area = self.visible(Edges::of(rect), clip);
        (area.x0 < area.x1 && area.y0 < area.y1).then_some([
            area.x0 as i32,
            area.y0 as i32,
            area.x1 as i32,
            area.y1 as i32,
        ])
    }

    pub(crate) fn blit(
        &mut self,
        rect: Rect,
        clip: Option<Rect>,
        source: &RgbaImage,
        crop: [u32; 4],
        options: BlitOptions,
    ) -> bool {
        let crop_w = (crop[2] - crop[0]) as f32;
        let crop_h = (crop[3] - crop[1]) as f32;
        let dest = match options.fit {
            Fit::Stretch => rect,
            Fit::Natural => Rect {
                x: rect.x,
                y: rect.y,
                w: crop_w,
                h: crop_h,
            },
        };
        if options.tint[3] <= 0.0 {
            return false;
        }
        let Some([mut x0, mut y0, mut x1, mut y1]) = self.pixel_bounds(dest, clip) else {
            return false;
        };
        if options.fit == Fit::Natural {
            let Some(inside) = self.pixel_bounds(rect, clip) else {
                return false;
            };
            (x0, y0, x1, y1) = (
                x0.max(inside[0]),
                y0.max(inside[1]),
                x1.min(inside[2]),
                y1.min(inside[3]),
            );
            if x0 >= x1 || y0 >= y1 {
                return false;
            }
        }
        let full = Edges::of(dest);
        let box_w = (full.x1 - full.x0) as f32;
        let box_h = (full.y1 - full.y0) as f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let mut fx = ((i64::from(x) - full.x0) as f32 + 0.5) / box_w;
                let mut fy = ((i64::from(y) - full.y0) as f32 + 0.5) / box_h;
                if options.flip_u {
                    fx = 1.0 - fx;
                }
                if options.flip_v {
                    fy = 1.0 - fy;
                }
                let sx = crop[0] as f32 + fx * crop_w;
                let sy = crop[1] as f32 + fy * crop_h;
                let mut texel = sample(source, crop, (sx, sy), options.smooth, options.use_alpha);
                if !options.use_alpha {
                    texel[3] = 1.0;
                }
                self.blend(i64::from(x), i64::from(y), modulate(texel, options.tint));
            }
        }
        true
    }

    fn visible(&self, full: Edges, clip: Option<Rect>) -> Edges {
        let canvas = Edges {
            x0: 0,
            y0: 0,
            x1: i64::from(self.image.width),
            y1: i64::from(self.image.height),
        };
        let cut = full.within(canvas);
        match clip {
            Some(clip) => cut.within(Edges::of(clip)),
            None => cut,
        }
    }

    fn plot(
        &mut self,
        area: Edges,
        points: impl IntoIterator<Item = (i64, i64)>,
        rgba: [f32; 4],
    ) -> bool {
        if rgba[3] <= 0.0 {
            return false;
        }
        let mut painted = false;
        for (x, y) in points {
            if area.contains(x, y) {
                self.blend(x, y, rgba);
                painted = true;
            }
        }
        painted
    }

    fn blend(&mut self, x: i64, y: i64, src: [f32; 4]) {
        let width = self.image.width as usize;
        let at = (y as usize * width + x as usize) * 4;
        let dst = &mut self.image.pixels[at..at + 4];
        let [red, green, blue, alpha] = src;
        if alpha >= 1.0 {
            dst.copy_from_slice(&[channel(red), channel(green), channel(blue), 255]);
            return;
        }
        let dst_alpha = f32::from(dst[3]) / 255.0;
        let out_alpha = alpha + dst_alpha * (1.0 - alpha);
        let mix = |source: f32, index: usize| {
            let destination = f32::from(dst[index]) / 255.0;
            (source * alpha + destination * dst_alpha * (1.0 - alpha)) / out_alpha
        };
        let out = [
            channel(mix(red, 0)),
            channel(mix(green, 1)),
            channel(mix(blue, 2)),
            channel(out_alpha),
        ];
        dst.copy_from_slice(&out);
    }
}

fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn texel(source: &RgbaImage, x: u32, y: u32) -> [f32; 4] {
    let at = (y as usize * source.width as usize + x as usize) * 4;
    let bytes = &source.pixels[at..at + 4];
    [0, 1, 2, 3].map(|index| f32::from(bytes[index]) / 255.0)
}

/// Nearest texel, or bilinear between the four around the point; neighbours stay inside `crop`.
fn sample(
    source: &RgbaImage,
    crop: [u32; 4],
    point: (f32, f32),
    smooth: bool,
    use_alpha: bool,
) -> [f32; 4] {
    let (x, y) = point;
    if !smooth {
        return texel(
            source,
            index(x, crop[0], crop[2]),
            index(y, crop[1], crop[3]),
        );
    }
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let at = |dx: f32, dy: f32| {
        texel(
            source,
            index(x0 + dx, crop[0], crop[2]),
            index(y0 + dy, crop[1], crop[3]),
        )
    };
    // Weighting colour by alpha keeps transparent texels from bleeding in, but only when alpha is drawn.
    let premultiplied = |dx: f32, dy: f32| {
        let [red, green, blue, alpha] = at(dx, dy);
        if use_alpha {
            [red * alpha, green * alpha, blue * alpha, alpha]
        } else {
            [red, green, blue, alpha]
        }
    };
    let top = mix(premultiplied(0.0, 0.0), premultiplied(1.0, 0.0), ax);
    let bottom = mix(premultiplied(0.0, 1.0), premultiplied(1.0, 1.0), ax);
    let [red, green, blue, alpha] = mix(top, bottom, ay);
    if !use_alpha {
        return [red, green, blue, alpha];
    }
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    [red / alpha, green / alpha, blue / alpha, alpha]
}

fn index(value: f32, start: u32, end: u32) -> u32 {
    (value.floor() as i64).clamp(i64::from(start), i64::from(end) - 1) as u32
}

fn mix(from: [f32; 4], to: [f32; 4], amount: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|index| from[index] + (to[index] - from[index]) * amount)
}

fn modulate(texel: [f32; 4], tint: [f32; 4]) -> [f32; 4] {
    [0, 1, 2, 3].map(|index| texel[index] * tint[index])
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Fit {
    Stretch,
    Natural,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlitOptions {
    pub(crate) tint: [f32; 4],
    pub(crate) use_alpha: bool,
    pub(crate) flip_u: bool,
    pub(crate) flip_v: bool,
    pub(crate) smooth: bool,
    pub(crate) fit: Fit,
}

/// The pixels of the line from `from` to `to` that fall in `area`, walked along the longer axis,
/// so a huge box costs no more than the canvas.
fn diagonal(from: (i64, i64), to: (i64, i64), area: Edges) -> impl Iterator<Item = (i64, i64)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let along_x = dx.abs() >= dy.abs();
    let (start, span, minor_start, minor_span, lo, hi) = if along_x {
        (from.0, dx, from.1, dy, area.x0, area.x1 - 1)
    } else {
        (from.1, dy, from.0, dx, area.y0, area.y1 - 1)
    };
    let first = lo.max(start.min(start + span));
    let last = hi.min(start.max(start + span));
    (first..=last).map(move |major| {
        let t = if span == 0 {
            0.0
        } else {
            (major - start) as f64 / span as f64
        };
        let minor = minor_start + (t * minor_span as f64).round() as i64;
        if along_x {
            (major, minor)
        } else {
            (minor, major)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(canvas: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * canvas.image.width + x) * 4) as usize;
        canvas.image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn fill_covers_the_pixels_between_the_rounded_edges() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.4,
            y: 0.0,
            w: 2.2,
            h: 1.0,
        };
        assert!(canvas.fill(rect, None, [1.0, 1.0, 1.0, 1.0]));
        assert_eq!(pixel(&canvas, 2, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn fill_is_cut_to_the_clip() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        let clip = Rect {
            x: 1.0,
            y: 0.0,
            w: 2.0,
            h: 1.0,
        };
        assert!(canvas.fill(rect, Some(clip), [1.0, 1.0, 1.0, 1.0]));
        assert_eq!(pixel(&canvas, 0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn translucent_fill_mixes_into_an_opaque_background() {
        let mut canvas = Canvas::new(1, 1, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        assert!(canvas.fill(rect, None, [1.0, 1.0, 1.0, 0.2]));
        assert_eq!(pixel(&canvas, 0, 0), [51, 51, 51, 255]);
    }

    #[test]
    fn translucent_fill_over_nothing_keeps_its_alpha() {
        let mut canvas = Canvas::new(1, 1, [0, 0, 0, 0]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        assert!(canvas.fill(rect, None, [1.0, 1.0, 1.0, 0.5]));
        assert_eq!(pixel(&canvas, 0, 0), [255, 255, 255, 128]);
    }

    #[test]
    fn outline_is_a_frame_just_inside_the_edge() {
        let mut canvas = Canvas::new(4, 4, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 4.0,
        };
        assert!(canvas.outline(rect, None, [1.0, 1.0, 1.0, 1.0]));
        assert_eq!(pixel(&canvas, 0, 2), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 2), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 2, 3), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 1, 1), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 2, 2), [0, 0, 0, 255]);
    }

    #[test]
    fn outline_of_a_thin_rect_is_filled() {
        let mut canvas = Canvas::new(4, 4, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 3.0,
        };
        assert!(canvas.outline(rect, None, [1.0, 1.0, 1.0, 1.0]));
        assert_eq!(pixel(&canvas, 0, 1), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 1, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn cross_draws_both_diagonals() {
        let mut canvas = Canvas::new(5, 5, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 5.0,
            h: 5.0,
        };
        assert!(canvas.cross(rect, None, [1.0, 1.0, 1.0, 1.0]));
        for (x, y) in [(0, 0), (4, 0), (0, 4), (4, 4), (2, 2), (1, 1), (3, 1)] {
            assert_eq!(pixel(&canvas, x, y), [255, 255, 255, 255], "({x}, {y})");
        }
        assert_eq!(pixel(&canvas, 2, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn a_box_beyond_the_float_range_still_paints_without_overflow() {
        let mut canvas = Canvas::new(4, 4, [0, 0, 0, 255]);
        let rect = Rect {
            x: -1.0e30,
            y: -1.0e30,
            w: 2.0e30,
            h: 2.0e30,
        };
        canvas.cross(rect, None, [1.0, 1.0, 1.0, 1.0]);
        canvas.outline(rect, None, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn cross_of_a_huge_box_paints_only_the_visible_diagonal() {
        let mut canvas = Canvas::new(4, 4, [0, 0, 0, 255]);
        let rect = Rect {
            x: -5.0e8,
            y: -5.0e8,
            w: 1.0e9,
            h: 1.0e9,
        };
        assert!(canvas.cross(rect, None, [1.0, 1.0, 1.0, 1.0]));
        for i in 0..4 {
            assert_eq!(pixel(&canvas, i, i), [255, 255, 255, 255]);
        }
        assert_eq!(pixel(&canvas, 0, 1), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 2, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn pixel_bounds_is_the_clipped_rectangle_fill_paints() {
        let canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        let clip = Rect {
            x: 1.0,
            y: 0.0,
            w: 2.0,
            h: 1.0,
        };
        assert_eq!(canvas.pixel_bounds(rect, Some(clip)), Some([1, 0, 3, 1]));
    }

    #[test]
    fn pixel_bounds_matches_the_pixels_fill_paints() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let rect = Rect {
            x: 0.4,
            y: 0.0,
            w: 2.2,
            h: 1.0,
        };
        let bounds = canvas.pixel_bounds(rect, None);
        assert_eq!(bounds, Some([0, 0, 3, 1]));
        assert!(canvas.fill(rect, None, [1.0, 1.0, 1.0, 1.0]));
        assert_eq!(pixel(&canvas, 2, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 0), [0, 0, 0, 255]);
    }

    fn stretch(smooth: bool) -> BlitOptions {
        BlitOptions {
            tint: [1.0; 4],
            use_alpha: true,
            flip_u: false,
            flip_v: false,
            smooth,
            fit: Fit::Stretch,
        }
    }

    #[test]
    fn nearest_blit_stretches_each_texel_into_a_block() {
        let mut canvas = Canvas::new(4, 4, [0, 0, 0, 255]);
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let source = RgbaImage {
            width: 2,
            height: 2,
            pixels: [white, black, black, white].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 4.0,
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 2], stretch(false)));
        assert_eq!(pixel(&canvas, 1, 1), white);
        assert_eq!(pixel(&canvas, 2, 0), black);
        assert_eq!(pixel(&canvas, 0, 2), black);
        assert_eq!(pixel(&canvas, 3, 3), white);
    }

    #[test]
    fn flip_u_mirrors_the_picture_across_the_box() {
        let mut canvas = Canvas::new(2, 1, [0, 0, 0, 255]);
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let source = RgbaImage {
            width: 2,
            height: 1,
            pixels: [black, white].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 2.0,
            h: 1.0,
        };
        let options = BlitOptions {
            flip_u: true,
            ..stretch(false)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 1], options));
        assert_eq!(pixel(&canvas, 0, 0), white);
        assert_eq!(pixel(&canvas, 1, 0), black);
    }

    #[test]
    fn blit_multiplies_each_texel_by_the_tint() {
        let mut canvas = Canvas::new(1, 1, [0, 0, 0, 255]);
        let source = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![255, 255, 255, 255],
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let options = BlitOptions {
            tint: [1.0, 0.0, 0.0, 1.0],
            ..stretch(false)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 1, 1], options));
        assert_eq!(pixel(&canvas, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn blit_cut_to_a_clip_keeps_the_picture_in_place() {
        let mut canvas = Canvas::new(4, 1, [9, 9, 9, 255]);
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let source = RgbaImage {
            width: 4,
            height: 1,
            pixels: [black, black, white, white].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        let clip = Rect {
            x: 2.0,
            y: 0.0,
            w: 2.0,
            h: 1.0,
        };
        assert!(canvas.blit(rect, Some(clip), &source, [0, 0, 4, 1], stretch(false)));
        assert_eq!(pixel(&canvas, 0, 0), [9, 9, 9, 255]);
        assert_eq!(pixel(&canvas, 2, 0), white);
        assert_eq!(pixel(&canvas, 3, 0), white);
    }

    #[test]
    fn texel_alpha_is_ignored_without_use_alpha() {
        let mut canvas = Canvas::new(1, 1, [0, 0, 0, 255]);
        let source = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![255, 255, 255, 128],
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let options = BlitOptions {
            use_alpha: false,
            ..stretch(false)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 1, 1], options));
        assert_eq!(pixel(&canvas, 0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn natural_fit_draws_the_crop_at_its_texel_size_in_the_top_left() {
        let mut canvas = Canvas::new(4, 4, [9, 9, 9, 255]);
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let source = RgbaImage {
            width: 2,
            height: 2,
            pixels: [white, black, black, black].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 4.0,
        };
        let options = BlitOptions {
            fit: Fit::Natural,
            ..stretch(false)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 2], options));
        assert_eq!(pixel(&canvas, 0, 0), white);
        assert_eq!(pixel(&canvas, 1, 0), black);
        assert_eq!(pixel(&canvas, 3, 3), [9, 9, 9, 255]);
    }

    #[test]
    fn natural_fit_is_cut_to_the_box() {
        let mut canvas = Canvas::new(6, 6, [9, 9, 9, 255]);
        let source = RgbaImage {
            width: 4,
            height: 4,
            pixels: [[255, 255, 255, 255]; 16].concat(),
        };
        let rect = Rect {
            x: 1.0,
            y: 1.0,
            w: 2.0,
            h: 2.0,
        };
        let options = BlitOptions {
            fit: Fit::Natural,
            ..stretch(false)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 4, 4], options));
        assert_eq!(pixel(&canvas, 2, 2), [255, 255, 255, 255]);
        assert_eq!(pixel(&canvas, 3, 3), [9, 9, 9, 255]);
        assert_eq!(pixel(&canvas, 4, 4), [9, 9, 9, 255]);
    }

    #[test]
    fn a_fully_transparent_tint_paints_nothing() {
        let mut canvas = Canvas::new(1, 1, [0, 0, 0, 0]);
        let source = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![255, 255, 255, 255],
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let options = BlitOptions {
            tint: [1.0, 1.0, 1.0, 0.0],
            ..stretch(false)
        };
        assert!(!canvas.blit(rect, None, &source, [0, 0, 1, 1], options));
        assert_eq!(pixel(&canvas, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn smooth_blit_blends_neighbouring_texels() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let black = [0, 0, 0, 255];
        let source = RgbaImage {
            width: 2,
            height: 1,
            pixels: [black, [255, 255, 255, 255]].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 1], stretch(true)));
        assert_eq!(pixel(&canvas, 0, 0), black);
        assert_eq!(pixel(&canvas, 1, 0), [64, 64, 64, 255]);
    }

    #[test]
    fn smooth_blit_does_not_pull_colour_from_transparent_texels() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 0]);
        let source = RgbaImage {
            width: 2,
            height: 1,
            pixels: [[255, 0, 0, 255], [0, 0, 0, 0]].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 1], stretch(true)));
        assert_eq!(pixel(&canvas, 1, 0), [255, 0, 0, 191]);
    }

    #[test]
    fn smooth_blit_without_alpha_blends_the_colour_of_every_texel() {
        let mut canvas = Canvas::new(4, 1, [0, 0, 0, 255]);
        let source = RgbaImage {
            width: 2,
            height: 1,
            pixels: [[255, 255, 255, 255], [0, 0, 0, 0]].concat(),
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 4.0,
            h: 1.0,
        };
        let options = BlitOptions {
            use_alpha: false,
            ..stretch(true)
        };
        assert!(canvas.blit(rect, None, &source, [0, 0, 2, 1], options));
        assert_eq!(pixel(&canvas, 1, 0), [191, 191, 191, 255]);
    }
}
