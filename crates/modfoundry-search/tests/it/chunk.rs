use modfoundry_search::chunk::{config_chunks, docs_chunks, layout_chunks, script_chunks, Kind};

const SCRIPT: &str = "class Vehicle
{
    int m_Speed;
    float m_Mass;

    void Start()
    {
    }

    int GetSpeed()
    {
        return m_Speed;
    }
}

void Helper()
{
}

enum Gear
{
    Park,
    Drive
}
";

#[test]
fn script_chunks_cut_class_head_methods_functions_and_enums() {
    let chunks = script_chunks(SCRIPT);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| {
            (
                c.kind.clone(),
                c.lines,
                c.class.as_deref(),
                c.method.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (Kind::Script, [1, 4], Some("Vehicle"), None),
            (Kind::Script, [6, 8], Some("Vehicle"), Some("Start")),
            (Kind::Script, [10, 13], Some("Vehicle"), Some("GetSpeed")),
            (Kind::Script, [16, 18], None, Some("Helper")),
            (Kind::Script, [20, 24], None, None),
        ]
    );
    let method_heads: Vec<&str> = chunks
        .iter()
        .filter(|c| c.method.is_some())
        .filter_map(|c| c.text.lines().next())
        .collect();
    assert_eq!(
        method_heads,
        ["void Start()", "int GetSpeed()", "void Helper()"]
    );
}

const CONFIG: &str = "class CfgPatches
{
    class X
    {
        units[] = {};
    };
};
class CfgVehicles
{
    class A;
    class B: A
    {
        scope = 2;
        class C
        {
            x = 1;
        };
    };
};
";

#[test]
fn config_chunks_keep_each_item_and_fold_its_small_sub_blocks() {
    let chunks = config_chunks(CONFIG);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.kind.clone(), c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (Kind::Config, Some("CfgPatches"), [1, 1], "class CfgPatches"),
            (
                Kind::Config,
                Some("CfgPatches/X"),
                [3, 5],
                "class X\nunits[] = {};"
            ),
            (
                Kind::Config,
                Some("CfgVehicles"),
                [8, 8],
                "class CfgVehicles"
            ),
            (
                Kind::Config,
                Some("CfgVehicles/B"),
                [11, 16],
                "class B: A\nscope = 2;\nclass C {\nx = 1;\n};"
            ),
        ]
    );
}

#[test]
fn docs_chunks_split_at_headings_and_name_each_by_its_heading() {
    let source = "# Install\n\nCopy the mod folder.\n\n## Usage\n\nRun it.\n";
    let chunks = docs_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.kind.clone(), c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                Kind::Docs,
                Some("Install"),
                [1, 3],
                "# Install\n\nCopy the mod folder."
            ),
            (Kind::Docs, Some("Usage"), [5, 7], "## Usage\n\nRun it."),
        ]
    );
    assert!(chunks.iter().all(|c| c.method.is_none()));
}

#[test]
fn docs_chunks_cut_a_long_section_at_line_breaks_within_1500_characters() {
    let source = "A line of plain text in the body.\n".repeat(124);
    let chunks = docs_chunks(&source);
    let lines: Vec<[usize; 2]> = chunks.iter().map(|c| c.lines).collect();
    assert_eq!(lines, [[1, 44], [45, 88], [89, 124]]);
    assert!(chunks.iter().all(|c| c.text.chars().count() <= 1500));
}

#[test]
fn docs_chunks_group_blank_line_paragraphs_up_to_the_limit() {
    let lf = format!("{}\n\n", "a".repeat(598)).repeat(4);
    let line = "a".repeat(200);
    let crlf = format!("{line}\r\n{line}\r\n{line}\r\n\r\n").repeat(3);
    let cases = [(lf, [[1, 3], [5, 7]]), (crlf, [[1, 7], [9, 11]])];
    for (source, expected) in cases {
        let lines: Vec<[usize; 2]> = docs_chunks(&source).iter().map(|c| c.lines).collect();
        assert_eq!(lines, expected, "{:?}", &source[..10]);
    }
}

#[test]
fn docs_chunks_keep_a_section_of_1500_characters_plus_trailing_blank_lines_whole() {
    let line = format!("{}\n", "a".repeat(49));
    let source = format!("{}{}\n\n", line.repeat(29), "a".repeat(50));
    let chunks = docs_chunks(&source);
    let lines: Vec<[usize; 2]> = chunks.iter().map(|c| c.lines).collect();
    assert_eq!(lines, [[1, 30]]);
}

#[test]
fn script_chunks_come_out_in_source_order() {
    let source = "void Helper()
{
}

class Vehicle
{
    int m_Speed;

    void Start()
    {
    }
}
";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(
        summary,
        [
            (None, Some("Helper"), [1, 3]),
            (Some("Vehicle"), None, [5, 7]),
            (Some("Vehicle"), Some("Start"), [9, 11]),
        ]
    );
}

#[test]
fn docs_chunks_count_the_limit_in_characters_not_bytes() {
    let source = "é".repeat(1600);
    let chunks = docs_chunks(&source);
    let lengths: Vec<usize> = chunks.iter().map(|c| c.text.chars().count()).collect();
    assert_eq!(lengths, [1500, 100]);
}

#[test]
fn config_chunks_keep_the_classes_that_parsed_before_an_error() {
    let source = "class CfgPatches
{
    class X
    {
        units[] = {};
    };
};
class CfgVehicles
{
    scope = ;
};
";
    let chunks = config_chunks(source);
    let paths: Vec<Option<&str>> = chunks.iter().map(|c| c.class.as_deref()).collect();
    assert_eq!(
        paths,
        [
            Some("CfgPatches"),
            Some("CfgPatches/X"),
            Some("CfgVehicles")
        ]
    );
}

#[test]
fn docs_chunks_cut_only_at_markdown_headings_outside_code_fences() {
    let source = "# Title\n\n```c\nclass A\n{\n#ifdef SERVER\n void F();\n#endif\n}\n```\n\n#hashtag text\n\n## Sub ##\n\nmore\n";
    let chunks = docs_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines))
        .collect();
    assert_eq!(summary, [(Some("Title"), [1, 12]), (Some("Sub"), [14, 16])]);
    assert_eq!(docs_chunks("# C#\ntext\n")[0].class.as_deref(), Some("C#"));
}

#[test]
fn script_chunks_keep_method_comments_and_members_after_the_first_method() {
    let source = "class Vehicle
{
    int m_Speed;

    //! Starts the engine.
    void Start()
    {
    }

    int m_Late;

    /* Speed in m/s. */
    int GetSpeed()
    {
        return m_Speed;
    }
}
";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| {
            (
                c.method.as_deref(),
                c.lines,
                c.text.lines().next().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (None, [1, 3], "class Vehicle"),
            (Some("Start"), [5, 8], "//! Starts the engine."),
            (None, [10, 10], "int m_Late;"),
            (Some("GetSpeed"), [12, 16], "/* Speed in m/s. */"),
        ]
    );
}

#[test]
fn config_headers_read_the_same_in_either_brace_style() {
    let chunks = config_chunks("class A: Base {\n    x = 1;\n};\nclass B\n{\n    y = 2;\n};\n");
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["class A: Base\nx = 1;", "class B\ny = 2;"]);
}

#[test]
fn layout_chunks_report_a_broken_layout_by_line() {
    let err = layout_chunks("FrameWidgetClass root {\n visible 1\n}\n visible 0\n").unwrap_err();
    assert_eq!(err, "4:2: property outside any widget");
}

/// `source` one level down, under `class Cfg`, where its classes are items whose small sub-blocks fold.
fn under_cfg(source: &str) -> String {
    format!("class Cfg\n{{\n{source}}};\n")
}

#[test]
fn nested_classes_fold_only_under_200_characters() {
    for (piece_chars, folds) in [(199, true), (200, false)] {
        let value = format!("é{}", "a".repeat(piece_chars - 23));
        let statement = format!("text = \"{value}\";");
        let source = under_cfg(&format!(
            "class Outer\n{{\n    class Inner\n    {{\n        {statement}\n    }};\n}};\n"
        ));
        let inner_text = format!("class Inner\n{statement}");
        assert_eq!(
            inner_text.chars().count(),
            piece_chars,
            "{piece_chars} characters"
        );
        let chunks = config_chunks(&source);
        if folds {
            assert_eq!(chunks.len(), 2);
            assert_eq!(
                chunks[1].text,
                format!("class Outer\nclass Inner {{\n{statement}\n}};")
            );
        } else {
            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[2].class.as_deref(), Some("Cfg/Outer/Inner"));
            assert_eq!(chunks[2].text, inner_text);
        }
    }
}

#[test]
fn a_large_nested_class_keeps_its_piece_and_takes_its_small_children() {
    let source = format!(
        "class Top\n{{\n    class Large\n    {{\n        text = \"{}\";\n        class Small\n        {{\n            x = 1;\n        }};\n    }};\n}};\n",
        "a".repeat(200)
    );
    let large = format!(
        "class Large\ntext = \"{}\";\nclass Small {{\nx = 1;\n}};",
        "a".repeat(200)
    );
    let chunks = config_chunks(&source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("Top"), [1, 1], "class Top"),
            (Some("Top/Large"), [3, 8], large.as_str()),
        ]
    );
}

#[test]
fn unfolded_nested_classes_keep_source_order_below_a_top_level_class() {
    let big = format!("text = \"{}\";", "a".repeat(200));
    let source = format!(
        "class Top\n{{\n    class Large1\n    {{\n        {big}\n        class Large1a\n        {{\n            {big}\n        }};\n    }};\n    class Large2\n    {{\n        {big}\n    }};\n}};\n"
    );
    let chunks = config_chunks(&source);
    let paths: Vec<Option<&str>> = chunks.iter().map(|c| c.class.as_deref()).collect();
    assert_eq!(
        paths,
        [
            Some("Top"),
            Some("Top/Large1"),
            Some("Top/Large1/Large1a"),
            Some("Top/Large2"),
        ]
    );
}

#[test]
fn a_folded_class_keeps_its_unfolded_descendants_as_their_own_pieces() {
    let big = format!("text = \"{}\";", "a".repeat(200));
    let source = under_cfg(&format!(
        "class Top\n{{\n    class Mid\n    {{\n        class Fwd;\n        class Big\n        {{\n            {big}\n        }};\n    }};\n}};\n"
    ));
    let chunks = config_chunks(&source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    let big_text = format!("class Big\n{big}");
    assert_eq!(
        summary,
        [
            (Some("Cfg"), [1, 1], "class Cfg"),
            (Some("Cfg/Top"), [3, 5], "class Top\nclass Mid {\n};"),
            (Some("Cfg/Top/Mid/Big"), [8, 10], big_text.as_str()),
        ]
    );
}

#[test]
fn a_folded_child_counts_its_own_folded_children_toward_the_limit() {
    let value = "a".repeat(170);
    let source = format!(
        "class Top\n{{\n    class Mid\n    {{\n        text = \"{value}\";\n        class Small\n        {{\n            x = 1;\n        }};\n    }};\n}};\n"
    );
    let mid = format!("class Mid\ntext = \"{value}\";\nclass Small {{\nx = 1;\n}};");
    let chunks = config_chunks(&source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("Top"), [1, 1], "class Top"),
            (Some("Top/Mid"), [3, 8], mid.as_str()),
        ]
    );
}

#[test]
fn the_items_of_a_top_level_class_keep_their_own_pieces_and_their_sub_blocks_fold() {
    let chunks = config_chunks(
        "class CfgVehicles\n{\n    class Rifle: Base\n    {\n        scope = 2;\n        class Hit { value = 1; };\n    };\n    class Pistol: Base { scope = 2; };\n};\n",
    );
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("CfgVehicles"), "class CfgVehicles"),
            (
                Some("CfgVehicles/Rifle"),
                "class Rifle: Base\nscope = 2;\nclass Hit {\nvalue = 1;\n};"
            ),
            (Some("CfgVehicles/Pistol"), "class Pistol: Base\nscope = 2;"),
        ]
    );
}

#[test]
fn a_folded_header_drops_its_trailing_comment() {
    let chunks = config_chunks(&under_cfg(
        "class Top\n{\n    class C // note\n    {\n        x = 1;\n    };\n};\n",
    ));
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[1].text, "class Top\nclass C {\nx = 1;\n};");
}

const LAYOUT: &str = "FrameWidgetClass hud_se_frame_root {
 visible 1
 position 0 0
 {
  PanelWidgetClass {
   size 1 1
   {
    TextWidgetClass Label {
    }
   }
  }
 }
}
";

#[test]
fn layout_chunks_give_one_piece_per_widget_with_its_name_path_and_own_headers() {
    let chunks = layout_chunks(LAYOUT).unwrap();
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| {
            (
                c.kind.clone(),
                c.lines,
                c.class.as_deref(),
                c.method.as_deref(),
                c.text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                Kind::Layout,
                [1, 3],
                Some("hud_se_frame_root"),
                None,
                "FrameWidgetClass hud_se_frame_root {\n visible 1\n position 0 0",
            ),
            (
                Kind::Layout,
                [5, 6],
                Some("hud_se_frame_root/PanelWidgetClass"),
                None,
                "  PanelWidgetClass {\n   size 1 1",
            ),
            (
                Kind::Layout,
                [8, 8],
                Some("hud_se_frame_root/PanelWidgetClass/Label"),
                None,
                "    TextWidgetClass Label {",
            ),
        ]
    );
}

#[test]
fn script_chunks_cover_methods_in_both_arms_of_a_block() {
    let source = "class Vehicle
{
#ifdef SERVER
    void ServerTick()
    {
    }
#else
    void ClientTick()
    {
    }
#endif
    void Shared()
    {
    }
}
";
    let chunks = script_chunks(source);
    let methods: Vec<_> = chunks
        .iter()
        .filter(|c| c.method.is_some())
        .map(|c| (c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(
        methods,
        [
            (Some("ServerTick"), [4, 6]),
            (Some("ClientTick"), [8, 10]),
            (Some("Shared"), [12, 14]),
        ]
    );
    let members: Vec<&str> = chunks
        .iter()
        .filter(|c| c.method.is_none())
        .map(|c| c.text.as_str())
        .collect();
    assert!(
        members
            .iter()
            .all(|text| !text.contains("void ServerTick") && !text.contains("void ClientTick")),
        "{members:?}"
    );
}

#[test]
fn script_chunks_cover_a_function_and_an_enum_in_an_else_arm() {
    let source = "#ifdef SERVER
void ServerOnly()
{
}
#else
void ClientOnly()
{
}

enum Mode
{
    Off,
    On
}
#endif
";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("ServerOnly"), [2, 4]),
            (Some("ClientOnly"), [6, 8]),
            (None, [10, 14]),
        ]
    );
    assert!(chunks[2].text.starts_with("enum Mode"));
}

#[test]
fn script_chunks_give_one_piece_to_a_stretch_that_declares_globals_or_a_typedef() {
    let source = "#include \"<game>\\Scripts\\3_Game\\A.c\"

class First
{
}

const float MAX_SPEED = 10;
int g_Count = 0;
typedef array<string> Names;

class Second
{
}
";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| {
            (
                c.class.as_deref(),
                c.method.as_deref(),
                c.lines,
                c.text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (Some("First"), None, [3, 5], "class First\n{\n}"),
            (
                None,
                None,
                [7, 9],
                "const float MAX_SPEED = 10;\nint g_Count = 0;\ntypedef array<string> Names;"
            ),
            (Some("Second"), None, [11, 13], "class Second\n{\n}"),
        ]
    );
}

#[test]
fn script_chunks_give_no_piece_to_a_stretch_of_includes_and_comments() {
    let source = "#include \"<game>\\Scripts\\3_Game\\A.c\"

// Nothing but a note here.

class First
{
}
";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines))
        .collect();
    assert_eq!(summary, [(Some("First"), [5, 7])]);
}

#[test]
fn script_chunks_end_an_enum_at_its_own_closing_brace() {
    let source = "enum Mode
{
    Off, // }
    On
}
";
    let chunks = script_chunks(source);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].lines, [1, 5]);
    assert_eq!(chunks[0].text, "enum Mode\n{\n    Off, // }\n    On\n}");
}

#[test]
fn config_pieces_end_at_their_last_own_member() {
    let big = format!("text = \"{}\";", "a".repeat(200));
    let source = format!(
        "class CfgVehicles\n{{\n    class Vehicle\n    {{\n        scope = 2;\n        class Small\n        {{\n            x = 1;\n        }};\n        class Large\n        {{\n            {big}\n        }};\n    }};\n}};\n"
    );
    let chunks = config_chunks(&source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("CfgVehicles"), [1, 1]),
            (Some("CfgVehicles/Vehicle"), [3, 8]),
            (Some("CfgVehicles/Vehicle/Large"), [10, 12]),
        ]
    );
    assert_eq!(
        chunks[1].text,
        "class Vehicle\nscope = 2;\nclass Small {\nx = 1;\n};"
    );
}

#[test]
fn layout_chunks_carry_a_property_after_the_children_on_its_own_line() {
    let source = "FrameWidgetClass root {
 visible 1
 {
  TextWidgetClass Label {
   text \"hi\"
  }
 }
 priority 5
}
";
    let chunks = layout_chunks(source).unwrap();
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.lines, c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                Some("root"),
                [1, 2],
                "FrameWidgetClass root {\n visible 1\n priority 5"
            ),
            (
                Some("root/Label"),
                [4, 5],
                "  TextWidgetClass Label {\n   text \"hi\""
            ),
        ]
    );
}

#[test]
fn layout_chunks_name_a_widget_with_an_empty_name_by_its_class() {
    let source = "FrameWidgetClass root {
 {
  PanelWidgetClass \"\" {
   size 1 1
  }
  TextWidgetClass {
  }
 }
}
";
    let chunks = layout_chunks(source).unwrap();
    let paths: Vec<Option<&str>> = chunks.iter().map(|c| c.class.as_deref()).collect();
    assert_eq!(
        paths,
        [
            Some("root"),
            Some("root/PanelWidgetClass"),
            Some("root/TextWidgetClass")
        ]
    );
}

#[test]
fn layout_chunks_leave_a_byte_order_mark_out_of_the_text() {
    let chunks = layout_chunks("\u{feff}FrameWidgetClass root {\n visible 1\n}\n").unwrap();
    assert_eq!(chunks[0].text, "FrameWidgetClass root {\n visible 1");
}

#[test]
fn script_chunks_skip_an_other_arm_method_that_ends_inside_the_first_arm_one() {
    let cases = [
        "class A\n{\n#ifdef X\n  void Foo() {\n    if (a) {\n#else\n  void Foo() {\n#endif\n      y();\n    }\n  }\n  void Baz() {}\n}\n",
        "class A\n{\n#ifdef X\n  void Foo(int a)\n#else\n  void Foo(int a, int b)\n#endif\n  {\n    x();\n  }\n  void Baz() {}\n}\n",
    ];
    for source in cases {
        let chunks = script_chunks(source);
        assert!(chunks.iter().all(|c| !c.text.is_empty()), "{chunks:?}");
        let foos = chunks.iter().filter(|c| c.method.as_deref() == Some("Foo"));
        assert_eq!(foos.count(), 1, "{chunks:?}");
    }
}

#[test]
fn script_chunks_cut_a_class_and_a_global_found_only_in_the_other_arm() {
    let source = "#ifdef SERVER\nint g_Shown;\n#else\nint g_Hidden;\nclass Hidden\n{\n    void M()\n    {\n    }\n}\n#endif\n";
    let chunks = script_chunks(source);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.method.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            (None, None),
            (Some("Hidden"), None),
            (Some("Hidden"), Some("M"))
        ]
    );
    assert!(chunks[0].text.contains("g_Hidden"), "{chunks:?}");
}

#[test]
fn script_chunks_cut_one_class_and_one_function_when_the_header_differs_per_arm() {
    let class = "#ifdef X\nclass A : B\n#else\nclass A : C\n#endif\n{\n  void M() {}\n}\n";
    let chunks = script_chunks(class);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(
        summary,
        [(Some("A"), None, [2, 6]), (Some("A"), Some("M"), [7, 7])]
    );

    let function = "#ifdef X\nvoid F(int a)\n#else\nvoid F(int a, int b)\n#endif\n{\n  x();\n}\n";
    let chunks = script_chunks(function);
    let summary: Vec<_> = chunks
        .iter()
        .map(|c| (c.class.as_deref(), c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(summary, [(None, Some("F"), [2, 8])]);
}

#[test]
fn script_chunks_attach_only_comment_only_lines_to_the_piece_below() {
    type Piece<'a> = (Option<&'a str>, [usize; 2]);
    let cases: &[(&str, &[Piece])] = &[
        (
            "const int X = 1; /* limit */\nvoid F()\n{\n}\n",
            &[(None, [1, 1]), (Some("F"), [2, 4])],
        ),
        (
            "int a;\n/* limit */\nvoid F()\n{\n}\n",
            &[(None, [1, 1]), (Some("F"), [2, 5])],
        ),
        ("// note\nvoid F()\n{\n}\n", &[(Some("F"), [1, 4])]),
        (
            "/**\n * Doc.\n */\nvoid F()\n{\n}\n",
            &[(Some("F"), [1, 6])],
        ),
        (
            "/** doc */ /* more */\nvoid F()\n{\n}\n",
            &[(Some("F"), [1, 4])],
        ),
        ("// note\n\nvoid F()\n{\n}\n", &[(Some("F"), [3, 5])]),
        (
            "string s = \"//\";\nvoid F()\n{\n}\n",
            &[(None, [1, 1]), (Some("F"), [2, 4])],
        ),
    ];
    for (source, expected) in cases {
        let chunks = script_chunks(source);
        let summary: Vec<_> = chunks
            .iter()
            .map(|c| (c.method.as_deref(), c.lines))
            .collect();
        assert_eq!(summary, *expected, "{source:?}");
    }
}

#[test]
fn script_chunks_split_a_stretch_of_globals_at_the_piece_limit() {
    let source: String = (0..200)
        .map(|i| format!("const int CONSTANT_NUMBER_{i} = {i};\n"))
        .collect();
    let chunks = script_chunks(&source);
    assert!(chunks.len() > 1, "{chunks:?}");
    assert!(chunks
        .iter()
        .all(|c| c.class.is_none() && c.method.is_none()));
    assert!(
        chunks.iter().all(|c| c.text.chars().count() <= 1500),
        "{:?}",
        chunks
            .iter()
            .map(|c| c.text.chars().count())
            .collect::<Vec<_>>()
    );
    assert_eq!(chunks[0].lines[0], 1);
    assert_eq!(chunks[chunks.len() - 1].lines[1], 200);
    for pair in chunks.windows(2) {
        assert_eq!(pair[1].lines[0], pair[0].lines[1] + 1, "{:?}", pair);
    }
    let joined: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(joined.join("\n"), source.trim());
}

#[test]
fn script_chunks_keep_only_the_parts_of_a_long_stretch_that_declare_something() {
    let licence: String = (0..60)
        .map(|i| format!("// Licence line {i} of the header that sits above every file.\n"))
        .collect();
    let source = format!("{licence}const int LIMIT = 1;\n");
    let chunks = script_chunks(&source);
    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert!(chunks[0].text.ends_with("const int LIMIT = 1;"));
    assert_eq!(chunks[0].lines[1], 61);
}

#[test]
fn script_chunks_cover_a_method_in_an_arm_neither_basic_view_keeps() {
    type Piece<'a> = (Option<&'a str>, [usize; 2]);
    let class = "class A\n{\n#ifdef X\n  void One() {}\n#ifdef Y\n  void Two() {}\n#else\n  void Three() {}\n#endif\n#else\n  void Four() {}\n#ifdef Z\n  void Five() {}\n#else\n  void Six() {}\n#endif\n#endif\n}\n";
    let functions =
        "#ifdef X\n#ifdef Y\nvoid A() {}\n#else\nvoid B() {}\n#endif\n#else\nvoid C() {}\n#endif\n";
    let cases: &[(&str, &[Piece])] = &[
        (
            class,
            &[
                (Some("One"), [4, 4]),
                (Some("Two"), [6, 6]),
                (Some("Three"), [8, 8]),
                (Some("Four"), [11, 11]),
                (Some("Five"), [13, 13]),
                (Some("Six"), [15, 15]),
            ],
        ),
        (
            functions,
            &[
                (Some("A"), [3, 3]),
                (Some("B"), [5, 5]),
                (Some("C"), [8, 8]),
            ],
        ),
    ];
    for &(source, expected) in cases {
        let chunks = script_chunks(source);
        let summary: Vec<_> = chunks
            .iter()
            .filter(|c| c.method.is_some())
            .map(|c| (c.method.as_deref(), c.lines))
            .collect();
        assert_eq!(summary, *expected, "{source:?}");
        let members: Vec<&str> = chunks
            .iter()
            .filter(|c| c.method.is_none())
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            members
                .iter()
                .all(|text| !text.contains("void Three") && !text.contains("void Five")),
            "{members:?}"
        );
    }
}

#[test]
fn script_chunks_stop_adding_views_after_the_cap() {
    let source: String = (0..20)
        .map(|i| {
            format!(
                "#ifdef X{i}\n#ifdef Y{i}\nvoid A{i}() {{}}\n#else\nvoid B{i}() {{}}\n#endif\n#else\nvoid C{i}() {{}}\n#endif\n"
            )
        })
        .collect();
    let chunks = script_chunks(&source);
    let names: Vec<&str> = chunks.iter().filter_map(|c| c.method.as_deref()).collect();
    for i in 0..20 {
        assert!(names.contains(&format!("A{i}").as_str()), "A{i} missing");
        assert!(names.contains(&format!("C{i}").as_str()), "C{i} missing");
    }
    for i in 0..20 {
        let held = names.contains(&format!("B{i}").as_str());
        assert_eq!(held, i < 16, "B{i}");
    }
}

#[test]
fn script_chunks_cut_a_method_once_when_the_other_arm_closes_its_class_later() {
    let source = "class A
{
    void P() {}
#ifdef X
    void Q() {}
}
#else
    void R() {}
}
#endif
int g;
void H() {}
";
    let chunks = script_chunks(source);
    let holding_r: Vec<_> = chunks
        .iter()
        .filter(|c| c.text.contains("void R()"))
        .map(|c| (c.class.as_deref(), c.method.as_deref(), c.lines))
        .collect();
    assert_eq!(holding_r, [(Some("A"), Some("R"), [8, 8])], "{chunks:?}");
}
