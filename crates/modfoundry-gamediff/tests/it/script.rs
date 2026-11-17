use modfoundry_gamediff::script::{body_hash, script_model, Place, ScriptModel};

fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|&(path, source)| (path.to_owned(), source.to_owned()))
        .collect()
}

const A: (&str, &str) = (
    "scripts/3_Game/a.c",
    "class A : B\n{\n\tvoid F(int x) {}\n\tint m_X;\n}\n",
);
const MODDED_A: (&str, &str) = (
    "scripts/4_World/b.c",
    "modded class A\n{\n\toverride void F(int x) { super.F(x); }\n\tvoid G() {}\n}\n",
);

#[test]
fn a_modded_part_replaces_a_method_and_adds_its_own() {
    let model = script_model(&files(&[MODDED_A, A]));
    let class = &model.classes["A"];
    assert_eq!(class.name, "A");
    assert_eq!(class.base.as_deref(), Some("B"));
    assert_eq!(
        class.place,
        Some(Place {
            file: "scripts/3_Game/a.c".to_owned(),
            line: 1
        })
    );
    assert_eq!(
        class.modded_in,
        [Place {
            file: "scripts/4_World/b.c".to_owned(),
            line: 1
        }]
    );
    let keys: Vec<&str> = class.methods.keys().map(String::as_str).collect();
    assert_eq!(keys, ["F(int)", "G()"]);
    let f = &class.methods["F(int)"];
    assert_eq!(f.place.file, "scripts/4_World/b.c");
    assert!(f.modifiers.iter().any(|m| m == "override"));
    assert_eq!(class.fields["m_X"].place.file, "scripts/3_Game/a.c");
    assert_eq!(script_model(&files(&[A, MODDED_A])), model);
}

#[test]
fn members_in_ifdef_arms_are_keyed_by_their_condition() {
    let source = "class A\n{\n#ifdef X\n\tvoid F() {}\n#else\n\tvoid F() {}\n#endif\n#ifndef Y\n\tvoid H() {}\n#endif\n#ifdef Y\n#ifdef X\n\tvoid G() {}\n#endif\n#endif\n}\n";
    let model = script_model(&files(&[("scripts/3_Game/a.c", source)]));
    let keys: Vec<&str> = model.classes["A"]
        .methods
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["F() [!X]", "F() [X]", "G() [Y && X]", "H() [!Y]"]);
}

#[test]
fn body_hash_ignores_comments_and_spacing_but_not_code() {
    let cases = [
        (
            "void F()\n{\n    int a = 1;\n    Print(\"a\");\n}\n",
            "void F()\n{\n   int  a  =  1;\n\n    Print(\"a\");\n}\n",
            true,
        ),
        (
            "void F()\n{\n    int a = 1;\n    Print(\"a\");\n}\n",
            "void F()\n{\n    int a = 1; // one\n    /* two */ Print(\"a\");\n}\n",
            true,
        ),
        (
            "void F()\n{\n    int a = 1;\n}\n",
            "void F()\n{\n    int a = 2;\n}\n",
            false,
        ),
        (
            "void F()\n{\n    Print(\"a//b\");\n}\n",
            "void F()\n{\n    Print(\"a//c\");\n}\n",
            false,
        ),
        (
            "void F()\n{\n    Print(\"a\\\"//b\");\n}\n",
            "void F()\n{\n    Print(\"a\\\"//c\");\n}\n",
            false,
        ),
        (
            "void F()\n{\n    int/**/a = 1;\n}\n",
            "void F()\n{\n    inta = 1;\n}\n",
            false,
        ),
    ];
    for (left, right, matches) in cases {
        assert_eq!(hash(left) == hash(right), matches, "{left}\n{right}");
    }
}

fn hash(source: &str) -> String {
    let lines = source.lines().count();
    body_hash(source, [1, lines])
}

#[test]
fn a_file_with_a_syntax_error_is_listed_and_still_read() {
    let model = script_model(&files(&[
        ("scripts/3_Game/broken.c", "class Broken\n{\n\tvoid F() {\n"),
        ("scripts/3_Game/good.c", "class Good\n{\n\tvoid F() {}\n}\n"),
        ("scripts/3_Game/broken.c", "class Broken\n{\n\tvoid F() {\n"),
    ]));
    assert_eq!(model.error_files, ["scripts/3_Game/broken.c"]);
    assert!(model.classes.contains_key("Good"));
}

#[test]
fn the_model_round_trips_through_json() {
    let model = script_model(&files(&[MODDED_A, A]));
    let json = serde_json::to_string(&model).unwrap();
    let back: ScriptModel = serde_json::from_str(&json).unwrap();
    assert_eq!(back, model);
}

#[test]
fn overloads_and_the_destructor_are_separate_methods() {
    let source = "class A\n{\n\tvoid A() {}\n\tvoid ~A() {}\n\tvoid F(int a) {}\n\tvoid F(string s, float v[4][2]) {}\n}\n";
    let model = script_model(&files(&[("scripts/3_Game/a.c", source)]));
    let keys: Vec<&str> = model.classes["A"]
        .methods
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["A()", "F(int)", "F(string, float[4][2])", "~A()"]);
}

#[test]
fn a_modded_part_lines_up_with_a_class_inside_an_ifdef() {
    let vanilla = (
        "scripts/3_Game/a.c",
        "#ifdef X\nclass A\n{\n\tvoid F() {}\n#ifdef Y\n\tvoid G() {}\n#endif\n}\n#endif\n",
    );
    let early = (
        "scripts/2_GameLib/b.c",
        "#ifdef X\nmodded class A\n{\n\tvoid H() {}\n}\n#endif\n",
    );
    let late = (
        "scripts/5_Mission/c.c",
        "modded class A\n{\n\toverride void F() {}\n#ifdef DEVELOPER\n\tvoid D() {}\n#endif\n}\n",
    );
    let model = script_model(&files(&[vanilla, early, late]));
    let class = &model.classes["A"];
    assert_eq!(class.guards, ["X"]);
    assert_eq!(class.place.as_ref().map(|p| p.line), Some(2));
    let keys: Vec<&str> = class.methods.keys().map(String::as_str).collect();
    assert_eq!(keys, ["D() [DEVELOPER]", "F()", "G() [Y]", "H()"]);
    assert_eq!(class.methods["F()"].place.file, "scripts/5_Mission/c.c");
    assert_eq!(class.methods["G() [Y]"].guards, ["X", "Y"]);
    assert_eq!(class.methods["D() [DEVELOPER]"].guards, ["DEVELOPER"]);
}

#[test]
fn each_ifdef_arm_of_one_class_keeps_its_own_methods() {
    let source = "#ifdef A\nclass C\n{\n\tvoid F() {}\n}\n#else\nclass C\n{\n\tvoid F() {}\n\tvoid G() {}\n}\n#endif\n";
    let model = script_model(&files(&[("scripts/3_Game/c.c", source)]));
    let class = &model.classes["C"];
    let keys: Vec<&str> = class.methods.keys().map(String::as_str).collect();
    assert_eq!(keys, ["F()", "F() [!A]", "G() [!A]"]);
    assert_eq!(class.methods["F()"].guards, ["A"]);
}

#[test]
fn the_body_hash_leaves_out_the_signature() {
    let source = "class A\n{\n\tvoid F(int a = 1)\n\t{\n\t\tPrint(1);\n\t}\n}\n";
    let renamed = "class A\n{\n\tvoid F(int b = 2)\n\t{\n\t\tPrint(1);\n\t}\n}\n";
    let edited = "class A\n{\n\tvoid F(int a = 1)\n\t{\n\t\tPrint(2);\n\t}\n}\n";
    let body = |text: &str| {
        script_model(&files(&[("scripts/3_Game/a.c", text)])).classes["A"].methods["F(int)"]
            .body
            .clone()
    };
    assert_eq!(body(source), body(renamed));
    assert_ne!(body(source), body(edited));
}

#[test]
fn enums_keep_their_member_values() {
    let source = "enum E\n{\n\tA,\n\tB = 2,\n\tC = 1 << 3\n}\n";
    let model = script_model(&files(&[("scripts/3_Game/e.c", source)]));
    let e = &model.enums["E"];
    assert_eq!(e.members, ["A", "B", "C"]);
    assert_eq!(
        e.values,
        [None, Some("2".to_owned()), Some("1 << 3".to_owned())]
    );
}

#[test]
fn a_string_ends_at_the_end_of_its_line() {
    let left = "void F()\n{\n    Print(\"a); // one\n    int b = 1; // \" two\n}\n";
    let right = "void F()\n{\n    Print(\"a); // one\n    int b = 1; // \" four\n}\n";
    assert_eq!(hash(left), hash(right));
}
