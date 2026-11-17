use std::fs;
use std::path::{Path, PathBuf};

use enforce_check::{CheckResult, Diagnostic, Layer, Module, Severity, SourceFile, Target};
use modfoundry_commands::script_check::{
    merge_findings, parse_target, target_name, unchecked_files, unpack_sources, FailedModule,
    VanillaSource,
};
use modfoundry_core::{Actor, CommandError, Outcome, Severity as MfSeverity, Span};
use serde_json::{json, Value};

use crate::common;

fn diagnostic(file: &str, line: u32, severity: Severity, message: &str) -> Diagnostic {
    Diagnostic {
        file: file.into(),
        line,
        module: Module::Game,
        severity,
        message: message.into(),
        fixme: false,
        game_crash: false,
        reports_only: false,
        related: None,
    }
}

fn result(diagnostics: Vec<Diagnostic>) -> CheckResult {
    CheckResult {
        diagnostics,
        failed_modules: Vec::new(),
    }
}

fn source(path: &str, text: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        layer: Layer::Game,
        mod_index: 0,
        text: text.into(),
    }
}

#[test]
fn a_line_printed_by_every_build_is_one_error_naming_all_three() {
    let file = "Scripts/4_World/x.c";
    let line = diagnostic(file, 2, Severity::Error, "Undefined variable 'foo'");
    let results = [
        (Target::RetailServer, result(vec![line.clone()])),
        (Target::DiagServer, result(vec![line.clone()])),
        (Target::DiagClient, result(vec![line])),
    ];
    let files = [source(file, "a\nfoo\nc")];
    let disk = [PathBuf::from("/mods/x/Scripts/4_World/x.c")];

    let (findings, failed) = merge_findings(&results, &files, &disk);

    assert!(failed.is_empty());
    assert_eq!(findings.len(), 1);
    let finding = &findings[0];
    assert_eq!(finding.line, 2);
    assert_eq!(
        finding.targets,
        ["retail-server", "diag-server", "diag-client"]
    );
    assert_eq!(finding.report.diagnostic.code, "MF-E4001");
    assert_eq!(
        finding.report.diagnostic.severity,
        modfoundry_core::Severity::Error
    );
    assert_eq!(
        finding.report.diagnostic.message,
        "Undefined variable 'foo'"
    );
    assert_eq!(finding.report.path, disk[0]);
    assert_eq!(finding.report.diagnostic.span, Some(Span::new(2, 5)));
}

#[test]
fn code_is_the_first_rule_that_fits() {
    let file = "Scripts/4_World/x.c";
    let build = |severity, fixme, game_crash| Diagnostic {
        fixme,
        game_crash,
        ..diagnostic(file, 2, severity, "message")
    };
    let cases = [
        (
            "fixme in one build, error in another",
            vec![
                (Target::RetailServer, build(Severity::Warning, true, false)),
                (Target::DiagServer, build(Severity::Error, false, false)),
            ],
            "MF-W4002",
            MfSeverity::Warning,
        ),
        (
            "game crash",
            vec![(Target::RetailServer, build(Severity::Error, false, true))],
            "MF-E4003",
            MfSeverity::Fatal,
        ),
        (
            "game crash on a line another build only warns about",
            vec![
                (Target::RetailServer, build(Severity::Warning, true, false)),
                (Target::DiagServer, build(Severity::Error, false, true)),
            ],
            "MF-E4003",
            MfSeverity::Fatal,
        ),
        (
            "plain warning",
            vec![(Target::RetailServer, build(Severity::Warning, false, false))],
            "MF-W4004",
            MfSeverity::Warning,
        ),
    ];
    for (name, diagnostics, code, severity) in cases {
        let results: Vec<_> = diagnostics
            .into_iter()
            .map(|(target, d)| (target, result(vec![d])))
            .collect();
        let (findings, _) = merge_findings(
            &results,
            &[source(file, "a\nb")],
            &[PathBuf::from("/mods/x/Scripts/4_World/x.c")],
        );
        assert_eq!(findings.len(), 1, "{name}");
        assert_eq!(findings[0].report.diagnostic.code, code, "{name}");
        assert_eq!(findings[0].report.diagnostic.severity, severity, "{name}");
    }
}

#[test]
fn a_line_only_one_build_prints_names_that_build() {
    let file = "Scripts/4_World/x.c";
    let results = [
        (Target::RetailServer, result(Vec::new())),
        (Target::DiagServer, result(Vec::new())),
        (
            Target::DiagClient,
            result(vec![diagnostic(file, 1, Severity::Error, "Only here")]),
        ),
    ];
    let (findings, _) = merge_findings(
        &results,
        &[source(file, "a")],
        &[PathBuf::from("/mods/x/Scripts/4_World/x.c")],
    );

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].targets, ["diag-client"]);
}

#[test]
fn span_leaves_out_the_line_ending_and_missing_places_are_none() {
    let file = "Scripts/4_World/x.c";
    let cases = [
        (
            "crlf line",
            vec![source(file, "a\r\nfoo\r\nc")],
            vec![PathBuf::from("/mods/x.c")],
            2,
            Some(Span::new(3, 6)),
            PathBuf::from("/mods/x.c"),
        ),
        (
            "line past the end",
            vec![source(file, "a\nb")],
            vec![PathBuf::from("/mods/x.c")],
            9,
            None,
            PathBuf::from("/mods/x.c"),
        ),
        (
            "the line after a final newline",
            vec![source(file, "a\n")],
            vec![PathBuf::from("/mods/x.c")],
            2,
            Some(Span::new(2, 2)),
            PathBuf::from("/mods/x.c"),
        ),
        (
            "an empty file",
            vec![source(file, "")],
            vec![PathBuf::from("/mods/x.c")],
            1,
            Some(Span::new(0, 0)),
            PathBuf::from("/mods/x.c"),
        ),
        (
            "file not in the sources",
            Vec::new(),
            Vec::new(),
            1,
            None,
            PathBuf::from(file),
        ),
        (
            "a file with no disk path",
            vec![source(file, "a")],
            Vec::new(),
            1,
            Some(Span::new(0, 1)),
            PathBuf::from(file),
        ),
    ];
    for (name, files, disk, line, span, path) in cases {
        let results = [(
            Target::RetailServer,
            result(vec![diagnostic(file, line, Severity::Error, "message")]),
        )];
        let (findings, _) = merge_findings(&results, &files, &disk);
        assert_eq!(findings.len(), 1, "{name}");
        assert_eq!(findings[0].report.diagnostic.span, span, "{name}");
        assert_eq!(findings[0].report.path, path, "{name}");
    }
}

#[test]
fn a_repeated_game_path_takes_the_later_file() {
    let file = "Scripts/4_World/x.c";
    let results = [(
        Target::RetailServer,
        result(vec![diagnostic(file, 2, Severity::Error, "message")]),
    )];
    let files = [source(file, "a\nold line\nc"), source(file, "a\nfoo\nc")];
    let disk = [PathBuf::from("/vanilla/x.c"), PathBuf::from("/mods/x.c")];

    let (findings, _) = merge_findings(&results, &files, &disk);

    assert_eq!(findings[0].report.path, PathBuf::from("/mods/x.c"));
    assert_eq!(findings[0].report.diagnostic.span, Some(Span::new(2, 5)));
}

#[test]
fn related_place_maps_to_its_disk_path_and_line() {
    let file = "Scripts/4_World/x.c";
    let other = "Scripts/4_World/y.c";
    let mut declared = diagnostic(file, 2, Severity::Error, "Redeclared");
    declared.related = Some((other.into(), 7));
    let results = [(Target::RetailServer, result(vec![declared]))];
    let files = [
        source(file, "a\nb\nc"),
        source(other, "1\n2\n3\n4\n5\n6\n7"),
    ];
    let disk = [PathBuf::from("/mods/x/x.c"), PathBuf::from("/mods/x/y.c")];

    let (findings, _) = merge_findings(&results, &files, &disk);

    let related = findings[0].related.as_ref().expect("related place");
    assert_eq!(related.path, PathBuf::from("/mods/x/y.c"));
    assert_eq!(related.line, 7);
}

#[test]
fn failed_modules_merge_with_their_targets_in_first_seen_order() {
    let failing = |modules: Vec<Module>| CheckResult {
        diagnostics: Vec::new(),
        failed_modules: modules,
    };
    let results = [
        (Target::RetailServer, failing(vec![Module::Mission])),
        (
            Target::DiagServer,
            failing(vec![Module::Game, Module::Mission]),
        ),
        (Target::DiagClient, failing(vec![Module::Game])),
    ];

    let (findings, failed) = merge_findings(&results, &[], &[]);

    assert!(findings.is_empty());
    assert_eq!(
        failed,
        [
            FailedModule {
                module: "Mission".into(),
                targets: vec!["retail-server".into(), "diag-server".into()],
            },
            FailedModule {
                module: "Game".into(),
                targets: vec!["diag-server".into(), "diag-client".into()],
            },
        ]
    );
}

#[test]
fn target_names_parse_back_and_unknown_names_are_none() {
    for target in [Target::RetailServer, Target::DiagServer, Target::DiagClient] {
        assert_eq!(parse_target(target_name(target)), Some(target));
    }
    assert_eq!(parse_target("diag-tools"), None);
}

#[test]
fn findings_differ_by_file_line_or_message_and_keep_first_seen_order() {
    let x = "Scripts/4_World/x.c";
    let y = "Scripts/4_World/y.c";
    let results = [
        (
            Target::RetailServer,
            result(vec![
                diagnostic(y, 1, Severity::Error, "second file"),
                diagnostic(x, 1, Severity::Error, "one"),
            ]),
        ),
        (
            Target::DiagServer,
            result(vec![
                diagnostic(x, 1, Severity::Error, "two"),
                diagnostic(x, 2, Severity::Error, "one"),
                diagnostic(x, 1, Severity::Error, "one"),
            ]),
        ),
    ];
    let files = [source(x, "a\nb"), source(y, "a")];
    let disk = [PathBuf::from("/mods/x.c"), PathBuf::from("/mods/y.c")];

    let (findings, _) = merge_findings(&results, &files, &disk);

    let seen: Vec<(&str, u32, &str, usize)> = findings
        .iter()
        .map(|finding| {
            (
                finding.report.path.to_str().unwrap(),
                finding.line,
                finding.report.diagnostic.message.as_str(),
                finding.targets.len(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("/mods/y.c", 1, "second file", 1),
            ("/mods/x.c", 1, "one", 2),
            ("/mods/x.c", 1, "two", 1),
            ("/mods/x.c", 2, "one", 1),
        ]
    );
}

#[test]
fn a_related_place_only_a_later_build_gives_is_kept() {
    let file = "Scripts/4_World/x.c";
    let plain = diagnostic(file, 1, Severity::Error, "Redeclared");
    let mut with_place = plain.clone();
    with_place.related = Some((file.into(), 3));
    let results = [
        (Target::RetailServer, result(vec![plain])),
        (Target::DiagServer, result(vec![with_place])),
    ];

    let (findings, _) = merge_findings(
        &results,
        &[source(file, "a\nb\nc")],
        &[PathBuf::from("/mods/x.c")],
    );

    let related = findings[0].related.as_ref().expect("related place");
    assert_eq!(
        (related.path.to_str(), related.line),
        (Some("/mods/x.c"), 3)
    );
}

fn run_check(args: Value) -> Result<Value, CommandError> {
    let outcome = common::core().dispatch(Actor::CliSession, "script.check", args)?;
    let Outcome::Data(value) = outcome else {
        panic!("script.check returns data");
    };
    Ok(value)
}

const ALL_BUILDS: [&str; 3] = ["retail-server", "diag-server", "diag-client"];

#[test]
fn a_broken_mod_reports_the_same_error_in_every_build() {
    let vanilla = common::tmp("check-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-broken");
    let bad = common::write(
        &root,
        "Scripts/3_Game/bad.c",
        "void F()\n{\n\tint a = b;\n}\n",
    );
    let later = common::write(&root, "Scripts/4_World/later.c", "class Later\n{\n}\n");

    let value = run_check(json!({ "root": &root, "vanilla": &vanilla })).unwrap();

    assert_eq!(
        value,
        json!({
            "targets": ALL_BUILDS,
            "findings": [{
                "code": "MF-E4001",
                "severity": "error",
                "span": { "start": 11, "end": 22 },
                "message": "Can't find variable 'b'",
                "path": bad,
                "line": 3,
                "targets": ALL_BUILDS,
            }],
            "failed_modules": [{ "module": "Game", "targets": ALL_BUILDS }],
            "unchecked": [later],
            "notes": [],
            "settings_used": [],
        })
    );
}

#[test]
fn a_script_file_the_game_never_loads_is_unchecked() {
    let vanilla = common::tmp("check-stray-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-stray");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let stray = common::write(&root, "Scripts/Stray/stray.c", "void F(\n");

    let value = run_check(json!({ "root": &root, "vanilla": &vanilla })).unwrap();

    assert_eq!(value["findings"], json!([]), "{value}");
    assert_eq!(value["unchecked"], json!([stray]), "{value}");
}

#[test]
fn targets_narrow_the_builds_checked_once_each_in_the_order_given() {
    let cases: [(&[&str], &[&str]); 3] = [
        (&["diag-client"], &["diag-client"]),
        (&["diag-client", "diag-client"], &["diag-client"]),
        (
            &["diag-server", "retail-server"],
            &["diag-server", "retail-server"],
        ),
    ];
    for (given, expected) in cases {
        let vanilla = common::tmp("check-narrow-vanilla");
        common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
        let root = common::tmp("check-narrow");
        common::write(
            &root,
            "Scripts/3_Game/bad.c",
            "void F()\n{\n\tint a = b;\n}\n",
        );

        let value = run_check(json!({
            "root": &root,
            "vanilla": &vanilla,
            "targets": given,
        }))
        .unwrap();

        assert_eq!(value["targets"], json!(expected));
        assert_eq!(value["findings"][0]["targets"], json!(expected));
    }
}

#[test]
fn a_dependency_supplies_the_types_its_mod_declares() {
    let vanilla = common::tmp("check-dep-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-dep-user");
    common::write(
        &root,
        "Scripts/3_Game/use.c",
        "class User\n{\n\tDepThing m_Thing;\n}\n",
    );
    let dependency = common::tmp("check-dep-lib");
    common::write(
        &dependency,
        "Scripts/3_Game/dep.c",
        "class DepThing\n{\n}\n",
    );

    let alone = run_check(json!({ "root": &root, "vanilla": &vanilla })).unwrap();
    assert_eq!(alone["findings"][0]["message"], "Unknown type 'DepThing'");
    assert_eq!(alone["findings"][0]["line"], 3);

    let with_dependency = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "dependencies": [&dependency],
    }))
    .unwrap();
    assert_eq!(with_dependency["findings"], json!([]));
    assert_eq!(with_dependency["failed_modules"], json!([]));

    let named_again = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "dependencies": [&dependency, dependency.join("."), &root],
    }))
    .unwrap();
    assert_eq!(named_again, with_dependency);
}

#[test]
fn an_empty_dependency_folder_gives_one_note() {
    let vanilla = common::tmp("check-note-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-note-root");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let empty = common::tmp("check-note-empty");
    fs::create_dir_all(&empty).unwrap();

    let value = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "dependencies": [&empty],
    }))
    .unwrap();

    let notes = value["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0]
        .as_str()
        .unwrap()
        .starts_with("no scripts found under"));
}

#[test]
fn bad_arguments_are_refused_with_the_reason() {
    let vanilla = common::tmp("check-refuse-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-refuse-root");
    let file = common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let missing = common::tmp("check-refuse-missing");
    let not_a_folder =
        |argument: &str, path: &Path| format!("{argument} {} is not a folder", path.display());
    let cases = [
        (
            json!({ "root": &file, "vanilla": &vanilla }),
            not_a_folder("root", &file),
        ),
        (
            json!({ "root": &root, "vanilla": &missing }),
            not_a_folder("vanilla", &missing),
        ),
        (
            json!({ "root": &root, "vanilla": &file }),
            not_a_folder("vanilla", &file),
        ),
        (
            json!({ "root": &root, "vanilla": &vanilla, "dependencies": [&missing] }),
            not_a_folder("dependency", &missing),
        ),
        (
            json!({ "root": &root, "vanilla": &vanilla, "dependencies": [&file] }),
            not_a_folder("dependency", &file),
        ),
        (
            json!({ "root": &root, "vanilla": &vanilla, "targets": ["retail-client"] }),
            "unknown target retail-client: use retail-server, diag-server or diag-client"
                .to_owned(),
        ),
    ];
    for (args, expected) in cases {
        match run_check(args) {
            Err(CommandError::InvalidArguments(message)) => assert_eq!(message, expected),
            other => panic!("expected the refusal {expected:?}, got {other:?}"),
        }
    }
}

#[test]
fn a_game_folder_supplies_the_vanilla_scripts_from_its_packed_copy() {
    let game = common::tmp("check-game");
    common::pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("1_Core\\base.c", b"class PrBase\n{\n}\n")],
    );
    let cache = common::tmp("check-game-cache");
    let root = common::tmp("check-game-broken");
    let bad = common::write(
        &root,
        "Scripts/3_Game/bad.c",
        "void F()\n{\n\tint a = b;\n}\n",
    );

    let value = run_check(json!({ "root": &root, "game": &game, "cache": &cache })).unwrap();

    assert_eq!(value["findings"][0]["message"], "Can't find variable 'b'");
    assert_eq!(value["findings"][0]["path"], json!(bad));
    assert_eq!(value["findings"][0]["line"], 3);
    assert_eq!(value["findings"][0]["targets"], json!(ALL_BUILDS));
}

#[test]
fn a_packed_dependency_supplies_the_types_its_mod_declares() {
    let vanilla = common::tmp("check-packed-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-packed-cache");
    let root = common::tmp("check-packed-user");
    common::write(
        &root,
        "Scripts/3_Game/use.c",
        "class User\n{\n\tDepThing m_Thing;\n}\n",
    );
    let dependency = common::tmp("check-packed-lib");
    common::pack(
        &dependency.join("addons/dep.pbo"),
        &[("prefix", "Dep")],
        &[("Scripts\\3_Game\\dep.c", b"class DepThing\n{\n}\n")],
    );

    let alone = run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })).unwrap();
    assert_eq!(alone["findings"][0]["message"], "Unknown type 'DepThing'");

    let with_dependency = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "cache": &cache,
        "dependencies": [&dependency],
    }))
    .unwrap();
    assert_eq!(with_dependency["findings"], json!([]));
    assert_eq!(with_dependency["failed_modules"], json!([]));
}

#[test]
fn vanilla_is_used_instead_of_a_game_folder() {
    let vanilla = common::tmp("check-prefer-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-prefer-cache");
    let root = common::tmp("check-prefer-root");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let missing_game = common::tmp("check-prefer-missing-game");

    let value = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "game": &missing_game,
        "cache": &cache,
    }))
    .unwrap();

    assert_eq!(value["findings"], json!([]));
    assert_eq!(value["notes"], json!([]));
}

#[test]
fn a_game_without_usable_scripts_is_refused_with_the_reason() {
    let cache = common::tmp("check-refuse-game-cache");
    let root = common::tmp("check-refuse-game-root");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let empty_game = common::tmp("check-refuse-empty-game");
    fs::create_dir_all(&empty_game).unwrap();
    let no_core_game = common::tmp("check-refuse-no-core-game");
    common::pack(
        &no_core_game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("readme.txt", b"no scripts here")],
    );

    let missing_pbo = format!(
        "no vanilla scripts at {}: pass game or vanilla",
        empty_game.join("dta/scripts.pbo").display()
    );
    match run_check(json!({ "root": &root, "game": &empty_game, "cache": &cache })) {
        Err(CommandError::Failed(message)) => assert_eq!(message, missing_pbo),
        other => panic!("expected the refusal {missing_pbo:?}, got {other:?}"),
    }

    let no_core = format!(
        "{} holds no 1_Core folder",
        no_core_game.join("dta/scripts.pbo").display()
    );
    match run_check(json!({ "root": &root, "game": &no_core_game, "cache": &cache })) {
        Err(CommandError::Failed(message)) => assert_eq!(message, no_core),
        other => panic!("expected the refusal {no_core:?}, got {other:?}"),
    }
}

#[test]
fn a_packed_dependency_pbo_that_will_not_open_is_a_note() {
    let vanilla = common::tmp("check-bad-pbo-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-bad-pbo-cache");
    let root = common::tmp("check-bad-pbo-user");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let dependency = common::tmp("check-bad-pbo-lib");
    common::pack(
        &dependency.join("addons/dep.pbo"),
        &[("prefix", "Dep")],
        &[("Scripts\\3_Game\\dep.c", b"class DepThing\n{\n}\n")],
    );
    fs::write(dependency.join("addons/bad.pbo"), b"not a pbo at all").unwrap();

    let value = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "cache": &cache,
        "dependencies": [&dependency],
    }))
    .unwrap();

    let notes = value["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    let note = notes[0].as_str().unwrap();
    assert!(
        note.starts_with(&format!("{}: ", dependency.display())),
        "{note}"
    );
    assert!(note.contains("bad.pbo"), "{note}");
}

#[test]
fn a_packed_mod_is_checked_from_its_unpacked_copy() {
    let vanilla = common::tmp("check-packed-root-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-packed-root-cache");
    let root = common::tmp("check-packed-root-mod");
    common::pack(
        &root.join("addons/mod.pbo"),
        &[("prefix", "Mod")],
        &[("Scripts\\3_Game\\bad.c", b"void F()\n{\n\tint a = b;\n}\n")],
    );

    let value = run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })).unwrap();

    assert_eq!(value["findings"][0]["message"], "Can't find variable 'b'");
    let path = PathBuf::from(value["findings"][0]["path"].as_str().unwrap());
    assert!(path.starts_with(&cache), "{}", path.display());
    assert!(path.ends_with("mod/Mod/Scripts/3_Game/bad.c"));
    assert_eq!(value["notes"], json!([]));
}

#[test]
fn a_folder_with_sources_beside_its_pbos_says_the_pbos_were_checked() {
    let vanilla = common::tmp("check-both-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-both-cache");
    let root = common::tmp("check-both-mod");
    common::write(&root, "Scripts/3_Game/new.c", "class Newer\n{\n}\n");
    common::pack(
        &root.join("addons/mod.pbo"),
        &[("prefix", "Mod")],
        &[("Scripts\\3_Game\\old.c", b"class Older\n{\n}\n")],
    );

    let value = run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })).unwrap();

    assert_eq!(
        value["notes"],
        json!([format!(
            "{}: checked from its PBOs, not from the scripts beside them",
            root.display()
        )])
    );
}

#[test]
fn a_game_with_no_scripts_is_refused_before_anything_is_unpacked() {
    let cache = common::tmp("check-order-cache");
    let root = common::tmp("check-order-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let dependency = common::tmp("check-order-lib");
    common::pack(
        &dependency.join("addons/dep.pbo"),
        &[("prefix", "Dep")],
        &[("Scripts\\3_Game\\dep.c", b"class DepThing\n{\n}\n")],
    );
    let game = common::tmp("check-order-game");

    let outcome = run_check(json!({
        "root": &root,
        "game": &game,
        "cache": &cache,
        "dependencies": [&dependency],
    }));

    assert!(
        matches!(outcome, Err(CommandError::Failed(_))),
        "{outcome:?}"
    );
    assert!(!cache.exists());
}

#[test]
fn a_scripts_pbo_whose_folders_are_spelled_differently_is_refused() {
    let cache = common::tmp("check-case-cache");
    let root = common::tmp("check-case-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let game = common::tmp("check-case-game");
    let pbo = game.join("dta/scripts.pbo");
    common::pack(
        &pbo,
        &[("prefix", "scripts")],
        &[("1_core\\base.c", b"class PrBase\n{\n}\n")],
    );

    match run_check(json!({ "root": &root, "game": &game, "cache": &cache })) {
        Err(CommandError::Failed(message)) => {
            assert_eq!(message, format!("{} holds no 1_Core folder", pbo.display()));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_cache_that_cannot_hold_the_copies_is_refused() {
    let vanilla = common::tmp("check-cache-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-cache-mod");
    let file = common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let dependency = common::tmp("check-cache-lib");
    common::write(&dependency, "Scripts/3_Game/dep.c", "class Dep\n{\n}\n");
    let inside = |cache: &Path, folder: &Path| {
        format!(
            "cache {} is inside {}: its copies would be read as part of that mod",
            cache.display(),
            folder.display()
        )
    };
    let cases = [
        (
            file.clone(),
            format!("cache {} is not a folder", file.display()),
        ),
        (root.join("cache"), inside(&root.join("cache"), &root)),
        (
            dependency.join("deep/cache"),
            inside(&dependency.join("deep/cache"), &dependency),
        ),
    ];
    for (cache, expected) in cases {
        let outcome = run_check(json!({
            "root": &root,
            "vanilla": &vanilla,
            "cache": &cache,
            "dependencies": [&dependency],
        }));
        match outcome {
            Err(CommandError::InvalidArguments(message)) => assert_eq!(message, expected),
            other => panic!("expected the refusal {expected:?}, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_cache_reached_through_a_link_into_the_mod_is_refused() {
    let vanilla = common::tmp("check-cache-link-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-cache-link-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let outside = common::tmp("check-cache-link-outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&root, outside.join("into-mod")).unwrap();
    let cache = outside.join("into-mod/deep/cache");

    match run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })) {
        Err(CommandError::InvalidArguments(message)) => assert_eq!(
            message,
            format!(
                "cache {} is inside {}: its copies would be read as part of that mod",
                cache.display(),
                root.display()
            )
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_cache_that_climbs_back_into_the_mod_is_refused() {
    let vanilla = common::tmp("check-cache-climb-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-cache-climb-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let beside = common::tmp("check-cache-climb-beside");
    std::fs::create_dir_all(&beside).unwrap();
    let cache = beside
        .join("missing/../..")
        .join(root.file_name().unwrap())
        .join("cache");

    match run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })) {
        Err(CommandError::InvalidArguments(message)) => {
            assert!(message.starts_with(&format!("cache {} is inside", cache.display())))
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_cache_behind_a_link_in_the_mod_that_leads_out_is_refused() {
    let vanilla = common::tmp("check-cache-out-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-cache-out-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let elsewhere = common::tmp("check-cache-out-elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("out")).unwrap();
    let cache = root.join("out/cache");

    match run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })) {
        Err(CommandError::InvalidArguments(message)) => {
            assert!(message.starts_with(&format!("cache {} is inside", cache.display())))
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_cache_that_climbs_out_of_the_mod_is_accepted() {
    let vanilla = common::tmp("check-cache-out-of-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let root = common::tmp("check-cache-out-of-mod");
    common::write(&root, "Scripts/3_Game/ok.c", "class Ok\n{\n}\n");
    let beside = common::tmp("check-cache-out-of-beside");
    std::fs::create_dir_all(&beside).unwrap();
    let cache = root
        .join("missing/../..")
        .join(beside.file_name().unwrap())
        .join("cache");

    let outcome = run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache }));
    assert!(
        outcome.is_ok(),
        "expected the check to run, got {outcome:?}"
    );
}

#[test]
fn packed_sources_need_a_cache_folder() {
    let game = common::tmp("check-no-cache-game");
    common::pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("1_Core\\base.c", b"class PrBase\n{\n}\n")],
    );

    let error = unpack_sources(None, VanillaSource::Game(game), Vec::new())
        .err()
        .expect("nowhere to unpack");

    assert_eq!(
        error.to_string(),
        "no cache folder: set XDG_CACHE_HOME or HOME"
    );
}

#[test]
fn a_dependency_that_cannot_be_unpacked_is_left_out_with_a_note() {
    let vanilla = common::tmp("check-dead-dep-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-dead-dep-cache");
    let root = common::tmp("check-dead-dep-user");
    common::write(
        &root,
        "Scripts/3_Game/use.c",
        "class User\n{\n\tDepThing m_Thing;\n}\n",
    );
    let dependency = common::tmp("check-dead-dep-lib");
    fs::create_dir_all(dependency.join("addons")).unwrap();
    fs::write(dependency.join("addons/bad.pbo"), b"not a pbo at all").unwrap();

    let value = run_check(json!({
        "root": &root,
        "vanilla": &vanilla,
        "cache": &cache,
        "dependencies": [&dependency],
    }))
    .unwrap();

    assert_eq!(value["findings"][0]["message"], "Unknown type 'DepThing'");
    let notes = value["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let start = format!(
        "{}: left out, nothing to unpack from {}; 1 PBO(s) failed (first: bad.pbo: ",
        dependency.display(),
        dependency.display()
    );
    assert!(notes[0].as_str().unwrap().starts_with(&start), "{notes:?}");
}

#[test]
fn a_mod_that_cannot_be_unpacked_is_refused() {
    let vanilla = common::tmp("check-dead-root-vanilla");
    common::write(&vanilla, "1_Core/base.c", "class PrBase\n{\n}\n");
    let cache = common::tmp("check-dead-root-cache");
    let root = common::tmp("check-dead-root-mod");
    fs::create_dir_all(root.join("addons")).unwrap();
    fs::write(root.join("addons/bad.pbo"), b"not a pbo at all").unwrap();

    match run_check(json!({ "root": &root, "vanilla": &vanilla, "cache": &cache })) {
        Err(CommandError::Failed(message)) => {
            let start = format!("nothing to unpack from {}; ", root.display());
            assert!(message.starts_with(&start), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_checked_mods_files_after_the_first_refused_module_on_any_build_are_unchecked() {
    let file = |path: &str, layer: Layer, mod_index: usize| SourceFile {
        path: path.into(),
        layer,
        mod_index,
        text: String::new(),
    };
    let files = [
        file("Scripts/3_Game/a.c", Layer::Game, 2),
        file("Scripts/4_World/b.c", Layer::World, 2),
        file("Scripts/5_Mission/c.c", Layer::Mission, 2),
        file("Scripts/5_Mission/dependency.c", Layer::Mission, 1),
        file("scripts/4_world/vanilla.c", Layer::World, 0),
    ];
    let disk: Vec<PathBuf> = ["a.c", "b.c", "c.c", "dependency.c", "vanilla.c"]
        .iter()
        .map(|name| PathBuf::from("/m").join(name))
        .collect();
    let failing = |modules: Vec<Module>| CheckResult {
        diagnostics: Vec::new(),
        failed_modules: modules,
    };
    let results = [
        (Target::RetailServer, failing(vec![Module::Mission])),
        (Target::DiagServer, failing(vec![Module::Game])),
        (Target::DiagClient, failing(Vec::new())),
    ];

    assert_eq!(
        unchecked_files(&results, &files, &disk, 2),
        [PathBuf::from("/m/b.c"), PathBuf::from("/m/c.c")]
    );
    assert!(unchecked_files(
        &[(Target::DiagClient, failing(Vec::new()))],
        &files,
        &disk,
        2
    )
    .is_empty());
}
