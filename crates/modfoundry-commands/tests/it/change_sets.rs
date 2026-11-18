//! Change-set behaviour of the mutating commands: a preview writes nothing, an
//! apply reports the change-set that undoes it, and `workspace.undo` replays it.

use std::fs;
use std::path::Path;

use crate::common::{self, tmp};
use modfoundry_core::{Actor, CommandError, Core, EffectKind, Outcome};
use serde_json::{json, Value};

fn dispatch(core: &Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn changed(out: &Outcome) -> (&modfoundry_core::ChangeSummary, Value) {
    match out {
        Outcome::Changed {
            summary,
            change_set: Some(change_set),
        } => (summary, change_set.clone()),
        other => panic!("expected a recorded change, got {other:?}"),
    }
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else {
                out.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn scaffolding_previews_every_file_without_writing_one() {
    let core = common::core();
    let dir = tmp("scaffold-preview");

    let out = dispatch(
        &core,
        "project.scaffold",
        json!({ "name": "Preview", "dir": dir, "preview": true }),
    );
    let Outcome::Preview(plan) = out else {
        panic!("expected a preview, got {out:?}");
    };

    assert_eq!(plan["root"], "mod:Preview");
    assert_eq!(plan["host_root"], json!(dir));
    let changes = plan["changes"].as_array().expect("changes array");
    assert!(
        changes.len() >= 3,
        "a skeleton is more than a config: {changes:?}"
    );
    assert!(
        changes
            .iter()
            .any(|c| c["path"] == "mod:Preview:config.cpp" && c["op"] == "create"),
        "the skeleton's config is planned: {changes:?}"
    );
    assert!(!dir.exists(), "a preview creates nothing");
}

#[test]
fn a_scaffold_is_undone_by_replaying_its_change_set() {
    let core = common::core();
    let dir = tmp("scaffold-undo");

    let out = dispatch(
        &core,
        "project.scaffold",
        json!({ "name": "Undoable", "dir": dir }),
    );
    let (summary, change_set) = changed(&out);
    assert_eq!(summary.kind, EffectKind::Mutate);
    assert!(summary.files_changed >= 3);
    assert!(dir.join("config.cpp").is_file());
    let written = files_under(&dir);
    assert_eq!(written.len(), summary.files_changed);

    let out = dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    let Outcome::Changed {
        summary,
        change_set: None,
    } = &out
    else {
        panic!("undo must not offer another undo: {out:?}");
    };
    assert_eq!(summary.files_changed, written.len());
    assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));
    assert!(
        !dir.join("Scripts").exists(),
        "the directories the scaffold created are pruned"
    );
}

#[test]
fn a_scaffold_refuses_a_non_empty_dir_and_accepts_an_empty_one() {
    let core = common::core();
    let occupied = tmp("scaffold-occupied");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("notes.txt"), "keep me").unwrap();

    let err = core
        .dispatch(
            Actor::CliSession,
            "project.scaffold",
            json!({ "name": "Demo", "dir": occupied }),
        )
        .expect_err("a non-empty dir is refused");
    assert!(
        err.to_string().contains("already exists and is not empty"),
        "{err}"
    );
    assert!(!occupied.join("config.cpp").exists(), "nothing was written");

    let err = core
        .dispatch(
            Actor::CliSession,
            "project.scaffold",
            json!({ "name": "Demo", "dir": occupied.join("notes.txt") }),
        )
        .expect_err("a file is refused");
    assert!(err.to_string().contains("is a file, not a folder"), "{err}");

    let empty = tmp("scaffold-empty");
    fs::create_dir_all(&empty).unwrap();
    let out = dispatch(
        &core,
        "project.scaffold",
        json!({ "name": "Demo", "dir": empty }),
    );
    assert!(matches!(out, Outcome::Changed { .. }), "{out:?}");
    assert!(empty.join("config.cpp").is_file());

    let _ = fs::remove_dir_all(&occupied);
    let _ = fs::remove_dir_all(&empty);
}

#[test]
fn binarizing_a_config_writes_the_output_and_undoes_it() {
    let core = common::core();
    let dir = tmp("binarize");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("config.cpp"),
        "class CfgPatches { class X { units[] = {}; }; };",
    )
    .unwrap();
    let args = json!({ "path": dir.join("config.cpp") });

    let mut preview = args.clone();
    preview["preview"] = json!(true);
    let Outcome::Preview(plan) = dispatch(&core, "config.binarize", preview) else {
        panic!("expected a preview");
    };
    assert_eq!(
        plan["changes"][0]["path"],
        format!(
            "mod:{}:config.bin",
            dir.file_name().unwrap().to_string_lossy()
        )
    );
    assert!(!dir.join("config.bin").exists());

    let out = dispatch(&core, "config.binarize", args);
    let (summary, change_set) = changed(&out);
    assert_eq!(summary.files_changed, 1);
    let bin = fs::read(dir.join("config.bin")).unwrap();
    assert!(!bin.is_empty());

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert!(!dir.join("config.bin").exists());
    assert!(dir.join("config.cpp").is_file(), "the source is untouched");
}

#[test]
fn generating_keys_reports_a_change_set_that_removes_them_again() {
    let core = common::core();
    let dir = tmp("keygen");

    let out = dispatch(
        &core,
        "sign.keygen",
        json!({ "authority": "MF", "dir": dir }),
    );
    let (summary, change_set) = changed(&out);
    assert_eq!(files_under(&dir).len(), 2);
    for key in ["MF.biprivatekey", "MF.bikey"] {
        assert!(
            summary.description.contains(key),
            "{key} is not reported: {}",
            summary.description
        );
        assert!(dir.join(key).is_file(), "missing {key}");
    }

    let preview = dispatch(
        &core,
        "sign.keygen",
        json!({ "authority": "Other", "dir": dir, "preview": true }),
    );
    let Outcome::Preview(plan) = preview else {
        panic!("expected a preview");
    };
    assert_eq!(plan["changes"].as_array().unwrap().len(), 2);

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert!(files_under(&dir).is_empty());
}

#[test]
fn undo_without_a_change_set_is_an_argument_error() {
    let core = common::core();
    let err = core
        .dispatch(Actor::CliSession, "workspace.undo", json!({}))
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");

    let err = core
        .dispatch(
            Actor::CliSession,
            "workspace.undo",
            json!({ "change_set": { "root": "mod:X", "host_root": "/tmp", "changes": "nope" } }),
        )
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
}

#[test]
fn editing_text_previews_the_write_before_touching_the_file() {
    let core = common::core();
    let dir = tmp("text-edit-preview");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("A.c");
    fs::write(&target, "old").unwrap();

    let Outcome::Preview(plan) = dispatch(
        &core,
        "text.edit",
        json!({ "path": target, "text": "new", "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["op"], "update");
    assert!(
        changes[0]["path"].as_str().unwrap().ends_with(":A.c"),
        "{changes:?}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "old",
        "a preview writes nothing"
    );
}

#[test]
fn editing_text_applies_and_is_undone_by_its_change_set() {
    let core = common::core();
    let dir = tmp("text-edit-undo");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("A.c");
    fs::write(&target, "old").unwrap();

    let out = dispatch(&core, "text.edit", json!({ "path": target, "text": "new" }));
    let (summary, change_set) = changed(&out);
    assert_eq!(summary.files_changed, 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");

    let out = dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    let Outcome::Changed {
        summary,
        change_set: None,
    } = &out
    else {
        panic!("undo must not offer another undo: {out:?}");
    };
    assert_eq!(summary.files_changed, 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
}

#[test]
fn a_dependency_edit_counts_a_file_only_when_it_wrote_one() {
    let core = common::core();
    let dir = tmp("deps-files-changed");
    let root = dir.join("Mod");
    let dep = dir.join("Dep");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&dep).unwrap();
    let args = json!({ "root": root, "action": "add", "dependency": dep });

    let first = dispatch(&core, "project.edit_dependencies", args.clone());
    let second = dispatch(&core, "project.edit_dependencies", args);

    let files = |out: &Outcome| match out {
        Outcome::Changed { summary, .. } => summary.files_changed,
        other => panic!("expected a change: {other:?}"),
    };
    assert_eq!(files(&first), 1);
    assert_eq!(files(&second), 0);
    assert!(root.join(".modfoundry/dependencies.json").is_file());
}

/// The optional destination is part of the plan, not a surprise of the apply: a
/// preview of an edit that names `out` reports the file the edit would write and
/// still writes nothing.
#[test]
fn a_text_edit_can_write_its_result_to_another_file() {
    let core = common::core();
    let dir = tmp("text-edit-out");
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("A.c");
    let dest = dir.join("B.c");
    fs::write(&source, "old A").unwrap();
    fs::write(&dest, "old B").unwrap();

    let Outcome::Preview(plan) = dispatch(
        &core,
        "text.edit",
        json!({ "path": source, "text": "new", "out": dest, "preview": true }),
    ) else {
        panic!("expected a preview");
    };
    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["op"], "update");
    assert!(
        changes[0]["path"].as_str().unwrap().ends_with(":B.c"),
        "the plan names the destination, not the source: {changes:?}"
    );
    assert_eq!(fs::read_to_string(&dest).unwrap(), "old B");

    let out = dispatch(
        &core,
        "text.edit",
        json!({ "path": source, "text": "new", "out": dest }),
    );
    let (_summary, change_set) = changed(&out);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "old A",
        "the edited path is left alone when `out` names another file"
    );
    assert_eq!(fs::read_to_string(&dest).unwrap(), "new");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&dest).unwrap(), "old B");
}

/// `expected_sha256` is the revision of `path` the caller read, so it cannot
/// guard a write that lands on `out`: the destination is the wrong file to
/// compare it against.
#[test]
fn text_edit_refuses_out_with_a_digest() {
    let core = common::core();
    let dir = tmp("text-edit-out-digest");
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("A.c");
    let dest = dir.join("B.c");
    fs::write(&source, "old A").unwrap();
    fs::write(&dest, "old B").unwrap();

    let args = json!({
        "path": source,
        "out": dest,
        "text": "new",
        "expected_sha256": "0".repeat(64),
    });

    let mut preview = args.clone();
    preview["preview"] = json!(true);
    for attempt in [preview, args] {
        let err = core
            .dispatch(Actor::CliSession, "text.edit", attempt)
            .unwrap_err();
        let CommandError::InvalidArguments(message) = err else {
            panic!("expected InvalidArguments, got {err:?}");
        };
        assert!(
            message.contains("cannot be combined with `out`"),
            "{message}"
        );
    }
    assert_eq!(
        fs::read_to_string(&dest).unwrap(),
        "old B",
        "a refused edit writes nothing"
    );
}

#[test]
fn editing_a_file_that_does_not_exist_creates_it_and_undo_removes_it() {
    let core = common::core();
    let dir = tmp("text-edit-create");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("New.c");

    let out = dispatch(
        &core,
        "text.edit",
        json!({ "path": target, "text": "class New {};" }),
    );
    let (_summary, change_set) = changed(&out);
    assert_eq!(change_set["changes"][0]["op"], "create");
    assert_eq!(fs::read_to_string(&target).unwrap(), "class New {};");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert!(!target.exists(), "undo removes the file the edit created");
}

/// A session that read a file, planned an edit, and had the file move under it
/// must not clobber the newer content — the lost update `VfsError::Stale`
/// protects an undo from, guarded on the way in.
#[test]
fn a_text_edit_refuses_a_base_revision_that_moved_on() {
    let core = common::core();
    let dir = tmp("text-edit-stale");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("A.c");
    fs::write(&target, "old").unwrap();

    // The digest of the revision the session read, as the change-set records it.
    let out = dispatch(&core, "text.edit", json!({ "path": target, "text": "old" }));
    let (_summary, change_set) = changed(&out);
    let digest = change_set["changes"][0]["after_sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    // Somebody else edits the file before the planned edit lands.
    fs::write(&target, "newer").unwrap();
    let args = json!({
        "path": target,
        "text": "mine",
        "expected_sha256": digest,
    });

    // The preview reports the staleness rather than a clean plan…
    let mut preview = args.clone();
    preview["preview"] = json!(true);
    let err = core
        .dispatch(Actor::CliSession, "text.edit", preview)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert!(err.to_string().contains("stale"), "{err}");

    // …and the apply refuses too, leaving the newer content alone.
    let err = core
        .dispatch(Actor::CliSession, "text.edit", args)
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)), "{err:?}");
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "newer",
        "a stale base revision must never clobber the edit it did not see"
    );
}

#[test]
fn a_text_edit_applies_against_the_expected_revision_and_undoes() {
    let core = common::core();
    let dir = tmp("text-edit-base-ok");
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("A.c");
    fs::write(&target, "old").unwrap();

    let out = dispatch(&core, "text.edit", json!({ "path": target, "text": "old" }));
    let (_summary, change_set) = changed(&out);
    let digest = change_set["changes"][0]["after_sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    let out = dispatch(
        &core,
        "text.edit",
        json!({ "path": target, "text": "mine", "expected_sha256": digest }),
    );
    let (_summary, change_set) = changed(&out);
    assert_eq!(fs::read_to_string(&target).unwrap(), "mine");

    dispatch(&core, "workspace.undo", json!({ "change_set": change_set }));
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
}

#[test]
fn a_command_advertises_the_preview_argument_exactly_when_it_honours_it() {
    let core = common::core();
    let mut previewable = Vec::new();
    for command in core.commands() {
        let id = command.spec().id;
        let advertised = !command.args_schema()["properties"]["preview"].is_null();
        assert_eq!(
            advertised,
            command.previewable(),
            "{id}: the schema and the trait disagree about previews"
        );
        if command.previewable() {
            assert_eq!(
                command.args_schema()["properties"]["preview"],
                json!({
                    "type": "boolean",
                    "description": "return the change-set instead of applying it",
                }),
                "{id} advertises the wrong preview argument"
            );
            previewable.push(id);
        }
    }
    // The previewable set is what can plan, not the undo that inverts a recorded change-set.
    previewable.sort();
    assert_eq!(
        previewable,
        vec![
            "build.run",
            "ce.edit",
            "config.binarize",
            "config.derapify",
            "config.edit",
            "diag.learn_error",
            "diag.run",
            "diag.run_client",
            "diag.run_diag",
            "font.build",
            "image.convert",
            "imageset.build",
            "layout.render",
            "pbo.unpack",
            "project.edit_dependencies",
            "project.scaffold",
            "publish.save_details",
            "sign.keygen",
            "text.edit",
        ]
    );
}

#[test]
fn a_keygen_refuses_an_authority_that_already_has_a_key() {
    let core = common::core();
    let dir = tmp("keygen-existing");
    fs::create_dir_all(&dir).unwrap();
    let private = dir.join("MF.biprivatekey");
    fs::write(&private, b"OLD PRIVATE KEY MATERIAL").unwrap();
    fs::write(dir.join("MF.bikey"), b"OLD PUBLIC KEY MATERIAL").unwrap();

    for preview in [false, true] {
        let mut args = json!({ "authority": "MF", "dir": dir });
        if preview {
            args["preview"] = json!(true);
        }
        let err = core
            .dispatch(Actor::CliSession, "sign.keygen", args)
            .expect_err("an existing key must not be replaced");
        assert!(
            err.to_string().contains(&private.display().to_string()),
            "the refusal must name the existing private key: {err}"
        );
    }

    assert_eq!(fs::read(&private).unwrap(), b"OLD PRIVATE KEY MATERIAL");
    assert_eq!(
        fs::read(dir.join("MF.bikey")).unwrap(),
        b"OLD PUBLIC KEY MATERIAL"
    );
}

#[cfg(unix)]
#[test]
fn a_new_private_key_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let core = common::core();
    let dir = tmp("keygen-mode");
    let _ = dispatch(
        &core,
        "sign.keygen",
        json!({ "authority": "MF", "dir": dir }),
    );
    let mode = fs::metadata(dir.join("MF.biprivatekey"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_preview_carries_no_file_content_at_all() {
    let core = common::core();
    let dir = tmp("preview-content");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("config.cpp"),
        "class CfgPatches { class X { units[] = {}; }; };",
    )
    .unwrap();
    // A file the preview will report as an update, since it already exists.
    fs::write(dir.join("config.bin"), b"OLD BIN CONTENT").unwrap();

    let Outcome::Preview(plan) = dispatch(
        &core,
        "config.binarize",
        json!({ "path": dir.join("config.cpp"), "preview": true }),
    ) else {
        panic!("expected a preview");
    };

    let changes = plan["changes"].as_array().unwrap();
    assert_eq!(changes[0]["op"], "update");
    for change in changes {
        assert!(
            change["before"].is_null(),
            "a preview is a shape, not a copy of the tree: {change}"
        );
    }
    assert!(!plan.to_string().contains("OLD BIN CONTENT"));
}

#[test]
fn derapifying_previews_only_when_it_would_write_a_file() {
    let core = common::core();
    let dir = tmp("derapify-preview");
    fs::create_dir_all(&dir).unwrap();
    let cpp = dir.join("config.cpp");
    let bin = dir.join("out.bin");
    fs::write(&cpp, "class CfgPatches { class X { units[] = {}; }; };").unwrap();
    dispatch(&core, "config.binarize", json!({ "path": cpp, "out": bin }));

    // With `out`, the conversion writes a file and can be previewed.
    let Outcome::Preview(plan) = dispatch(
        &core,
        "config.derapify",
        json!({ "path": bin, "out": dir.join("text.cpp"), "preview": true }),
    ) else {
        panic!("expected a preview");
    };
    assert_eq!(plan["changes"][0]["op"], "create");
    assert!(!dir.join("text.cpp").exists());

    // Without `out` the text *is* the result, so there is nothing to preview.
    let err = core
        .dispatch(
            Actor::CliSession,
            "config.derapify",
            json!({ "path": bin, "preview": true }),
        )
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");

    // …and the plain call still returns the text.
    let out = dispatch(&core, "config.derapify", json!({ "path": bin }));
    assert!(out.to_json().as_str().unwrap().contains("class CfgPatches"));
}
