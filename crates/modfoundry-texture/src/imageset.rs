use crate::edds::encode_edds;
use crate::mips::{edds_level_count, mip_chain};
use crate::RgbaImage;
use anyhow::{bail, ensure, Result};
use std::collections::HashSet;
use std::fmt::Write as _;

const LIMIT: u32 = 4096;
const MARGIN: u32 = 2;
const PAD: u32 = 2 * MARGIN;

/// One image going into a set: its name, the 1x image and an optional @2x image.
pub struct SetImage {
    pub name: String,
    pub image: RgbaImage,
    pub double: Option<RgbaImage>,
}

/// A built set: the `.imageset` text, the 1x `.edds` bytes, and the @2x `.edds` bytes when the images had them.
pub struct BuiltSet {
    pub text: String,
    pub texture: Vec<u8>,
    pub double_texture: Option<Vec<u8>>,
}

/// Packs the images into one atlas and writes the set text.
pub fn build_set(set_name: &str, game_dir: &str, images: Vec<SetImage>) -> Result<BuiltSet> {
    ensure!(!images.is_empty(), "a set needs at least one image");
    check_ident(set_name, "set name")?;
    check_game_dir(game_dir)?;
    let has_doubles = check_images(&images)?;
    // The @2x texture is twice the size, so it must still fit the limit.
    let limit = if has_doubles { LIMIT / 2 } else { LIMIT };

    let mut by_size: Vec<&SetImage> = images.iter().collect();
    by_size.sort_by(|a, b| {
        b.image
            .height
            .cmp(&a.image.height)
            .then(b.image.width.cmp(&a.image.width))
            .then_with(|| a.name.cmp(&b.name))
    });
    let Layout {
        width,
        height,
        positions,
    } = choose_layout(&by_size, limit)?;
    let mut placed: Vec<(&SetImage, (u32, u32))> = by_size.into_iter().zip(positions).collect();
    placed.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    let singles: Vec<(&RgbaImage, (u32, u32))> =
        placed.iter().map(|(item, at)| (&item.image, *at)).collect();
    let atlas = render(&singles, width, height);
    let texture = encode_edds(&mip_chain(atlas, edds_level_count(width, height)))?;

    let doubles: Option<Vec<(&RgbaImage, (u32, u32))>> = placed
        .iter()
        .map(|(item, (x, y))| Some((item.double.as_ref()?, (*x * 2, *y * 2))))
        .collect();
    let text = write_text(
        set_name,
        game_dir,
        width,
        height,
        &placed,
        doubles.is_some(),
    );
    let double_texture = doubles
        .map(|images| {
            let atlas = render(&images, width * 2, height * 2);
            encode_edds(&mip_chain(atlas, edds_level_count(width * 2, height * 2)))
        })
        .transpose()?;
    Ok(BuiltSet {
        text,
        texture,
        double_texture,
    })
}

fn check_ident(value: &str, what: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{what} cannot be empty");
    ensure!(
        value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "{what} {value:?} may only use letters, digits and _"
    );
    Ok(())
}

fn check_game_dir(game_dir: &str) -> Result<()> {
    ensure!(
        game_dir
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
            && !game_dir.contains(|c: char| c == '\\' || c == '"' || c.is_control()),
        "game_dir {game_dir:?} must be folder names joined by /, with no empty, . or .. folder, \
         and no quotes or control characters"
    );
    Ok(())
}

fn check_images(images: &[SetImage]) -> Result<bool> {
    let mut seen = HashSet::new();
    for item in images {
        check_ident(&item.name, "image name")?;
        ensure!(
            item.image.width > 0 && item.image.height > 0,
            "image {:?} is {}x{}, with no pixels",
            item.name,
            item.image.width,
            item.image.height
        );
        ensure!(
            seen.insert(item.name.to_ascii_lowercase()),
            "image name {:?} appears twice, ignoring case",
            item.name
        );
    }
    let has_doubles = images.iter().any(|item| item.double.is_some());
    if let Some(missing) = images.iter().find(|item| item.double.is_none()) {
        ensure!(
            !has_doubles,
            "image {:?} has no 2x image, but another does",
            missing.name
        );
    }
    for item in images {
        if let Some(double) = &item.double {
            let (wide, tall) = (item.image.width * 2, item.image.height * 2);
            ensure!(
                (double.width, double.height) == (wide, tall),
                "image {:?} is {}x{}, so its 2x image must be {wide}x{tall}, not {}x{}",
                item.name,
                item.image.width,
                item.image.height,
                double.width,
                double.height
            );
        }
    }
    Ok(has_doubles)
}

struct Layout {
    width: u32,
    height: u32,
    positions: Vec<(u32, u32)>,
}

fn choose_layout(by_size: &[&SetImage], limit: u32) -> Result<Layout> {
    let widest = by_size
        .iter()
        .map(|item| item.image.width + PAD)
        .max()
        .unwrap_or(0);
    let first = widest.next_power_of_two();
    let mut best: Option<Layout> = None;
    let mut width = first;
    // Every candidate is at least the widest padded image, so an image always fits a fresh shelf.
    while width <= limit {
        let (height, positions) = pack(by_size, width);
        let height = height.next_power_of_two();
        if height <= limit
            && best
                .as_ref()
                .is_none_or(|b| better((width, height), (b.width, b.height)))
        {
            best = Some(Layout {
                width,
                height,
                positions,
            });
        }
        width *= 2;
    }
    if let Some(layout) = best {
        return Ok(layout);
    }
    let width = first.max(limit);
    let (height, _) = pack(by_size, width);
    bail!(
        "the set needs a texture of {width}x{}, past the {limit} limit",
        height.next_power_of_two()
    )
}

fn better(candidate: (u32, u32), current: (u32, u32)) -> bool {
    let rank = |(width, height): (u32, u32)| {
        let area = u64::from(width) * u64::from(height);
        let skew = u64::from(width.max(height)) * 1_000_000 / u64::from(width.min(height));
        (area, skew, width)
    };
    rank(candidate) < rank(current)
}

fn pack(by_size: &[&SetImage], width: u32) -> (u32, Vec<(u32, u32)>) {
    let mut positions = Vec::with_capacity(by_size.len());
    let (mut x, mut y, mut shelf) = (0, 0, 0);
    for item in by_size {
        let (wide, tall) = (item.image.width + PAD, item.image.height + PAD);
        if x + wide > width {
            y += shelf;
            x = 0;
            shelf = 0;
        }
        positions.push((x + MARGIN, y + MARGIN));
        x += wide;
        shelf = shelf.max(tall);
    }
    (y + shelf, positions)
}

fn render(placed: &[(&RgbaImage, (u32, u32))], width: u32, height: u32) -> RgbaImage {
    let mut atlas = RgbaImage {
        width,
        height,
        pixels: vec![0; width as usize * height as usize * 4],
    };
    for (image, (x, y)) in placed {
        blit(&mut atlas, image, *x, *y);
    }
    atlas
}

fn blit(atlas: &mut RgbaImage, source: &RgbaImage, x: u32, y: u32) {
    let row = source.width as usize * 4;
    let stride = atlas.width as usize * 4;
    for line in 0..source.height as usize {
        let from = line * row;
        let to = (y as usize + line) * stride + x as usize * 4;
        atlas.pixels[to..to + row].copy_from_slice(&source.pixels[from..from + row]);
    }
}

fn write_text(
    set_name: &str,
    game_dir: &str,
    width: u32,
    height: u32,
    placed: &[(&SetImage, (u32, u32))],
    doubles: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "ImageSetClass {{");
    let _ = writeln!(out, " Name \"{set_name}\"");
    let _ = writeln!(out, " RefSize {width} {height}");
    let _ = writeln!(out, " Textures {{");
    let _ = writeln!(out, "  ImageSetTextureClass {{");
    let _ = writeln!(out, "   mpix 1");
    let _ = writeln!(out, "   path \"{game_dir}/{set_name}.edds\"");
    let _ = writeln!(out, "  }}");
    if doubles {
        let _ = writeln!(out, "  ImageSetTextureClass {{");
        let _ = writeln!(out, "   mpix 2");
        let _ = writeln!(out, "   path \"{game_dir}/{set_name}@2x.edds\"");
        let _ = writeln!(out, "  }}");
    }
    let _ = writeln!(out, " }}");
    let _ = writeln!(out, " Images {{");
    for (item, (x, y)) in placed {
        let _ = writeln!(out, "  ImageSetDefClass {} {{", item.name);
        let _ = writeln!(out, "   Name \"{}\"", item.name);
        let _ = writeln!(out, "   Pos {x} {y}");
        let _ = writeln!(out, "   Size {} {}", item.image.width, item.image.height);
        let _ = writeln!(out, "   Flags 0");
        let _ = writeln!(out, "  }}");
    }
    let _ = writeln!(out, " }}");
    let _ = writeln!(out, " Groups {{");
    let _ = writeln!(out, " }}");
    let _ = writeln!(out, "}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edds::decode_edds;

    fn solid(width: u32, height: u32, colour: [u8; 4]) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: colour.repeat(width as usize * height as usize),
        }
    }

    fn image(name: &str, width: u32, height: u32, colour: [u8; 4]) -> SetImage {
        SetImage {
            name: name.to_string(),
            image: solid(width, height, colour),
            double: None,
        }
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let start = (y as usize * image.width as usize + x as usize) * 4;
        image.pixels[start..start + 4].try_into().unwrap()
    }

    #[test]
    fn two_images_write_the_vanilla_text_and_land_at_their_positions() {
        let images = vec![
            image("beta", 4, 16, [40, 50, 60, 255]),
            image("alpha", 8, 8, [10, 20, 30, 255]),
        ];

        let built = build_set("spark", "SampleUI/gui/imagesets", images).unwrap();

        let expected = "\
ImageSetClass {
 Name \"spark\"
 RefSize 16 32
 Textures {
  ImageSetTextureClass {
   mpix 1
   path \"SampleUI/gui/imagesets/spark.edds\"
  }
 }
 Images {
  ImageSetDefClass alpha {
   Name \"alpha\"
   Pos 2 22
   Size 8 8
   Flags 0
  }
  ImageSetDefClass beta {
   Name \"beta\"
   Pos 2 2
   Size 4 16
   Flags 0
  }
 }
 Groups {
 }
}
";
        assert_eq!(built.text, expected);

        let atlas = decode_edds(&built.texture).unwrap();
        let atlas = &atlas[0];
        assert_eq!((atlas.width, atlas.height), (16, 32));
        assert_eq!(pixel(atlas, 2, 2), [40, 50, 60, 255]);
        assert_eq!(pixel(atlas, 5, 17), [40, 50, 60, 255]);
        assert_eq!(pixel(atlas, 2, 22), [10, 20, 30, 255]);
        assert_eq!(pixel(atlas, 9, 29), [10, 20, 30, 255]);
        assert_eq!(pixel(atlas, 2, 18), [0, 0, 0, 0]);
        assert_eq!(pixel(atlas, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(atlas, 2, 30), [0, 0, 0, 0]);
        assert_eq!(pixel(atlas, 15, 2), [0, 0, 0, 0]);
        assert!(built.double_texture.is_none());
    }

    #[test]
    fn the_input_order_does_not_change_the_text_or_the_texture() {
        let alpha = || image("alpha", 8, 8, [10, 20, 30, 255]);
        let beta = || image("beta", 4, 16, [40, 50, 60, 255]);

        let forward = build_set("spark", "SampleUI/gui/imagesets", vec![beta(), alpha()]).unwrap();
        let backward = build_set("spark", "SampleUI/gui/imagesets", vec![alpha(), beta()]).unwrap();

        assert_eq!(forward.text, backward.text);
        assert_eq!(forward.texture, backward.texture);
    }

    #[test]
    fn a_double_image_goes_into_a_second_texture_at_twice_the_size() {
        let mut icon = image("icon", 8, 8, [1, 2, 3, 255]);
        icon.double = Some(solid(16, 16, [4, 5, 6, 255]));

        let built = build_set("spark", "SampleUI/gui/imagesets", vec![icon]).unwrap();

        let expected = "\
ImageSetClass {
 Name \"spark\"
 RefSize 16 16
 Textures {
  ImageSetTextureClass {
   mpix 1
   path \"SampleUI/gui/imagesets/spark.edds\"
  }
  ImageSetTextureClass {
   mpix 2
   path \"SampleUI/gui/imagesets/spark@2x.edds\"
  }
 }
 Images {
  ImageSetDefClass icon {
   Name \"icon\"
   Pos 2 2
   Size 8 8
   Flags 0
  }
 }
 Groups {
 }
}
";
        assert_eq!(built.text, expected);
        let texture = decode_edds(&built.texture).unwrap();
        let atlas = &texture[0];
        assert_eq!((atlas.width, atlas.height), (16, 16));
        assert_eq!(pixel(atlas, 2, 2), [1, 2, 3, 255]);

        let double = decode_edds(&built.double_texture.unwrap()).unwrap();
        let atlas = &double[0];
        assert_eq!((atlas.width, atlas.height), (32, 32));
        assert_eq!(pixel(atlas, 4, 4), [4, 5, 6, 255]);
        assert_eq!(pixel(atlas, 19, 19), [4, 5, 6, 255]);
        assert_eq!(pixel(atlas, 3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn equal_areas_pick_the_squarer_texture() {
        let images = vec![
            image("a", 12, 28, [1, 1, 1, 255]),
            image("b", 12, 28, [2, 2, 2, 255]),
        ];

        let built = build_set("set", "gui", images).unwrap();

        assert!(built.text.contains(" RefSize 32 32\n"), "{}", built.text);
    }

    #[test]
    fn invalid_sets_are_refused() {
        let doubled = |name: &str, wide: u32, tall: u32| SetImage {
            name: name.to_string(),
            image: solid(wide, tall, [1, 1, 1, 255]),
            double: Some(solid(wide * 2, tall * 2, [2, 2, 2, 255])),
        };
        let cases: [(&str, &str, &str, Vec<SetImage>, &str); 15] = [
            (
                "2x images for only some names",
                "set",
                "gui/imagesets",
                vec![
                    doubled("with", 4, 4),
                    image("without", 4, 4, [3, 3, 3, 255]),
                ],
                "\"without\" has no 2x image",
            ),
            (
                "a 2x image of the wrong size",
                "set",
                "gui/imagesets",
                vec![SetImage {
                    name: "icon".to_string(),
                    image: solid(4, 4, [1, 1, 1, 255]),
                    double: Some(solid(8, 7, [2, 2, 2, 255])),
                }],
                "must be 8x8, not 8x7",
            ),
            (
                "a name with a space",
                "set",
                "gui/imagesets",
                vec![image("bad name", 4, 4, [1, 1, 1, 255])],
                "image name \"bad name\" may only use letters, digits and _",
            ),
            (
                "an empty name",
                "set",
                "gui/imagesets",
                vec![image("", 4, 4, [1, 1, 1, 255])],
                "image name cannot be empty",
            ),
            (
                "two names that differ only in case",
                "set",
                "gui/imagesets",
                vec![
                    image("Icon", 4, 4, [1, 1, 1, 255]),
                    image("icon", 4, 4, [2, 2, 2, 255]),
                ],
                "image name \"icon\" appears twice, ignoring case",
            ),
            (
                "a game folder that climbs out",
                "set",
                "../gui",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "game_dir \"../gui\"",
            ),
            (
                "a game folder with a quote",
                "set",
                "gui\"imagesets",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "no quotes or control characters",
            ),
            (
                "an empty game folder",
                "set",
                "",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "game_dir \"\"",
            ),
            (
                "a game folder with a backslash",
                "set",
                "gui\\imagesets",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "game_dir",
            ),
            (
                "a game folder with an empty segment",
                "set",
                "gui//imagesets",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "game_dir \"gui//imagesets\"",
            ),
            (
                "an image with no pixels",
                "set",
                "gui/imagesets",
                vec![image("flat", 0, 4, [1, 1, 1, 255])],
                "image \"flat\" is 0x4",
            ),
            (
                "2x images whose 1x texture passes 2048",
                "set",
                "gui/imagesets",
                vec![doubled("wide", 2045, 1)],
                "past the 2048 limit",
            ),
            (
                "a set name with a dash",
                "bad-name",
                "gui/imagesets",
                vec![image("icon", 4, 4, [1, 1, 1, 255])],
                "set name \"bad-name\" may only use letters, digits and _",
            ),
            (
                "no images",
                "set",
                "gui/imagesets",
                vec![],
                "at least one image",
            ),
            (
                "an image that cannot fit 4096",
                "set",
                "gui/imagesets",
                vec![image("huge", 4093, 1, [1, 1, 1, 255])],
                "needs a texture of 8192x8, past the 4096 limit",
            ),
        ];
        for (label, set_name, game_dir, images, expected) in cases {
            let error = match build_set(set_name, game_dir, images) {
                Ok(_) => panic!("{label} was accepted"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains(expected), "{label}: {error}");
        }
    }
}
