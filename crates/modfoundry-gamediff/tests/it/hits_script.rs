//! Mod scripts checked against a script compare.

use modfoundry_gamediff::hits::{Hit, HitKind, Level, ModInput};
use modfoundry_gamediff::hits_script::script_hits;
use modfoundry_gamediff::script::{script_model, ScriptModel};
use modfoundry_gamediff::script_diff::script_diff;

fn model(source: &str) -> ScriptModel {
    script_model(&[("scripts/3_Game/a.c".to_owned(), source.to_owned())])
}

fn hits(old: &str, new: &str, scripts: &[(&str, &str)]) -> Vec<Hit> {
    let old = model(old);
    let new = model(new);
    let diff = script_diff(&old, &new);
    let input = ModInput {
        name: "TestMod".to_owned(),
        scripts: scripts
            .iter()
            .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
            .collect(),
        configs: Vec::new(),
    };
    script_hits(&old, &new, &diff, &input, &input.outlines())
}

fn listed(hits: &[Hit]) -> Vec<(HitKind, Level, usize, String)> {
    hits.iter()
        .map(|hit| (hit.kind, hit.level, hit.place.line, hit.vanilla.clone()))
        .collect()
}

#[test]
fn modded_and_extended_removed_classes_break() {
    let hits = hits(
        "class A\n{\n\tvoid F() {}\n}\n",
        "",
        &[(
            "scripts/4_World/mod.c",
            "modded class A\n{\n\toverride void F() {}\n}\nclass MyBase extends A\n{\n}\nclass M extends MyBase\n{\n}\n",
        )],
    );

    assert_eq!(
        listed(&hits),
        [
            (
                HitKind::ModdedClassRemoved,
                Level::Breaks,
                1,
                "A".to_owned()
            ),
            (HitKind::BaseClassRemoved, Level::Breaks, 5, "A".to_owned()),
            (HitKind::BaseClassRemoved, Level::Breaks, 8, "A".to_owned()),
        ]
    );
    assert!(hits[1].detail.contains("MyBase") && hits[1].detail.contains('A'));
    assert_eq!(hits[0].mod_name, "TestMod");
}

#[test]
fn overrides_by_change_kind() {
    let cases = [
        (
            "void F(int x) {}",
            "void F(string x) {}",
            "override void F(int x) {}",
            Some(HitKind::OverrideSignature),
        ),
        (
            "void R() {}",
            "",
            "override void R() {}",
            Some(HitKind::OverrideRemoved),
        ),
        (
            "void D(int x = 1) {}",
            "void D(int x = 2) {}",
            "override void D(int x = 1) {}",
            Some(HitKind::OverrideDefaults),
        ),
        (
            "void P(int x) {}",
            "void P(int y) {}",
            "override void P(int x) {}",
            None,
        ),
        (
            "void B() { Print(1); }",
            "void B() { Print(2); }",
            "override void B() { }",
            Some(HitKind::OverrideBody),
        ),
    ];
    for (before, after, over, expected) in cases {
        let hits = hits(
            &format!("class A\n{{\n\t{before}\n}}\n"),
            &format!("class A\n{{\n\t{after}\n}}\n"),
            &[(
                "scripts/4_World/mod.c",
                &format!("modded class A\n{{\n\t{over}\n}}\n"),
            )],
        );
        let kinds: Vec<HitKind> = hits.iter().map(|hit| hit.kind).collect();
        assert_eq!(kinds, expected.into_iter().collect::<Vec<_>>(), "{over}");
        if let [hit] = &hits[..] {
            assert_eq!(hit.place.line, 3, "{over}");
            assert_eq!(hit.level, expected.unwrap().level());
        }
    }

    let says_never_calls_super = |over: &str| {
        let hits = hits(
            "class A\n{\n\tvoid G() { Print(1); }\n}\n",
            "class A\n{\n\tvoid G() { Print(2); }\n}\n",
            &[(
                "scripts/4_World/mod.c",
                &format!("modded class A\n{{\n\t{over}\n}}\n"),
            )],
        );
        hits[0].detail.contains("never calls super")
    };
    assert!(says_never_calls_super("override void G() { }"));
    assert!(!says_never_calls_super("override void G() { super.G(); }"));
}

#[test]
fn overrides_are_found_through_ancestors() {
    let found = hits(
        "class A\n{\n\tvoid F() { Print(1); }\n}\nclass B extends A\n{\n}\n",
        "class A\n{\n\tvoid F() { Print(2); }\n}\nclass B extends A\n{\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "class M extends B\n{\n\toverride void F() { super.F(); }\n}\n",
        )],
    );
    assert_eq!(
        listed(&found),
        [(HitKind::OverrideBody, Level::Check, 3, "A.F()".to_owned())]
    );

    let moved = hits(
        "class A\n{\n}\nclass B extends A\n{\n\tvoid F() {}\n}\n",
        "class A\n{\n\tvoid F() {}\n}\nclass B extends A\n{\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "class M extends B\n{\n\toverride void F() {}\n}\n",
        )],
    );
    assert!(moved.is_empty(), "{moved:?}");
}

#[test]
fn calls_count_when_one_class_owns_the_name() {
    let old = "class A\n{\n\tvoid Unique() {}\n}\nclass B\n{\n\tvoid Shared() {}\n}\nclass C\n{\n\tvoid Shared() {}\n}\n";
    let new = "class A\n{\n}\nclass B\n{\n}\nclass C\n{\n\tvoid Shared() {}\n}\n";
    let source = "class M extends A\n{\n\tvoid Run()\n\t{\n\t\tUnique();\n\t\tShared();\n\t\tsuper.Unique();\n\t}\n}\n";

    let found = hits(old, new, &[("scripts/4_World/mod.c", source)]);
    assert_eq!(
        listed(&found),
        [(HitKind::CallRemoved, Level::Check, 5, "A.Unique".to_owned())]
    );
    assert!(found[0].maybe);

    let with_own = format!("{source}void Unique() {{}}\n");
    let own = hits(old, new, &[("scripts/4_World/mod.c", &with_own)]);
    assert!(own.is_empty(), "{own:?}");
}

#[test]
fn diag_only_carries_over() {
    let old = "class A\n{\n#ifdef DIAG_DEVELOPER\n\tvoid D() { Print(1); }\n#endif\n}\n";
    let new = "class A\n{\n#ifdef DIAG_DEVELOPER\n\tvoid D() { Print(2); }\n#endif\n}\n";
    let found = hits(
        old,
        new,
        &[(
            "scripts/4_World/mod.c",
            "modded class A\n{\n\toverride void D() { super.D(); }\n}\n",
        )],
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, HitKind::OverrideBody);
    assert!(found[0].diag_only);
}

#[test]
fn an_override_checks_every_ifdef_arm_of_its_method() {
    let vanilla = |then_body: &str| {
        format!("class A\n{{\n#ifdef X\n\tvoid F() {{ Print({then_body}); }}\n#else\n\tvoid F() {{ Print(9); }}\n#endif\n}}\n")
    };
    let hits = hits(
        &vanilla("1"),
        &vanilla("2"),
        &[(
            "scripts/4_World/mod.c",
            "modded class A\n{\n\toverride void F() { super.F(); }\n}\n",
        )],
    );

    assert_eq!(
        listed(&hits),
        [(HitKind::OverrideBody, Level::Check, 3, "A.F()".to_owned())]
    );
}

#[test]
fn a_call_to_a_name_the_new_build_still_has_is_no_hit() {
    let hits = hits(
        "class A\n{\n}\nclass B extends A\n{\n\tvoid Mv() {}\n}\nvoid Fn(int a) {}\nvoid Fn(string a) {}\n",
        "class A\n{\n\tvoid Mv() {}\n}\nclass B extends A\n{\n}\nvoid Fn(string a) {}\n",
        &[(
            "scripts/4_World/mod.c",
            "class Mine\n{\n\tvoid R(B b)\n\t{\n\t\tb.Mv();\n\t\tFn(\"x\");\n\t}\n}\n",
        )],
    );

    assert_eq!(listed(&hits), []);
}

#[test]
fn a_call_to_a_name_the_mod_declares_is_no_call_hit() {
    let hits = hits(
        "class A\n{\n\tvoid Gone() {}\n}\n",
        "class A\n{\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "class M extends A\n{\n\tvoid Gone() {}\n\tvoid R() { Gone(); }\n}\n",
        )],
    );

    assert_eq!(
        listed(&hits),
        [(
            HitKind::OverrideRemoved,
            Level::Breaks,
            3,
            "A.Gone()".to_owned()
        )]
    );
}

#[test]
fn a_call_to_a_diag_only_method_is_diag_only() {
    let hits = hits(
        "class A\n{\n#ifdef DIAG_DEVELOPER\n\tvoid DiagGone() {}\n#endif\n}\n",
        "class A\n{\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "class Mine\n{\n\tvoid R(A a) { a.DiagGone(); }\n}\n",
        )],
    );

    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].kind, HitKind::CallRemoved);
    assert!(hits[0].diag_only);
}

#[test]
fn a_changed_parent_is_named_for_modded_and_extending_classes() {
    let hits = hits(
        "class P\n{\n}\nclass Q\n{\n}\nclass A extends P\n{\n}\n",
        "class P\n{\n}\nclass Q\n{\n}\nclass A extends Q\n{\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "modded class A\n{\n}\nclass M extends A\n{\n}\n",
        )],
    );

    let details: Vec<(HitKind, &str)> = hits
        .iter()
        .map(|hit| (hit.kind, hit.detail.as_str()))
        .collect();
    assert_eq!(
        details,
        [
            (
                HitKind::BaseClassChanged,
                "modded class A: its parent changed from P to Q"
            ),
            (
                HitKind::BaseClassChanged,
                "class M extends A, whose parent changed from P to Q"
            ),
        ]
    );
}

#[test]
fn a_method_of_a_removed_class_that_moved_is_no_call_hit() {
    let hits = hits(
        "class Old\n{\n\tvoid Moved() {}\n\tvoid Gone() {}\n}\n",
        "class Fresh\n{\n\tvoid Moved() {}\n}\n",
        &[(
            "scripts/4_World/mod.c",
            "class Mine\n{\n\tvoid R(Fresh f)\n\t{\n\t\tf.Moved();\n\t\tf.Gone();\n\t}\n}\n",
        )],
    );

    let calls: Vec<String> = hits.iter().map(|hit| hit.vanilla.clone()).collect();
    assert_eq!(calls, ["Old.Gone"]);
}
