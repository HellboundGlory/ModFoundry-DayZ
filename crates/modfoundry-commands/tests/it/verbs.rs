//! Contract tests for the shared command bodies.
//!
//! These pin what the CLI and MCP bind through the registry: `validate_file`
//! reads a config (file or dir) and returns `MF-*` diagnostics, `lint_target`
//! finds Enforce Script issues (syntax `MF-E1001`, module path `MF-E2001`),
//! `scaffold` writes a loadable skeleton, and `build_project`,
//! `binarize_config` and `derapify_config` pack and convert it.

use std::path::{Path, PathBuf};

use crate::common::tmp;
use modfoundry_commands::{
    binarize_config, build_project, derapify_config, lint_target, scaffold, validate_file,
};
use modfoundry_config::{Diagnostic, ScaffoldOptions};
use modfoundry_enforce::Finding;

fn write_skeleton(dir: &Path) {
    for f in modfoundry_config::mod_skeleton(&ScaffoldOptions::new("MyMod")).unwrap() {
        let p = dir.join(&f.path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, f.contents).unwrap();
    }
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

fn finding_codes(findings: &[Finding]) -> Vec<String> {
    findings.iter().map(|f| f.diagnostic.code.clone()).collect()
}

#[test]
fn validate_clean_skeleton_has_no_dayz_errors() {
    let dir = tmp("clean");
    write_skeleton(&dir);
    let diags = validate_file(&dir).expect("validate runs").1;
    assert!(
        !codes(&diags)
            .iter()
            .any(|c| c.starts_with("MF-C2") || c.starts_with("MF-C3")),
        "clean skeleton produced {codes:?}",
        codes = codes(&diags)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn validate_accepts_a_single_config_file() {
    let dir = tmp("file");
    write_skeleton(&dir);
    let file = dir.join("config.cpp");
    let diags = validate_file(&file).expect("validate runs on a file").1;
    assert!(
        !codes(&diags).iter().any(|c| c == "MF-C400"),
        "file target misread: {:?}",
        codes(&diags)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scaffold_writes_the_template_files() {
    let dir = tmp("scaffold");
    scaffold(&dir, &ScaffoldOptions::new("MyMod")).expect("scaffold succeeds");
    assert!(dir.join("config.cpp").is_file());
    assert!(dir.join("mod.cpp").is_file());
    assert!(dir.join(".gitignore").is_file());
    for d in ["3_Game", "4_World"] {
        assert!(
            dir.join(format!("Scripts/{d}/.gitkeep")).is_file(),
            "missing {d}"
        );
    }
    assert!(dir.join("Scripts/5_Mission/MyModMission.c").is_file());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scaffold_rejects_unsafe_name_without_writing() {
    let dir = tmp("unsafe");
    let err = scaffold(&dir, &ScaffoldOptions::new("../evil")).unwrap_err();
    assert!(err.to_string().contains("not a safe mod name"));
    assert!(!dir.exists(), "must not create dir on invalid name");
}

#[test]
fn stray_script_outside_module_reports_mf_e2001() {
    let dir = tmp("lint-stray");
    scaffold(&dir, &ScaffoldOptions::new("MyMod")).expect("scaffold succeeds");
    let stray = dir.join("loose/scratch.c");
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, "class Scratch {}").unwrap();
    let findings = lint_target(&dir).expect("lint runs");
    assert!(
        finding_codes(&findings).iter().any(|c| c == "MF-E2001"),
        "expected MF-E2001, got {:?}",
        finding_codes(&findings)
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn mod_dir(label: &str) -> PathBuf {
    let d = tmp(label);
    std::fs::create_dir_all(&d).unwrap();
    scaffold(&d, &ScaffoldOptions::new("MyMod")).unwrap();
    d
}

#[test]
fn build_stages_signed_pbo_layout_from_scaffolded_mod() {
    let dir = mod_dir("build-ok");
    let report = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("MyMod".into()),
        output: None,
        profile: Some("dev".into()),
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .expect("unsigned build succeeds");
    assert_eq!(report.name, "MyMod");
    assert_eq!(report.addons, vec!["MyMod.pbo"]);
    assert!(!report.signed);
    let out = dir.join("builds/dev/@MyMod");
    assert!(out.join("addons/MyMod.pbo").is_file());
    assert!(out.join("mod.cpp").is_file());
    // deterministic: same input yields identical pbo bytes
    let again = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("MyMod".into()),
        output: None,
        profile: Some("dev".into()),
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .unwrap();
    let p1 = std::fs::read(out.join("addons/MyMod.pbo")).unwrap();
    std::fs::remove_dir_all(&out).ok();
    let _ = again;
    let r2 = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("MyMod".into()),
        output: None,
        profile: Some("dev".into()),
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .unwrap();
    let p2 = std::fs::read(r2.output_dir.join("addons/MyMod.pbo")).unwrap();
    assert_eq!(p1, p2, "build output must be byte-deterministic");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_packs_config_and_script_modules() {
    let dir = mod_dir("build-content");
    std::fs::create_dir_all(dir.join("Scripts/1_Core")).unwrap();
    std::fs::write(dir.join("Scripts/1_Core/Foo.c"), "class Foo {}").unwrap();
    let report = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("MyMod".into()),
        output: None,
        profile: Some("p".into()),
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .unwrap();
    let bytes = std::fs::read(report.output_dir.join("addons/MyMod.pbo")).unwrap();
    let (hdr, _) = modfoundry_pbo::read_pbo(&bytes).unwrap();
    let names: Vec<String> = hdr
        .entries
        .iter()
        .map(|e| e.name_str().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "config.cpp"),
        "config.cpp packed, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("1_Core\\Foo.c")),
        "script module packed, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "mod.cpp"),
        "mod.cpp must not be packed into the addon, got {names:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_derives_the_name_from_the_target_dir_when_not_given() {
    let dir = tmp("build-name").join("BuildName");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let expected = dir.file_name().unwrap().to_string_lossy().into_owned();
    scaffold(&dir, &ScaffoldOptions::new(expected.as_str())).unwrap();
    let report = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: None,
        output: None,
        profile: None,
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .unwrap();
    assert_eq!(report.name, expected);
    let bytes = std::fs::read(report.output_dir.join(format!("addons/{expected}.pbo"))).unwrap();
    assert_eq!(
        modfoundry_pbo::read_pbo(&bytes)
            .unwrap()
            .0
            .property("prefix"),
        Some(expected.as_str()),
        "the PBO prefix must equal the derived name"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_rejects_a_config_binding_a_different_prefix() {
    let dir = mod_dir("build-mismatch");
    let err = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("Renamed".into()),
        output: None,
        profile: None,
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: false,
        strip_comments: false,
    })
    .expect_err("the scaffolded config binds MyMod/Scripts/... and would bind nothing as Renamed");
    assert!(
        err.to_string().contains("files[]"),
        "error must name the binding, got: {err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_prefix_override_builds_an_imported_tree_unchanged() {
    let dir = tmp("build-prefix");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Scripts/3_Game")).unwrap();
    std::fs::write(
        dir.join("config.cpp"),
        b"class CfgPatches { class CF_Scripts { requiredAddons[] = {\"DZ_Data\"}; }; };\n\
          class CfgMods { class CF { dir = \"JM/CF\"; type = \"mod\"; class defs {\n\
              class gameScriptModule { files[] = { \"JM/CF/Scripts/3_Game\" }; };\n\
          }; }; };\n",
    )
    .unwrap();
    std::fs::write(dir.join("Scripts/3_Game/Foo.c"), "class Foo {}").unwrap();

    let args = |prefix: Option<&str>| modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("CFImported".into()),
        output: None,
        profile: Some("prefix".into()),
        authority: None,
        keys_dir: None,
        prefix: prefix.map(str::to_owned),
        tools: None,
        binarize_config: false,
        strip_comments: false,
    };

    assert!(
        build_project(&args(None)).is_err(),
        "the derived prefix cannot satisfy a JM/CF-rooted config"
    );

    let report =
        build_project(&args(Some("JM/CF"))).expect("--prefix makes the imported tree build");
    let bytes = std::fs::read(report.output_dir.join("addons/CFImported.pbo")).unwrap();
    assert_eq!(
        modfoundry_pbo::read_pbo(&bytes)
            .unwrap()
            .0
            .property("prefix"),
        Some("JM/CF"),
        "the PBO prefix must be the override, not the mod name"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_binarize_packs_config_bin_instead_of_config_cpp() {
    let dir = mod_dir("build-bin");
    let report = build_project(&modfoundry_commands::BuildArgs {
        mod_root: dir.clone(),
        name: Some("MyMod".into()),
        output: None,
        profile: None,
        authority: None,
        keys_dir: None,
        prefix: None,
        tools: None,
        binarize_config: true,
        strip_comments: false,
    })
    .unwrap();
    let bytes = std::fs::read(report.output_dir.join("addons/MyMod.pbo")).unwrap();
    let (hdr, payloads) = modfoundry_pbo::read_pbo(&bytes).unwrap();
    let names: Vec<String> = hdr
        .entries
        .iter()
        .map(|e| e.name_str().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "config.bin"),
        "binarized config packed, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "config.cpp"),
        "raw config.cpp must not be packed alongside config.bin, got {names:?}"
    );
    let idx = names.iter().position(|n| n == "config.bin").unwrap();
    let file = modfoundry_config::read_rap(&payloads[idx]).expect("packed config.bin parses");
    assert!(modfoundry_config::to_text(&file).contains("class CfgPatches"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_bin_then_derapify_round_trips() {
    let dir = mod_dir("config-bin-rt");
    let cpp = dir.join("config.cpp");
    let bin = binarize_config(&cpp).expect("scaffolded config.bin binarizes");
    assert_eq!(&bin[..4], b"\0raP", "RaP signature");

    let bin_path = dir.join("out.bin");
    std::fs::write(&bin_path, &bin).unwrap();
    let text = derapify_config(&bin_path).expect("derapifies");
    assert!(text.contains("class CfgPatches"));
    assert!(text.contains("class MyMod_Scripts"));
    assert!(text.contains("\"MyMod/Scripts/3_Game\""));

    let reparsed = modfoundry_config::parser::parse(&text);
    assert!(
        reparsed.issues.is_empty(),
        "derapified text must parse clean: {:?}",
        reparsed
            .issues
            .iter()
            .map(|i| &i.message)
            .collect::<Vec<_>>()
    );
    let again = modfoundry_config::write_rap(&reparsed.file).unwrap();
    assert_eq!(again, bin, "bin -> text -> bin must be byte-stable");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_bin_refuses_a_config_with_parse_issues() {
    let dir = tmp("config-bin-bad");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("config.cpp");
    std::fs::write(&bad, "class Broken { units[] = ").unwrap();
    let err = binarize_config(&bad).expect_err("a recovered AST must not be binarized");
    assert!(
        err.to_string().contains("parse issue"),
        "error must name the parse issue, got: {err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
