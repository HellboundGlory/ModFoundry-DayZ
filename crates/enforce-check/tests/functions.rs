use enforce_check::{CheckInput, Layer, ModInfo, SourceFile, Severity, Target, check};

// One row per `decl-` probe for the function rules: the source, every line the
// game printed on the retail server, and the same on the Diag server, where a
// soft rule fails the module instead of warning.
struct Row {
    id: &'static str,
    files: &'static [(&'static str, Layer, &'static str)],
    retail: &'static [(u32, bool, &'static str)],
    diag: &'static [(u32, &'static str)],
}

const GAME: Layer = Layer::Game;
const WORLD: Layer = Layer::World;

fn input(files: &[(&str, Layer, &str)], target: Target) -> CheckInput {
    CheckInput {
        target,
        mods: vec![ModInfo {
            name: "probe".into(),
            cfg_class: "Probe_functions".into(),
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

fn rows() -> Vec<Row> {
    vec![
        Row {
            id: "decl-051",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO1\n{\n\tvoid F()\n\t{\n\t}\n}\n\nclass ProbeO2 : ProbeO1\n{\n\tvoid F()\n\t{\n\t}\n}\n",
            )],
            retail: &[(10, true, "Overriding function 'F' but not marked as 'override'")],
            diag: &[(10, "Overriding function 'F' but not marked as 'override'")],
        },
        Row {
            id: "decl-052",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO3\n{\n\tvoid F(int a)\n\t{\n\t}\n}\n\nclass ProbeO4 : ProbeO3\n{\n\toverride void F(float a)\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                false,
                "Function 'F' is marked as override, but there is no function with this name in the base class",
            )],
            diag: &[(
                10,
                "Function 'F' is marked as override, but there is no function with this name in the base class",
            )],
        },
        Row {
            id: "decl-053",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO5\n{\n\tvoid F(int a)\n\t{\n\t}\n}\n\nclass ProbeO6 : ProbeO5\n{\n\toverride void F(int b)\n\t{\n\t}\n}\n",
            )],
            retail: &[(10, true, "Overloaded method 'F', argument name 'b' differs from prototype 'a'")],
            diag: &[(10, "Overloaded method 'F', argument name 'b' differs from prototype 'a'")],
        },
        Row {
            id: "decl-054",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO7\n{\n\tint F()\n\t{\n\t\treturn 0;\n\t}\n}\n\nclass ProbeO8 : ProbeO7\n{\n\toverride string F()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
            )],
            retail: &[(11, false, "Overloaded function 'F' not compatible")],
            diag: &[(11, "Overloaded function 'F' not compatible")],
        },
        Row {
            id: "decl-055",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO9\n{\n\tvoid F(int a)\n\t{\n\t}\n}\n\nclass ProbeO10 : ProbeO9\n{\n\toverride void F(int a, int b = 0)\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                false,
                "Function 'F' is marked as override, but there is no function with this name in the base class",
            )],
            diag: &[(
                10,
                "Function 'F' is marked as override, but there is no function with this name in the base class",
            )],
        },
        Row {
            id: "decl-056",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO11\n{\n\tvoid F()\n\t{\n\t}\n}\n\nclass ProbeO12 : ProbeO11\n{\n\toverride static void F()\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                true,
                "Function 'F' override signature conflict: differing 'static' usage",
            )],
            diag: &[(10, "Function 'F' override signature conflict: differing 'static' usage")],
        },
        Row {
            id: "decl-057",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeO13\n{\n\tsealed void F()\n\t{\n\t}\n}\n\nclass ProbeO14 : ProbeO13\n{\n\toverride void F()\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                false,
                "'ProbeO14.F': cannot override inherited member 'F' because it is sealed",
            )],
            diag: &[(
                10,
                "'ProbeO14.F': cannot override inherited member 'F' because it is sealed",
            )],
        },
        Row {
            id: "decl-058",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeE\n{\n\tevent void F(int a)\n\t{\n\t}\n\tvoid F(float a)\n\t{\n\t}\n}\n",
            )],
            retail: &[(6, true, "Overloading event 'F' is not allowed")],
            diag: &[(6, "Overloading event 'F' is not allowed")],
        },
        Row {
            id: "decl-071",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeDt\n{\n\tvoid ~ProbeOther()\n\t{\n\t}\n}\n",
            )],
            retail: &[(3, false, "Invalid destructor name '~ProbeOther'")],
            diag: &[(3, "Invalid destructor name '~ProbeOther'")],
        },
        Row {
            id: "decl-059",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeD\n{\n\tvoid F(int a)\n\t{\n\t}\n\tvoid F(int b)\n\t{\n\t}\n}\n",
            )],
            retail: &[(6, false, "Multiple declaration of function 'F'")],
            diag: &[(6, "Multiple declaration of function 'F'")],
        },
        Row {
            id: "decl-060",
            files: &[
                ("Scripts/3_Game/a.c", GAME, "void ProbeGlobal(int a)\n{\n}\n"),
                ("Scripts/4_World/b.c", WORLD, "void ProbeGlobal(int a)\n{\n}\n"),
            ],
            retail: &[(1, true, "Multiple declaration of function 'ProbeGlobal'")],
            diag: &[(1, "Multiple declaration of function 'ProbeGlobal'")],
        },
        Row {
            id: "decl-061",
            files: &[
                ("Scripts/3_Game/a.c", GAME, "void ProbeGlobal2(int a)\n{\n}\n"),
                ("Scripts/3_Game/b.c", GAME, "void ProbeGlobal2(int a)\n{\n}\n"),
            ],
            retail: &[(1, false, "Multiple declaration of function 'ProbeGlobal2'")],
            diag: &[(1, "Multiple declaration of function 'ProbeGlobal2'")],
        },
        Row {
            id: "decl-062",
            files: &[("Scripts/3_Game/a.c", GAME, "private void ProbeGlobalPrivate()\n{\n}\n")],
            retail: &[(1, false, "Function 'ProbeGlobalPrivate' cannot be private/protected")],
            diag: &[(1, "Function 'ProbeGlobalPrivate' cannot be private/protected")],
        },
        Row {
            id: "decl-063",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeMax17\n{\n\tvoid F(int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8, int p9, int p10, int p11, int p12, int p13, int p14, int p15, int p16, int p17)\n\t{\n\t}\n}\n",
            )],
            retail: &[(3, false, "Maximum arguments count 16 exceeded in method 'F'")],
            diag: &[(3, "Maximum arguments count 16 exceeded in method 'F'")],
        },
        Row {
            id: "decl-064",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeX\n{\n\texternal void F(int p1, int p2, int p3, int p4, int p5, int p6, int p7, int p8, int p9, int p10, int p11, int p12, int p13, int p14, int p15, int p16)\n\t{\n\t}\n}\n",
            )],
            retail: &[(3, false, "Maximum arguments count 16 exceeded in method 'F'")],
            diag: &[(3, "Maximum arguments count 16 exceeded in method 'F'")],
        },
        Row {
            id: "decl-065",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeQ\n{\n\tvoid F(int a = 1, int b)\n\t{\n\t}\n}\n",
            )],
            retail: &[(3, false, "Misplaced non-default parameter b")],
            diag: &[(3, "Misplaced non-default parameter b")],
        },
        Row {
            id: "decl-066",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeC1\n{\n\tvoid ProbeC1(int x)\n\t{\n\t}\n}\n\nclass ProbeC2 : ProbeC1\n{\n\tvoid ProbeC2(string x)\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                false,
                "Constructor 'ProbeC2', argument 'x' is not compatible with prototype argument type or spec 'int'",
            )],
            diag: &[(
                10,
                "Constructor 'ProbeC2', argument 'x' is not compatible with prototype argument type or spec 'int'",
            )],
        },
        Row {
            id: "decl-067",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeC3\n{\n\tvoid ProbeC3(int x)\n\t{\n\t}\n}\n\nclass ProbeC4 : ProbeC3\n{\n\tvoid ProbeC4(int y)\n\t{\n\t}\n}\n",
            )],
            retail: &[(
                10,
                true,
                "Constructor 'ProbeC4', argument 'y' name differs from prototype 'x'",
            )],
            diag: &[(10, "Constructor 'ProbeC4', argument 'y' name differs from prototype 'x'")],
        },
        Row {
            id: "decl-068",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeC5\n{\n\tvoid ProbeC5(int x)\n\t{\n\t}\n}\n\nclass ProbeC6 : ProbeC5\n{\n\tvoid ProbeC6()\n\t{\n\t}\n}\n",
            )],
            retail: &[(10, false, "Overloaded function 'ProbeC6' not compatible")],
            diag: &[(10, "Overloaded function 'ProbeC6' not compatible")],
        },
        Row {
            id: "decl-069",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeC7\n{\n\tvoid ProbeC7(int x)\n\t{\n\t}\n}\n\nmodded class ProbeC7\n{\n\tvoid ProbeC7()\n\t{\n\t}\n}\n",
            )],
            retail: &[(10, false, "Overloaded function 'ProbeC7' not compatible")],
            diag: &[(10, "Overloaded function 'ProbeC7' not compatible")],
        },
        Row {
            id: "decl-070",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeC8\n{\n\tvoid ProbeC8()\n\t{\n\t}\n\tvoid ProbeC8(int a)\n\t{\n\t}\n}\n",
            )],
            retail: &[],
            diag: &[],
        },
        Row {
            id: "decl-072",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeW\n{\n\tvoid F(int a, int b)\n\t{\n\t}\n\tvoid F(int a, int b, int c = 5)\n\t{\n\t}\n}\n",
            )],
            retail: &[],
            diag: &[],
        },
        Row {
            id: "decl-073",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeT<Class T>\n{\n}\n\nclass ProbeU\n{\n\tProbeT<int,int> m_X;\n}\n",
            )],
            retail: &[
                (7, true, "Too many template parameters for 'ProbeT' type"),
                (7, true, "Too many template parameters for 'ProbeT' type"),
            ],
            diag: &[
                (7, "Too many template parameters for 'ProbeT' type"),
                (7, "Too many template parameters for 'ProbeT' type"),
            ],
        },
        Row {
            id: "decl-074",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeT2<Class T, Class U>\n{\n}\n\nclass ProbeU2\n{\n\tProbeT2<int> m_X;\n}\n",
            )],
            retail: &[
                (7, true, "Not enough parameters for template class 'ProbeT2'"),
                (7, false, "Wrong number of template parameters"),
            ],
            diag: &[
                (7, "Not enough parameters for template class 'ProbeT2'"),
                (7, "Wrong number of template parameters"),
            ],
        },
        Row {
            id: "decl-078",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeN\n{\n\tproto native void F(out int x);\n}\n",
            )],
            retail: &[(3, false, "Native functions don't support 'out' arguments 'F'")],
            diag: &[(3, "Native functions don't support 'out' arguments 'F'")],
        },
        Row {
            id: "decl-080",
            files: &[(
                "Scripts/3_Game/a.c",
                GAME,
                "class ProbeNative\n{\n\tproto native void F()\n\t{\n\t}\n}\n",
            )],
            retail: &[(3, false, "Cannot find function '(null)'")],
            diag: &[(3, "Cannot find function '(null)'")],
        },
    ]
}

// A destructor is its own record, so a class may declare `~Object` beside its
// constructor `Object()` without the two looking like a duplicate.
#[test]
fn a_destructor_is_no_twin_of_the_constructor() {
    let text = "class ProbePair\n{\n\tprivate void ~ProbePair();\n\tprivate void ProbePair();\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A destructor's name is the plain class name, so without treating it as its own
// record it would be read as a parameterless constructor beside the base's.
#[test]
fn a_destructor_beside_a_base_constructor_is_no_constructor() {
    let text = "class ProbeBase\n{\n\tvoid ProbeBase(int x)\n\t{\n\t}\n}\n\nclass ProbeKid : ProbeBase\n{\n\tvoid ProbeKid(int x)\n\t{\n\t}\n\tvoid ~ProbeKid()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A class wrapped by `modded` keeps its destructor under the plain name, so the
// rename the merge applies does not turn it into an invalid destructor.
#[test]
fn a_modded_class_keeps_its_destructor_name() {
    let text = "class ProbeWrapped\n{\n\tvoid ~ProbeWrapped()\n\t{\n\t}\n}\n\nmodded class ProbeWrapped\n{\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// Only the first function named like the class is its constructor; a later one
// with other parameters is an ordinary overload and is not compared with the
// base constructor.
#[test]
fn a_derived_second_constructor_is_a_plain_overload() {
    let text = "class ProbeCtorA\n{\n\tvoid ProbeCtorA(int x)\n\t{\n\t}\n}\n\nclass ProbeCtorB : ProbeCtorA\n{\n\tvoid ProbeCtorB(int x)\n\t{\n\t}\n\tvoid ProbeCtorB(string s)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// The same for a `modded` class, whose first constructor is compared with the
// renamed original's.
#[test]
fn a_modded_second_constructor_is_a_plain_overload() {
    let text = "class ProbeCtorC\n{\n\tvoid ProbeCtorC(int x)\n\t{\n\t}\n}\n\nmodded class ProbeCtorC\n{\n\tvoid ProbeCtorC(int x)\n\t{\n\t}\n\tvoid ProbeCtorC(float f, float g)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// 1_Core's `Class` is the root every script class derives from, so a class with
// no written base still overrides `Class`'s methods.
#[test]
fn a_class_without_a_written_base_derives_from_the_root_class() {
    let files = [
        (
            "Scripts/1_Core/root.c",
            Layer::Core,
            "class Class\n{\n\tstring GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeArg\n{\n\toverride string GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base declared with template arguments hands those arguments to the derived
// class's comparison: `T` stands for the argument the base was instantiated with.
#[test]
fn template_arguments_reach_the_overridden_signature() {
    let text = "class ProbeTBase<Class T>\n{\n\tvoid Set(T v)\n\t{\n\t}\n\tT Get()\n\t{\n\t\treturn null;\n\t}\n}\n\nclass ProbeTInt : ProbeTBase<int>\n{\n\toverride void Set(int v)\n\t{\n\t}\n\toverride int Get()\n\t{\n\t\treturn 0;\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// Two parameters that differ only in their template arguments are different
// types, so the second function is an overload, not a duplicate.
#[test]
fn template_arguments_keep_parameters_distinct() {
    let text = "class array<Class T>\n{\n}\n\nclass ProbeTArgs\n{\n\tvoid F(array<int> a)\n\t{\n\t}\n\tvoid F(array<string> a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// An override whose parameter differs only in its template arguments does not
// match the base function.
#[test]
fn an_override_with_other_template_arguments_does_not_match() {
    let text = "class array<Class T>\n{\n}\n\nclass ProbeTOver\n{\n\tvoid F(array<int> a)\n\t{\n\t}\n}\n\nclass ProbeTOverKid : ProbeTOver\n{\n\toverride void F(array<float> a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Function 'F' is marked as override, but there is no function with this name in the base class"]
    );
}

// A typedef is compared as the type it stands for: an override written with the
// alias matches the base's target type.
#[test]
fn a_typedef_parameter_matches_its_target_type() {
    let text = "typedef float Meters;\n\nclass ProbeM\n{\n\tvoid H(Meters m)\n\t{\n\t}\n}\n\nclass ProbeMKid : ProbeM\n{\n\toverride void H(float m)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// The same for a typedef of a template instantiation.
#[test]
fn a_typedef_of_a_template_matches_its_target_type() {
    let text = "class array<Class T>\n{\n}\n\ntypedef array<string> TS;\n\nclass ProbeTS\n{\n\tvoid H(TS a)\n\t{\n\t}\n}\n\nclass ProbeTSKid : ProbeTS\n{\n\toverride void H(array<string> a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A chain of typedefs is followed to the last target.
#[test]
fn a_typedef_chain_resolves_to_its_last_target() {
    let text = "typedef int A1;\ntypedef A1 A2;\n\nclass ProbeChainT\n{\n\tvoid H(A2 a)\n\t{\n\t}\n}\n\nclass ProbeChainTKid : ProbeChainT\n{\n\toverride void H(int a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A typedef from an earlier module resolves the same way.
#[test]
fn a_typedef_from_an_earlier_module_resolves() {
    let files = [
        ("Scripts/3_Game/a.c", GAME, "typedef float Meters;\n"),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeEarly2\n{\n\tvoid H(Meters m)\n\t{\n\t}\n}\n\nclass ProbeEarly2Kid : ProbeEarly2\n{\n\toverride void H(float m)\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A return type written as an alias is the same category as its target.
#[test]
fn a_typedef_return_type_matches_its_target() {
    let text = "typedef float Meters;\n\nclass ProbeRet\n{\n\tMeters R()\n\t{\n\t\treturn 0;\n\t}\n}\n\nclass ProbeRetKid : ProbeRet\n{\n\toverride float R()\n\t{\n\t\treturn 0;\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// Two declarations that differ only by alias-versus-target are one signature.
#[test]
fn an_alias_and_its_target_are_one_signature() {
    let text = "typedef float Meters;\n\nclass ProbeSig\n{\n\tvoid F(Meters a)\n\t{\n\t}\n\tvoid F(float b)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Multiple declaration of function 'F'"]
    );
}

// An enum and an `int` are the same type to the game.
#[test]
fn an_enum_and_an_int_are_one_signature() {
    let text = "enum ProbeColour\n{\n\tRED\n}\n\nclass ProbeEnumSig\n{\n\tvoid F(ProbeColour c)\n\t{\n\t}\n\tvoid F(int i)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Multiple declaration of function 'F'"]
    );
}

// A base declared in an earlier module is still an ancestor: the class's
// override marks and constructor are checked against its records.
#[test]
fn a_base_in_an_earlier_module_is_still_an_ancestor() {
    let files = [
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeEarly\n{\n\tvoid ProbeEarly(int x)\n\t{\n\t}\n\tvoid F()\n\t{\n\t}\n}\n",
        ),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeLate : ProbeEarly\n{\n\tvoid ProbeLate(int x)\n\t{\n\t}\n\toverride void F()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A template chain that starts in an earlier module keeps the arguments each
// class wrote for its base, so `T` is bound through two levels and an alias.
#[test]
fn a_two_level_template_chain_from_an_earlier_module_binds_its_parameters() {
    let early = "class ProbeA<Class T>\n{\n\tvoid Set(T v)\n\t{\n\t}\n\tT Get()\n\t{\n\t\treturn null;\n\t}\n}\nclass ProbeB<Class U> : ProbeA<U>\n{\n}\ntypedef ProbeB<string> ProbeBS;\n";
    for late in [
        "class ProbeC : ProbeB<int>\n{\n\toverride void Set(int v)\n\t{\n\t}\n\toverride int Get()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        "class ProbeD : ProbeBS\n{\n\toverride void Set(string v)\n\t{\n\t}\n}\n",
    ] {
        let files = [("Scripts/3_Game/a.c", GAME, early), ("Scripts/4_World/b.c", WORLD, late)];
        let got = check(&input(&files, Target::RetailServer));
        assert!(got.diagnostics.is_empty(), "{late}: {:?}", got.diagnostics);
    }
}

// Without the `override` the same cross-module base reports the missing mark,
// which proves the earlier module's function table was read.
#[test]
fn a_cross_module_override_without_the_mark_is_reported() {
    let files = [
        ("Scripts/3_Game/a.c", GAME, "class ProbeEarly2\n{\n\tvoid F()\n\t{\n\t}\n}\n"),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeLate2 : ProbeEarly2\n{\n\tvoid F()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.fixme, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(3, true, "Overriding function 'F' but not marked as 'override'")]
    );
}

// An override whose parameter differs in `out` reports the specifier, which the
// exact-type search let through.
#[test]
fn an_override_with_a_different_specifier_reports_it() {
    let text = "class ProbeSpec\n{\n\tvoid F(out int a)\n\t{\n\t}\n}\n\nclass ProbeSpecKid : ProbeSpec\n{\n\toverride void F(int a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Overloaded function 'F', argument 'a' is not compatible with prototype argument type or spec 'out int'"]
    );
}

// Two global `proto` declarations of one name are a duplicate: neither waits for
// an engine record.
#[test]
fn two_global_prototypes_of_one_name_collide() {
    let text = "proto void ProbeProto();\nproto void ProbeProto();\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Multiple declaration of function 'ProbeProto'"]
    );
}

// A second function named like the class is an overload, never a second
// constructor; only an identical signature collides, as a plain duplicate.
#[test]
fn a_second_constructor_is_an_overload() {
    let text = "class ProbeTwoCtor\n{\n\tvoid ProbeTwoCtor()\n\t{\n\t}\n\tvoid ProbeTwoCtor(int a)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);

    let same = "class ProbeSameCtor\n{\n\tvoid ProbeSameCtor()\n\t{\n\t}\n\tvoid ProbeSameCtor()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, same)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Multiple declaration of function 'ProbeSameCtor'"]
    );
}

// Pass 2 runs only when pass 1 reported no hard error, so a module whose
// declarations already failed never reaches the `proto` body check.
#[test]
fn pass_two_runs_only_when_pass_one_is_clean() {
    let text = "class ProbeGated\n{\n\tvoid F(int a = 1, int b)\n\t{\n\t}\n\tproto native void G()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Misplaced non-default parameter b"]
    );
}

// The count limit is exactly 16 for a plain function: the sixteenth parameter is
// accepted and the seventeenth is not.
#[test]
fn the_parameter_limit_is_sixteen() {
    let sixteen: String = (1..=16).map(|n| format!("int p{n}")).collect::<Vec<_>>().join(", ");
    let seventeen: String = (1..=17).map(|n| format!("int p{n}")).collect::<Vec<_>>().join(", ");
    for (list, expected) in [
        (&sixteen, 0usize),
        (&seventeen, 1),
    ] {
        let text = format!("class ProbeCount\n{{\n\tvoid F({list})\n\t{{\n\t}}\n}}\n");
        let got = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)], Target::RetailServer));
        assert_eq!(got.diagnostics.len(), expected, "{list}");
    }
}

// The destructor rows print as errors but the game loads every module anyway.
#[test]
fn a_destructor_error_does_not_fail_the_module() {
    let text = "class ProbeDT\n{\n\tvoid ~ProbeOther()\n\t{\n\t}\n}\n";
    for target in [Target::RetailServer, Target::DiagServer] {
        let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], target));
        assert_eq!(got.diagnostics.len(), 1, "{target:?}");
        assert!(got.failed_modules.is_empty(), "{target:?}: {:?}", got.failed_modules);
    }
}

// Extra template arguments are dropped, so the Diag server prints the two
// lines as errors and loads the module all the same (decl-073). Unobserved on
// Diag: any place but a member.
#[test]
fn extra_template_arguments_do_not_fail_the_module() {
    let text = "class ProbeT<Class T>\n{\n}\n\nclass ProbeU\n{\n\tProbeT<int,int> m_X;\n\tvoid F()\n\t{\n\t\tProbeT<int,int> x;\n\t}\n}\n";
    for (target, severity) in [(Target::RetailServer, Severity::Warning), (Target::DiagServer, Severity::Error)] {
        let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], target));
        let lines: Vec<(u32, Severity)> = got.diagnostics.iter().map(|d| (d.line, d.severity)).collect();
        assert_eq!(lines, [(7, severity), (7, severity), (10, severity), (10, severity)], "{target:?}");
        assert!(got.failed_modules.is_empty(), "{target:?}: {:?}", got.failed_modules);
    }
}

// A template declared in an earlier module still has its parameter count when a
// later module writes a use of it.
#[test]
fn a_template_declared_in_an_earlier_module_keeps_its_count() {
    let files = [
        ("Scripts/3_Game/a.c", GAME, "class ProbeSpan<Class T>\n{\n}\n"),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeSpanUse\n{\n\tProbeSpan<int,int> m_X;\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Too many template parameters for 'ProbeSpan' type",
            "Too many template parameters for 'ProbeSpan' type",
        ]
    );
}

// A class wrapped by `modded` in a later module keeps the functions of the class
// it wrapped, so a subclass declared after it still overrides them.
#[test]
fn a_modded_chain_in_an_earlier_module_keeps_its_functions() {
    let files = [
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeChain\n{\n\tvoid F()\n\t{\n\t}\n}\n\nmodded class ProbeChain\n{\n}\n",
        ),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeChainKid : ProbeChain\n{\n\toverride void F()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// `bool` and `int` share a category: vanilla overrides `bool IsEat()` with an
// `int` one and compiles.
#[test]
fn bool_and_int_share_a_return_category() {
    let text = "class ProbeCat\n{\n\tbool F()\n\t{\n\t\treturn false;\n\t}\n}\n\nclass ProbeCatKid : ProbeCat\n{\n\toverride int F()\n\t{\n\t\treturn 0;\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A constructor argument whose type derives from the base's is a soft error; an
// unrelated one is a hard error.
#[test]
fn a_derived_constructor_argument_is_a_soft_error() {
    let base = "class ProbeArgBase\n{\n\tvoid ProbeArgBase(ProbeArgParent p)\n\t{\n\t}\n}\n\nclass ProbeArgParent\n{\n}\n\nclass ProbeArgChild : ProbeArgParent\n{\n}\n\nclass ProbeArgUse : ProbeArgBase\n{\n\tvoid ProbeArgUse(ProbeArgChild p)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, base)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.fixme, diagnostic.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            true,
            "Constructor 'ProbeArgUse', argument 'p' is not compatible with prototype argument type or spec 'ProbeArgParent'"
        )]
    );
}

// The function rules run on whatever the parser produced, including hostile
// text, and must never panic.
#[test]
fn any_function_declarations_are_checked_without_panicking() {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = "ab01_{}[]()<>,;:=+-~#\"'\n class void proto native override external \
                               static sealed event ref out inout notnull destructor"
        .chars()
        .collect();
    for _ in 0..400 {
        let len = (next() % 240) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let _ = check(&input(&[("Scripts/3_Game/a.c", GAME, &text)], Target::RetailServer));
    }

    // Whole short sequences of the words the rules branch on, in a class body
    // and at file level, so `modded` chains and template bases are reached.
    let words = [
        "class", "void", "proto", "native", "override", "external", "static", "sealed", "event",
        "A", "B", "int", "float", "T", "~A", "<", ">", "(", ")", "{", "}", ";", ",", ":", "=", "1",
        "typedef", "modded", "array",
    ];
    let mut texts = vec![String::new()];
    for _ in 0..2 {
        texts = texts
            .iter()
            .flat_map(|prefix| words.iter().map(move |word| format!("{prefix} {word}")))
            .collect();
        for text in &texts {
            for shape in [
                format!("class A {{ {text} }}"),
                format!("class A {{ }} modded class A {{ {text} }}"),
                format!("class Base<Class T> {{ {text} }}\nclass A : Base<int> {{ {text} }}"),
                text.clone(),
            ] {
                let _ = check(&input(&[("Scripts/3_Game/a.c", GAME, &shape)], Target::RetailServer));
            }
        }
    }
}

// A body on a built-in's name extends that pseudo-class; it does not derive from
// the root `Class`, so the root's methods are not overridden there.
#[test]
fn a_pseudo_class_does_not_inherit_the_root_class() {
    let files = [
        (
            "Scripts/1_Core/root.c",
            Layer::Core,
            "class Class\n{\n\tproto external string ToString();\n}\n",
        ),
        (
            "Scripts/1_Core/convert.c",
            Layer::Core,
            "class bool\n{\n\tstring ToString()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base in an earlier module that has a base of its own is followed all the way
// to the root, so a root method is still found.
#[test]
fn a_chain_through_an_earlier_module_reaches_the_root() {
    let files = [
        (
            "Scripts/1_Core/root.c",
            Layer::Core,
            "class Class\n{\n\tstring GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
        ("Scripts/1_Core/managed.c", Layer::Core, "class Managed\n{\n}\n"),
        ("Scripts/3_Game/effect.c", GAME, "class ProbeEffect : Managed\n{\n}\n"),
        (
            "Scripts/4_World/use.c",
            WORLD,
            "class ProbeEffectKid : ProbeEffect\n{\n\toverride string GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A chain that passes through several classes of an earlier module is followed
// to the class that declares the method, however many levels that takes.
#[test]
fn a_deep_chain_through_an_earlier_module_finds_the_method() {
    let files = [
        (
            "Scripts/3_Game/a.c",
            GAME,
            "class ProbeRoot\n{\n\tvoid Deep()\n\t{\n\t}\n}\n\nclass ProbeMid : ProbeRoot\n{\n}\n\nclass ProbeTop : ProbeMid\n{\n}\n",
        ),
        (
            "Scripts/4_World/b.c",
            WORLD,
            "class ProbeLeaf : ProbeTop\n{\n\toverride void Deep()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// The same when several of the chain's classes sit in the later module and the
// method is declared further back still.
#[test]
fn a_chain_that_starts_in_this_module_reaches_an_earlier_method() {
    let files = [
        (
            "Scripts/3_Game/game.c",
            GAME,
            "class ProbeEntity\n{\n\tvoid OnSpawn()\n\t{\n\t}\n}\n\nclass ProbeBuilding : ProbeEntity\n{\n}\n",
        ),
        (
            "Scripts/4_World/world.c",
            WORLD,
            "class ProbeBase : ProbeBuilding\n{\n}\n\nclass ProbeHouse : ProbeBase\n{\n}\n\nclass ProbeArea : ProbeHouse\n{\n\toverride void OnSpawn()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base written as a typedef names the class the alias stands for, and the
// chain is followed through it.
#[test]
fn a_base_written_as_a_typedef_is_followed() {
    let files = [
        (
            "Scripts/3_Game/game.c",
            GAME,
            "class ProbeDeep\n{\n\tvoid OnSpawn()\n\t{\n\t}\n}\n\ntypedef ProbeDeep ProbeAlias;\n",
        ),
        (
            "Scripts/4_World/world.c",
            WORLD,
            "class ProbeBelow : ProbeAlias\n{\n\toverride void OnSpawn()\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base's template parameters are substituted wherever they appear, including
// inside another template's arguments: `FSMTransition<T1,...>` compared with
// `FSMTransition<WeaponStateBase,...>`.
#[test]
fn template_parameters_inside_another_template_are_substituted() {
    let text = "class ProbeSpan<Class A, Class B>\n{\n}\n\nclass ProbeFsm<Class S, Class E>\n{\n\tvoid Add(ProbeSpan<S, E> t)\n\t{\n\t}\n}\n\nclass ProbeState\n{\n}\n\nclass ProbeEvent\n{\n}\n\nclass ProbeWeapon : ProbeFsm<ProbeState, ProbeEvent>\n{\n\toverride void Add(ProbeSpan<ProbeState, ProbeEvent> t)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A class whose ancestor in this module wrote no base still derives from the
// root, so the root's methods are overridable through it.
#[test]
fn an_ancestor_with_no_written_base_still_derives_from_the_root() {
    let files = [
        (
            "Scripts/1_Core/root.c",
            Layer::Core,
            "class Class\n{\n\tstring GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
        (
            "Scripts/1_Core/managed.c",
            Layer::Core,
            "class Managed\n{\n}\n",
        ),
        (
            "Scripts/1_Core/cf.c",
            Layer::Core,
            "class ProbeFile : Managed\n{\n\toverride string GetDebugName()\n\t{\n\t\treturn \"\";\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base written as an alias of a template instantiation still hands the
// template its arguments, so the inherited signature is substituted.
#[test]
fn a_template_base_written_as_a_typedef_keeps_its_arguments() {
    let text = "class Base<Class T>\n{\n\tvoid Set(T v)\n\t{\n\t}\n}\n\ntypedef Base<int> BI;\n\nclass D : BI\n{\n\toverride void Set(int v)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A constructor compared against a base reached through such an alias.
#[test]
fn a_constructor_through_a_typedef_base_matches() {
    let text = "class Base<Class T>\n{\n\tvoid Base(T init)\n\t{\n\t}\n}\n\ntypedef Base<int> BI;\n\nclass D : BI\n{\n\tvoid D(int init)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A base that passes its own parameter straight on to its ancestor: the
// substitution uses the ancestor's parameter names, not the derived class's.
#[test]
fn a_parameter_passed_through_two_templates_is_substituted() {
    let text = "class A<Class T>\n{\n\tT Get()\n\t{\n\t\treturn null;\n\t}\n}\n\nclass B<Class U> : A<U>\n{\n}\n\nclass C : B<int>\n{\n\toverride int Get()\n\t{\n\t\treturn 0;\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// An argument that is itself a template with its closers next to the outer
// base's: only the outermost closer belongs to the base.
#[test]
fn a_nested_template_argument_keeps_its_own_closers() {
    let text = "class A<Class T>\n{\n\tvoid Set(T v)\n\t{\n\t}\n}\n\nclass array<Class T>\n{\n}\n\nclass B : A<array<string>>\n{\n\toverride void Set(array<string> v)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// Two arguments, the second of them nested.
#[test]
fn two_template_arguments_with_a_nested_second() {
    let text = "class P<Class K, Class V>\n{\n\tvoid Put(K k, V v)\n\t{\n\t}\n}\n\nclass array<Class T>\n{\n}\n\nclass U : P<string, array<int>>\n{\n\toverride void Put(string k, array<int> v)\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A template error in pass 1 keeps the body pass from running, so its message
// comes first and the `(null)` lookup is never reached.
#[test]
fn a_template_error_in_pass_one_hides_the_body_pass() {
    let text = "class T<Class A, Class B>\n{\n}\n\nclass P\n{\n\tproto void F()\n\t{\n\t}\n}\n\nclass U\n{\n\tT<int> a;\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Not enough parameters for template class 'T'",
            "Wrong number of template parameters",
        ]
    );
}

// A destructor error keeps the module loading, so it does not stop the body pass
// either: the `(null)` lookup still runs.
#[test]
fn a_destructor_error_does_not_stop_the_body_pass() {
    let text = "class ProbeDT2\n{\n\tvoid ~ProbeOther()\n\t{\n\t}\n\tproto native void G()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Invalid destructor name '~ProbeOther'", "Cannot find function '(null)'"]
    );
}

// A base that is not declared is already an `Unknown type`; the methods written
// with `override` in that class are not also reported as having no base.
#[test]
fn an_unresolved_base_reports_only_the_unknown_type() {
    let text = "class ProbeOrphan : ProbeNoSuchBase\n{\n\toverride void F()\n\t{\n\t}\n\toverride void G()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Unknown type 'ProbeNoSuchBase'"]
    );
}

// The same for a class whose base chain loops.
#[test]
fn a_circular_base_reports_only_the_circle() {
    let text = "class ProbeCirA : ProbeCirB\n{\n}\n\nclass ProbeCirB : ProbeCirA\n{\n\toverride void F()\n\t{\n\t}\n}\n";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert_eq!(
        got.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Circle inheritance in class 'ProbeCirA'"]
    );
}

// A base declared in an earlier module that is itself derived: the method is
// found through the recorded base chain, not only in the first ancestor.
#[test]
fn a_method_on_an_earlier_modules_grandparent_is_found() {
    let files = [
        (
            "Scripts/4_World/w.c",
            WORLD,
            "class ProbeGrand\n{\n\tint Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
        ("Scripts/5_Mission/m.c", Layer::Mission, "class ProbeMid : ProbeGrand\n{\n}\n"),
        (
            "Scripts/5_Mission/m2.c",
            Layer::Mission,
            "class ProbeLow : ProbeMid\n{\n\toverride int Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A class in the later module derives from one declared in the same module,
// which derives from an earlier module's class, which derives from another in
// the same module as the first.
#[test]
fn a_chain_that_reenters_the_later_module_finds_the_method() {
    let files = [
        (
            "Scripts/4_World/w.c",
            WORLD,
            "class ProbeA\n{\n\tint Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
        ("Scripts/5_Mission/m.c", Layer::Mission, "class ProbeB : ProbeA\n{\n}\n"),
        ("Scripts/5_Mission/m2.c", Layer::Mission, "class ProbeC : ProbeB\n{\n}\n"),
        (
            "Scripts/5_Mission/m3.c",
            Layer::Mission,
            "class ProbeD : ProbeC\n{\n\toverride int Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// A method declared in a 4_World class, reached from a 5_Mission class through
// a 5_Mission class in between.
#[test]
fn a_method_two_modules_back_is_found() {
    let files = [
        (
            "Scripts/4_World/w.c",
            WORLD,
            "class ProbeWorldBase : ProbeRoot\n{\n\tint Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
        (
            "Scripts/3_Game/g.c",
            GAME,
            "class ProbeRoot\n{\n}\n",
        ),
        (
            "Scripts/5_Mission/m.c",
            Layer::Mission,
            "class ProbeMissionBase : ProbeWorldBase\n{\n}\n",
        ),
        (
            "Scripts/5_Mission/m2.c",
            Layer::Mission,
            "class ProbeServer : ProbeMissionBase\n{\n\toverride int Handler()\n\t{\n\t\treturn 0;\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

// The Expansion shape: vanilla aliases `Link<Object>`, the mod derives from the
// alias and repeats the instantiated constructor argument.
#[test]
fn a_constructor_through_an_aliased_vanilla_template_matches() {
    let files = [
        (
            "Scripts/1_Core/proto.c",
            Layer::Core,
            "class Object\n{\n}\n\nclass Link<Class T>\n{\n\tT m_Value;\n\tvoid Link(T init)\n\t{\n\t\tm_Value = init;\n\t}\n}\n",
        ),
        ("Scripts/3_Game/gameplay.c", GAME, "typedef Link<Object> OLinkT;\n"),
        (
            "Scripts/4_World/mod.c",
            WORLD,
            "class ProbeRemovedObject : OLinkT\n{\n\tvoid ProbeRemovedObject(Object init)\n\t{\n\t}\n}\n",
        ),
    ];
    let got = check(&input(&files, Target::RetailServer));
    assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
}

#[test]
fn function_rules_report_the_lines_the_game_printed() {
    let mut failures = Vec::new();
    for row in rows() {
        let retail: Vec<(u32, bool, String)> = check(&input(row.files, Target::RetailServer))
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line, diagnostic.fixme, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, bool, String)> = row
            .retail
            .iter()
            .map(|(line, fixme, message)| (*line, *fixme, (*message).to_string()))
            .collect();
        if retail != expected {
            failures.push(format!("{}: got {retail:?}, want {expected:?}", row.id));
        }
        let diag: Vec<(u32, String)> = check(&input(row.files, Target::DiagServer))
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| (diagnostic.line, diagnostic.message.clone()))
            .collect();
        let expected: Vec<(u32, String)> = row
            .diag
            .iter()
            .map(|(line, message)| (*line, (*message).to_string()))
            .collect();
        if diag != expected {
            failures.push(format!("{}: diag got {diag:?}, want {expected:?}", row.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}




// A typedef chain that loops is a `Circle inheritance in typedef` error, not a
// reason to follow the chain forever.
#[test]
fn a_typedef_cycle_does_not_overflow_the_stack() {
    let text = "typedef B A;\ntypedef A B;\nvoid F(A a){}";
    let got = check(&input(&[("Scripts/3_Game/a.c", GAME, text)], Target::RetailServer));
    assert!(
        got.diagnostics.iter().any(|diagnostic| diagnostic.message.contains("Circle inheritance in typedef")),
        "{:?}",
        got.diagnostics
    );
}



