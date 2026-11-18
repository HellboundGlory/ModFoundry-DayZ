use modfoundry_layout::geometry::{resolve, Layout, Rect, Screen, WidgetBox};
use modfoundry_layout::parse::parse;

const FIXTURE: &str = include_str!("calibration_probe.layout");
const SIZES: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (1280, 960)];

fn layout_at(width: u32, height: u32) -> Layout {
    resolve(&parse(FIXTURE).unwrap(), FIXTURE, Screen { width, height })
}

fn find<'a>(layout: &'a Layout, name: &str) -> &'a WidgetBox {
    layout
        .boxes
        .iter()
        .find(|item| item.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no box named {name}"))
}

fn assert_near(actual: Rect, want: [f32; 4], what: &str) {
    let got = [actual.x, actual.y, actual.w, actual.h];
    for (g, w) in got.iter().zip(want) {
        assert!(
            (g - w).abs() <= 1.0,
            "{what}: {got:?} is not {want:?} within 1 px"
        );
    }
}

/// Expected boxes as x, y, w, h, derived from the measured rules (`s` is height / 1080).
fn expected(width: f32, height: f32) -> Vec<(&'static str, [f32; 4])> {
    let (w, h) = (width, height);
    let s = h / 1080.0;
    vec![
        ("MFCalRoot", [0.0, 0.0, w, h]),
        ("RedFraction", [0.05 * w, 0.05 * h, 0.2 * w, 0.2 * h]),
        ("GreenExact", [100.0 * s, 300.0 * s, 200.0 * s, 100.0 * s]),
        ("BlueScaled", [400.0 * s, 300.0 * s, 200.0 * s, 100.0 * s]),
        ("OrangeMixed", [700.0 * s, 300.0 * s, 0.1 * w, 0.1 * h]),
        ("YellowCenter", [w / 2.0, h / 2.0, 200.0 * s, 100.0 * s]),
        (
            "MagentaCorner",
            [w - 200.0 * s, h - 110.0 * s, 150.0 * s, 80.0 * s],
        ),
        ("HiddenBox", [700.0 * s, 450.0 * s, 100.0 * s, 100.0 * s]),
        (
            "PrioHighFirst",
            [100.0 * s, 500.0 * s, 150.0 * s, 150.0 * s],
        ),
        (
            "PrioLowSecond",
            [175.0 * s, 575.0 * s, 150.0 * s, 150.0 * s],
        ),
        (
            "NestedParent",
            [1200.0 * s, 100.0 * s, 400.0 * s, 300.0 * s],
        ),
        (
            "NestedFraction",
            [
                1200.0 * s + 40.0 * s,
                100.0 * s + 30.0 * s,
                200.0 * s,
                150.0 * s,
            ],
        ),
        (
            "NestedCorner",
            [
                1600.0 * s - 10.0 * s - 80.0 * s,
                400.0 * s - 10.0 * s - 40.0 * s,
                80.0 * s,
                40.0 * s,
            ],
        ),
        ("ClipOn", [1200.0 * s, 450.0 * s, 200.0 * s, 100.0 * s]),
        ("ClipOnChild", [1350.0 * s, 500.0 * s, 200.0 * s, 100.0 * s]),
        ("ClipOff", [1450.0 * s, 450.0 * s, 200.0 * s, 100.0 * s]),
        (
            "ClipOffChild",
            [1600.0 * s, 500.0 * s, 200.0 * s, 100.0 * s],
        ),
        (
            "AspectInside",
            [
                0.05 * w,
                0.75 * h + 0.1 * h - 0.012 * w,
                0.12 * w,
                0.024 * w,
            ],
        ),
        ("AspectOutside", [0.3 * w, 0.75 * h, 0.12 * w, 0.2 * h]),
        (
            "AspectFixWidth",
            [
                0.55 * w + 0.06 * w - 0.05 * h,
                0.75 * h + 0.1 * h - 0.01 * h,
                0.1 * h,
                0.02 * h,
            ],
        ),
        ("GridPlainA", [400.0 * s, 520.0 * s, 100.0 * s, 100.0 * s]),
        ("GridPlainB", [500.0 * s, 520.0 * s, 100.0 * s, 100.0 * s]),
        ("GridPlainC", [600.0 * s, 520.0 * s, 100.0 * s, 100.0 * s]),
        ("GridGappedA", [410.0 * s, 670.0 * s, 86.667 * s, 80.0 * s]),
        (
            "GridGappedB",
            [506.667 * s, 670.0 * s, 86.667 * s, 80.0 * s],
        ),
        (
            "GridGappedC",
            [603.333 * s, 670.0 * s, 86.667 * s, 80.0 * s],
        ),
        ("WrapA", [800.0 * s, 520.0 * s, 120.0 * s, 60.0 * s]),
        ("WrapB", [920.0 * s, 520.0 * s, 120.0 * s, 60.0 * s]),
        ("WrapC", [800.0 * s, 580.0 * s, 120.0 * s, 60.0 * s]),
    ]
}

#[test]
fn calibration_boxes_match_the_game_at_three_sizes() {
    for (width, height) in SIZES {
        let layout = layout_at(width, height);
        for (name, want) in expected(width as f32, height as f32) {
            let what = format!("{name} at {width}x{height}");
            assert_near(find(&layout, name).rect, want, &what);
        }
    }
}

#[test]
fn calibration_clips_and_visibility_at_three_sizes() {
    for (width, height) in SIZES {
        let layout = layout_at(width, height);
        let s = height as f32 / 1080.0;
        let what = format!("ClipOnChild at {width}x{height}");
        let clip = find(&layout, "ClipOnChild").clip.unwrap();
        assert_near(clip, [1200.0 * s, 450.0 * s, 200.0 * s, 100.0 * s], &what);
        assert_eq!(find(&layout, "ClipOffChild").clip, None);
        assert!(!find(&layout, "HiddenBox").visible);
        assert!(find(&layout, "RedFraction").visible);
    }
}

#[test]
fn calibration_draw_order_at_1920() {
    let layout = layout_at(1920, 1080);
    assert!(find(&layout, "PrioHighFirst").draw_order > find(&layout, "PrioLowSecond").draw_order);
}

#[test]
fn calibration_findings_name_the_unmeasured_widgets() {
    let layout = layout_at(1920, 1080);
    let mut codes: Vec<_> = layout
        .findings
        .iter()
        .map(|item| item.code.as_str())
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["MF-L202"]);
    let outside = layout
        .findings
        .iter()
        .find(|item| item.code == "MF-L202")
        .unwrap();
    assert!(outside.message.ends_with(": AspectOutside"));
}
