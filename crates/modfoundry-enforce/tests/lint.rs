//! Contract tests for directory and file linting.
//!
//! Covers: module-located `.c` syntax checking, stray-`.c` module-path
//! finding, ignoring non-`.c` files, single-file lint, and clean trees.

use std::fs;
use std::path::{Path, PathBuf};

use modfoundry_enforce::lint_target;
use modfoundry_enforce::lint_text;
use modfoundry_enforce::parse_tree;
use modfoundry_enforce::Severity;

fn tmp(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mf-enforce-{label}-{}", std::process::id()))
}

/// Create a file under `dir` (creating parents), returning its path.
fn write(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, contents).unwrap();
    p
}

#[test]
fn module_source_that_parses_clean_produces_no_findings() {
    let root = tmp("clean");
    write(
        &root,
        "Scripts/3_Game/MyMod_Ok.c",
        "modded class PlayerBase {\n    void Ping() {}\n}\n",
    );
    assert!(lint_target(&root).unwrap().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn module_source_with_syntax_fault_reports_mf_e1001_with_path() {
    let root = tmp("syntax");
    write(&root, "Scripts/3_Game/MyMod_Bad.c", "}\n");
    let findings = lint_target(&root).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].diagnostic.code, "MF-E1001");
    assert_eq!(
        findings[0].path.file_name().unwrap().to_str(),
        Some("MyMod_Bad.c")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn stray_c_outside_module_is_reported_mf_e2001() {
    let root = tmp("stray");
    write(&root, "loose/scratch.c", "class Scratch {}\n");
    let findings = lint_target(&root).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].diagnostic.code, "MF-E2001");
    assert_eq!(findings[0].diagnostic.severity, Severity::Warning);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn non_c_files_are_ignored() {
    let root = tmp("nonc");
    write(&root, "Scripts/3_Game/a.c", "class A {}\n");
    write(&root, "config.cpp", "class CfgPatches { }");
    write(&root, "Scripts/3_Game/b.txt", "not source");
    write(&root, "Scripts/5_Mission/c.cpp", "void f() {}");
    let d = write(&root, "Scripts/5_Mission/d.cpp", "}\n");
    assert!(lint_target(&root).unwrap().is_empty());
    assert!(lint_target(&d).unwrap().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn single_file_lint_checks_that_file() {
    let dir = tmp("single");
    let file = write(&dir, "Scripts/4_World/X.c", "}\n");
    let findings = lint_target(&file).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].diagnostic.code, "MF-E1001");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lint_text_checks_one_file_without_the_filesystem() {
    let cases: [(&str, &str, &[&str]); 3] = [
        ("Scripts/4_World/a.c", "class A { @ % }", &["MF-E1001"]),
        ("stray.c", "class A {};", &["MF-E2001"]),
        (
            "Scripts/3_Game/MyMod_Ok.c",
            "modded class PlayerBase {\n    void Ping() {}\n}\n",
            &[],
        ),
    ];
    for (path, source, expected) in cases {
        let codes: Vec<String> = lint_text(Path::new(path), &parse_tree(source), source)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert_eq!(codes, expected, "{path}");
    }
}

#[test]
fn multiple_faults_report_each_with_its_path() {
    let root = tmp("multi");
    let _ = write(&root, "Scripts/3_Game/Bad.c", "}\n");
    let _ = write(&root, "loose/Stray.c", "class S {}\n");
    let findings = lint_target(&root).unwrap();
    let found: Vec<(String, String)> = findings
        .iter()
        .map(|f| {
            (
                f.path.file_name().unwrap().to_string_lossy().into_owned(),
                f.diagnostic.code.clone(),
            )
        })
        .collect();
    assert!(
        found.contains(&("Bad.c".to_owned(), "MF-E1001".to_owned())),
        "{found:?}"
    );
    assert!(
        found.contains(&("Stray.c".to_owned(), "MF-E2001".to_owned())),
        "{found:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_badly_encoded_file_does_not_stop_the_lint() {
    let root = tmp("encoding");
    let a = root.join("Scripts/3_Game/a.c");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::write(&a, b"// caf\xe9\nclass A {}\n").unwrap();
    write(&root, "Scripts/4_World/b.c", "}\n");
    let findings = lint_target(&root).unwrap();
    assert!(findings
        .iter()
        .any(|f| f.path.file_name().unwrap() == "b.c" && f.diagnostic.code == "MF-E1001"));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_symlink_cycle_is_not_followed() {
    let root = tmp("symlink");
    write(&root, "Scripts/3_Game/a.c", "}\n");
    std::os::unix::fs::symlink("..", root.join("Scripts/loop")).unwrap();
    assert_eq!(lint_target(&root).unwrap().len(), 1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn dot_folders_are_not_walked() {
    let root = tmp("dotdir");
    write(&root, ".cache/Scripts/3_Game/bad.c", "}\n");
    assert!(lint_target(&root).unwrap().is_empty());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn ifdef_split_declarations_are_not_syntax_errors() {
    let root = tmp("ifdef");
    write(
        &root,
        "Scripts/3_Game/x.c",
        "#ifdef A\nclass C extends Base1\n#else\nclass C extends Base2\n#endif\n{\n    int x;\n}\n\
         class CarScript\n{\n#ifndef DAYZ_1_29\n    override void OnCEIterate(float currentTime)\n    {\n        super.OnCEIterate(currentTime);\n#else\n    override void OnCEUpdate()\n    {\n        super.OnCEUpdate();\n\n        float elapsedSinceLast = 0;\n#endif\n        DoThing(elapsedSinceLast);\n    }\n}\n",
    );
    write(&root, "Scripts/3_Game/bad.c", "}\n");
    let findings = lint_target(&root).unwrap();
    let syntax: Vec<&str> = findings
        .iter()
        .filter(|finding| finding.diagnostic.code == "MF-E1001")
        .map(|finding| finding.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(syntax, ["bad.c"]);
    let _ = fs::remove_dir_all(&root);
}

fn duplicate_local_findings(source: &str) -> Vec<(String, String)> {
    lint_text(Path::new("Scripts/3_Game/a.c"), &parse_tree(source), source)
        .into_iter()
        .map(|d| {
            let span = d.span.expect("a duplicate local marks its name");
            (d.code, source[span.start..span.end].to_owned())
        })
        .collect()
}

#[test]
fn a_local_declared_twice_in_one_function_is_an_error() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "class A {\n    void f() {\n        for (int i = 0; i < 3; i++) {}\n        for (int i = 0; i < 3; i++) {}\n    }\n}\n",
            &[("MF-E1002", "i")],
        ),
        (
            "class A {\n    void f() {\n        foreach (int item : list) {}\n        if (true) {\n            int item = 1;\n        }\n    }\n}\n",
            &[("MF-E1002", "item")],
        ),
        (
            "class A {\n    void f(int count) {\n        int count;\n    }\n}\n",
            &[("MF-E1002", "count")],
        ),
        (
            "class A {\n    void f() {\n        int a, b;\n        int b;\n    }\n}\n",
            &[("MF-E1002", "b")],
        ),
        (
            "class A {\n    void f() {\n        int x;\n    }\n    void g() {\n        int x;\n    }\n}\n",
            &[],
        ),
        (
            "class A {\n    int count;\n    void f() {\n        int count;\n    }\n}\n",
            &[],
        ),
        (
            "class A {\n    void f() {\n#ifdef X\n        int a = 1;\n#else\n        int a = 2;\n#endif\n    }\n}\n",
            &[],
        ),
        (
            "class A {\n    void f() {\n#ifdef PLATFORM_PS4\n        Mission mission = g_Game.GetMission();\n#endif\n#ifdef PLATFORM_XBOX\n        Mission mission = g_Game.GetMission();\n#endif\n    }\n}\n",
            &[],
        ),
        (
            "class A {\n    void f() {\n        int a;\n#ifdef X\n        int a;\n#endif\n    }\n}\n",
            &[("MF-E1002", "a")],
        ),
        (
            "class A {\n    void f() {\n#ifdef X\n        int a;\n#endif\n        int a;\n    }\n}\n",
            &[("MF-E1002", "a")],
        ),
        (
            "class A {\n    void f() {\n        if (a) { b(); }\n        // c\n        if (k()) S(0);\n        else S(1);\n    }\n}\n",
            &[],
        ),
        (
            "class A {\n    void f() {\n#ifdef X\n        int a;\n        int a;\n#endif\n    }\n}\n",
            &[("MF-E1002", "a")],
        ),
        (
            "class A {\n    void f() {\n#ifdef X\n        int a;\n#ifdef Y\n        int a;\n#endif\n#endif\n    }\n}\n",
            &[("MF-E1002", "a")],
        ),
        (
            "class A {\n    void f(int a, int a) {\n    }\n}\n",
            &[("MF-E1002", "a")],
        ),
    ];
    for &(source, expected) in cases {
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(code, name)| (code.to_string(), name.to_string()))
            .collect();
        assert_eq!(duplicate_local_findings(source), expected, "{source}");
    }
}

#[test]
fn a_value_function_that_does_not_end_in_return_is_an_error() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "bool F() { while (true) { if (x) return true; } }",
            &[("MF-E1003", "F")],
        ),
        ("int F() { }", &[]),
        ("int F() { int a = 1; return a; }", &[]),
        ("int F() { if (x) { return 1; } else { return 2; } }", &[]),
        ("void F() { if (x) return; }", &[]),
        ("proto native int F();", &[]),
        ("int F() { return 1; // done\n}", &[]),
        ("int F() { return 1;; }", &[]),
        (
            "int F() { Foo();\n#ifdef SERVER\n return 1;\n#endif\n}",
            &[("MF-E1003", "F")],
        ),
        (
            "int F() {\n#ifdef SERVER\n return 1;\n#else\n return 2;\n#endif\n}",
            &[],
        ),
        (
            "class C { bool G() { while (true) { if (x) return true; } } }",
            &[("MF-E1003", "G")],
        ),
    ];
    let path = Path::new("Scripts/3_Game/x.c");
    for &(source, expected) in cases {
        let found: Vec<(String, String)> = lint_text(path, &parse_tree(source), source)
            .into_iter()
            .map(|d| {
                let span = d.span.expect("spans the method name");
                (d.code, source[span.start..span.end].to_owned())
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(code, name)| (code.to_string(), name.to_string()))
            .collect();
        assert_eq!(found, expected, "{source}");
    }
}

#[test]
fn a_method_that_fails_to_parse_is_left_to_the_syntax_check() {
    let source = "int F() { @ }";
    let codes: Vec<String> =
        lint_text(Path::new("Scripts/3_Game/x.c"), &parse_tree(source), source)
            .into_iter()
            .map(|d| d.code)
            .collect();
    assert_eq!(codes, ["MF-E1001"]);
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_named_in_the_error() {
    use std::os::unix::fs::PermissionsExt;

    let root = tmp("unreadable");
    let file = write(&root, "Scripts/3_Game/secret.c", "class A {}\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&file).is_ok() {
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let _ = fs::remove_dir_all(&root);
        return;
    }

    let error = lint_target(&root).unwrap_err();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        error.to_string().contains(file.to_str().unwrap()),
        "{error}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_foreach_straight_over_a_call_warns() {
    let method = |body: &str| format!("class A {{\n    void f() {{\n        {body}\n    }}\n}}\n");
    let cases: Vec<(String, &[&str])> = vec![
        (method("foreach (int x : Items()) {}"), &["Items()"]),
        (
            method("foreach (int i, string s : obj.Get()) {}"),
            &["obj.Get()"],
        ),
        (
            method("foreach (auto p : GetGame().GetPlayers()) {}"),
            &["GetGame().GetPlayers()"],
        ),
        (method("foreach (int x : (Items())) {}"), &["(Items())"]),
        (method("foreach (int x : m_Items) {}"), &[]),
        (method("foreach (int x : obj.m_Items) {}"), &[]),
        (method("foreach (int x : m_Lists[0]) {}"), &[]),
        (
            method("foreach (int x : Items()) {}\n        foreach (int y : Other()) {}"),
            &["Items()", "Other()"],
        ),
    ];
    for (source, expected) in cases {
        let found: Vec<(String, Severity, String)> = lint_text(
            Path::new("Scripts/3_Game/a.c"),
            &parse_tree(&source),
            &source,
        )
        .into_iter()
        .map(|d| {
            assert!(
                d.message.starts_with("foreach straight over"),
                "{}",
                d.message
            );
            let span = d.span.expect("spans the iterated expression");
            (d.code, d.severity, source[span.start..span.end].to_owned())
        })
        .collect();
        let expected: Vec<(String, Severity, String)> = expected
            .iter()
            .map(|text| ("MF-W3005".to_owned(), Severity::Warning, text.to_string()))
            .collect();
        assert_eq!(found, expected, "{source}");
    }
}
