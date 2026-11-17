//! Baking a TrueType face into multi-channel distance-field glyphs.

use anyhow::{anyhow, Result};
use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{correct_error_msdf, ErrorCorrectionConfig};
use fdsm::generate::generate_msdf;
use fdsm::render::correct_sign_msdf;
use fdsm::shape::{Contour, Shape};
use fdsm::transform::Transform;
use fdsm_ttf_parser::load_shape_from_face;
use image::Rgb32FImage;
use nalgebra::{Affine2, Matrix3};
use ttf_parser::{Face, GlyphId};

use crate::render::{code_points, kern_pairs, MISSING_BOX};
use crate::{CharSet, FieldFont, FieldGlyph};

const EM: u32 = 32;
const RANGE: u16 = 8;
const EDGE_SIN_ALPHA: f64 = 0.03;
const EDGE_SEED: u64 = 1234;
/// U+25A1's outer side as a share of the em.
const BOX_SIDE: f32 = 0.6;
/// U+25A1's stroke as a share of the em.
const BOX_STROKE: f32 = 0.08;
/// U+25A1's advance as a share of the em.
const BOX_ADVANCE: f32 = 0.7;
/// Glyphs whose box reaches further than this many ems from the pen are left out: they would
/// set every tile's size, and wild boxes in broken fonts would overflow the pixel fields.
const MAX_REACH_EMS: f32 = 2.0;

/// An outline, its tight box in font units, and the pixels per unit it bakes at.
struct Outline {
    shape: Shape<Contour>,
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
    scale: f32,
}

/// A glyph's padded ink box in tile pixels, with the outline that fills it.
struct Placed {
    outline: Outline,
    left: i16,
    top: i16,
    width: u16,
    height: u16,
}

impl Outline {
    fn from_face(face: &Face, glyph_id: GlyphId, scale: f32) -> Option<Self> {
        let bbox = face.glyph_bounding_box(glyph_id)?;
        let shape = load_shape_from_face(face, glyph_id)?;
        Some(Self {
            shape,
            x_min: f32::from(bbox.x_min),
            x_max: f32::from(bbox.x_max),
            y_min: f32::from(bbox.y_min),
            y_max: f32::from(bbox.y_max),
            scale,
        })
    }

    /// The U+25A1 a font without one gets: an outlined square on the baseline.
    fn missing_box(em: u32) -> Self {
        let side = f64::from(em as f32 * BOX_SIDE);
        let stroke = f64::from(em as f32 * BOX_STROKE);
        let outer = [
            Point::new(0.0, 0.0),
            Point::new(side, 0.0),
            Point::new(side, side),
            Point::new(0.0, side),
        ];
        let inner = [
            Point::new(stroke, stroke),
            Point::new(stroke, side - stroke),
            Point::new(side - stroke, side - stroke),
            Point::new(side - stroke, stroke),
        ];
        Self {
            shape: Shape {
                contours: [outer, inner].into_iter().map(ring).collect(),
            },
            x_min: 0.0,
            x_max: side as f32,
            y_min: 0.0,
            y_max: side as f32,
            scale: 1.0,
        }
    }

    /// One pixel above and left of the ink keeps the edge fringe inside the quad.
    fn place(self) -> Option<Placed> {
        let reach = MAX_REACH_EMS * EM as f32;
        let edges = [self.x_min, self.x_max, self.y_min, self.y_max].map(|edge| edge * self.scale);
        if edges.iter().any(|edge| !(-reach..=reach).contains(edge)) {
            return None;
        }
        let [x_min, x_max, y_min, y_max] = edges;
        let (x_min, x_max) = (x_min.floor() as i32, x_max.ceil() as i32);
        let (y_min, y_max) = (y_min.floor() as i32, y_max.ceil() as i32);
        Some(Placed {
            left: x_min as i16 - 1,
            top: y_max as i16 + 1,
            width: (x_max - x_min + 2) as u16,
            height: (y_max - y_min + 2) as u16,
            outline: self,
        })
    }
}

impl Placed {
    /// A `cell` square field with the ink box centred, on a half pixel when the difference is odd
    /// as the game centres it, and the outline at its sub-pixel place.
    fn field(&self, cell: u16) -> Vec<[u8; 3]> {
        let s = f64::from(self.outline.scale);
        let origin_x = f64::from(cell - self.width) / 2.0 - f64::from(self.left);
        let origin_y = f64::from(cell - self.height) / 2.0 + f64::from(self.top);
        let mut shape = self.outline.shape.clone();
        shape.transform(&Affine2::from_matrix_unchecked(Matrix3::new(
            s, 0.0, origin_x, 0.0, -s, origin_y, 0.0, 0.0, 1.0,
        )));
        let colored = Shape::edge_coloring_simple(shape, EDGE_SIN_ALPHA, EDGE_SEED);
        let prepared = colored.prepare();
        let mut field = Rgb32FImage::new(u32::from(cell), u32::from(cell));
        generate_msdf(&prepared, f64::from(RANGE), &mut field);
        correct_sign_msdf(&mut field, &prepared, FillRule::Nonzero);
        let config = ErrorCorrectionConfig::default();
        correct_error_msdf(&mut field, &colored, &prepared, f64::from(RANGE), &config);
        field
            .pixels()
            .map(|pixel| {
                pixel
                    .0
                    .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
            })
            .collect()
    }
}

fn ring(points: [Point; 4]) -> Contour {
    Contour {
        segments: (0..points.len())
            .map(|index| Segment::line(points[index], points[(index + 1) % points.len()]))
            .collect(),
    }
}

/// Bakes `font` into one distance field at `EM` pixels per em, unhinted.
pub fn render_field_font(font: &[u8], chars: CharSet) -> Result<FieldFont> {
    let not_a_font = || anyhow!("the file is not a font");
    let face = Face::parse(font, 0).map_err(|_| not_a_font())?;
    let font_ref = swash::FontRef::from_index(font, 0).ok_or_else(not_a_font)?;
    let charmap = font_ref.charmap();
    let code_points = code_points(&charmap, chars);

    let scale = EM as f32 / f32::from(face.units_per_em());
    let mut mapped = Vec::with_capacity(code_points.len());
    let mut slots = Vec::with_capacity(code_points.len());
    for &codepoint in &code_points {
        let glyph_id = charmap.map(codepoint);
        let (outline, advance) = match glyph_id {
            0 if codepoint == MISSING_BOX => (
                Some(Outline::missing_box(EM)),
                (EM as f32 * BOX_ADVANCE).round() as i16,
            ),
            0 => (None, (EM as f32 / 4.0).round() as i16),
            _ => {
                mapped.push((codepoint, glyph_id));
                (
                    Outline::from_face(&face, GlyphId(glyph_id), scale),
                    advance(&face, glyph_id, scale),
                )
            }
        };
        slots.push((codepoint, advance, outline.and_then(Outline::place)));
    }
    let widest = slots
        .iter()
        .filter_map(|(_, _, slot)| slot.as_ref())
        .map(|placed| placed.width.max(placed.height))
        .max()
        .unwrap_or(0);
    let cell = widest + RANGE;

    let glyphs: Vec<FieldGlyph> = slots
        .into_iter()
        .map(|(codepoint, advance, slot)| match slot {
            Some(placed) => FieldGlyph {
                codepoint,
                width: placed.width,
                height: placed.height,
                left: placed.left,
                top: placed.top,
                advance,
                field: placed.field(cell),
            },
            None => FieldGlyph {
                codepoint,
                width: 0,
                height: 0,
                left: 0,
                top: 0,
                advance,
                field: vec![[0; 3]; usize::from(cell) * usize::from(cell)],
            },
        })
        .collect();

    Ok(FieldFont {
        em: EM,
        range: RANGE,
        cap_height: cap_height(&face, scale, &glyphs),
        cell,
        glyphs,
        kerning: kern_pairs(&face, &mapped, scale),
    })
}

fn advance(face: &Face, glyph_id: u16, scale: f32) -> i16 {
    face.glyph_hor_advance(GlyphId(glyph_id))
        .map_or(0, |units| (f32::from(units) * scale).round() as i16)
}

fn cap_height(face: &Face, scale: f32, glyphs: &[FieldGlyph]) -> f32 {
    match face.tables().os2.and_then(|os2| os2.capital_height()) {
        Some(height) if height != 0 => f32::from(height) * scale,
        _ => match glyphs
            .iter()
            .find(|glyph| glyph.codepoint == u16::from(b'H'))
        {
            Some(h) if h.height > 0 => f32::from(h.top - 1),
            _ => 0.7 * EM as f32,
        },
    }
}
