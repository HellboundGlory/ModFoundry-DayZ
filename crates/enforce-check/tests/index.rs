use std::path::Path;

use enforce_check::probes::load_tree;
use enforce_check::{CheckInput, Diagnostic, Layer, ModInfo, Module, SourceFile, Target, check};

// One row per observed `decl-` probe: the probe source and every line the game
// printed for the probe's own files, in order ("Previously declared here"
// included, at the line it points at).
struct Row {
    id: &'static str,
    files: &'static [(&'static str, Layer, &'static str)],
    expected: &'static [(u32, bool, &'static str)],
    // A row about declaration order says nothing about the bodies of the same
    // files, so the lines those produce are left out.
    declarations_only: bool,
}

const GAME: Layer = Layer::Game;
const WORLD: Layer = Layer::World;

const TYPEDEF_AFTER_CLASS: &str = "class ProbeTD\n{\n}\n\ntypedef int ProbeTD;\n";
const MODDED_SAME_VALUE: &str = "class ProbeMod1\n{\n\tint m_A = 1;\n}\n\nmodded class ProbeMod1\n{\n\tint m_A = 2;\n}\n";
const MODDED_CLEARED: &str = "class ProbeMod3\n{\n\tint m_A = 1;\n}\n\nmodded class ProbeMod3\n{\n\tint m_A;\n}\n";
const ENUM_HEX: &str = "enum EProbeH\n{\n\tA = 0x10\n}\n";
const CONST_ARRAY: &str = "const int PROBE_N = 4;\n\nclass ProbeV13\n{\n\tint a[PROBE_N];\n}\n";

fn row(
    id: &'static str,
    files: &'static [(&'static str, Layer, &'static str)],
    expected: &'static [(u32, bool, &'static str)],
) -> Row {
    Row {
        id,
        files,
        expected,
        declarations_only: false,
    }
}

fn declaration_row(
    id: &'static str,
    files: &'static [(&'static str, Layer, &'static str)],
    expected: &'static [(u32, bool, &'static str)],
) -> Row {
    Row {
        id,
        files,
        expected,
        declarations_only: true,
    }
}

// `ProbeX#441#1` and `ProbeX#2#1` are the same wrapped class; only the id the
// merge assigned differs, so it is masked.
fn mask_merge_id(message: &str) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some(start) = rest.find('#') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest[1..].find('#') else {
            break;
        };
        out.push_str("#N");
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn rows() -> Vec<Row> {
    vec![
        row(
            "decl-001",
            &[
                ("Scripts/3_Game/a.c", GAME, "class ProbeDup\n{\n}\n"),
                ("Scripts/3_Game/b.c", GAME, "class ProbeDup\n{\n}\n"),
            ],
            &[
                (1, false, "Multiple declaration of class 'ProbeDup'"),
                (1, false, "Previously declared here"),
            ],
        ),
        row(
            "decl-002",
            &[
                ("Scripts/3_Game/a.c", GAME, "class ProbeDup2\n{\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "class ProbeDup2\n{\n}\n"),
            ],
            &[(1, false, "Multiple declaration of class 'ProbeDup2'")],
        ),
        row(
            "decl-004",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeBadBase : ProbeNoSuchBase\n{\n}\n")],
            &[(1, false, "Unknown type 'ProbeNoSuchBase'")],
        ),
        row(
            "decl-005",
            &[
                ("Scripts/3_Game/a.c", GAME, "class ProbeLateBase : ProbeWorldClass\n{\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "class ProbeWorldClass\n{\n}\n"),
            ],
            &[(1, false, "Unknown type 'ProbeWorldClass'")],
        ),
        row(
            "decl-006",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "enum EProbeEnum\n{\n\tA\n}\n\nclass ProbeEnumBase : EProbeEnum\n{\n}\n",
            )],
            &[(6, false, "Can't inherit class 'ProbeEnumBase'")],
        ),
        row(
            "decl-007",
            &[("Scripts/3_Game/a.c", GAME, "enum EProbeInt : int\n{\n\tA\n}\n")],
            &[(1, false, "Can't inherit class 'int'")],
        ),
        row(
            "decl-008",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "sealed class ProbeSealed\n{\n}\n\nclass ProbeSub : ProbeSealed\n{\n}\n",
            )],
            &[(5, false, "'ProbeSub': cannot derive from sealed type 'ProbeSealed'")],
        ),
        row(
            "decl-009",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "sealed class ProbeSealed2\n{\n}\n\nmodded class ProbeSealed2\n{\n}\n",
            )],
            &[(5, false, "Sealed type 'ProbeSealed2' can't be modded")],
        ),
        row(
            "decl-010",
            &[("Scripts/3_Game/a.c", GAME, "modded class ProbeNoTarget\n{\n}\n")],
            &[(1, false, "Unknown type 'ProbeNoTarget'")],
        ),
        row(
            "decl-011",
            &[
                ("Scripts/3_Game/a.c", GAME, "class ProbeGameClass\n{\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "modded class ProbeGameClass\n{\n}\n"),
            ],
            &[(1, false, "Unknown type 'ProbeGameClass'")],
        ),
        row(
            "decl-014",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeCirA : ProbeCirB\n{\n}\n\nclass ProbeCirB : ProbeCirA\n{\n}\n",
            )],
            &[(1, false, "Circle inheritance in class 'ProbeCirA'")],
        ),
        row(
            "decl-015",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeS1\n{\n\tstatic const int X = ProbeS2.Y;\n}\n\nclass ProbeS2\n{\n\tstatic const int Y = ProbeS1.X;\n}\n",
            )],
            &[(1, false, "Circle inheritance in class 'ProbeS1'")],
        ),
        row(
            "decl-016",
            &[("Scripts/3_Game/a.c", GAME, "typedef ProbeT2 ProbeT1;\ntypedef ProbeT1 ProbeT2;\n")],
            &[(1, false, "Circle inheritance in typedef 'ProbeT1'")],
        ),
        row(
            "decl-017",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "enum EProbeC1 : EProbeC2\n{\n\tA\n}\n\nenum EProbeC2 : EProbeC1\n{\n\tB\n}\n",
            )],
            &[(1, false, "Circle inheritance in enum 'EProbeC1'")],
        ),
        row(
            "decl-019",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeFwd;\n")],
            &[(1, false, "Forward declarations are deprecated!")],
        ),
        row(
            "decl-020",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "enum EProbeX\n{\n\tA\n}\n\nclass EProbeX\n{\n\tstatic const int B = 1;\n}\n",
            )],
            &[(6, false, "Multiple declaration of class 'EProbeX'")],
        ),
        row(
            "decl-021",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeShadow\n{\n\tint m_A;\n}\n\nclass ProbeShadow2 : ProbeShadow\n{\n\tint m_A;\n}\n",
            )],
            &[
                (8, false, "Possible variable name conflict 'm_A'"),
                (8, false, "Multiple declaration of variable 'm_A'"),
                (8, false, "Can't create variable 'm_A'"),
            ],
        ),
        row(
            "decl-023",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeMod2\n{\n\tint m_A = 1;\n}\n\nmodded class ProbeMod2\n{\n\tstring m_A = \"s\";\n}\n",
            )],
            &[(3, false, "Incompatible parameter 'm_A'")],
        ),
        row(
            "decl-026",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "enum EProbeM\n{\n\tA,\n\tB\n}\n\nmodded enum EProbeM\n{\n\tA\n}\n",
            )],
            &[(9, false, "Multiple declaration of variable 'A'")],
        ),
        row(
            "decl-027",
            &[("Scripts/3_Game/a.c", GAME, "enum EProbeV\n{\n\tA = 1,\n\tB = A\n}\n")],
            &[(4, false, "Can't convert enum value 'A' to number")],
        ),
        row(
            "decl-029",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV1\n{\n\tprivate protected int m_X;\n}\n")],
            &[(3, false, "Private conflicts with protected")],
        ),
        row(
            "decl-030",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV2\n{\n\tout int m_X;\n}\n")],
            &[(3, false, "inout/out/notnull can be used only on function parameters")],
        ),
        row(
            "decl-031",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV3\n{\n\tref int m_X;\n}\n")],
            &[(3, false, "Auto-pointer 'int' must be class-type")],
        ),
        row(
            "decl-032",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV4\n{\n\tvoid F(notnull int a)\n\t{\n\t}\n}\n")],
            &[
                (3, false, "'notnull' must not be used with 'int' type"),
                (3, false, "Error in parameters"),
            ],
        ),
        row(
            "decl-033",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV5\n{\n\tvoid F(ref ProbeV5 a)\n\t{\n\t}\n}\n")],
            &[(3, true, "Method argument can't be strong reference")],
        ),
        row(
            "decl-034",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV6\n{\n\tlocal int m_X;\n}\n")],
            &[(3, false, "Local variable 'm_X' outside function")],
        ),
        row(
            "decl-038",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV8\n{\n\tint a[0x10];\n}\n")],
            &[(3, false, "Expected numeric constant")],
        ),
        row(
            "decl-039",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV9\n{\n\tint a[0];\n}\n")],
            &[(3, false, "Wrong array size")],
        ),
        row(
            "decl-040",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV10\n{\n\tint a[65536];\n}\n")],
            &[(3, false, "Maximum static array index is 65535 'a'")],
        ),
        row(
            "decl-041",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV11\n{\n\tint a[40000];\n}\n")],
            &[
                (3, false, "Variable 'a' too large"),
                (3, false, "Can't create variable 'a'"),
            ],
        ),
        row(
            "decl-042",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV12\n{\n\tvector a[12000];\n}\n")],
            &[
                (3, false, "Variable 'a' too large"),
                (3, false, "Can't create variable 'a'"),
            ],
        ),
        row(
            "decl-044",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV14\n{\n\ttypename t[3];\n}\n")],
            &[(3, true, "Static array of 'typename' types is not supported")],
        ),
        row(
            "decl-045",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV15\n{\n\tvector v[2][2];\n}\n")],
            &[(3, true, "Static 2D array of 'vector' is not supported")],
        ),
        row(
            "decl-046",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV16\n{\n\tProbeNoSuch m_X;\n}\n")],
            &[(3, false, "Unknown type 'ProbeNoSuch'")],
        ),
        row(
            "decl-047",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV17\n{\n\tref ProbeNoSuch m_X;\n}\n")],
            &[
                (3, false, "Bad type 'ProbeNoSuch'"),
                (3, false, "Unknown type 'ProbeNoSuch'"),
            ],
        ),
        row(
            "decl-048",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV18\n{\n\tvoid F(ProbeNoSuch a)\n\t{\n\t}\n}\n")],
            &[
                (3, false, "Unknown type 'ProbeNoSuch'"),
                (3, false, "Error in parameters"),
            ],
        ),
        row(
            "decl-049",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeV19\n{\n\tvoid F(out ProbeNoSuch a)\n\t{\n\t}\n}\n")],
            &[
                (3, false, "Bad type 'ProbeNoSuch'"),
                (3, false, "Unknown type 'ProbeNoSuch'"),
                (3, false, "Error in parameters"),
            ],
        ),
        row(
            "decl-050",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeV20Type\n{\n}\n\nclass ProbeV21\n{\n\tint ProbeV20Type;\n}\n",
            )],
            &[(7, false, "Variable name 'ProbeV20Type' already used as type name")],
        ),
        // The probes that compile silently: a typedef after its class, and the
        // same-layer modded overrides (with and without an initialiser), a
        // module-level `const int` array size, and a modded target in another
        // layer.
        row("decl-018", &[("Scripts/3_Game/a.c", GAME, TYPEDEF_AFTER_CLASS)], &[]),
        row("decl-022", &[("Scripts/3_Game/a.c", GAME, MODDED_SAME_VALUE)], &[]),
        row("decl-024", &[("Scripts/3_Game/a.c", GAME, MODDED_CLEARED)], &[]),
        row("decl-028", &[("Scripts/3_Game/a.c", GAME, ENUM_HEX)], &[]),
        row("decl-043", &[("Scripts/3_Game/a.c", GAME, CONST_ARRAY)], &[]),
        row(
            "decl-025",
            &[
                ("Scripts/3_Game/a.c", GAME, "class ProbeMod4\n{\n\tint m_A;\n}\n\nclass ProbeMod5 : ProbeMod4\n{\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "modded class ProbeMod5\n{\n\tint m_A;\n}\n"),
            ],
            &[(1, false, "Unknown type 'ProbeMod5'")],
        ),
        // ord-001 to ord-003: a modded class may sit before its base in file
        // order and a later one wraps the first. The game's own line for each is
        // the body error on the modded class.
        declaration_row(
            "ord-001",
            &[
                ("Scripts/3_Game/a_mod.c", GAME, "modded class ProbeX\n{\n\tvoid F()\n\t{\n\t\tNopeA();\n\t}\n}\n"),
                ("Scripts/3_Game/z_base.c", GAME, "class ProbeX\n{\n}\n"),
            ],
            &[(5, false, "Undefined function 'ProbeX.NopeA'")],
        ),
        declaration_row(
            "ord-002",
            &[
                ("Scripts/3_Game/a_base.c", GAME, "class ProbeX\n{\n}\n"),
                ("Scripts/3_Game/z_mod.c", GAME, "modded class ProbeX\n{\n\tvoid F()\n\t{\n\t\tNopeZ();\n\t}\n}\n"),
            ],
            &[(5, false, "Undefined function 'ProbeX.NopeZ'")],
        ),
        declaration_row(
            "ord-003",
            &[
                (
                    "Scripts/3_Game/a.c",
                    GAME,
                    "class ProbeX\n{\n}\n\nmodded class ProbeX\n{\n\tvoid F()\n\t{\n\t\tNopeA();\n\t}\n}\n",
                ),
                ("Scripts/3_Game/b.c", GAME, "modded class ProbeX\n{\n\tvoid G()\n\t{\n\t}\n}\n"),
            ],
            &[(9, false, "Undefined function 'ProbeX#N#1.NopeA'")],
        ),
        // ord-005 and ord-006: `int x = ;` is accepted in a plain and a modded
        // class alike.
        row(
            "ord-005",
            &[("Scripts/3_Game/a.c", GAME, "class ProbeY\n{\n\tvoid F()\n\t{\n\t\tint x = ;\n\t}\n}\n")],
            &[],
        ),
        row(
            "ord-006",
            &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeY\n{\n}\n\nmodded class ProbeY\n{\n\tvoid F()\n\t{\n\t\tint x = ;\n\t}\n}\n",
            )],
            &[],
        ),
    ]
}

fn input(files: &[(&str, Layer, &str)]) -> CheckInput {
    CheckInput {
        target: Target::RetailServer,
        mods: vec![ModInfo {
            name: "probe".into(),
            cfg_class: "Probe_decl".into(),
        }],
        files: files
            .iter()
            .map(|(path, layer, text)| SourceFile {
                path: (*path).to_string(),
                layer: *layer,
                mod_index: 1,
                text: (*text).to_string(),
            })
            .collect(),
        extra_defines: Vec::new(),
        input_names: Vec::new(),
    }
}

fn diagnostics(files: &[(&str, Layer, &str)]) -> Vec<Diagnostic> {
    check(&input(files)).diagnostics
}

#[test]
fn declaration_rules_report_the_lines_the_game_printed() {
    let mut failures = Vec::new();
    for row in rows() {
        let mut got = diagnostics(row.files);
        if row.declarations_only {
            // These rows run the whole check; the body lines are compared too,
            // with the merge's own id masked since no readable source gives the
            // number the game prints.
            for diagnostic in &mut got {
                diagnostic.message = mask_merge_id(&diagnostic.message);
            }
        }
        // Every line, in order: the game prints `Previously declared here` as
        // its own entry, at the line it points at.
        let mut lines: Vec<(u32, bool, String)> = got
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.fixme, diagnostic.message.clone()))
            .collect();
        for diagnostic in &got {
            if let Some((_, line)) = &diagnostic.related {
                lines.push((*line, false, "Previously declared here".to_string()));
            }
        }
        let expected: Vec<(u32, bool, String)> = row
            .expected
            .iter()
            .map(|(line, fixme, message)| (*line, *fixme, (*message).to_string()))
            .collect();
        if lines != expected {
            failures.push(format!("{}: got {lines:?}, want {expected:?}", row.id));
            continue;
        }
        // Every diagnostic is in one of the row's files, in a module the row uses.
        for diagnostic in &got {
            let known = row
                .files
                .iter()
                .any(|(path, layer, _)| *path == diagnostic.file && Module::from(*layer) == diagnostic.module);
            let related_known = diagnostic.related.as_ref().is_some_and(|(file, _)| {
                row.files.iter().any(|(path, _, _)| *path == file)
            });
            if !known && !related_known {
                failures.push(format!(
                    "{}: {} at {} is not one of the row's files/modules",
                    row.id, diagnostic.message, diagnostic.file
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// The game stops the script load at the first module that fails, so the layer
// after it is never compiled and must not report `Unknown type` for its types.
#[test]
fn a_failed_module_stops_the_layers_after_it() {
    let case = input(&[
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeA\n{\n}\nclass ProbeA2\n{\n\tint ;\n}\n",
        ),
        ("Scripts/4_World/b.c", WORLD, "class ProbeB : ProbeA\n{\n}\n"),
    ]);
    let result = check(&case);
    assert_eq!(result.failed_modules, vec![Module::Game]);
    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.module == Module::Game),
        "{:?}",
        result.diagnostics
    );
}

// A soft parse error stops the module on a Diag target, where it is an error,
// so the index never runs and the unknown base is not reported.
#[test]
fn a_diag_soft_parse_error_stops_the_module_before_the_index() {
    let source = "class ProbeP\n{\n\tvoid F() int x;\n}\n\nclass ProbeC : ProbeNope\n{\n}\n";
    for (target, index_runs) in [(Target::RetailServer, true), (Target::DiagServer, false)] {
        let mut case = input(&[("Scripts/3_Game/a.c", GAME, source)]);
        case.target = target;
        let result = check(&case);
        assert_eq!(
            result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            match index_runs {
                true => vec!["Missing function scope", "Unknown type 'ProbeNope'"],
                false => vec!["Missing function scope"],
            },
            "{target:?}"
        );
        assert_eq!(result.failed_modules, vec![Module::Game], "{target:?}");
    }
}

// Two modded classes before the plain one in file order: the plain class is the
// first target, and each later modded class wraps the one before it.
#[test]
fn modded_classes_before_their_base_wrap_in_file_order() {
    let case = input(&[
        ("Scripts/3_Game/a_mod.c", GAME, "modded class ProbeM\n{\n\tint m_A = 1;\n}\n"),
        ("Scripts/3_Game/b_mod.c", GAME, "modded class ProbeM\n{\n\tint m_B = 2;\n}\n"),
        ("Scripts/3_Game/z_base.c", GAME, "class ProbeM\n{\n\tint m_A;\n\tint m_B;\n}\n"),
    ]);
    let result = check(&case);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// A duplicate class is caught while parsing, so a `modded class` of the same name
// still reports the duplicate.
#[test]
fn a_duplicate_class_beside_a_modded_class_still_reports() {
    let case = input(&[(
        "Scripts/3_Game/a.c",
        GAME,
        "class ProbeD\n{\n}\nclass ProbeD\n{\n}\nmodded class ProbeD\n{\n}\n",
    )]);
    let result = check(&case);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let first = &result.diagnostics[0];
    assert_eq!(first.message, "Multiple declaration of class 'ProbeD'");
    assert_eq!(first.line, 4);
    assert_eq!(
        first.related.as_ref().map(|(file, line)| (file.as_str(), *line)),
        Some(("Scripts/3_Game/a.c", 1))
    );
    assert_eq!(result.failed_modules, vec![Module::Game]);
}

// A `modded enum` whose target is not in this module, and a second modded enum
// that must wrap the first.
#[test]
fn a_modded_enum_reports_an_unknown_target_and_wraps_the_one_before_it() {
    let cases = [
        (
            vec![("Scripts/3_Game/a.c", GAME, "modded enum EProbeNope\n{\n\tA\n}\n")],
            vec!["Unknown type 'EProbeNope'"],
        ),
        (
            vec![
                ("Scripts/3_Game/a.c", GAME, "enum EProbeE\n{\n\tA\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "modded enum EProbeE\n{\n\tB\n}\n"),
            ],
            vec!["Unknown type 'EProbeE'"],
        ),
        (
            vec![(
                "Scripts/3_Game/a.c",
                GAME,
                "enum EProbeE2\n{\n\tA\n}\n\nmodded enum EProbeE2\n{\n\tB\n}\n\nmodded enum EProbeE2\n{\n\tA\n}\n",
            )],
            vec!["Multiple declaration of variable 'A'"],
        ),
    ];
    let mut failures = Vec::new();
    for (files, expected) in cases {
        let got: Vec<String> = check(&input(&files))
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        if got != expected {
            failures.push(format!("{files:?}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A `modded` class keeps the member table of what it wraps, so a subclass in a
// later layer sees the original's members as inherited.
#[test]
fn a_modded_class_keeps_the_members_of_the_class_it_wrapped() {
    let plain = &[
        ("Scripts/3_Game/a.c", GAME, "class ProbeK\n{\n\tint m_X;\n}\n"),
        ("Scripts/4_World/b.c", WORLD, "class ProbeK2 : ProbeK\n{\n\tint m_X;\n}\n"),
    ];
    let modded = &[
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeK\n{\n\tint m_X;\n}\n\nmodded class ProbeK\n{\n\tint m_Y;\n}\n",
        ),
        ("Scripts/4_World/b.c", WORLD, "class ProbeK2 : ProbeK\n{\n\tint m_X;\n}\n"),
    ];
    let failures = vec!["Possible variable name conflict 'm_X'", "Multiple declaration of variable 'm_X'", "Can't create variable 'm_X'"];
    for files in [plain, modded] {
        let got: Vec<String> = check(&input(files))
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .filter(|message| failures.contains(&message.as_str()))
            .collect();
        assert_eq!(got, failures, "{files:?}");
    }
}

#[test]
fn a_forward_declaration_warns_without_failing_the_module() {
    let result = check(&input(&[("Scripts/3_Game/a.c", GAME, "class ProbeFwd;\n")]));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].severity, enforce_check::Severity::Warning);
    assert!(result.failed_modules.is_empty());
}

#[test]
fn a_forward_declared_name_is_not_a_type_name_for_a_variable() {
    let text = "class FwdG;\nclass FwdM;\nclass FwdP;\nclass FwdL;\nint FwdG;\nclass ProbeUser\n{\n\tint FwdM;\n\tvoid Use(int FwdP)\n\t{\n\t\tint FwdL;\n\t}\n}\n";
    let result = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]));
    let messages: Vec<&str> = result.diagnostics.iter().map(|diagnostic| diagnostic.message.as_str()).collect();
    assert_eq!(messages, vec!["Forward declarations are deprecated!"; 4]);
    assert!(result.failed_modules.is_empty());
}

// Two mods can each ship `Scripts/3_Game/a.c`; the number in a wrapped class's
// name is the place of its file in the module, not of the first file with that path.
#[test]
fn files_with_one_path_get_their_own_place_in_a_wrapped_name() {
    let first = "class ProbeX\n{\n}\nmodded class ProbeX\n{\n\tvoid F()\n\t{\n\t\tNopeA();\n\t}\n}\n";
    let second = "modded class ProbeX\n{\n\tvoid G()\n\t{\n\t}\n}\n";
    let messages = |paths: [&str; 2]| -> Vec<String> {
        let files = [(paths[0], GAME, first), (paths[1], GAME, second)];
        diagnostics(&files).into_iter().map(|diagnostic| diagnostic.message).collect()
    };
    let distinct = messages(["Scripts/3_Game/a.c", "Scripts/3_Game/b.c"]);
    assert!(distinct.iter().any(|message| message.contains("ProbeX#")), "{distinct:?}");
    assert_eq!(messages(["Scripts/3_Game/a.c", "Scripts/3_Game/a.c"]), distinct);
}

#[test]
fn diag_targets_fail_soft_rules_that_retail_only_warns_about() {
    let source = "class ProbeV5\n{\n\tvoid F(ref ProbeV5 a)\n\t{\n\t}\n}\n";
    for (target, fixme, failed) in [
        (Target::RetailServer, true, false),
        (Target::DiagServer, false, true),
    ] {
        let mut case = input(&[("Scripts/3_Game/a.c", GAME, source)]);
        case.target = target;
        let result = check(&case);
        assert_eq!(result.diagnostics[0].fixme, fixme, "{target:?}");
        assert_eq!(
            result.diagnostics[0].severity == enforce_check::Severity::Error,
            failed,
            "{target:?}"
        );
        assert_eq!(!result.failed_modules.is_empty(), failed, "{target:?}");
    }
}

// A `modded` class takes the plain name, but the class it wrapped keeps its own
// base, members and parameters, so those are still checked.
#[test]
fn a_modded_class_still_checks_the_class_it_wrapped() {
    let cases: [(&str, Vec<(u32, &str)>); 2] = [
        (
            "class ProbeKeep\n{\n\tNoSuch m_X;\n\tvoid F(NoSuch p)\n\t{\n\t}\n}\n\nmodded class ProbeKeep\n{\n}\n",
            vec![
                (3, "Unknown type 'NoSuch'"),
                (4, "Unknown type 'NoSuch'"),
                (4, "Error in parameters"),
            ],
        ),
        (
            "class ProbeBad : ProbeNope\n{\n\tint m_X;\n}\n\nmodded class ProbeBad\n{\n\tint m_Y;\n}\n",
            vec![(1, "Unknown type 'ProbeNope'")],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(u32, String)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = expected
            .into_iter()
            .map(|(line, message)| (line, message.to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// The renamed original is still in the graph the circles are found in, so a
// cycle that runs through it is reported.
#[test]
fn a_circle_through_a_modded_class_is_reported() {
    let case = input(&[(
        "Scripts/3_Game/a.c",
        GAME,
        "class ProbeCirA : ProbeCirB\n{\n}\n\nclass ProbeCirB : ProbeCirA\n{\n}\n\nmodded class ProbeCirA\n{\n}\n",
    )]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Circle inheritance in class 'ProbeCirA'"]
    );
}

// Each module carries the whole earlier picture forward: a class declared in one
// module is still an ancestor two modules later.
#[test]
fn a_base_two_layers_back_is_still_inherited() {
    let case = input(&[
        ("Scripts/1_Core/a.c", Layer::Core, "class ProbeA\n{\n\tint m_X;\n}\n"),
        ("Scripts/3_Game/b.c", GAME, "class ProbeB : ProbeA\n{\n}\n"),
        ("Scripts/4_World/c.c", WORLD, "class ProbeC : ProbeA\n{\n\tint m_X;\n}\n"),
    ]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Possible variable name conflict 'm_X'",
            "Multiple declaration of variable 'm_X'",
            "Can't create variable 'm_X'",
        ]
    );
}

// The merge reads a base's members through the plain name, so a `modded` class
// that has not taken its name yet never stands in for the class it will wrap.
#[test]
fn the_merge_reads_members_through_the_plain_name() {
    let modded_first = &[
        (
            "Scripts/3_Game/a.c",
            GAME,
            "modded class ProbeB\n{\n\tint m_X;\n}\n\nmodded class ProbeA\n{\n\tint m_Y;\n}\n",
        ),
        (
            "Scripts/3_Game/z.c",
            GAME,
            "class ProbeB : ProbeA\n{\n}\n\nclass ProbeA\n{\n\tint m_X;\n}\n",
        ),
    ];
    let plain_first = &[
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeB : ProbeA\n{\n}\n\nclass ProbeA\n{\n\tint m_X;\n}\n",
        ),
        (
            "Scripts/3_Game/z.c",
            GAME,
            "modded class ProbeB\n{\n\tint m_X;\n}\n\nmodded class ProbeA\n{\n\tint m_Y;\n}\n",
        ),
    ];
    for files in [modded_first, plain_first] {
        let result = check(&input(files));
        assert!(result.diagnostics.is_empty(), "{files:?}: {:?}", result.diagnostics);
    }
}

// A forward declaration is only a name until the real class exists: beside one it
// is no record at all.
#[test]
fn a_forward_declaration_never_hides_the_real_class() {
    let cases: [(&str, Vec<(u32, &str)>); 2] = [
        (
            "class ProbeF\n;\n\nclass ProbeF\n{\n}\n\nclass ProbeG\n{\n\tProbeF f;\n}\n",
            vec![(1, "Forward declarations are deprecated!")],
        ),
        (
            "class ProbeBase;\n\nclass ProbeBase\n{\n\tint m_X;\n}\n\nclass ProbeD : ProbeBase\n{\n\tint m_X;\n}\n",
            vec![
                (1, "Forward declarations are deprecated!"),
                (10, "Possible variable name conflict 'm_X'"),
                (10, "Multiple declaration of variable 'm_X'"),
                (10, "Can't create variable 'm_X'"),
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(u32, String)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = expected
            .into_iter()
            .map(|(line, message)| (line, message.to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A forward declaration in an earlier module holds only the name, so the real
// class in the next module is not a duplicate.
#[test]
fn a_forward_declaration_in_an_earlier_module_is_no_duplicate() {
    let case = input(&[
        ("Scripts/3_Game/a.c", GAME, "class ProbeF2;\n"),
        ("Scripts/4_World/b.c", WORLD, "class ProbeF2\n{\n}\n"),
    ]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Forward declarations are deprecated!")]
    );
}

// The duplicate lookup starts in the class itself, so a member declared twice in
// one class reports like a base-class shadow.
#[test]
fn a_member_declared_twice_in_one_class_reports() {
    let case = input(&[("Scripts/3_Game/a.c", GAME, "class ProbeD\n{\n\tint m_X;\n\tint m_X;\n}\n")]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (4, "Possible variable name conflict 'm_X'"),
            (4, "Multiple declaration of variable 'm_X'"),
            (4, "Can't create variable 'm_X'"),
        ]
    );
}

// A name is only "used as a type name" when it names a declared type, so the
// words that are not types the game knows leave a variable name free.
#[test]
fn only_declared_types_take_a_name_away_from_a_variable() {
    let cases: [(&str, Vec<(u32, &str)>); 2] = [
        ("class ProbeBytes\n{\n\tint byte;\n\tstring uint;\n}\n", vec![]),
        ("class ProbeLong\n{\n\tlong q;\n}\n", vec![(3, "Unknown type 'long'")]),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(u32, String)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = expected
            .into_iter()
            .map(|(line, message)| (line, message.to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A global variable error is found before the classes are declared, and that
// pre-pass failing skips the class, member and function rows of the module.
#[test]
fn a_global_variable_error_skips_the_class_rows() {
    let case = input(&[(
        "Scripts/3_Game/a.c",
        GAME,
        "class ProbeA\n{\n}\n\nint ProbeA;\n\nclass ProbeB : Nope\n{\n}\n",
    )]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(5, "Variable name 'ProbeA' already used as type name")]
    );
}

// A file-level function's parameters and a file-level variable's type go through
// the same rows as a class's members.
#[test]
fn file_level_functions_and_variables_get_the_parameter_and_type_rows() {
    let cases: [(&str, Vec<(u32, &str)>); 3] = [
        (
            "class ProbeA\n{\n}\n\nvoid F(int ProbeA)\n{\n}\n",
            vec![(5, "Variable name 'ProbeA' already used as type name")],
        ),
        (
            "void F(NoSuch p)\n{\n}\n",
            vec![(1, "Unknown type 'NoSuch'"), (1, "Error in parameters")],
        ),
        ("NoSuch g;\n", vec![(1, "Unknown type 'NoSuch'")]),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(u32, String)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = expected
            .into_iter()
            .map(|(line, message)| (line, message.to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A modded redeclaration may change only the value, and the member it is compared
// with may be behind another wrap or in an ancestor of the target.
#[test]
fn a_modded_redeclaration_is_checked_against_the_member_it_replaces() {
    let cases: [(&str, Vec<(u32, &str)>); 3] = [
        (
            "class ProbeA\n{\n\tint m_X = 1;\n}\n\nclass ProbeB : ProbeA\n{\n}\n\nmodded class ProbeB\n{\n\tstring m_X = \"s\";\n}\n",
            vec![(3, "Incompatible parameter 'm_X'")],
        ),
        (
            "class ProbeC\n{\n\tint m_X = 1;\n}\n\nmodded class ProbeC\n{\n\tint m_Y;\n}\n\nmodded class ProbeC\n{\n\tstring m_X = \"s\";\n}\n",
            vec![(3, "Incompatible parameter 'm_X'")],
        ),
        (
            "class ProbeD\n{\n\tint m_X = 1;\n}\n\nmodded class ProbeD\n{\n\tstring m_X = \"s\";\n}\n",
            vec![(3, "Incompatible parameter 'm_X'")],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(u32, String)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = expected
            .into_iter()
            .map(|(line, message)| (line, message.to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// A modded class may come before its base in file order and is still wrapped.
#[test]
fn a_modded_class_wraps_a_base_that_comes_later() {
    let case = input(&[
        ("Scripts/3_Game/a_mod.c", GAME, "modded class ProbeX\n{\n\tint m_A = 2;\n}\n"),
        ("Scripts/3_Game/z_base.c", GAME, "class ProbeX\n{\n\tint m_A = 1;\n}\n"),
    ]);
    let result = check(&case);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// Two modded classes wrap in file-name order, each around the one before.
#[test]
fn two_modded_classes_wrap_in_file_order() {
    let case = input(&[
        ("Scripts/3_Game/a.c", GAME, "class ProbeY\n{\n}\nmodded class ProbeY\n{\n}\n"),
        ("Scripts/3_Game/b.c", GAME, "modded class ProbeY\n{\n}\n"),
    ]);
    let result = check(&case);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

// A modded class may sit before its base in file order, and a second one in a
// later file wraps the first. Bodies are not checked here, so the declarations
// report nothing.
#[test]
fn modded_order_probes_report_no_declaration_error() {
    let cases: [&[(&str, Layer, &str)]; 2] = [
        &[
            ("Scripts/3_Game/a_mod.c", GAME, "modded class ProbeX\n{\n\tint m_A = 2;\n}\n"),
            ("Scripts/3_Game/z_base.c", GAME, "class ProbeX\n{\n\tint m_A = 1;\n}\n"),
        ],
        &[
            (
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeX\n{\n\tint m_A = 1;\n}\n\nmodded class ProbeX\n{\n\tint m_A = 2;\n}\n",
            ),
            ("Scripts/3_Game/b.c", GAME, "modded class ProbeX\n{\n\tint m_A = 3;\n}\n"),
        ],
    ];
    for files in cases {
        let result = check(&input(files));
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }
}

// Two rows need a type the vanilla scripts declare: `Param` is a 1_Core class and
// `Man` is an engine class.
#[test]
#[ignore]
fn rows_that_need_vanilla_declarations() {
    if std::env::var("ENFORCE_CHECK_SLOW").as_deref() != Ok("1") {
        return;
    }
    let root = std::env::var("ENFORCE_CHECK_VANILLA").expect("ENFORCE_CHECK_VANILLA");
    let vanilla = load_tree(Path::new(&root), Layer::from_folder_name);
    assert!(!vanilla.is_empty(), "{root}");
    let cases = [
        ("class Param\n{\n}\n", "Multiple declaration of class 'Param'"),
        ("modded class Man\n{\n}\n", "Engine class 'Man' cannot be modded."),
    ];
    for (text, message) in cases {
        let mut case = input(&[]);
        case.mods = vec![ModInfo {
            name: "probe".into(),
            cfg_class: "Probe_decl".into(),
        }];
        case.files = vanilla.clone();
        case.files.push(SourceFile {
            path: "Scripts/3_Game/a.c".into(),
            layer: GAME,
            mod_index: 1,
            text: text.into(),
        });
        let result = check(&case);
        assert_eq!(
            result.diagnostics.first().map(|diagnostic| diagnostic.message.as_str()),
            Some(message)
        );
    }
}

// The index records each `modded` declaration and the plain name it replaced, in
// load order, for classes and enums alike.
#[test]
fn the_index_records_the_modded_chain() {
    let case = input(&[
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeZ\n{\n}\nmodded class ProbeZ\n{\n}\n\nenum EProbeZ\n{\n\tA\n}\n\nmodded enum EProbeZ\n{\n\tB\n}\n",
        ),
        ("Scripts/3_Game/b.c", GAME, "modded class ProbeZ\n{\n}\n"),
    ]);
    let parsed: Vec<(&SourceFile, enforce_check::parser::ParseOutput)> = case
        .files
        .iter()
        .map(|file| {
            let lexed = enforce_check::preproc::lex(file, &[]);
            (file, enforce_check::parser::parse(file, &lexed.tokens))
        })
        .collect();
    let files: Vec<(&SourceFile, &enforce_check::parser::FileAst)> =
        parsed.iter().map(|(file, output)| (*file, &output.ast)).collect();
    let index = enforce_check::index::build(&files, &enforce_check::index::Known::default(), false);
    let chains: Vec<(&str, &str, Option<&str>, u32)> = index
        .modded_chains
        .iter()
        .map(|chain| {
            (
                chain.name.as_str(),
                chain.file.as_str(),
                chain.wrapped.as_deref(),
                chain.line,
            )
        })
        .collect();
    assert_eq!(
        chains,
        vec![
            ("ProbeZ", "Scripts/3_Game/a.c", Some("ProbeZ"), 4),
            ("ProbeZ", "Scripts/3_Game/b.c", Some("ProbeZ"), 1),
            ("EProbeZ", "Scripts/3_Game/a.c", Some("EProbeZ"), 13),
        ]
    );
}

// The merge exception is same-module only, so a modded class redeclaring a member
// of an earlier layer's ancestor still reports.
#[test]
fn a_modded_class_redeclaring_a_core_member_reports() {
    let case = input(&[
        ("Scripts/1_Core/a.c", Layer::Core, "class ProbeBase\n{\n\tint m_X;\n}\n"),
        ("Scripts/3_Game/b.c", GAME, "class ProbeK : ProbeBase\n{\n}\n\nmodded class ProbeK\n{\n\tint m_X;\n}\n"),
    ]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Possible variable name conflict 'm_X'",
            "Multiple declaration of variable 'm_X'",
            "Can't create variable 'm_X'",
        ]
    );
}

// Any existing member name is a duplicate, but the warning is limited to an
// existing plain instance member.
#[test]
fn a_redeclared_member_is_a_duplicate_whatever_the_new_modifiers_are() {
    let cases: [(&str, Vec<&str>); 2] = [
        (
            "class ProbeN\n{\n\tstatic const int N = 1;\n}\n\nclass ProbeN2 : ProbeN\n{\n\tstatic const int N = 2;\n}\n",
            vec!["Multiple declaration of variable 'N'", "Can't create variable 'N'"],
        ),
        (
            "class ProbeS\n{\n\tstatic const int N;\n}\n\nclass ProbeS2 : ProbeS\n{\n\tint N;\n}\n",
            vec!["Multiple declaration of variable 'N'", "Can't create variable 'N'"],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<String> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .filter(|message| message.contains("'N'"))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// The game prints the circle once, however many classes share it.
#[test]
fn a_circle_is_reported_once_however_many_classes_share_it() {
    let case = input(&[(
        "Scripts/3_Game/a.c",
        GAME,
        "class ProbeA : ProbeB\n{\n}\n\nclass ProbeB : ProbeA\n{\n}\n\nclass ProbeC : ProbeA\n{\n}\n\nclass ProbeD : ProbeA\n{\n}\n",
    )]);
    let result = check(&case);
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "Circle inheritance in class 'ProbeA'")]
    );
}

// A diamond of static initialisers is a lattice of edges, not a tree to re-walk:
// every class below reaches both of the next level's, so a walk that does not
// mark them costs 2^22 visits. It must finish inside the suite's budget.
#[test]
fn a_deep_diamond_of_static_initialisers_is_checked_in_linear_time() {
    let mut text = String::new();
    for level in 0..22 {
        for branch in 0..2 {
            text.push_str(&format!("class ProbeL{level}_{branch}\n{{\n"));
            for below in 0..2 {
                text.push_str(&format!(
                    "\tstatic const int X{below} = ProbeL{}_{below}.X0;\n",
                    level + 1
                ));
            }
            text.push_str("}\n");
        }
    }
    for branch in 0..2 {
        text.push_str(&format!("class ProbeL22_{branch}\n{{\n\tstatic const int X0 = 1;\n}}\n"));
    }
    let started = std::time::Instant::now();
    let result = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)]));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

// A cycle deeper than the old cutoff is still reported.
#[test]
fn a_cycle_deeper_than_sixty_four_classes_is_reported() {
    let mut text = String::new();
    for level in 0..70 {
        text.push_str(&format!(
            "class ProbeD{level} : ProbeD{}\n{{\n}}\n",
            level + 1
        ));
    }
    text.push_str("class ProbeD70 : ProbeD0\n{\n}\n");
    let result = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)]));
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.line)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(result.diagnostics[0].message, "Circle inheritance in class 'ProbeD0'");
}

// A merge error is a parse error, so the later rows never run and none appears
// beside it.
#[test]
fn a_merge_error_stops_the_module_before_pass_one() {
    let cases: [(&str, Vec<&str>); 3] = [
        (
            "sealed class ProbeS\n{\n}\n\nmodded class ProbeS\n{\n}\n\nclass ProbeT : ProbeNope\n{\n}\n",
            vec!["Sealed type 'ProbeS' can't be modded"],
        ),
        (
            "modded class ProbeNoTarget\n{\n}\n\nclass ProbeT : ProbeNope\n{\n}\n",
            vec!["Unknown type 'ProbeNoTarget'"],
        ),
        (
            "class ProbeD\n{\n\tint x;\n}\n\nclass ProbeD\n{\n\tProbeNoSuch y;\n}\n",
            vec!["Multiple declaration of class 'ProbeD'"],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<String> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Rows that need their own shape: a pseudo-class body, a forward-only class used
// as a member type, `auto` on a static/const, and a parameter or global named
// like a type.
#[test]
fn the_remaining_declaration_rows_report_the_game_text() {
    let cases: [(&str, Vec<(&str, bool)>); 5] = [
        (
            "class int\n{\n\tint x;\n}\n",
            vec![("Variable 'x' in pseudo-class 'int' must be static/const", false)],
        ),
        (
            "class ProbeFwd2;\n\nclass ProbeFwdUse\n{\n\tProbeFwd2 f;\n}\n",
            vec![
                ("Forward declarations are deprecated!", false),
                ("Unknown type 'ProbeFwd2'", true),
            ],
        ),
        (
            "class ProbeAuto\n{\n\tstatic auto x = 1;\n}\n",
            vec![("static/const variables can't be auto", false)],
        ),
        (
            "class ProbeName\n{\n}\n\nclass ProbeNameUse\n{\n\tvoid F(int ProbeName)\n\t{\n\t}\n}\n",
            vec![("Variable name 'ProbeName' already used as type name", false)],
        ),
        (
            "class ProbeName2\n{\n}\n\nint ProbeName2;\n",
            vec![("Variable name 'ProbeName2' already used as type name", false)],
        ),
    ];
    let mut failures = Vec::new();
    for (text, expected) in cases {
        let got: Vec<(String, bool)> = check(&input(&[("Scripts/3_Game/a.c", GAME, text)]))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.message.clone(), diagnostic.fixme))
            .collect();
        let expected: Vec<(String, bool)> = expected
            .into_iter()
            .map(|(message, fixme)| (message.to_string(), fixme))
            .collect();
        if got != expected {
            failures.push(format!("{text}: got {got:?}, want {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// An enum chain longer than 64 enums is followed to its end: a member of the
// first enum is still an inherited name 70 levels down.
#[test]
fn an_enum_chain_longer_than_sixty_four_enums_is_followed() {
    let mut text = String::from("enum EProbeDeep0\n{\n\tA\n}\n");
    for level in 1..70 {
        text.push_str(&format!("enum EProbeDeep{level} : EProbeDeep{}\n{{\n\tM{level}\n}}\n", level - 1));
    }
    text.push_str("enum EProbeDeepUse : EProbeDeep69\n{\n\tA\n}\n");
    let result = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)]));
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Multiple declaration of variable 'A'"]
    );
}

// Two enums of one name in one module is the duplicate found while parsing, with
// the first declaration as the related line.
#[test]
fn two_enums_of_one_name_report_the_first_as_related() {
    let case = input(&[
        ("Scripts/3_Game/a.c", GAME, "enum ProbeE1\n{\n\tA\n}\n"),
        ("Scripts/3_Game/b.c", GAME, "enum ProbeE1\n{\n\tB\n}\n"),
    ]);
    let result = check(&case);
    let first = &result.diagnostics[0];
    assert_eq!(first.message, "Multiple declaration of enum 'ProbeE1'");
    assert_eq!(first.file, "Scripts/3_Game/b.c");
    assert_eq!(first.related.as_ref().map(|(file, line)| (file.as_str(), *line)),
        Some(("Scripts/3_Game/a.c", 1)));
}

// An enum matching an earlier module's enum prints the class row.
#[test]
fn an_enum_on_an_earlier_enum_name_reports_the_class_row() {
    let case = input(&[
        ("Scripts/3_Game/a.c", GAME, "enum ProbeClash\n{\n\tA\n}\n"),
        ("Scripts/4_World/b.c", WORLD, "enum ProbeClash\n{\n\tB\n}\n"),
    ]);
    let result = check(&case);
    let first = &result.diagnostics[0];
    assert_eq!(first.message, "Multiple declaration of class 'ProbeClash'");
    assert_eq!(first.file, "Scripts/4_World/b.c");
}

// An enum on the name of an earlier non-enum type is the soft row.
#[test]
fn an_enum_on_an_earlier_class_name_is_the_soft_row() {
    for earlier in ["class ProbeClash\n{\n}\n", "typedef int ProbeClash;\n"] {
        let case = input(&[
            ("Scripts/3_Game/a.c", GAME, earlier),
            ("Scripts/4_World/b.c", WORLD, "enum ProbeClash\n{\n\tA\n}\n"),
        ]);
        let result = check(&case);
        let first = &result.diagnostics[0];
        assert_eq!(first.message, "Multiple declaration of 'ProbeClash'", "{earlier}");
        assert!(first.fixme, "{earlier}");
    }
}

// The index runs on whatever the parser produced, including hostile text, and
// must never panic.
#[test]
fn any_declarations_are_indexed_without_panicking() {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "ab01_{}[]()<>;:,.=+-*/%!&|^~?#\"'\n class void enum typedef modded \
                              sealed int ref const static private protected struct extends"
        .chars()
        .collect();
    for _ in 0..300 {
        let len = (next() % 200) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let _ = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)]));
    }

    // The same text spread over several layers and files, with `modded` names
    // that repeat across them, so the merge and the layer gate see it too.
    for _ in 0..200 {
        let mut files: Vec<(String, Layer, String)> = Vec::new();
        for layer in [GAME, WORLD, Layer::Mission] {
            for file in 0..2 {
                let len = (next() % 120) as usize;
                let text: String = (0..len)
                    .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                    .collect();
                files.push((format!("Scripts/{layer:?}/{file}.c"), layer, text));
            }
        }
        let files: Vec<(&str, Layer, &str)> = files
            .iter()
            .map(|(path, layer, text)| (path.as_str(), *layer, text.as_str()))
            .collect();
        let _ = check(&input(&files));
    }

    // Every short sequence of the words that open and close declarations.
    let words = ["class", "enum", "typedef", "modded", "sealed", "A", "B", "int", ":", "{", "}", ";", "=", "X"];
    let mut texts = vec![String::new()];
    for _ in 0..3 {
        texts = texts
            .iter()
            .flat_map(|prefix| words.iter().map(move |word| format!("{prefix} {word}")))
            .collect();
        for text in &texts {
            for shape in [
                text.clone(),
                format!("class A {{ {text} }}"),
                format!("enum E {{ {text} }}"),
                format!("typedef {text}"),
                format!("class A {{ }} modded class A {{ {text} }}"),
            ] {
                for files in [
                    vec![("Scripts/3_Game/a.c", GAME, shape.as_str())],
                    vec![
                        ("Scripts/3_Game/a.c", GAME, shape.as_str()),
                        ("Scripts/4_World/b.c", WORLD, shape.as_str()),
                    ],
                ] {
                    let _ = check(&input(&files));
                }
            }
        }
    }
}

// ord-003: the game names a wrapped class after the wrapping declaration: the
// place of its file among the module's files, counted from 0, and its line.
#[test]
fn a_wrapped_class_is_named_after_the_file_and_line_of_its_wrapper() {
    let files = [
        ("Scripts/3_Game/a.c", GAME, "class ProbeY\n{\n}\n"),
        (
            "Scripts/3_Game/b.c",
            GAME,
            "class ProbeX\n{\n}\n\nmodded class ProbeX\n{\n\tvoid F()\n\t{\n\t\tNopeA();\n\t}\n}\n",
        ),
        ("Scripts/3_Game/c.c", GAME, "\nmodded class ProbeX\n{\n}\n"),
    ];
    let got: Vec<(u32, String)> = check(&input(&files))
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
        .collect();
    assert_eq!(got, vec![(9, "Undefined function 'ProbeX#2#2.NopeA'".to_string())]);
}
