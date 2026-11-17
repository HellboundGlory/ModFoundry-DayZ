use modfoundry_gamediff::script::script_model;
use modfoundry_gamediff::script_diff::{script_diff, signature_text, MethodChangeKind, ScriptDiff};

fn diff_sources(old: &str, new: &str) -> ScriptDiff {
    let model =
        |source: &str| script_model(&[("scripts/3_Game/a.c".to_owned(), source.to_owned())]);
    script_diff(&model(old), &model(new))
}

fn class(method: &str) -> String {
    format!("class A\n{{\n\t{method}\n}}\n")
}

#[test]
fn classes_added_removed_and_rebased() {
    let old = "class Gone {}\nclass B : A {}\nclass Same : Root {}\n";
    let new = "class Fresh {}\nclass B : C {}\nclass Same : Root {}\n";
    let diff = diff_sources(old, new);

    let names = |items: &[modfoundry_gamediff::script_diff::ItemRef]| -> Vec<String> {
        items.iter().map(|item| item.name.clone()).collect()
    };
    assert_eq!(names(&diff.classes_added), ["Fresh"]);
    assert_eq!(names(&diff.classes_removed), ["Gone"]);

    let changed = &diff.classes_changed;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].name, "B");
    let base = changed[0].base.as_ref().unwrap();
    assert_eq!(base.name, "base");
    assert_eq!(base.old.as_deref(), Some("A"));
    assert_eq!(base.new.as_deref(), Some("C"));
}

#[test]
fn method_change_kinds() {
    use MethodChangeKind::*;
    let cases = [
        ("void F() {}", "int F() {}", vec![Signature]),
        ("void F() {}", "protected void F() {}", vec![Signature]),
        ("void F() {}", "override void F() {}", vec![]),
        ("void F(int a) {}", "void F(int b) {}", vec![ParamsRenamed]),
        (
            "void F(int a = 1) {}",
            "void F(int a = 2) {}",
            vec![Defaults],
        ),
        ("void F(int a) {}", "void F(int a = 1) {}", vec![Defaults]),
        ("void F(int a = 1) {}", "void F(int a) {}", vec![Defaults]),
        (
            "void F() { Print(1); }",
            "void F() { Print(2); }",
            vec![Body],
        ),
        (
            "void F()\n\t{\n\t\tint a = 1;\n\t}",
            "void F()\n\t{\n\t\tint  a  =  1; // note\n\t}",
            vec![],
        ),
        (
            "void F(int a) { Print(1); }",
            "void F(int b) { Print(2); }",
            vec![ParamsRenamed, Body],
        ),
    ];
    for (before, after, expected) in cases {
        let diff = diff_sources(&class(before), &class(after));
        if expected.is_empty() {
            assert_eq!(diff, ScriptDiff::default(), "{before} -> {after}");
            continue;
        }
        let methods = &diff.classes_changed[0].methods;
        assert!(methods.added.is_empty() && methods.removed.is_empty());
        assert_eq!(methods.changed[0].kinds, expected, "{before} -> {after}");
    }
}

#[test]
fn an_overload_with_new_parameters_is_paired_by_name() {
    let diff = diff_sources(
        &class("void ItemFall(string a) { }"),
        &class("void ItemFall(string a, vector b, vector c) { }"),
    );
    let changed = &diff.classes_changed[0].methods;
    assert!(changed.added.is_empty());
    assert!(changed.removed.is_empty());
    assert_eq!(changed.changed.len(), 1);
    let entry = &changed.changed[0];
    assert_eq!(entry.old_key, "ItemFall(string)");
    assert_eq!(entry.key, "ItemFall(string, vector, vector)");
    assert_eq!(entry.kinds, [MethodChangeKind::Signature]);
    assert_eq!(entry.old_signature, "void ItemFall(string a)");
    assert_eq!(
        entry.new_signature,
        "void ItemFall(string a, vector b, vector c)"
    );

    let both = diff_sources(
        &class("void F(int a) {}\n\tvoid F(string s) {}"),
        &class("void F(int a, int b) {}\n\tvoid F(string s, int n) {}"),
    );
    let changed = &both.classes_changed[0].methods;
    assert!(changed.changed.is_empty());
    let names = |items: &[modfoundry_gamediff::script_diff::ItemRef]| -> Vec<String> {
        items.iter().map(|item| item.key.clone()).collect()
    };
    assert_eq!(names(&changed.removed), ["F(int)", "F(string)"]);
    assert_eq!(names(&changed.added), ["F(int, int)", "F(string, int)"]);
}

#[test]
fn fields_globals_and_typedefs_list_their_changed_parts() {
    let changes = |list: &[modfoundry_gamediff::ValueChange]| -> Vec<(String, Option<String>, Option<String>)> {
        list.iter()
            .map(|change| (change.name.clone(), change.old.clone(), change.new.clone()))
            .collect()
    };

    let diff = diff_sources(
        "class A\n{\n\tint m_X;\n}\n",
        "class A\n{\n\tfloat m_X;\n}\n",
    );
    let fields = &diff.classes_changed[0].fields;
    assert!(fields.added.is_empty() && fields.removed.is_empty());
    assert_eq!(
        changes(&fields.changed[0].changes),
        [(
            "type".to_owned(),
            Some("int".to_owned()),
            Some("float".to_owned())
        )]
    );

    let diff = diff_sources("const float X = 1.0;\n", "const float X = 2.0;\n");
    assert_eq!(
        changes(&diff.globals.changed[0].changes),
        [(
            "value".to_owned(),
            Some("1.0".to_owned()),
            Some("2.0".to_owned())
        )]
    );

    let diff = diff_sources("typedef array<string> T;\n", "typedef array<int> T;\n");
    assert_eq!(
        changes(&diff.typedefs.changed[0].changes),
        [(
            "target".to_owned(),
            Some("array<string>".to_owned()),
            Some("array<int>".to_owned())
        )]
    );

    let diff = diff_sources("int g_Gone;\n", "int g_Stays;\n");
    assert_eq!(diff.globals.removed[0].name, "g_Gone");
    assert_eq!(diff.globals.added[0].name, "g_Stays");
}

#[test]
fn enum_members_are_compared_by_effective_value() {
    let members = |diff: &ScriptDiff| -> Vec<(String, Option<String>, Option<String>)> {
        diff.enums.changed[0]
            .members
            .iter()
            .map(|change| (change.name.clone(), change.old.clone(), change.new.clone()))
            .collect()
    };

    let diff = diff_sources("enum E { A, B, C }\n", "enum E { A, X, B, C }\n");
    assert_eq!(
        members(&diff),
        [
            ("X".to_owned(), None, Some("1".to_owned())),
            ("B".to_owned(), Some("1".to_owned()), Some("2".to_owned())),
            ("C".to_owned(), Some("2".to_owned()), Some("3".to_owned())),
        ]
    );

    let diff = diff_sources("enum F { A = 4, B }\n", "enum F { A = 4, B, C }\n");
    assert_eq!(
        members(&diff),
        [("C".to_owned(), None, Some("6".to_owned()))]
    );

    let diff = diff_sources("enum G { A = 1 << 2 }\n", "enum G { A = 1 << 2, B }\n");
    assert_eq!(
        members(&diff),
        [("B".to_owned(), None, Some("1 << 2 + 1".to_owned()))]
    );

    let diff = diff_sources("enum H { A }\n", "enum H : int { A }\n");
    let base = diff.enums.changed[0].base.as_ref().unwrap();
    assert_eq!(base.name, "base");
    assert_eq!(base.new.as_deref(), Some("int"));
}

#[test]
fn diag_only_items_are_marked() {
    let old = "class A\n{\n#ifdef DEVELOPER\n#ifdef SERVER\n\tvoid Dev() {}\n#endif\n#endif\n}\n";
    let new = "class A\n{\n#ifdef DIAG_DEVELOPER\n\tvoid Diag() {}\n#endif\n#ifndef DIAG_DEVELOPER\n\tvoid Release() {}\n#endif\n}\n";
    let diff = diff_sources(old, new);
    let marked: Vec<(String, bool)> = diff.classes_changed[0]
        .methods
        .added
        .iter()
        .map(|item| (item.name.clone(), item.diag_only))
        .collect();
    assert_eq!(
        marked,
        [("Diag".to_owned(), true), ("Release".to_owned(), false),]
    );
    let removed = diff.classes_changed[0].methods.removed[0].clone();
    assert_eq!(removed.name, "Dev");
    assert!(removed.diag_only);

    let class = "#ifdef DIAG_DEVELOPER\nclass OnlyDiag\n{\n}\n#endif\n";
    let diff = diff_sources("", class);
    assert_eq!(diff.classes_added[0].name, "OnlyDiag");
    assert!(diff.classes_added[0].diag_only);
}

#[test]
fn free_functions_are_compared_like_methods() {
    let diff = diff_sources(
        "void Helper() { Print(1); }\n",
        "void Helper() { Print(2); }\nvoid Fresh() {}\n",
    );
    assert_eq!(diff.functions.added[0].name, "Fresh");
    let changed = &diff.functions.changed[0];
    assert_eq!(changed.name, "Helper");
    assert_eq!(changed.kinds, [MethodChangeKind::Body]);
    assert_eq!(changed.old_signature, "void Helper()");
    assert_eq!(changed.new_signature, "void Helper()");
}

#[test]
fn the_diff_round_trips_through_json() {
    let old = "class B : A\n{\n\tvoid F(int a) {}\n\tint m_X;\n}\nclass Gone {}\nconst float C = 1.0;\nenum E { A, B }\ntypedef array<string> T;\nvoid Helper() {}\n";
    let new = "class B : C\n{\n\tvoid F(int b) { Print(1); }\n\tfloat m_X;\n\tvoid G() {}\n}\nclass Fresh {}\nconst float C = 2.0;\nenum E { A, X, B }\ntypedef array<int> T;\nvoid Helper() { Print(2); }\n";
    let diff = diff_sources(old, new);
    assert!(!diff.classes_added.is_empty());
    assert!(!diff.classes_removed.is_empty());
    assert!(!diff.classes_changed.is_empty());
    assert!(!diff.functions.changed.is_empty());
    assert!(!diff.globals.changed.is_empty());
    assert!(!diff.enums.changed.is_empty());
    assert!(!diff.typedefs.changed.is_empty());

    let json = serde_json::to_string(&diff).unwrap();
    assert!(json.contains("\"params_renamed\""));
    let back: ScriptDiff = serde_json::from_str(&json).unwrap();
    assert_eq!(back, diff);
}

#[test]
fn a_signature_reads_like_the_source() {
    let model = script_model(&[(
        "scripts/3_Game/a.c".to_owned(),
        "class A\n{\n\tproto native int GetValue(out float v, int n = 2);\n\tvoid ~A() {}\n\tvoid F(int a[2][3]);\n}\n"
            .to_owned(),
    )]);
    let methods = &model.classes["A"].methods;
    assert_eq!(
        signature_text(&methods["GetValue(float, int)"]),
        "proto native int GetValue(out float v, int n = 2)"
    );
    assert_eq!(signature_text(&methods["~A()"]), "void ~A()");
    assert_eq!(
        signature_text(&methods["F(int[2][3])"]),
        "void F(int a[2][3])"
    );
}

#[test]
fn an_overload_with_brackets_in_an_array_size_is_paired() {
    let diff = diff_sources(
        &class("void F(int a[(X + 1)]) {}"),
        &class("void F(int a[(X + 1)], int b) {}"),
    );
    let methods = &diff.classes_changed[0].methods;
    assert!(methods.added.is_empty() && methods.removed.is_empty());
    assert_eq!(methods.changed[0].kinds, [MethodChangeKind::Signature]);
}

#[test]
fn a_class_moving_under_a_condition_is_changed() {
    let plain = "class A\n{\n\tvoid F() {}\n}\n";
    let guarded = format!("#ifdef DIAG_DEVELOPER\n{plain}#endif\n");
    let diff = diff_sources(plain, &guarded);

    let changed = &diff.classes_changed[0];
    let conditions = changed.conditions.as_ref().unwrap();
    assert_eq!(conditions.name, "conditions");
    assert_eq!(conditions.old, None);
    assert_eq!(conditions.new.as_deref(), Some("DIAG_DEVELOPER"));
    assert!(changed.diag_only);
}

#[test]
fn an_enum_value_written_another_way_is_unchanged() {
    let diff = diff_sources("enum E { A = 16, B }\n", "enum E { A = 0x10, B }\n");
    assert_eq!(diff, ScriptDiff::default());
}
