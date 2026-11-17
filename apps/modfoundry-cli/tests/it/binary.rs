//! End-to-end CLI tests: run the real `mf` binary and assert argv → registry →
//! rendered output. The verbs are thin binders over the core registry,
//! so this is where the wiring itself is pinned —
//! including that the agent broker's policy really gates the CLI's dispatch.
//!
//! Every invocation is pointed at its own `MODFOUNDRY_DB`, so
//! the tests never read or write the host's agent database.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::layout_cli::pack;

/// A path under the temp dir, removed when it goes.
pub(crate) struct Temp(PathBuf);

impl Temp {
    pub(crate) fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp =
            Self(std::env::temp_dir().join(format!("mf-bin-{label}-{}-{n}", std::process::id())));
        temp.remove();
        temp
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Point `mf` at its database and a temp home beside it: without one, `doctor`
/// reads the host's Steam install and other verbs write to the real data dir.
pub(crate) fn mf_command(db: &Path) -> Command {
    mf_command_at(Path::new(env!("CARGO_BIN_EXE_mf")), db)
}

/// [`mf_command`] for a copy of the binary somewhere else, so a helper placed
/// beside it is the one it runs.
pub(crate) fn mf_command_at(program: &Path, db: &Path) -> Command {
    let home = db.parent().expect("database has a directory").join("home");
    // A test can point the database at a path whose parent is a file; the home
    // then stays absent, which still keeps the real one out of reach.
    let _ = std::fs::create_dir_all(&home);
    let mut cmd = Command::new(program);
    cmd.env("MODFOUNDRY_DB", db)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("MODFOUNDRY_DATA_DIR", home.join("modfoundry"));
    cmd
}

/// Run `mf` against its own database file: the CLI opens the agent database on
/// every dispatch, so tests must never touch the host's.
pub(crate) fn mf_in(db: &Path, args: &[&str]) -> Output {
    mf_command(db).args(args).output().expect("mf runs")
}

pub(crate) fn mf(args: &[&str]) -> Output {
    let db = Temp::new("db");
    mf_in(&db.join("modfoundry.sqlite"), args)
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn scaffold(dir: &Path) {
    modfoundry_commands::scaffold(dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .expect("scaffold");
}

#[test]
fn formats_lists_the_recognized_families() {
    let out = mf(&["formats"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("recognized formats:"), "got: {text}");
    assert!(text.contains("- Pbo"), "got: {text}");
    assert!(text.contains("- EnforceScript"), "got: {text}");
}

#[test]
fn publish_preview_checks_the_mod_and_reports_it_not_ready() {
    let base = Temp::new("publish-preview");
    let dir = base.join("MyMod");
    scaffold(&dir);

    let out = mf(&[
        "--preview",
        "publish",
        dir.to_str().unwrap(),
        "--profile",
        "pub",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("not ready"), "got: {text}");
    assert!(text.contains("MF-P012"), "got: {text}");
}

#[test]
fn validate_exits_zero_on_a_clean_tree_and_one_on_a_broken_config() {
    let dir = Temp::new("validate");
    scaffold(dir.path());
    let ok = mf(&["validate", dir.path().to_str().unwrap()]);
    assert!(ok.status.success(), "clean tree: {}", stdout(&ok));

    let cfg = dir.join("config.cpp");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("Scripts/4_World", "Scripts/3_Game")).unwrap();
    let bad = mf(&["validate", dir.path().to_str().unwrap()]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(stdout(&bad).contains("MF-C310"), "got: {}", stdout(&bad));
}

#[test]
fn validate_counts_only_errors_as_errors() {
    let dir = Temp::new("validate-warnings");
    std::fs::create_dir_all(dir.join("M/Scripts/4_World")).unwrap();
    let cfg = dir.join("config.cpp");
    std::fs::write(
        &cfg,
        "class CfgPatches { class M { requiredAddons[] = { \"DZ_Data\" }; }; };\n\
         class CfgMods { class M { class defs { class worldScriptModule { \
         files[] = { \"M/Scripts/4_World\" }; }; }; }; };\n\
         class A { x = 1; x = 2; };\n",
    )
    .unwrap();

    let out = mf(&["validate", cfg.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("0 error(s)"),
        "a warning is not an error: {}",
        stdout(&out)
    );
}

#[test]
fn validate_prints_where_a_finding_is() {
    let dir = Temp::new("validate-location");
    scaffold(dir.path());
    let cfg = dir.join("config.cpp");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace("Scripts/4_World", "Scripts/3_Game")).unwrap();

    let text = std::fs::read_to_string(&cfg).unwrap();
    let offset = text.rfind("\"MyMod/Scripts/3_Game\"").unwrap();
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap().chars().count() + 1;

    for target in [cfg.as_path(), dir.path()] {
        let out = mf(&["validate", target.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
        assert!(
            stdout(&out).contains(&format!(
                "[MF-C310] error {}:{line}:{column}: ",
                cfg.display()
            )),
            "got: {}",
            stdout(&out)
        );
    }
}

#[test]
fn lint_prints_the_severity_and_where_the_finding_is() {
    let dir = Temp::new("lint-location");
    std::fs::create_dir_all(dir.path()).unwrap();
    let src = dir.join("broken.c");
    std::fs::write(
        &src,
        "class Ok\n{\n};\n\nclass Broken\n{\n    void F() { x = ; }\n};\n",
    )
    .unwrap();

    let game = Temp::new("lint-location-no-game");
    let out = mf(&[
        "lint",
        src.to_str().unwrap(),
        "--game",
        game.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    let prefix = format!("] error {}:", src.display());
    let start = text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no severity and location: {text}"));
    let mut parts = text[start + prefix.len()..].splitn(3, ':');
    assert!(
        parts
            .next()
            .is_some_and(|line| line.parse::<usize>().is_ok()),
        "no line: {text}"
    );
    assert!(
        parts
            .next()
            .is_some_and(|column| column.parse::<usize>().is_ok()),
        "no column: {text}"
    );
}

#[test]
fn script_outline_prints_one_file_as_json() {
    let dir = Temp::new("script-outline");
    let src = dir.join("Scripts/3_Game/thing.c");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, "class Thing\n{\n};\n").unwrap();

    let out = mf(&["script", "outline", src.to_str().unwrap()]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let value: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON output");
    assert_eq!(value["files"].as_array().map(Vec::len), Some(1));
}

#[test]
fn script_outline_of_a_folder_with_a_broken_file_exits_1() {
    let dir = Temp::new("script-outline-errors");
    let scripts = dir.join("Scripts/3_Game");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("good.c"), "class Good\n{\n};\n").unwrap();
    std::fs::write(scripts.join("bad.c"), "class Bad\n{\n};\n}\n").unwrap();

    let out = mf(&["script", "outline", dir.path().to_str().unwrap()]);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("1 file(s) with syntax errors"),
        "{}",
        stderr(&out)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON output");
    let files = value["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    let errors_of = |name: &str| {
        let file = files.iter().find(|f| f["path"] == name).expect(name);
        file["errors"].as_array().map(Vec::len)
    };
    assert_eq!(errors_of("Scripts/3_Game/good.c"), Some(0));
    assert_eq!(errors_of("Scripts/3_Game/bad.c"), Some(1));
}

#[test]
fn script_outline_of_a_missing_path_exits_2() {
    let dir = Temp::new("script-outline-missing");
    let missing = dir.join("Scripts/3_Game/absent.c");

    let out = mf(&["script", "outline", missing.to_str().unwrap()]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("Error:"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn script_outline_exits_2_when_the_database_cannot_be_opened() {
    let dir = Temp::new("script-outline-bad-db");
    let src = dir.join("Scripts/3_Game/thing.c");
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, "class Thing\n{\n};\n").unwrap();
    let db = dir.join("database");
    std::fs::create_dir_all(&db).unwrap();

    let out = mf_in(&db, &["script", "outline", src.to_str().unwrap()]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("agent database"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn lint_reports_a_variable_named_like_a_visible_type() {
    let base = Temp::new("lint-semantic");
    let db = base.join("modfoundry.sqlite");
    let dir = base.join("MyMod");
    scaffold(&dir);
    std::fs::write(dir.join("Scripts/3_Game/foo.c"), "class Foo\n{\n}\n").unwrap();
    let module = dir.join("Scripts/5_Mission");
    let bar = module.join("bar.c");
    std::fs::write(
        &bar,
        "class Zed\n{\n    Unknown u;\n\n    void Run(int Foo)\n    {\n    }\n}\n",
    )
    .unwrap();
    let game = Temp::new("lint-semantic-no-game");
    let dir_arg = dir.to_str().unwrap();
    let bar_arg = bar.to_str().unwrap();
    let game_arg = game.path().to_str().unwrap();

    let from_the_mod = mf_command(&db)
        .args(["lint", ".", "--game", game_arg])
        .current_dir(&dir)
        .output()
        .expect("mf runs");
    let runs = [
        ("folder", mf_in(&db, &["lint", dir_arg, "--game", game_arg])),
        ("file", mf_in(&db, &["lint", bar_arg, "--game", game_arg])),
        (
            "module folder",
            mf_in(&db, &["lint", module.to_str().unwrap(), "--game", game_arg]),
        ),
        ("current folder", from_the_mod),
    ];
    for (label, out) in runs {
        assert_eq!(out.status.code(), Some(1), "{label}: {}", stdout(&out));
        let text = stdout(&out);
        assert!(text.contains("MF-E3003"), "{label}: {text}");
        assert!(!text.contains("MF-W3002"), "{label}: {text}");
    }
}

fn fake_game(base: &Temp) -> PathBuf {
    let game = base.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        "scripts",
        &[("1_Core\\base.c", b"class PrBase\n{\n}\n")],
    );
    game
}

fn game_check_fixture(base: &Temp) -> (PathBuf, PathBuf) {
    let game = fake_game(base);
    let dir = base.join("mod");
    let bad = dir.join("Scripts/3_Game/bad.c");
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(
        &bad,
        "class Bad\n{\n\tvoid F()\n\t{\n\t\tint a = b;\n\t}\n}\n",
    )
    .unwrap();
    let world = dir.join("Scripts/4_World");
    std::fs::create_dir_all(&world).unwrap();
    std::fs::write(world.join("good.c"), "class Good\n{\n}\n").unwrap();
    (game, dir)
}

fn missing_return_fixture(base: &Temp) -> (PathBuf, PathBuf) {
    let game = fake_game(base);
    let dir = base.join("mod");
    let file = dir.join("Scripts/4_World/b.c");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        "class Helper\n{\n}\n\nclass B\n{\n\tint F()\n\t{\n\t\tint a = 1;\n\t\tif (a > 0)\n\t\t\treturn 1;\n\t}\n\n\tvoid Helper()\n\t{\n\t}\n\n\tvoid G()\n\t{\n\t\tHelper();\n\t}\n}\n",
    )
    .unwrap();
    (game, dir)
}

#[test]
fn lint_prints_the_game_findings_for_the_path_given() {
    let base = Temp::new("lint-game-findings");
    let (game, dir) = game_check_fixture(&base);
    let world = dir.join("Scripts/4_World");
    let game_arg = game.to_str().unwrap();
    let refused = "game check: module Game refused on retail-server, diag-server, diag-client";

    let whole = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game_arg,
        "--no-dependencies",
    ]);
    assert_eq!(whole.status.code(), Some(1), "{}", stdout(&whole));
    let text = stdout(&whole);
    let finding_line = text.lines().any(|line| {
        line.starts_with("[MF-E4001] error")
            && line.contains("bad.c:5:")
            && line.contains("Can't find variable 'b'")
    });
    assert!(finding_line, "{text}");
    assert!(text.contains(refused), "{text}");

    let only_world = mf(&[
        "lint",
        world.to_str().unwrap(),
        "--game",
        game_arg,
        "--no-dependencies",
    ]);
    assert_eq!(only_world.status.code(), Some(0), "{}", stdout(&only_world));
    let text = stdout(&only_world);
    assert!(!text.contains("MF-E4001"), "{text}");
    assert!(text.contains(refused), "{text}");
}

#[test]
fn lint_prints_the_game_findings_of_a_packed_mod() {
    let base = Temp::new("lint-game-packed");
    let game = fake_game(&base);
    let packed = base.join("packed");
    pack(
        &packed.join("addons/mod.pbo"),
        "mod",
        &[(
            "Scripts\\3_Game\\bad.c",
            b"class Bad\n{\n\tvoid F()\n\t{\n\t\tint a = b;\n\t}\n}\n",
        )],
    );

    let out = mf(&[
        "lint",
        packed.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
        "--no-dependencies",
    ]);

    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    let finding_line = text
        .lines()
        .any(|line| line.starts_with("[MF-E4001] error") && line.contains("bad.c:5:"));
    assert!(finding_line, "{text}");
}

#[test]
fn lint_checks_only_the_game_builds_given() {
    let base = Temp::new("lint-game-target");
    let (game, dir) = game_check_fixture(&base);

    let out = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
        "--no-dependencies",
        "--target",
        "diag-client",
    ]);

    let text = stdout(&out);
    assert!(
        text.contains("game check: module Game refused on diag-client\n"),
        "{}{text}",
        stderr(&out)
    );
}

#[test]
fn lint_prints_game_findings_under_the_path_given() {
    let base = Temp::new("lint-game-relative");
    let (game, _) = game_check_fixture(&base);
    let db = base.join("modfoundry.sqlite");

    let out = mf_command(&db)
        .args([
            "lint",
            "mod",
            "--game",
            game.to_str().unwrap(),
            "--no-dependencies",
        ])
        .current_dir(base.path())
        .output()
        .expect("mf runs");

    let text = stdout(&out);
    let shown = text
        .lines()
        .any(|line| line.starts_with("[MF-E4001] error mod/Scripts/3_Game/bad.c:5:"));
    assert!(shown, "{text}");
}

#[test]
fn lint_skips_the_game_check_without_known_dependencies() {
    let base = Temp::new("lint-game-skipped");
    let (game, dir) = game_check_fixture(&base);

    let out = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    let text = stdout(&out);
    assert!(
        text.contains("note: game check skipped: the mod's dependencies are not known"),
        "{text}"
    );
    assert!(!text.contains("MF-E4001"), "{text}");
}

#[test]
fn lint_leaves_out_the_guesses_the_game_check_covers() {
    let base = Temp::new("lint-game-covered");
    let (game, dir) = missing_return_fixture(&base);

    let out = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
        "--no-dependencies",
    ]);

    let text = stdout(&out);
    assert!(!text.contains("[MF-E1003]"), "{text}");
    assert!(text.contains("No return statement"), "{text}");
    assert!(text.contains("[MF-E3004]"), "{text}");
}

const MISSING_RETURN: &str =
    "class Good\n{\n\tint F()\n\t{\n\t\tint a = 1;\n\t\tif (a > 0)\n\t\t\treturn 1;\n\t}\n}\n";

#[test]
fn lint_keeps_its_guesses_in_files_the_game_never_loads() {
    let base = Temp::new("lint-game-unloaded-guesses");
    let game = fake_game(&base);
    let source = base.join("mod");
    let stray = source.join("Scripts/Stray/good.c");
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, MISSING_RETURN).unwrap();
    let packed = base.join("packed");
    pack(
        &packed.join("addons/mod.pbo"),
        "mod",
        &[("Scripts\\3_Game\\ok.c", b"class Ok\n{\n}\n")],
    );
    let loose = packed.join("Scripts/4_World/good.c");
    std::fs::create_dir_all(loose.parent().unwrap()).unwrap();
    std::fs::write(&loose, MISSING_RETURN).unwrap();

    for dir in [&source, &packed] {
        let out = mf(&[
            "lint",
            dir.to_str().unwrap(),
            "--game",
            game.to_str().unwrap(),
            "--no-dependencies",
        ]);

        let text = stdout(&out);
        assert!(text.contains("[MF-E1003]"), "{}: {text}", dir.display());
    }
}

#[test]
fn lint_keeps_its_guesses_when_the_game_refuses_a_module() {
    let base = Temp::new("lint-game-refused-guesses");
    let (game, dir) = game_check_fixture(&base);
    std::fs::write(
        dir.join("Scripts/4_World/good.c"),
        "class Good\n{\n\tint F()\n\t{\n\t\tint a = 1;\n\t\tif (a > 0)\n\t\t\treturn 1;\n\t}\n}\n",
    )
    .unwrap();

    let out = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
        "--no-dependencies",
    ]);

    let text = stdout(&out);
    assert!(text.contains("module Game refused"), "{text}");
    assert!(text.contains("[MF-E1003]"), "{text}");
}

#[test]
fn lint_keeps_its_guesses_when_the_game_check_is_skipped() {
    let base = Temp::new("lint-game-skipped-guesses");
    let (game, dir) = missing_return_fixture(&base);

    let out = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
    ]);

    let text = stdout(&out);
    assert!(text.contains("note: game check skipped"), "{text}");
    assert!(text.contains("[MF-E1003]"), "{text}");
}

#[test]
fn lint_takes_a_dependency_folder() {
    let base = Temp::new("lint-dependency");
    let dep = base.join("dep");
    let dep_file = dep.join("Scripts/3_Game/dep.c");
    std::fs::create_dir_all(dep_file.parent().unwrap()).unwrap();
    std::fs::write(&dep_file, "class DepThing\n{\n}\n").unwrap();
    let dir = base.join("mod");
    let use_c = dir.join("Scripts/4_World/use.c");
    std::fs::create_dir_all(use_c.parent().unwrap()).unwrap();
    std::fs::write(&use_c, "class Use\n{\n\tDepThing d;\n}\n").unwrap();
    let game = fake_game(&base);
    let game_arg = game.to_str().unwrap();
    let dep_arg = dep.to_str().unwrap();

    let with_dependency = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game_arg,
        "--dependency",
        dep_arg,
    ]);
    assert_eq!(
        with_dependency.status.code(),
        Some(0),
        "{}",
        stdout(&with_dependency)
    );
    let text = stdout(&with_dependency);
    assert!(!text.contains("MF-E4001"), "{text}");
    assert!(!text.contains("unknown type `DepThing`"), "{text}");

    let without = mf(&[
        "lint",
        dir.to_str().unwrap(),
        "--game",
        game_arg,
        "--no-dependencies",
    ]);
    assert!(
        stdout(&without).contains("Unknown type 'DepThing'"),
        "{}",
        stdout(&without)
    );
}

#[test]
fn lint_refuses_a_dependency_folder_with_no_dependencies() {
    let base = Temp::new("lint-dependency-conflict");
    let game = base.join("game");
    let out = mf(&[
        "lint",
        base.join("mod").to_str().unwrap(),
        "--game",
        game.to_str().unwrap(),
        "--dependency",
        base.join("dep").to_str().unwrap(),
        "--no-dependencies",
    ]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn derapify_prints_the_config_text_to_stdout() {
    let dir = Temp::new("derapify");
    let base = Temp::new("derapify-db");
    let db = base.join("modfoundry.sqlite");
    scaffold(dir.path());
    let bin = dir.join("config.bin");
    let built = mf_in(
        &db,
        &[
            "config-bin",
            dir.join("config.cpp").to_str().unwrap(),
            "--out",
            bin.to_str().unwrap(),
        ],
    );
    assert!(built.status.success(), "{}", stdout(&built));

    let out = mf_in(&db, &["derapify", bin.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("class CfgPatches"),
        "got: {}",
        stdout(&out)
    );

    // stdout must be exactly what `--out` writes (no extra blank line).
    let file = dir.join("roundtrip.cpp");
    let written = mf_in(
        &db,
        &[
            "derapify",
            bin.to_str().unwrap(),
            "--out",
            file.to_str().unwrap(),
        ],
    );
    assert!(written.status.success(), "{}", stdout(&written));
    assert_eq!(stdout(&out), std::fs::read_to_string(&file).unwrap());
}

#[test]
fn every_dispatch_lands_in_the_audit_trail() {
    let base = Temp::new("audit-db");
    let db = base.join("modfoundry.sqlite");
    assert!(mf_in(&db, &["formats"]).status.success());
    assert!(mf_in(&db, &["formats"]).status.success());

    let out = mf_in(&db, &["agent", "audit"]);
    assert!(out.status.success(), "{}", stdout(&out));
    let text = stdout(&out);
    assert_eq!(text.matches("formats.list").count(), 2, "got: {text}");
    assert!(text.contains("cli"), "the actor is recorded: {text}");
    assert!(text.contains("ok"), "the outcome is recorded: {text}");

    // The trail can be narrowed to one actor, and to a row limit.
    let filtered = mf_in(&db, &["agent", "audit", "--actor", "agent:0"]);
    assert!(stdout(&filtered).contains("no operations audited"));
    let limited = mf_in(&db, &["agent", "audit", "--limit", "1"]);
    assert_eq!(stdout(&limited).matches("formats.list").count(), 1);
}

#[test]
fn a_deny_rule_refuses_the_command_and_records_the_refusal() {
    let base = Temp::new("deny-db");
    let db = base.join("modfoundry.sqlite");
    let granted = mf_in(
        &db,
        &[
            "agent",
            "grant",
            "cli",
            "formats.list",
            "--decision",
            "deny",
        ],
    );
    assert!(granted.status.success(), "{}", stdout(&granted));

    let denied = mf_in(&db, &["formats"]);
    assert!(!denied.status.success(), "the policy must refuse the call");
    assert!(
        stderr(&denied).contains("permission policy"),
        "got: {}",
        stderr(&denied)
    );

    let audit = stdout(&mf_in(&db, &["agent", "audit"]));
    assert!(audit.contains("denied"), "got: {audit}");
    assert!(
        !audit.contains("ok"),
        "the refused call did not run: {audit}"
    );

    // Revoking puts the command back within reach.
    let revoked = mf_in(&db, &["agent", "revoke", "cli", "formats.list"]);
    assert!(revoked.status.success(), "{}", stdout(&revoked));
    assert!(mf_in(&db, &["formats"]).status.success());

    let again = mf_in(&db, &["agent", "revoke", "cli", "formats.list"]);
    assert_eq!(
        again.status.code(),
        Some(1),
        "a second revoke matches nothing"
    );
}

#[test]
fn an_unknown_effect_class_is_rejected() {
    let base = Temp::new("effect-db");
    let db = base.join("modfoundry.sqlite");
    let out = mf_in(
        &db,
        &["agent", "grant", "agent:0", "build.run", "--effect", "nope"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown effect class"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn a_rule_that_could_never_match_is_rejected() {
    let base = Temp::new("selector-db");
    let db = base.join("modfoundry.sqlite");
    for (actor, command) in [("agent0", "formats.list"), ("agent:0", "formats.lst")] {
        let out = mf_in(&db, &["agent", "grant", actor, command]);
        assert_eq!(out.status.code(), Some(1), "{actor} / {command}");
        assert!(
            stderr(&out).contains("unknown"),
            "{actor} / {command}: {}",
            stderr(&out)
        );
    }
    // The wildcards are still accepted.
    let ok = mf_in(&db, &["agent", "grant", "*", "*"]);
    assert!(ok.status.success(), "{}", stderr(&ok));
}

#[test]
fn an_unusable_default_database_degrades_instead_of_failing_the_verb() {
    let blocker = Temp::new("blocked-data-dir");
    std::fs::write(blocker.path(), "a file, not a directory").unwrap();

    // Both overrides are cleared: `app_data_dir` reads MODFOUNDRY_DATA_DIR
    // before XDG_DATA_HOME, so an ambient value would silently replace the
    // unusable directory this test depends on.
    let base = Temp::new("blocked-db");
    let out = mf_command(&base.join("modfoundry.sqlite"))
        .arg("formats")
        .env_remove("MODFOUNDRY_DB")
        .env_remove("MODFOUNDRY_DATA_DIR")
        .env("XDG_DATA_HOME", blocker.path())
        .output()
        .expect("mf runs");

    assert!(
        out.status.success(),
        "a read-only verb must survive an unusable state dir: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("session-only"),
        "the degradation must be visible: {}",
        stderr(&out)
    );
}

#[test]
fn preview_shows_the_change_set_writes_nothing_and_is_audited() {
    let base = Temp::new("preview-db");
    let db = base.join("modfoundry.sqlite");
    let dir = Temp::new("preview-mod");

    let out = mf_in(
        &db,
        &[
            "new",
            "Previewed",
            "--dir",
            dir.path().to_str().unwrap(),
            "--preview",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("would change"), "got: {text}");
    assert!(text.contains("config.cpp"), "got: {text}");
    assert!(!dir.path().exists(), "a preview writes nothing");

    let audit = mf_in(&db, &["agent", "audit"]);
    assert!(
        stdout(&audit).contains("preview"),
        "the preview is on the trail: {}",
        stdout(&audit)
    );
}

#[test]
fn undo_rolls_back_the_last_mutating_verb() {
    let base = Temp::new("undo-db");
    let db = base.join("modfoundry.sqlite");
    let dir = Temp::new("undo-mod");

    let created = mf_in(
        &db,
        &["new", "Undoable", "--dir", dir.path().to_str().unwrap()],
    );
    assert!(created.status.success(), "{}", stderr(&created));
    assert!(dir.join("config.cpp").is_file());

    let undone = mf_in(&db, &["undo"]);
    assert!(undone.status.success(), "{}", stderr(&undone));
    assert!(
        stdout(&undone).contains("undid"),
        "got: {}",
        stdout(&undone)
    );
    assert!(!dir.join("config.cpp").exists(), "the scaffold is gone");
    assert!(
        !dir.join("Scripts").exists(),
        "directories the scaffold created are pruned: {:?}",
        std::fs::read_dir(dir.path()).map(|d| d.count())
    );

    // The stack only ever walks backwards.
    let again = mf_in(&db, &["undo"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("nothing to undo"),
        "got: {}",
        stdout(&again)
    );
}

#[test]
fn a_named_database_that_cannot_be_opened_is_an_error() {
    let blocker = Temp::new("named-db-blocked");
    std::fs::write(blocker.path(), "a file, not a directory").unwrap();

    let out = mf_in(&blocker.join("agent.sqlite"), &["formats"]);

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("agent database"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn errors_maps_a_logged_script_failure_and_exits_one() {
    let dir = Temp::new("errors");
    let logs = dir.join("profiles");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("DayZServer_x64.RPT"),
        " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'x'\r\n",
    )
    .unwrap();
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("scripts/3_game")).unwrap();
    std::fs::write(src.join("scripts/3_game/broken.c"), "class Broken {}\n").unwrap();

    let out = mf(&[
        "errors",
        logs.to_str().unwrap(),
        "--source-root",
        src.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("broken.c:5"), "got: {text}");
    assert!(text.contains("Can't find variable 'x'"), "got: {text}");
    assert!(
        text.contains("(id "),
        "the line names the error's id: {text}"
    );
}

#[test]
fn doctor_reports_the_host() {
    let out = mf(&["doctor"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("host platform:"), "got: {text}");
    assert!(text.contains("steam libraries found: 0"), "got: {text}");
    assert!(text.contains("dayz game: NOT FOUND"), "got: {text}");
    assert!(text.contains("gpu helper:"), "got: {text}");
    assert!(text.contains("cuda driver (libcuda.so.1):"), "got: {text}");
    assert!(text.contains("cuda libraries folder:"), "got: {text}");
    assert!(text.contains("cuda libraries:"), "got: {text}");
    assert!(text.contains("gpu device:"), "got: {text}");
}

#[test]
fn undo_within_removes_only_the_named_folders_scaffold() {
    let base = Temp::new("within-db");
    let db = base.join("modfoundry.sqlite");
    let a = Temp::new("within-a");
    let b = Temp::new("within-b");
    let a_arg = a.path().to_str().unwrap();
    let b_arg = b.path().to_str().unwrap();
    assert!(
        mf_in(&db, &["new", "A", "--dir", a_arg]).status.success(),
        "scaffold A"
    );
    assert!(
        mf_in(&db, &["new", "B", "--dir", b_arg]).status.success(),
        "scaffold B"
    );

    let undone = mf_in(&db, &["undo", "--within", a_arg]);
    assert!(undone.status.success(), "{}", stderr(&undone));
    assert!(
        stdout(&undone).contains("undid"),
        "got: {}",
        stdout(&undone)
    );
    assert!(!a.join("config.cpp").exists(), "A's scaffold is gone");
    assert!(b.join("config.cpp").is_file(), "B's scaffold is intact");

    let again = mf_in(&db, &["undo", "--within", a_arg]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(
        stdout(&again).contains("nothing to undo under"),
        "got: {}",
        stdout(&again)
    );
    assert!(b.join("config.cpp").is_file());

    let last = mf_in(&db, &["undo"]);
    assert!(last.status.success(), "{}", stderr(&last));
    assert!(!b.join("config.cpp").exists(), "B's scaffold is now gone");
}

#[test]
fn undo_within_takes_a_relative_path_from_the_working_directory() {
    let base = Temp::new("within-rel-db");
    let db = base.join("modfoundry.sqlite");
    let a = Temp::new("within-rel-a");
    assert!(
        mf_in(&db, &["new", "Rel", "--dir", a.path().to_str().unwrap()])
            .status
            .success(),
        "scaffold"
    );
    let parent = a.path().parent().unwrap();
    let name = a.path().file_name().unwrap().to_str().unwrap();

    let out = mf_command(&db)
        .args(["undo", "--within", name])
        .current_dir(parent)
        .output()
        .expect("mf runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!a.join("config.cpp").exists(), "the scaffold is gone");
}

#[test]
fn undo_refuses_a_preview_with_or_without_within() {
    for (label, within) in [("bare", false), ("within", true)] {
        let base = Temp::new(&format!("undo-preview-{label}-db"));
        let db = base.join("modfoundry.sqlite");
        let dir = Temp::new(&format!("undo-preview-{label}-mod"));
        let dir_arg = dir.path().to_str().unwrap();
        let created = mf_in(&db, &["new", "Keep", "--dir", dir_arg]);
        assert!(created.status.success(), "{label}: {}", stderr(&created));

        let mut args = vec!["undo"];
        if within {
            args.extend(["--within", dir_arg]);
        }
        args.push("--preview");
        let out = mf_in(&db, &args);
        assert_eq!(out.status.code(), Some(1), "{label}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("cannot be previewed"),
            "{label}: {}",
            stderr(&out)
        );
        assert!(
            dir.join("config.cpp").is_file(),
            "{label}: a preview must never apply the undo it was asked about"
        );
    }
}

#[test]
fn agent_refuses_a_preview_instead_of_changing_the_policy() {
    let base = Temp::new("agent-preview-db");
    let db = base.join("modfoundry.sqlite");

    let out = mf_in(
        &db,
        &[
            "--preview",
            "agent",
            "grant",
            "cli",
            "formats.list",
            "--decision",
            "deny",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("cannot be previewed"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        mf_in(&db, &["formats"]).status.success(),
        "a preview must never write the rule it was asked about"
    );
}

#[test]
fn new_prints_a_build_line_naming_the_mod_when_the_folder_differs() {
    let base = Temp::new("hint-db");
    let db = base.join("modfoundry.sqlite");
    for (folder, wanted, unwanted) in [
        ("work", " --name Hinted`", None),
        ("Hinted", "`", Some("--name")),
        ("@Hinted", "`", Some("--name")),
    ] {
        let dir = base.join(folder);
        let dir_arg = dir.to_str().unwrap();
        let out = mf_in(&db, &["new", "Hinted", "--dir", dir_arg]);
        assert!(out.status.success(), "{}", stderr(&out));
        let text = stdout(&out);
        assert!(
            text.contains(&format!("mf build {dir_arg}{wanted}")),
            "{folder}: {text}"
        );
        if let Some(unwanted) = unwanted {
            assert!(!text.contains(unwanted), "{folder}: {text}");
        }
    }
}

#[test]
fn build_list_prints_each_file_with_its_decision_then_a_summary_and_warnings() {
    let base = Temp::new("build-list");
    let dir = base.join("MyMod");
    for (rel, text) in [
        ("config.cpp", "class CfgPatches {}"),
        ("Scripts/3_Game/a.c", "class A {}"),
        ("docs/a.c", "class D {}"),
        ("AGENTS.md", "# notes"),
        (".pboignore", "!AGENTS.md\n"),
    ] {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    let out = mf(&["build", dir.to_str().unwrap(), "--list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let line = |mark: char, size: u64, path: &str, reason: &str| {
        format!("{mark} {size:>8}  {path}  [{reason}]")
    };
    let expected = [
        line('+', 7, "AGENTS.md", ".pboignore line 1: AGENTS.md"),
        line('+', 10, "Scripts/3_Game/a.c", "game file"),
        line('+', 19, "config.cpp", "game file"),
        line('-', 10, "docs/a.c", "skipped folder"),
        "3 of 4 files packed, 36 bytes".to_owned(),
        "warning: AGENTS.md is packed by .pboignore line 1 but the game does not load .md files"
            .to_owned(),
    ];
    assert_eq!(stdout(&out).lines().collect::<Vec<_>>(), expected);
    assert!(!dir.join("builds").exists(), "--list must not build");
}

#[test]
fn pbo_unpack_writes_the_files_of_a_pbo_and_prints_the_summary() {
    let base = Temp::new("pbo-unpack");
    let dir = base.join("MyMod");
    scaffold(&dir);
    let db = base.join("modfoundry.sqlite");
    let built = mf_in(&db, &["build", dir.to_str().unwrap()]);
    assert!(built.status.success(), "{}", stderr(&built));
    let pbo = dir.join("builds/default/@MyMod/addons/MyMod.pbo");
    let out_dir = base.join("unpacked");

    let out = mf_in(
        &db,
        &[
            "pbo-unpack",
            pbo.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ],
    );

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("unpacked "), "{}", stdout(&out));
    assert!(out_dir.join("config.cpp").is_file());
}

#[test]
fn build_warns_that_an_unsigned_mod_needs_a_signing_key() {
    let base = Temp::new("unsigned-warning");
    let db = base.join("modfoundry.sqlite");
    let dir = base.join("MyMod");
    scaffold(&dir);

    let out = mf_in(&db, &["build", dir.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(
            "warning: this build is unsigned and no signing key is set; a retail server refuses \
             it. Pick a key in the app's Settings › Signing, or pass --authority and --keys-dir"
        ),
        "got: {text}"
    );
}

/// A project whose set `session` holds the scaffolded local mods `Core` and `Hud`.
fn project_with_a_set(base: &Temp) -> PathBuf {
    let project = base.join("project");
    for name in ["Core", "Hud"] {
        modfoundry_commands::scaffold(
            &project.join("mods").join(name),
            &modfoundry_commands::ScaffoldOptions::new(name),
        )
        .unwrap();
    }
    let server = project.join("server");
    std::fs::create_dir_all(&server).unwrap();
    std::fs::write(
        server.join("launch-sets.json"),
        r#"{"sets": {"session": {"mods": [{"local": "mods/Core"}, {"local": "mods/Hud"}]}}}"#,
    )
    .unwrap();
    project
}

#[test]
fn build_set_preview_prints_each_mods_plan_and_builds_nothing() {
    let base = Temp::new("build-set-preview");
    let project = project_with_a_set(&base);
    let db = base.join("modfoundry.sqlite");

    let out = mf_in(
        &db,
        &[
            "--preview",
            "build",
            "--set",
            "session",
            "--source-root",
            project.to_str().unwrap(),
        ],
    );

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    for name in ["Core", "Hud"] {
        assert!(
            text.contains(&format!("building {name}\nwould change ")),
            "got: {text}"
        );
    }
    assert!(
        !project.join("mods/Core/builds").exists(),
        "a preview built"
    );
}

#[test]
fn build_set_reads_the_project_from_the_current_folder() {
    let base = Temp::new("build-set-cwd");
    let project = project_with_a_set(&base);
    let db = base.join("modfoundry.sqlite");

    let mut cmd = mf_command(&db);
    cmd.current_dir(&project)
        .args(["build", "--set", "session"]);
    let out = cmd.output().expect("mf runs");

    assert!(out.status.success(), "{}", stderr(&out));
    let built = project.join("mods/Hud/builds/default/@Hud/addons/Hud.pbo");
    assert!(built.is_file(), "{} is missing", built.display());
}

#[test]
fn build_set_prints_each_mods_summary_under_its_name() {
    let base = Temp::new("build-set");
    let project = project_with_a_set(&base);
    let db = base.join("modfoundry.sqlite");

    let out = mf_in(
        &db,
        &[
            "build",
            "--set",
            "session",
            "--source-root",
            project.to_str().unwrap(),
        ],
    );

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    fn heading(lines: &[&str], name: &str) -> usize {
        lines
            .iter()
            .position(|line| *line == format!("building {name}"))
            .unwrap_or_else(|| panic!("no `building {name}` line in {lines:?}"))
    }
    let (core, hud) = (heading(&lines, "Core"), heading(&lines, "Hud"));
    assert!(core < hud, "{text}");
    assert!(lines[core + 1].starts_with("built @Core -> "), "{text}");
    assert!(lines[hud + 1].starts_with("built @Hud -> "), "{text}");
}

#[test]
fn check_signatures_exits_zero_on_a_clean_build_and_nonzero_on_a_doubled_signature() {
    let base = Temp::new("check-signatures");
    let db = base.join("modfoundry.sqlite");
    let dir = base.join("MyMod");
    scaffold(&dir);
    let keys = base.join("keys");
    let made = mf_in(&db, &["keygen", "MF", "--dir", keys.to_str().unwrap()]);
    assert!(made.status.success(), "{}", stderr(&made));
    let built = mf_in(
        &db,
        &[
            "build",
            dir.to_str().unwrap(),
            "--authority",
            "MF",
            "--keys-dir",
            keys.to_str().unwrap(),
        ],
    );
    assert!(built.status.success(), "{}", stderr(&built));
    let build = dir.join("builds/default/@MyMod");

    let clean = mf_in(&db, &["check-signatures", build.to_str().unwrap()]);
    assert!(clean.status.success(), "{}", stderr(&clean));
    assert!(
        stdout(&clean).contains(&format!("ok: signatures match in {}", build.display())),
        "got: {}",
        stdout(&clean)
    );

    std::fs::copy(
        build.join("addons/MyMod.pbo.MF.bisign"),
        build.join("addons/MyMod.pbo.Other.bisign"),
    )
    .unwrap();
    let doubled = mf_in(&db, &["check-signatures", build.to_str().unwrap()]);
    assert_eq!(doubled.status.code(), Some(1), "{}", stdout(&doubled));
    assert!(
        stdout(&doubled).contains("MF-P008"),
        "got: {}",
        stdout(&doubled)
    );
}

#[test]
fn font_msdf_writes_one_font_pair_without_sizes() {
    let base = Temp::new("font-msdf");
    let db = base.join("modfoundry.sqlite");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/fonts/IBMPlexMono-Regular.ttf");
    let out_dir = base.join("fonts");
    std::fs::create_dir_all(&out_dir).unwrap();

    let out = mf_in(
        &db,
        &[
            "font",
            source.to_str().unwrap(),
            "--msdf",
            "--name",
            "sample_display",
            "--out",
            out_dir.to_str().unwrap(),
        ],
    );

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("wrote sample_display as one distance-field font"),
        "{}",
        stdout(&out)
    );
    assert!(out_dir.join("sample_display.fnt").is_file());
    assert!(out_dir.join("sample_display.edds").is_file());
}

#[cfg(unix)]
#[test]
fn a_launch_from_the_cli_is_recorded_under_the_cli_actor() {
    use modfoundry_agent::Store;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    let dir = Temp::new("cli-actor");
    let install = dir.join("srv");
    std::fs::create_dir_all(&install).unwrap();
    let exe = install.join("DayZServer");
    std::fs::write(
        &exe,
        "#!/bin/sh\nn=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, PermissionsExt::from_mode(0o755)).unwrap();
    let mod_dir = dir.join("mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let db = dir.join("modfoundry.sqlite");

    let mut run = mf_command(&db)
        .arg("run")
        .arg(&mod_dir)
        .arg("--server")
        .arg(&install)
        .arg("--timeout")
        .arg("30")
        .spawn()
        .expect("mf starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    let actor = loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let live = Store::open(&db)
            .ok()
            .and_then(|store| store.live_runs(now, 10).ok())
            .unwrap_or_default();
        if let Some(row) = live.first() {
            break row.actor.clone();
        }
        assert!(Instant::now() < deadline, "no live run was recorded");
        std::thread::sleep(Duration::from_millis(50));
    };
    run.kill().unwrap();
    run.wait().unwrap();
    assert_eq!(actor, "cli");
}

#[cfg(unix)]
#[test]
fn mf_status_prints_a_recorded_run() {
    use modfoundry_agent::Store;
    use modfoundry_commands::registry::Session;
    use modfoundry_core::Actor;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    let dir = Temp::new("status");
    let db = dir.join("modfoundry.sqlite");
    std::fs::create_dir_all(dir.path()).unwrap();
    let install = dir.join("srv");
    std::fs::create_dir_all(&install).unwrap();
    let profiles = dir.join("profiles");
    let release = dir.join("release");
    let exe = install.join("DayZServer");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\necho 'Player connect enabled' >> '{}/DayZServer_x64.RPT'\n\
             n=0; while [ ! -e '{}' ] && [ -e \"$0\" ] && [ $n -lt 600 ]; do sleep 0.05; n=$((n+1)); done\n",
            profiles.display(),
            release.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, PermissionsExt::from_mode(0o755)).unwrap();
    let mod_dir = dir.join("mod").join("@Mod");
    std::fs::create_dir_all(&mod_dir).unwrap();

    let core = modfoundry_commands::registry::core_with(
        &Session::new().with_db(db.clone()).with_actor("cli"),
    );
    core.dispatch(
        Actor::CliSession,
        "diag.run",
        json!({
            "mod_dir": mod_dir,
            "server_dir": install,
            "profiles_dir": profiles,
            "run_id": "r1",
            "wait": false,
        }),
    )
    .unwrap_or_else(|e| panic!("diag.run failed: {e}"));

    let out = mf_in(&db, &["status", "r1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("run r1: running\n  command: diag.run\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("  log folder: {}\n", profiles.display())),
        "{text}"
    );

    let store = Store::open(&db).unwrap();
    let started = store.run("r1").unwrap().unwrap().started_unix;
    store
        .set_run_result("r1", &json!({"errors": [], "hint": "check the world"}))
        .unwrap();
    store.end_run("r1", started + 30, "exited").unwrap();
    std::fs::write(&release, "").unwrap();

    let out = mf_in(&db, &["status", "r1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let expected = format!(
        "run r1: ended (exited)\n  command: diag.run\n  ran for 30 s\n  log folder: {}\n\
         status: 0 issue(s) (0 file-mapped)\nhint: check the world\n",
        profiles.display()
    );
    assert!(text.contains(&expected), "{text}");

    let out = mf_in(&db, &["status", "nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no run `nope` is recorded"),
        "{}",
        stderr(&out)
    );
}

#[cfg(unix)]
#[test]
fn mf_errors_on_a_folder_whose_name_is_not_utf8_does_not_panic() {
    use std::os::unix::ffi::OsStrExt;
    let dir = Temp::new("errors-non-utf8");
    let odd = dir.join("").join(std::ffi::OsStr::from_bytes(b"logs\xff"));
    std::fs::create_dir_all(&odd).unwrap();

    let out = mf_command(&dir.join("mf.db"))
        .arg("errors")
        .arg(&odd)
        .output()
        .unwrap();

    assert_ne!(out.status.code(), Some(101), "{}", stderr(&out));
}

#[test]
fn output_into_a_closed_pipe_ends_quietly_without_a_panic() {
    let db = Temp::new("db");
    let mut child = mf_command(&db.join("modfoundry.sqlite"))
        .arg("changelog")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("mf starts");
    drop(child.stdout.take());
    let out = child.wait_with_output().expect("mf ends");
    assert!(!stderr(&out).contains("panicked"), "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(141));
}

#[test]
fn a_message_into_a_closed_error_pipe_ends_with_the_closed_pipe_status() {
    let dir = Temp::new("closed-stderr");
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.join("bad.c"), "class Bad\n{\n};\n}\n").unwrap();
    let db = Temp::new("db");
    let mut child = mf_command(&db.join("modfoundry.sqlite"))
        .args(["script", "outline", dir.path().to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("mf starts");
    drop(child.stderr.take());
    assert_eq!(child.wait().expect("mf ends").code(), Some(141));
}
