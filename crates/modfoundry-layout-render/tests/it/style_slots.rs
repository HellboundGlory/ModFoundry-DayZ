use std::collections::HashMap;

use modfoundry_layout::geometry::{resolve, Screen};
use modfoundry_layout::imageset::{Imageset, ImagesetImage, ImagesetTexture};
use modfoundry_layout::parse::parse;
use modfoundry_layout::styles::StyleImages;
use modfoundry_layout_render::{render, MemoryResources, RenderOptions, Rendered};
use modfoundry_texture::edds::encode_edds;
use modfoundry_texture::RgbaImage;

use crate::common::{pixel, solid_image};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const ORANGE: [u8; 4] = [255, 128, 0, 255];
const PURPLE: [u8; 4] = [128, 0, 255, 255];

fn draw(text: &str, resources: &MemoryResources) -> Rendered {
    draw_on(
        text,
        resources,
        Screen {
            width: 4,
            height: 4,
        },
    )
}

fn draw_on(text: &str, resources: &MemoryResources, screen: Screen) -> Rendered {
    let widgets = parse(text).unwrap();
    let layout = resolve(&widgets, text, screen);
    render(
        &layout,
        &widgets,
        text,
        &RenderOptions {
            background: BLACK,
            show_hidden: false,
        },
        resources,
    )
}

fn findings_but_size_hint(rendered: &Rendered) -> Vec<(&str, &str)> {
    rendered
        .findings
        .iter()
        .filter(|finding| finding.code != "MF-L237")
        .map(|finding| (finding.code.as_str(), finding.message.as_str()))
        .collect()
}

fn panel_style(slots: &[(&str, &str)]) -> StyleImages {
    StyleImages {
        imageset: "gui".to_owned(),
        slots: slots
            .iter()
            .map(|(slot, image)| (slot.to_string(), image.to_string()))
            .collect(),
    }
}

/// Set `gui` on `gui/s.edds`, where image `panel` covers the whole texture.
fn gui_resources(texels: RgbaImage, slots: &[(&str, &str)]) -> MemoryResources {
    gui_resources_flagged(texels, slots, "")
}

fn gui_resources_flagged(
    texels: RgbaImage,
    slots: &[(&str, &str)],
    flags: &str,
) -> MemoryResources {
    let set = Imageset {
        name: "gui".to_owned(),
        ref_size: (2, 2),
        textures: vec![ImagesetTexture {
            mpix: 0,
            path: "gui/s.edds".to_owned(),
        }],
        images: vec![ImagesetImage {
            name: "panel".to_owned(),
            x: 0,
            y: 0,
            w: 2,
            h: 2,
            flags: flags.to_owned(),
        }],
    };
    imageset_resources(set, texels, slots)
}

fn imageset_resources(set: Imageset, texels: RgbaImage, slots: &[(&str, &str)]) -> MemoryResources {
    MemoryResources {
        imagesets: HashMap::from([("gui".to_owned(), set)]),
        files: HashMap::from([("gui/s.edds".to_owned(), encode_edds(&[texels]).unwrap())]),
        style_images: HashMap::from([(
            ("PanelWidget".to_owned(), "DayZDefaultPanel".to_owned()),
            panel_style(slots),
        )]),
        ..MemoryResources::default()
    }
}

const ATLAS: u32 = 16;

struct Region {
    name: &'static str,
    at: (u32, u32),
    size: (u32, u32),
    colour: [u8; 4],
}

fn atlas(regions: &[Region], slots: &[(&str, &str)]) -> MemoryResources {
    let mut texels = RgbaImage {
        width: ATLAS,
        height: ATLAS,
        pixels: vec![0; (ATLAS * ATLAS * 4) as usize],
    };
    for region in regions {
        for y in region.at.1..region.at.1 + region.size.1 {
            for x in region.at.0..region.at.0 + region.size.0 {
                let at = ((y * ATLAS + x) * 4) as usize;
                texels.pixels[at..at + 4].copy_from_slice(&region.colour);
            }
        }
    }
    let set = Imageset {
        name: "gui".to_owned(),
        ref_size: (ATLAS, ATLAS),
        textures: vec![ImagesetTexture {
            mpix: 0,
            path: "gui/s.edds".to_owned(),
        }],
        images: regions
            .iter()
            .map(|region| ImagesetImage {
                name: region.name.to_owned(),
                x: region.at.0,
                y: region.at.1,
                w: region.size.0,
                h: region.size.1,
                flags: String::new(),
            })
            .collect(),
    };
    imageset_resources(set, texels, slots)
}

const PANEL: &str =
    "PanelWidgetClass Panel {\n position 0 0\n size 1 1\n style DayZDefaultPanel\n}";

#[test]
fn a_center_slot_fills_the_widget_box() {
    let resources = gui_resources(solid_image(RED), &[("Center", "panel")]);
    let rendered = draw(PANEL, &resources);
    assert_eq!(pixel(&rendered, 0, 0), RED);
    assert_eq!(pixel(&rendered, 3, 3), RED);
    assert_eq!(rendered.drawn, 1);
    assert!(findings_but_size_hint(&rendered).is_empty());
}

#[test]
fn a_missing_slot_image_is_reported_and_other_widgets_still_draw() {
    let mut resources = gui_resources(solid_image(RED), &[("Center", "panel")]);
    resources.style_images.insert(
        ("PanelWidget".to_owned(), "Broken".to_owned()),
        panel_style(&[("Center", "missing")]),
    );
    let text = "PanelWidgetClass Missing {\n position 0 0\n size 1 1\n style Broken\n}\n\
                PanelWidgetClass Panel {\n position 0 0\n size 1 1\n style DayZDefaultPanel\n}";
    let rendered = draw(text, &resources);
    assert_eq!(pixel(&rendered, 0, 0), RED);
    assert_eq!(rendered.drawn, 1);
    assert_eq!(
        findings_but_size_hint(&rendered),
        [(
            "MF-L214",
            "image missing of style Broken is not drawn: image missing is not in imageset gui: Missing"
        )]
    );
}

#[test]
fn a_style_with_a_drawn_slot_and_an_undrawn_one_stays_an_outline() {
    let resources = gui_resources(
        solid_image(RED),
        &[("Center", "panel"), ("BarCenter", "panel")],
    );
    let rendered = draw(PANEL, &resources);
    let codes: Vec<&str> = rendered.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["MF-L210"]);
}

#[test]
fn a_style_without_a_center_slot_draws_its_edges_only() {
    let resources = atlas(
        &[Region {
            name: "strip_left",
            at: (0, 0),
            size: (2, 2),
            colour: GREEN,
        }],
        &[("Left", "strip_left")],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 0, 500), pixel(&rendered, 1, 500)],
        [GREEN; 2]
    );
    assert_eq!(pixel(&rendered, 2, 500), BLACK);
    assert!(findings_but_size_hint(&rendered).is_empty());
}

#[test]
fn a_tiled_image_repeats_at_its_size_in_reference_units() {
    let texels = RgbaImage {
        width: 2,
        height: 2,
        pixels: [RED, BLUE, RED, BLUE].concat(),
    };
    let resources = gui_resources_flagged(texels, &[("Center", "panel")], "ISHorizontalTile");
    let screen = Screen {
        width: 12,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    let row: Vec<[u8; 4]> = (0..4).map(|x| pixel(&rendered, x, 0)).collect();
    assert_eq!(row, [RED, BLUE, RED, BLUE]);
}

#[test]
fn the_style_colour_tints_a_slice() {
    let mut resources = gui_resources(solid_image([255, 255, 255, 255]), &[("Center", "panel")]);
    resources.style_colors.insert(
        ("PanelWidget".to_owned(), "DayZDefaultPanel".to_owned()),
        0xff00_00ff,
    );
    let rendered = draw(PANEL, &resources);
    assert_eq!(pixel(&rendered, 1, 1), BLUE);
}

#[test]
fn a_left_strip_sits_at_the_left_and_the_centre_starts_after_it() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_left",
                at: (2, 0),
                size: (2, 2),
                colour: GREEN,
            },
        ],
        &[("Center", "middle"), ("Left", "strip_left")],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 0, 500), pixel(&rendered, 1, 500)],
        [GREEN; 2]
    );
    assert_eq!(
        [pixel(&rendered, 2, 500), pixel(&rendered, 7, 500)],
        [BLUE; 2]
    );
}

#[test]
fn a_top_strip_sits_at_the_top_and_the_centre_starts_below_it() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_top",
                at: (2, 0),
                size: (2, 2),
                colour: YELLOW,
            },
        ],
        &[("Center", "middle"), ("Top", "strip_top")],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 4, 0), pixel(&rendered, 4, 1)],
        [YELLOW; 2]
    );
    assert_eq!(
        [pixel(&rendered, 4, 2), pixel(&rendered, 0, 500)],
        [BLUE; 2]
    );
}

#[test]
fn a_tiled_top_strip_repeats_along_the_top() {
    let texels = RgbaImage {
        width: 2,
        height: 2,
        pixels: [RED, BLUE, RED, BLUE].concat(),
    };
    let resources = gui_resources_flagged(texels, &[("Top", "panel")], "ISHorizontalTile");
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    let row: Vec<[u8; 4]> = (0..8).map(|x| pixel(&rendered, x, 0)).collect();
    assert_eq!(row, [RED, BLUE].repeat(4));
}

#[test]
fn a_right_and_a_bottom_strip_sit_at_the_far_sides() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_right",
                at: (2, 0),
                size: (2, 2),
                colour: WHITE,
            },
            Region {
                name: "strip_bottom",
                at: (4, 0),
                size: (2, 2),
                colour: YELLOW,
            },
        ],
        &[
            ("Center", "middle"),
            ("Right", "strip_right"),
            ("Bottom", "strip_bottom"),
        ],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 6, 500), pixel(&rendered, 7, 500)],
        [WHITE; 2]
    );
    assert_eq!(
        [pixel(&rendered, 5, 500), pixel(&rendered, 4, 1077)],
        [BLUE; 2]
    );
    assert_eq!(
        [pixel(&rendered, 4, 1078), pixel(&rendered, 4, 1079)],
        [YELLOW; 2]
    );
}

#[test]
fn corners_sit_at_the_box_corners_over_the_ends_of_the_edges() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_left",
                at: (2, 0),
                size: (2, 2),
                colour: GREEN,
            },
            Region {
                name: "strip_top",
                at: (4, 0),
                size: (2, 2),
                colour: YELLOW,
            },
            Region {
                name: "strip_right",
                at: (6, 0),
                size: (2, 2),
                colour: WHITE,
            },
            Region {
                name: "strip_bottom",
                at: (8, 0),
                size: (2, 2),
                colour: CYAN,
            },
            Region {
                name: "corner_left_top",
                at: (10, 0),
                size: (3, 3),
                colour: RED,
            },
            Region {
                name: "corner_right_top",
                at: (13, 0),
                size: (3, 3),
                colour: MAGENTA,
            },
            Region {
                name: "corner_left_bottom",
                at: (0, 4),
                size: (3, 3),
                colour: ORANGE,
            },
            Region {
                name: "corner_right_bottom",
                at: (3, 4),
                size: (3, 3),
                colour: PURPLE,
            },
        ],
        &[
            ("Center", "middle"),
            ("Left", "strip_left"),
            ("Top", "strip_top"),
            ("Right", "strip_right"),
            ("Bottom", "strip_bottom"),
            ("LeftTop", "corner_left_top"),
            ("RightTop", "corner_right_top"),
            ("LeftBottom", "corner_left_bottom"),
            ("RightBottom", "corner_right_bottom"),
        ],
    );
    let screen = Screen {
        width: 10,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 0, 0), pixel(&rendered, 9, 0)],
        [RED, MAGENTA]
    );
    assert_eq!(
        [pixel(&rendered, 0, 1079), pixel(&rendered, 9, 1079)],
        [ORANGE, PURPLE]
    );
    assert_eq!(
        [pixel(&rendered, 5, 1), pixel(&rendered, 5, 1079)],
        [YELLOW, CYAN]
    );
}

#[test]
fn an_absent_corner_lets_the_edge_run_to_the_box_end() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_top",
                at: (2, 0),
                size: (2, 2),
                colour: YELLOW,
            },
            Region {
                name: "corner_right_top",
                at: (4, 0),
                size: (3, 3),
                colour: MAGENTA,
            },
        ],
        &[
            ("Center", "middle"),
            ("Top", "strip_top"),
            ("RightTop", "corner_right_top"),
        ],
    );
    let screen = Screen {
        width: 10,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 0, 1), pixel(&rendered, 6, 1)],
        [YELLOW; 2]
    );
    assert_eq!(pixel(&rendered, 9, 0), MAGENTA);
}

#[test]
fn a_strip_is_half_as_thick_on_a_540_high_screen() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_left",
                at: (2, 0),
                size: (2, 2),
                colour: GREEN,
            },
        ],
        &[("Center", "middle"), ("Left", "strip_left")],
    );
    let screen = Screen {
        width: 8,
        height: 540,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 0, 100), pixel(&rendered, 1, 100)],
        [GREEN, BLUE]
    );
}

#[test]
fn a_failed_edge_is_reported_and_the_other_slots_still_draw() {
    let resources = atlas(
        &[
            Region {
                name: "middle",
                at: (0, 0),
                size: (2, 2),
                colour: BLUE,
            },
            Region {
                name: "strip_top",
                at: (2, 0),
                size: (2, 2),
                colour: YELLOW,
            },
        ],
        &[
            ("Center", "middle"),
            ("Left", "not_in_the_atlas"),
            ("Top", "strip_top"),
        ],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(
        [pixel(&rendered, 4, 1), pixel(&rendered, 0, 500)],
        [YELLOW, BLUE]
    );
    let failed = rendered
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L214")
        .count();
    assert_eq!(failed, 1);
}

#[test]
fn the_size_hint_is_given_once_with_the_number_of_styled_widgets_drawn() {
    let resources = atlas(
        &[Region {
            name: "middle",
            at: (0, 0),
            size: (2, 2),
            colour: BLUE,
        }],
        &[("Center", "middle")],
    );
    let second = "PanelWidgetClass Second {\n position 0 0\n size 1 1\n style DayZDefaultPanel\n}";
    let text = format!("{PANEL}\n{second}");
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(&text, &resources, screen);
    let hints: Vec<&str> = rendered
        .findings
        .iter()
        .filter(|finding| finding.code == "MF-L237")
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        hints,
        ["styled panel images are drawn at their atlas size times screen height / 1080; the game's size is not measured; 2 widgets drawn"]
    );
}

#[test]
fn no_size_hint_when_no_styled_widget_is_drawn() {
    let resources = gui_resources(solid_image(RED), &[("Center", "missing")]);
    let rendered = draw(PANEL, &resources);
    assert_eq!(rendered.drawn, 0);
    assert!(rendered
        .findings
        .iter()
        .all(|finding| finding.code != "MF-L237"));
}

fn region(name: &'static str, at: (u32, u32), size: (u32, u32), colour: [u8; 4]) -> Region {
    Region {
        name,
        at,
        size,
        colour,
    }
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];

#[test]
fn the_centre_stays_inside_a_transparent_edge() {
    let resources = atlas(
        &[
            region("middle", (0, 0), (2, 2), BLUE),
            region("strip_left", (2, 0), (2, 2), CLEAR),
        ],
        &[("Center", "middle"), ("Left", "strip_left")],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(pixel(&rendered, 1, 500), BLACK);
    assert_eq!(pixel(&rendered, 2, 500), BLUE);
}

#[test]
fn an_edge_starts_below_a_transparent_corner() {
    let resources = atlas(
        &[
            region("strip_left", (0, 0), (2, 2), GREEN),
            region("corner_left_top", (4, 0), (3, 3), CLEAR),
        ],
        &[("Left", "strip_left"), ("LeftTop", "corner_left_top")],
    );
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    assert_eq!(pixel(&rendered, 0, 1), BLACK);
    assert_eq!(pixel(&rendered, 0, 3), GREEN);
}

#[test]
fn edges_wider_than_the_box_stay_inside_it() {
    let resources = atlas(
        &[
            region("strip_left", (0, 0), (2, 2), GREEN),
            region("strip_right", (2, 0), (2, 2), YELLOW),
        ],
        &[("Left", "strip_left"), ("Right", "strip_right")],
    );
    let text = "PanelWidgetClass Narrow {\n hexactpos 1\n vexactpos 1\n hexactsize 1\n vexactsize 1\n position 3 0\n size 1 1080\n style DayZDefaultPanel\n}";
    let screen = Screen {
        width: 8,
        height: 1080,
    };
    let rendered = draw_on(text, &resources, screen);
    assert_eq!(pixel(&rendered, 2, 500), BLACK);
    assert_ne!(pixel(&rendered, 3, 500), BLACK);
    assert_eq!(pixel(&rendered, 4, 500), BLACK);
}

#[test]
fn a_tiled_edge_repeats_at_the_scaled_length() {
    let texels = RgbaImage {
        width: 2,
        height: 2,
        pixels: [RED, BLUE, RED, BLUE].concat(),
    };
    let resources = gui_resources_flagged(texels, &[("Top", "panel")], "ISHorizontalTile");
    let screen = Screen {
        width: 3,
        height: 540,
    };
    let rendered = draw_on(PANEL, &resources, screen);
    let row: Vec<[u8; 4]> = (0..3).map(|x| pixel(&rendered, x, 0)).collect();
    assert_eq!(row[0], row[1]);
    assert_eq!(row[1], row[2]);
    assert_ne!(row[0], RED);
}
