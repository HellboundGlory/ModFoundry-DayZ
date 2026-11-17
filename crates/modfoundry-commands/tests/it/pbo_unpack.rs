//! `pbo.unpack` through the registry: a PBO's files land in a folder as stored, its prefix in `$PBOPREFIX$`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{self, pack, tmp};
use modfoundry_core::{Actor, CommandError, Core, Outcome};
use modfoundry_vfs::sha256_hex;
use serde_json::{json, Value};

fn dispatch(core: &Core, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, "pbo.unpack", args)
        .unwrap_or_else(|e| panic!("pbo.unpack failed: {e}"))
}

fn summary_of(outcome: &Outcome) -> String {
    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected a change: {outcome:?}");
    };
    summary.description.clone()
}

#[test]
fn entries_land_as_stored_with_their_bytes_and_the_summary_counts_them() {
    let core = common::core();
    let dir = tmp("pbo-unpack-layout");
    let pbo = dir.join("Mod.pbo");
    let config: &[u8] = b"class CfgPatches {};";
    let script: &[u8] = b"class A {};";
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", config), ("Scripts\\3_Game\\a.c", script)],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 2 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("config.cpp")).unwrap(), config);
    assert_eq!(fs::read(out.join("Scripts/3_Game/a.c")).unwrap(), script);
    assert_eq!(fs::read(out.join("$PBOPREFIX$")).unwrap(), b"My\\Mod");
}

#[test]
fn a_traversing_name_is_left_out_and_counted_and_nothing_lands_outside_the_folder() {
    let core = common::core();
    let dir = tmp("pbo-unpack-traversal");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[
            ("config.cpp", b"class A {};"),
            ("..\\evil.c", b"escape"),
            ("Scripts\\a.c", b"class B {};"),
        ],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 2 file(s) from Mod.pbo into {}; left out 1: 1 unsafe name(s)",
            out.display()
        )
    );
    let outside: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != "out" && name != "Mod.pbo")
        .collect();
    assert!(outside.is_empty(), "{outside:?}");
}

#[test]
fn a_folder_spelled_two_ways_lands_in_the_first_spelling() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folding");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("Data\\a.c", b"first"), ("data\\b.c", b"second")],
    );
    let out = dir.join("out");

    dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(fs::read(out.join("Data/a.c")).unwrap(), b"first");
    assert_eq!(fs::read(out.join("Data/b.c")).unwrap(), b"second");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 2);
}

#[test]
fn a_case_duplicate_entry_is_counted_and_left_out() {
    let core = common::core();
    let dir = tmp("pbo-unpack-case-duplicate");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("Data\\a.c", b"first"), ("DATA\\A.C", b"second")],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from Mod.pbo into {}; left out 1: 1 case duplicate(s)",
            out.display()
        )
    );
}

#[test]
fn a_file_named_like_a_folder_of_another_entry_is_left_out_and_counted() {
    let core = common::core();
    let dir = tmp("pbo-unpack-file-over-folder");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("data", b"a file"), ("data\\a.c", b"class A {};")],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from Mod.pbo into {}; left out 1: 1 unsafe name(s)",
            out.display()
        )
    );
    assert_eq!(fs::read(out.join("data/a.c")).unwrap(), b"class A {};");
}

/// Every file and folder under `root` with its bytes (none for a folder), in order.
fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
                entries.push((path, None));
            } else {
                let bytes = fs::read(&path).unwrap();
                entries.push((path, Some(bytes)));
            }
        }
    }
    entries.sort();
    entries
}

#[test]
fn an_unpack_that_cannot_proceed_is_refused_and_writes_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-refusals");
    fs::create_dir_all(&dir).unwrap();
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[
            ("config.cpp", b"class A {};"),
            ("Scripts\\a.c", b"class B {};"),
        ],
    );
    let ebo = dir.join("Mod.ebo");
    fs::copy(&pbo, &ebo).unwrap();
    let file = dir.join("not-a-folder");
    fs::write(&file, b"x").unwrap();
    let taken = dir.join("taken");
    fs::create_dir_all(&taken).unwrap();
    fs::write(taken.join("config.cpp"), b"keep me").unwrap();
    let cased = dir.join("cased");
    fs::create_dir_all(cased.join("scripts")).unwrap();
    let before = tree(&dir);

    let cases = [
        (&pbo, &taken, "already holds 1 of these files"),
        (
            &ebo,
            &dir.join("from-ebo"),
            "unsupported protected container",
        ),
        (
            &dir.join("missing.pbo"),
            &dir.join("from-missing"),
            "is not a file",
        ),
        (&pbo, &file, "is a file, not a folder"),
        (&pbo, &cased, "collides with"),
    ];
    for (path, out, expected) in cases {
        let error = core
            .dispatch(
                Actor::CliSession,
                "pbo.unpack",
                json!({ "path": path, "out": out }),
            )
            .expect_err("the unpack must be refused");
        assert!(error.to_string().contains(expected), "{error}");
    }

    assert_eq!(tree(&dir), before);
    assert_eq!(fs::read(taken.join("config.cpp")).unwrap(), b"keep me");
}

#[test]
fn a_preview_writes_nothing_and_lists_each_file_with_the_digest_of_its_content() {
    let core = common::core();
    let dir = tmp("pbo-unpack-preview");
    let pbo = dir.join("Mod.pbo");
    let config: &[u8] = b"class CfgPatches {};";
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("config.cpp", config), ("Scripts\\a.c", b"class B {};")],
    );
    let out = dir.join("out");

    let Outcome::Preview(plan) =
        dispatch(&core, json!({ "path": pbo, "out": out, "preview": true }))
    else {
        panic!("expected a preview");
    };

    assert!(!out.exists());
    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    assert!(changes
        .iter()
        .any(|change| change["path"] == "mod:out:$PBOPREFIX$" && change["op"] == "create"));
    let config_change = changes
        .iter()
        .find(|change| change["path"] == "mod:out:config.cpp")
        .expect("config.cpp is planned");
    assert_eq!(config_change["op"], "create");
    assert_eq!(config_change["after_sha256"], sha256_hex(config));
    assert!(changes
        .iter()
        .any(|change| change["path"] == "mod:out:Scripts/a.c" && change["op"] == "create"));
}

#[test]
fn undoing_an_unpack_removes_its_files_and_the_folders_it_made() {
    let core = common::core();
    let dir = tmp("pbo-unpack-undo");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[
            ("config.cpp", b"class A {};"),
            ("Scripts\\3_Game\\a.c", b"class B {};"),
        ],
    );
    let out = dir.join("out");
    let applied = dispatch(&core, json!({ "path": pbo, "out": out }));
    let change_set = applied
        .change_set()
        .expect("the unpack records its change-set")
        .clone();
    assert!(out.join("Scripts/3_Game/a.c").is_file());
    assert!(out.join("$PBOPREFIX$").is_file());

    core.dispatch(
        Actor::CliSession,
        "workspace.undo",
        json!({ "change_set": change_set }),
    )
    .unwrap_or_else(|e| panic!("workspace.undo failed: {e}"));

    assert!(!out.join("Scripts").exists());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

#[test]
fn a_pbo_with_nothing_writable_is_refused_with_what_was_left_out() {
    let core = common::core();
    let dir = tmp("pbo-unpack-nothing");
    let pbo = dir.join("Mod.pbo");
    pack(&pbo, &[("prefix", "Mod")], &[("..\\evil.c", b"escape")]);
    let out = dir.join("out");

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": pbo, "out": out }),
        )
        .expect_err("nothing can be unpacked");

    assert!(
        error
            .to_string()
            .contains("nothing to unpack from Mod.pbo; left out 1: 1 unsafe name(s)"),
        "{error}"
    );
    assert!(!out.exists());
}

/// Rewrites the packing field of the header record of `name` in a packed container.
fn set_packing(path: &Path, name: &str, packing: u32) {
    let mut bytes = fs::read(path).unwrap();
    let mut needle = name.as_bytes().to_vec();
    needle.push(0);
    let at = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("entry name in the header")
        + needle.len();
    bytes[at..at + 4].copy_from_slice(&packing.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

#[test]
fn an_entry_that_cannot_be_decoded_is_left_out_and_the_rest_are_written() {
    let core = common::core();
    let dir = tmp("pbo-unpack-undecodable");
    let pbo = dir.join("Mod.pbo");
    let config: &[u8] = b"class CfgPatches {};";
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("config.cpp", config), ("Scripts\\a.c", b"not compressed")],
    );
    set_packing(&pbo, "Scripts\\a.c", modfoundry_pbo::PACKING_CPRS);
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from Mod.pbo into {}; left out 1: 1 unreadable (first: Scripts/a.c)",
            out.display()
        )
    );
    assert_eq!(fs::read(out.join("config.cpp")).unwrap(), config);
    assert!(!out.join("Scripts/a.c").exists());
}

#[test]
fn names_too_long_for_the_filesystem_are_left_out_and_counted() {
    let core = common::core();
    let dir = tmp("pbo-unpack-long-names");
    let pbo = dir.join("Mod.pbo");
    let long_name = "z".repeat(250);
    let long_path = vec!["deep"; 600].join("\\") + "\\a.c";
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[
            ("config.cpp", b"class A {};"),
            (&long_name, b"long name"),
            (&long_path, b"long path"),
        ],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from Mod.pbo into {}; left out 2: 2 unsafe name(s)",
            out.display()
        )
    );
}

#[test]
fn a_mod_folder_unpacks_every_pbo_of_its_addons_into_its_own_folder() {
    let core = common::core();
    let dir = tmp("pbo-unpack-mod-folder");
    let mod_dir = dir.join("@Mod");
    let a: &[u8] = b"class A {};";
    let b: &[u8] = b"class B {};";
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", a)],
    );
    pack(
        &mod_dir.join("Addons/b.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b)],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": mod_dir, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 2 file(s) from 2 PBO(s) under {} into {}",
            mod_dir.display(),
            out.display()
        )
    );
    assert_eq!(fs::read(out.join("a/config.cpp")).unwrap(), a);
    assert_eq!(fs::read(out.join("a/$PBOPREFIX$")).unwrap(), b"My\\Mod");
    assert_eq!(fs::read(out.join("b/config.cpp")).unwrap(), b);
    assert_eq!(fs::read(out.join("b/$PBOPREFIX$")).unwrap(), b"Other");
}

#[test]
fn a_folder_with_pbos_directly_in_it_unpacks_them_the_same_way() {
    let core = common::core();
    let dir = tmp("pbo-unpack-direct-pbos");
    let mod_dir = dir.join("@Mod");
    let a: &[u8] = b"class A {};";
    let b: &[u8] = b"class B {};";
    pack(
        &mod_dir.join("a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", a)],
    );
    pack(
        &mod_dir.join("b.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b)],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": mod_dir, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 2 file(s) from 2 PBO(s) under {} into {}",
            mod_dir.display(),
            out.display()
        )
    );
    assert_eq!(fs::read(out.join("a/config.cpp")).unwrap(), a);
    assert_eq!(fs::read(out.join("b/config.cpp")).unwrap(), b);
}

#[test]
fn a_folder_with_no_pbo_is_refused_and_creates_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-no-pbos");
    let mod_dir = dir.join("@Mod");
    fs::create_dir_all(mod_dir.join("Scripts")).unwrap();
    let out = dir.join("out");

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": mod_dir, "out": out }),
        )
        .expect_err("a folder without PBOs is refused");

    assert!(
        matches!(&error, CommandError::InvalidArguments(message) if message.starts_with("no PBOs under ")),
        "{error}"
    );
    assert!(!out.exists());
}

#[test]
fn a_pbo_that_cannot_be_opened_is_named_and_the_good_one_still_lands() {
    let core = common::core();
    let dir = tmp("pbo-unpack-bad-pbo");
    let mod_dir = dir.join("@Mod");
    let good: &[u8] = b"class A {};";
    pack(
        &mod_dir.join("good.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", good)],
    );
    fs::write(mod_dir.join("bad.pbo"), b"not a pbo at all").unwrap();
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": mod_dir, "out": out }));

    let summary = summary_of(&outcome);
    let expected = format!(
        "unpacked 1 file(s) from 1 PBO(s) under {} into {}; 1 PBO(s) failed (first: bad.pbo: ",
        mod_dir.display(),
        out.display()
    );
    assert!(summary.starts_with(&expected), "{summary}");
    assert_eq!(fs::read(out.join("good/config.cpp")).unwrap(), good);
}

#[test]
fn a_pbo_with_nothing_writable_in_a_folder_is_counted_and_the_others_land() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-nothing");
    let mod_dir = dir.join("@Mod");
    let good: &[u8] = b"class A {};";
    pack(
        &mod_dir.join("good.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", good)],
    );
    pack(
        &mod_dir.join("empty.pbo"),
        &[("prefix", "Mod")],
        &[("..\\evil.c", b"escape")],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": mod_dir, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from 1 PBO(s) under {} into {}; left out 1: 1 unsafe name(s); 1 PBO(s) failed (first: empty.pbo: nothing to unpack)",
            mod_dir.display(),
            out.display()
        )
    );
    assert_eq!(fs::read(out.join("good/config.cpp")).unwrap(), good);
}

#[test]
fn a_file_in_the_way_of_a_later_pbo_refuses_the_whole_folder() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-clash");
    let mod_dir = dir.join("@Mod");
    // Larger than one write batch, so a check made PBO by PBO would already have written it.
    let large = vec![0u8; 65 * 1024 * 1024];
    pack(
        &mod_dir.join("a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};"), ("large.bin", &large)],
    );
    drop(large);
    pack(
        &mod_dir.join("b.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b"class B {};")],
    );
    let out = dir.join("out");
    fs::create_dir_all(out.join("b")).unwrap();
    fs::write(out.join("b/config.cpp"), b"keep me").unwrap();
    let before = tree(&dir);

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": mod_dir, "out": out }),
        )
        .expect_err("the unpack must be refused");

    assert_eq!(
        error.to_string(),
        format!(
            "operation failed: {} already holds 1 of these files (first: {}); unpack into another folder or delete them",
            out.display(),
            out.join("b/config.cpp").display()
        )
    );
    assert_eq!(tree(&dir), before);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pbos_that_would_share_a_folder_do_not_overwrite_each_other() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-twins");
    let mod_dir = dir.join("@Mod");
    for (name, body) in [("a.PBO", b"class First {};"), ("a.pbo", b"class Other {};")] {
        pack(
            &mod_dir.join(name),
            &[("prefix", "P")],
            &[("config.cpp", body)],
        );
    }
    let out = dir.join("out");

    let summary = summary_of(&dispatch(&core, json!({ "path": mod_dir, "out": out })));

    assert_eq!(
        summary,
        format!(
            "unpacked 1 file(s) from 1 PBO(s) under {} into {}; 1 PBO(s) failed (first: a.pbo: another PBO already unpacks into the folder a)",
            mod_dir.display(),
            out.display()
        )
    );
    assert_eq!(
        fs::read(out.join("a/config.cpp")).unwrap(),
        b"class First {};"
    );
}

#[test]
fn a_pbo_whose_name_cannot_be_a_folder_is_counted_and_the_others_land() {
    let core = common::core();
    for (label, bad) in [("colon", "a:b.pbo"), ("dots", "...pbo")] {
        let dir = tmp(&format!("pbo-unpack-folder-name-{label}"));
        let mod_dir = dir.join("@Mod");
        for name in [bad, "good.pbo"] {
            pack(
                &mod_dir.join(name),
                &[("prefix", "P")],
                &[("config.cpp", b"class A {};")],
            );
        }
        let out = dir.join("out");

        let summary = summary_of(&dispatch(&core, json!({ "path": mod_dir, "out": out })));

        let start = format!(
            "unpacked 1 file(s) from 1 PBO(s) under {} into {}; 1 PBO(s) failed (first: {bad}: ",
            mod_dir.display(),
            out.display()
        );
        assert!(summary.starts_with(&start), "{label}: {summary}");
        assert!(out.join("good/config.cpp").is_file(), "{label}");
    }
}

#[test]
fn a_folder_where_every_pbo_fails_is_refused_and_creates_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-all-bad");
    let mod_dir = dir.join("@Mod");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(mod_dir.join("bad.pbo"), b"not a pbo").unwrap();
    let out = dir.join("out");

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": mod_dir, "out": out }),
        )
        .expect_err("nothing could be unpacked");

    let start = format!(
        "operation failed: nothing to unpack from {}; 1 PBO(s) failed (first: bad.pbo: ",
        mod_dir.display()
    );
    assert!(error.to_string().starts_with(&start), "{error}");
    assert!(!out.exists());
}

#[test]
fn a_folder_preview_lists_every_pbo_under_its_own_folder_and_writes_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-preview");
    let mod_dir = dir.join("@Mod");
    for name in ["a", "b"] {
        pack(
            &mod_dir.join(format!("{name}.pbo")),
            &[("prefix", "P")],
            &[("config.cpp", b"class A {};")],
        );
    }
    let out = dir.join("out");

    let Outcome::Preview(plan) = dispatch(
        &core,
        json!({ "path": mod_dir, "out": out, "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    assert!(!out.exists());
    let planned: Vec<&str> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        planned,
        [
            "mod:out:a/config.cpp",
            "mod:out:a/$PBOPREFIX$",
            "mod:out:b/config.cpp",
            "mod:out:b/$PBOPREFIX$"
        ]
    );
}

#[test]
fn an_addons_folder_with_pbos_wins_over_pbos_beside_it() {
    let core = common::core();
    let dir = tmp("pbo-unpack-addons-wins");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("ADDONS/a.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", b"class A {};")],
    );
    pack(
        &mod_dir.join("x.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b"class X {};")],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": mod_dir, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!(
            "unpacked 1 file(s) from 1 PBO(s) under {} into {}",
            mod_dir.display(),
            out.display()
        )
    );
    assert!(out.join("a/config.cpp").is_file());
    assert!(!out.join("x").exists());
}

#[test]
fn undoing_a_folder_unpack_removes_every_pbo_it_wrote_in_one_step() {
    let core = common::core();
    let dir = tmp("pbo-unpack-folder-undo");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    pack(
        &mod_dir.join("Addons/b.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b"class B {};")],
    );
    let out = dir.join("out");
    let applied = dispatch(&core, json!({ "path": mod_dir, "out": out }));
    let change_set = applied
        .change_set()
        .expect("the unpack records its change-set")
        .clone();
    assert!(out.join("a/config.cpp").is_file());
    assert!(out.join("b/config.cpp").is_file());
    assert!(out.join("a/$PBOPREFIX$").is_file());

    core.dispatch(
        Actor::CliSession,
        "workspace.undo",
        json!({ "change_set": change_set }),
    )
    .unwrap_or_else(|e| panic!("workspace.undo failed: {e}"));

    assert!(!out.join("a").exists());
    assert!(!out.join("b").exists());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

/// A `config.bin` of `text`, binarized the way the game tools write one.
fn config_bin(text: &str) -> Vec<u8> {
    let parsed = modfoundry_config::parser::parse(text);
    modfoundry_config::write_rap(&parsed.file).unwrap()
}

#[test]
fn a_config_bin_is_unpacked_with_a_config_cpp_beside_it() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-bin");
    let pbo = dir.join("Mod.pbo");
    let bin = config_bin("class CfgPatches { class A {}; };");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[
            ("config.bin", bin.as_slice()),
            ("Scripts\\3_Game\\a.c", b"class B {};"),
        ],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 2 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("config.bin")).unwrap(), bin);
    let text = modfoundry_config::to_text(&modfoundry_config::read_rap(&bin).unwrap());
    assert_eq!(fs::read(out.join("config.cpp")).unwrap(), text.as_bytes());
}

#[test]
fn a_config_cpp_in_the_pbo_is_kept_over_the_conversion() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-cpp-kept");
    let pbo = dir.join("Mod.pbo");
    let bin = config_bin("class CfgPatches { class A {}; };");
    let own: &[u8] = b"// the packed text\n";
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("config.bin", bin.as_slice()), ("config.cpp", own)],
    );
    let out = dir.join("out");

    dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(fs::read(out.join("config.cpp")).unwrap(), own);
    assert_eq!(fs::read(out.join("config.bin")).unwrap(), bin);
}

#[test]
fn a_config_bin_that_does_not_convert_is_named_and_the_rest_still_lands() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-bin-garbage");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[
            ("config.bin", b"not a config"),
            ("Scripts\\a.c", b"class A {};"),
        ],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    let summary = summary_of(&outcome);
    let head = format!(
        "unpacked 2 file(s) from Mod.pbo into {}; 1 config.bin not converted (first: config.bin: ",
        out.display()
    );
    assert!(summary.starts_with(&head), "{summary}");
    assert!(summary.ends_with(')'), "{summary}");
    assert_eq!(fs::read(out.join("config.bin")).unwrap(), b"not a config");
    assert_eq!(fs::read(out.join("Scripts/a.c")).unwrap(), b"class A {};");
    assert!(!out.join("config.cpp").exists());
}

#[test]
fn an_unpack_into_a_folder_that_holds_the_config_cpp_is_refused_and_writes_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-cpp-taken");
    let pbo = dir.join("Mod.pbo");
    let bin = config_bin("class CfgPatches { class A {}; };");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("config.bin", bin.as_slice())],
    );
    let out = dir.join("out");
    common::write(&out, "config.cpp", "keep me");
    let before = tree(&out);

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": pbo, "out": out }),
        )
        .expect_err("the unpack must be refused");

    assert!(
        error.to_string().contains("already holds 1 of these files"),
        "{error}"
    );
    assert_eq!(tree(&out), before);
    assert!(!out.join("config.bin").exists());
}

#[test]
fn a_config_bin_in_a_subfolder_gets_its_config_cpp_in_that_folder() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-bin-subfolder");
    let pbo = dir.join("Mod.pbo");
    let bin = config_bin("class CfgPatches { class A {}; };");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[("addons_sub\\config.bin", bin.as_slice())],
    );
    let out = dir.join("out");

    dispatch(&core, json!({ "path": pbo, "out": out }));

    let text = modfoundry_config::to_text(&modfoundry_config::read_rap(&bin).unwrap());
    assert_eq!(
        fs::read(out.join("addons_sub/config.cpp")).unwrap(),
        text.as_bytes()
    );
    assert!(!out.join("config.cpp").exists());
}

#[test]
fn a_config_cpp_folder_in_the_pbo_keeps_its_folder_and_skips_the_conversion() {
    let core = common::core();
    let dir = tmp("pbo-unpack-config-cpp-folder");
    let pbo = dir.join("Mod.pbo");
    let bin = config_bin("class CfgPatches { class A {}; };");
    pack(
        &pbo,
        &[("prefix", "Mod")],
        &[
            ("config.bin", bin.as_slice()),
            ("config.cpp\\x.txt", b"inside"),
        ],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 2 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("config.cpp/x.txt")).unwrap(), b"inside");
    assert!(out.join("config.cpp").is_dir());
}

#[test]
fn a_pbo_without_a_prefix_lands_at_the_folder_root_with_no_prefix_file() {
    let core = common::core();
    let dir = tmp("pbo-unpack-no-prefix");
    let pbo = dir.join("Mod.pbo");
    pack(&pbo, &[], &[("config.cpp", b"class A {};")]);
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 1 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("config.cpp")).unwrap(), b"class A {};");
    assert!(!out.join("$PBOPREFIX$").exists());
}

#[test]
fn a_prefix_file_inside_the_pbo_keeps_its_own_bytes() {
    let core = common::core();
    let dir = tmp("pbo-unpack-own-prefix-file");
    let pbo = dir.join("Mod.pbo");
    let own: &[u8] = b"Other\\Thing";
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};"), ("$PBOPREFIX$", own)],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 2 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("$PBOPREFIX$")).unwrap(), own);
}

#[test]
fn a_folder_named_like_the_prefix_file_takes_its_place_and_no_prefix_file_is_written() {
    let core = common::core();
    let dir = tmp("pbo-unpack-prefix-file-folder");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("$PBOPREFIX$\\x.txt", b"inside")],
    );
    let out = dir.join("out");

    let outcome = dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(
        summary_of(&outcome),
        format!("unpacked 1 file(s) from Mod.pbo into {}", out.display())
    );
    assert_eq!(fs::read(out.join("$PBOPREFIX$/x.txt")).unwrap(), b"inside");
}

#[test]
fn an_existing_prefix_file_refuses_the_unpack_and_writes_nothing() {
    let core = common::core();
    let dir = tmp("pbo-unpack-prefix-file-taken");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    let out = dir.join("out");
    common::write(&out, "$PBOPREFIX$", "keep me");
    let before = tree(&out);

    let error = core
        .dispatch(
            Actor::CliSession,
            "pbo.unpack",
            json!({ "path": pbo, "out": out }),
        )
        .expect_err("the unpack must be refused");

    assert!(
        error.to_string().contains("already holds 1 of these files"),
        "{error}"
    );
    assert_eq!(tree(&out), before);
    assert!(!out.join("config.cpp").exists());
}

#[test]
fn only_the_prefix_itself_is_taken_off_an_entry_that_repeats_it() {
    let core = common::core();
    let dir = tmp("pbo-unpack-repeated-prefix");
    let pbo = dir.join("Mod.pbo");
    pack(&pbo, &[("prefix", "Mod")], &[("Mod\\x.c", b"class A {};")]);
    let out = dir.join("out");

    dispatch(&core, json!({ "path": pbo, "out": out }));

    assert_eq!(fs::read(out.join("Mod/x.c")).unwrap(), b"class A {};");
    assert!(!out.join("x.c").exists());
}

#[test]
fn an_own_prefix_entry_in_lower_case_is_kept_over_the_prefix_file() {
    let core = common::core();
    let dir = tmp("pbo-unpack-own-lower-marker");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod\\Sub")],
        &[
            ("config.cpp", b"class A {};"),
            ("$pboprefix$", b"Own\\Text"),
        ],
    );
    let out = dir.join("out");

    dispatch(&core, json!({ "path": pbo, "out": out }));

    let mut names: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["$pboprefix$", "config.cpp"]);
    assert_eq!(fs::read(out.join("$pboprefix$")).unwrap(), b"Own\\Text");
}
