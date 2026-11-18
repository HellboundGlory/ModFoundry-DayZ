use modfoundry_layout::imageset::{parse_imageset, ImagesetImage, ImagesetTexture};

const SAMPLE: &str = r#"ImageSetClass {
 Name "BleedingDrops"
 RefSize 512 512
 Textures {
  ImageSetTextureClass {
   mpix 0
   path "{35363C01F72D2EBE}Gui/imagesets/BleedingDrops.edds"
  }
 }
 Images {
  ImageSetDefClass Bleeding1 {
   Name "Bleeding1"
   Pos 0 0
   Size 128 128
   Flags 0
  }
 }
 Groups {
 }
}
"#;

#[test]
fn reads_the_name_size_texture_and_image_of_the_sample() {
    let set = parse_imageset(SAMPLE).expect("sample parses");
    assert_eq!(set.name, "BleedingDrops");
    assert_eq!(set.ref_size, (512, 512));
    assert_eq!(
        set.textures,
        [ImagesetTexture {
            mpix: 0,
            path: "Gui/imagesets/BleedingDrops.edds".into(),
        }]
    );
    assert_eq!(
        set.images,
        [ImagesetImage {
            name: "Bleeding1".into(),
            x: 0,
            y: 0,
            w: 128,
            h: 128,
            flags: "0".into(),
        }]
    );
}

const TWO_SCALES: &str = r#"ImageSetClass {
 Name "Icons"
 RefSize 256 256
 Textures {
  ImageSetTextureClass {
   mpix 1
   path "gui/imagesets/Icons.edds"
  }
  ImageSetTextureClass {
   mpix 2
   path "gui/imagesets/Icons@2x.edds"
  }
 }
 Images {
  ImageSetDefClass a {
   Name "a"
   Pos 0 0
   Size 16 16
   Flags 0
  }
  ImageSetDefClass b {
   Name "b"
   Pos 16 0
   Size 16 16
   Flags 0
  }
  ImageSetDefClass c {
   Name "c"
   Pos 32 0
   Size 16 16
   Flags 0
  }
 }
}
"#;

#[test]
fn keeps_every_texture_and_picks_the_highest_mpix_as_best() {
    let set = parse_imageset(TWO_SCALES).expect("two scales parse");
    assert_eq!(set.textures.len(), 2);
    assert_eq!(set.best_texture().map(|texture| texture.mpix), Some(2));
    assert_eq!(
        set.image("b").map(|image| (image.x, image.y)),
        Some((16, 0))
    );
}

#[test]
fn texel_scale_is_texture_size_over_ref_size_per_axis() {
    let cases = [
        ("1024 1024", (512, 512), (0.5, 0.5)),
        ("1024 512", (512, 128), (0.5, 0.25)),
    ];
    for (ref_size, texture, expected) in cases {
        let text = SAMPLE.replace("RefSize 512 512", &format!("RefSize {ref_size}"));
        let set = parse_imageset(&text).expect("sample parses");
        assert_eq!(set.texel_scale(texture.0, texture.1), expected);
    }
}

#[test]
fn errors_name_the_line_of_the_first_problem() {
    let cases = [
        ("\n\nImages {\n}\n", 3, "the first block must be ImageSetClass"),
        ("", 1, "no ImageSetClass block"),
        ("ImageSetClass {\n RefSize 8 8\n", 1, "block is not closed"),
        ("ImageSetClass {\n}\n}\n", 3, "} with no block to close"),
        ("ImageSetClass {\n Name \"x\n}\n", 2, "unterminated string"),
        ("ImageSetClass {\n Name \"x\" }\n}\n", 2, "unexpected brace"),
        ("ImageSetClass {\n RefSize 4 4 4\n}\n", 2, "RefSize needs two numbers"),
        (
            "ImageSetClass {\n RefSize 4 4\n Textures {\n  ImageSetTextureClass {\n   path \"a.edds\"\n  }\n }\n}\n",
            4,
            "mpix is missing",
        ),
        ("ImageSetClass {\n Name \"x\"\n}\n", 1, "RefSize is missing"),
        ("ImageSetClass {\n RefSize 0 4\n}\n", 2, "RefSize has a zero side"),
        ("ImageSetClass {\n RefSize 4 x\n}\n", 2, "RefSize is not a whole number"),
        (
            "ImageSetClass {\n RefSize 4 4\n Images {\n  ImageSetDefClass a {\n   Name \"a\"\n   Pos 1\n   Size 1 1\n  }\n }\n}\n",
            6,
            "Pos needs two numbers",
        ),
        (
            "ImageSetClass {\n RefSize 4 4\n Images {\n  ImageSetDefClass a {\n   Name \"a\"\n   Pos 1 1\n   Size 1 1y\n  }\n }\n}\n",
            7,
            "Size is not a whole number",
        ),
        (
            "ImageSetClass {\n RefSize 4 4\n Images {\n  ImageSetDefClass {\n   Pos 1 1\n   Size 1 1\n  }\n }\n}\n",
            4,
            "image has no name",
        ),
        (
            "ImageSetClass {\n RefSize 4 4\n Images {\n  ImageSetDefClass a {\n   Name \"a\"\n   Size 1 1\n  }\n }\n}\n",
            4,
            "Pos is missing",
        ),
    ];
    for (text, line, message) in cases {
        let error = parse_imageset(text).expect_err(text);
        assert_eq!(
            (error.line, error.message.as_str()),
            (line, message),
            "{text}"
        );
    }
}

#[test]
fn an_image_takes_its_header_word_when_it_has_no_name_property() {
    let text = "ImageSetClass {\n RefSize 8 8\n Images {\n  ImageSetDefClass Bare {\n   Pos 0 0\n   Size 2 2\n  }\n }\n}\n";
    let set = parse_imageset(text).expect("header word name");
    assert_eq!(set.image("Bare").map(|image| image.w), Some(2));
}

#[test]
fn comments_blank_lines_and_quoted_header_names_are_read() {
    let text = "// game file\nImageSetClass {\n\n Name \"Quiet\" // set name\n RefSize 8 8\n Images {\n  ImageSetDefClass \"Quoted Name\" {\n   Pos 1 2 // at\n   Size 3 4\n  }\n }\n}\n";
    let set = parse_imageset(text).expect("commented file");
    assert_eq!(set.name, "Quiet");
    let image = set.image("Quoted Name").expect("quoted name");
    assert_eq!((image.x, image.y, image.w, image.h), (1, 2, 3, 4));
}

#[test]
fn flags_are_kept_as_written_and_default_to_empty() {
    let text = "ImageSetClass {\n RefSize 8 8\n Images {\n  ImageSetDefClass a {\n   Name \"a\"\n   Pos 0 0\n   Size 1 1\n   Flags ISHorizontalTile|ISVerticalTile\n  }\n  ImageSetDefClass b {\n   Name \"b\"\n   Pos 0 0\n   Size 1 1\n  }\n }\n}\n";
    let set = parse_imageset(text).expect("flags");
    assert_eq!(
        set.image("a").map(|image| image.flags.as_str()),
        Some("ISHorizontalTile|ISVerticalTile")
    );
    assert_eq!(set.image("b").map(|image| image.flags.as_str()), Some(""));
}

#[test]
#[ignore]
fn every_game_imageset_parses_with_an_image_and_a_texture() {
    let Some(folder) = std::env::var_os("MF_GUI_IMAGESETS") else {
        return;
    };
    let mut checked = 0;
    for entry in std::fs::read_dir(folder).expect("imageset folder") {
        let path = entry.expect("folder entry").path();
        if path.extension().is_none_or(|ext| ext != "imageset") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("imageset text");
        let set =
            parse_imageset(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(!set.images.is_empty(), "{}", path.display());
        assert!(!set.textures.is_empty(), "{}", path.display());
        checked += 1;
    }
    assert!(checked > 0, "no .imageset files in the folder");
}

#[test]
fn only_a_sixteen_hex_digit_prefix_is_dropped_from_a_path() {
    let text = SAMPLE.replace("{35363C01F72D2EBE}", "{notahexid}");
    let set = parse_imageset(&text).expect("sample parses");
    assert_eq!(
        set.textures[0].path,
        "{notahexid}Gui/imagesets/BleedingDrops.edds"
    );
}

#[test]
fn a_byte_order_mark_before_the_root_is_skipped() {
    let text = format!("\u{feff}{SAMPLE}");
    let set = parse_imageset(&text).expect("bom file");
    assert_eq!(set.name, "BleedingDrops");
}

#[test]
fn a_texture_with_an_empty_path_is_an_error() {
    let text = SAMPLE.replace(
        "\"{35363C01F72D2EBE}Gui/imagesets/BleedingDrops.edds\"",
        "\"\"",
    );
    let error = parse_imageset(&text).expect_err("empty path");
    assert_eq!((error.line, error.message.as_str()), (7, "path is empty"));
}

#[test]
fn an_empty_name_property_falls_back_to_the_header_word() {
    let text = "ImageSetClass {\n RefSize 8 8\n Images {\n  ImageSetDefClass Bare {\n   Name \"\"\n   Pos 0 0\n   Size 2 2\n  }\n }\n}\n";
    let set = parse_imageset(text).expect("empty name");
    assert_eq!(set.image("Bare").map(|image| image.w), Some(2));
}

#[test]
fn a_comment_may_follow_a_value_without_a_space() {
    let text = "ImageSetClass {\n RefSize 8 8\n Images {\n  ImageSetDefClass a {\n   Name \"a\"\n   Pos 0 0\n   Size 2 2\n   Flags 3// note\n  }\n }\n}\n";
    let set = parse_imageset(text).expect("glued comment");
    assert_eq!(set.image("a").map(|image| image.flags.as_str()), Some("3"));
}

#[test]
fn a_sixteen_character_prefix_that_is_not_hex_is_kept() {
    let text = SAMPLE.replace("{35363C01F72D2EBE}", "{ZZZZZZZZZZZZZZZZ}");
    let set = parse_imageset(&text).expect("sample parses");
    assert_eq!(
        set.textures[0].path,
        "{ZZZZZZZZZZZZZZZZ}Gui/imagesets/BleedingDrops.edds"
    );
}

#[test]
fn slashes_inside_a_quoted_path_are_not_a_comment() {
    let text = SAMPLE.replace("Gui/imagesets", "Gui//imagesets");
    let set = parse_imageset(&text).expect("sample parses");
    assert_eq!(set.textures[0].path, "Gui//imagesets/BleedingDrops.edds");
}

#[test]
fn deeply_nested_blocks_are_an_error_not_a_crash() {
    let text = format!(
        "ImageSetClass {{\n{}{}}}\n",
        "A {\n".repeat(5000),
        "}\n".repeat(5000)
    );
    let error = parse_imageset(&text).expect_err("too deep");
    assert_eq!(error.message, "blocks nest too deeply");
}

#[test]
fn a_long_line_of_words_tokenizes_in_linear_time() {
    let text = format!(
        "ImageSetClass {{\n RefSize 8 8\n Name {}\n}}\n",
        "a ".repeat(200_000)
    );
    let started = std::time::Instant::now();
    let _ = parse_imageset(&text);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn a_quoted_string_may_hold_an_escaped_quote() {
    let text = "ImageSetClass {\n Name \"a\\\"b\"\n RefSize 8 8\n}\n";
    assert_eq!(parse_imageset(text).expect("escape").name, "a\"b");
}

#[test]
fn images_inside_a_group_are_found_by_name_after_the_ungrouped_ones() {
    let text = r#"ImageSetClass {
 Name "Grouped"
 RefSize 64 64
 Textures {
  ImageSetTextureClass {
   mpix 0
   path "gui/imagesets/Grouped.edds"
  }
 }
 Images {
  ImageSetDefClass Plain {
   Name "Plain"
   Pos 0 0
   Size 8 8
   Flags 0
  }
 }
 Groups {
  ImageSetGroupClass Arrows {
   Name "Arrows"
   Images {
    ImageSetDefClass arrow_left {
     Name "arrow_left"
     Pos 10 20
     Size 4 6
     Flags 0
    }
   }
  }
 }
}
"#;
    let set = parse_imageset(text).expect("parses");
    let names: Vec<_> = set.images.iter().map(|image| image.name.as_str()).collect();
    assert_eq!(names, ["Plain", "arrow_left"]);
    assert_eq!(
        set.image("arrow_left").map(|image| (image.x, image.w)),
        Some((10, 4))
    );
}
