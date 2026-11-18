//! Saved dependency mods: listing them and adding or removing one in the app's settings.

use std::path::{Path, PathBuf};

use crate::common::{self, tmp};
use modfoundry_commands::project_deps::{self, DependencyRef, EditAction, EditTarget};
use modfoundry_commands::project_settings::{
    project_id, ModSource, ProjectSettings, PREFS_DEFAULTS_KEY, PREFS_PROJECT_KEY,
};
use modfoundry_commands::registry::Session;
use modfoundry_core::{Actor, CommandError, Outcome, PREVIEW_ARG};
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde_json::json;

fn scratch(label: &str) -> (PathBuf, PathBuf) {
    let base = tmp(label);
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    (project.canonicalize().unwrap(), base.join("agents.db"))
}

#[test]
fn listing_without_a_database_answers_none_and_creates_no_file() {
    let (project, db) = scratch("deps-no-db");

    let listed = project_deps::list(&db, &project, || None).unwrap();

    assert!(listed.is_empty());
    assert!(!db.exists());
}

fn mod_folder(base: &Path, name: &str) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn saved_settings(db: &Path, root: &Path) -> ProjectSettings {
    let store = Store::open(db).unwrap();
    let defaults = store
        .get_setting(&Scope::Global, PREFS_DEFAULTS_KEY)
        .unwrap();
    let project = store
        .get_setting(&Scope::Workspace(project_id(root)), PREFS_PROJECT_KEY)
        .unwrap();
    ProjectSettings::from_values(defaults.as_ref(), project.as_ref(), None)
}

#[test]
fn adding_a_folder_saves_an_enabled_dependency_the_lint_reads() {
    let (project, db) = scratch("deps-add");
    let dep = mod_folder(project.parent().unwrap(), "DepA");

    let outcome = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep.clone()),
        None,
        true,
    )
    .unwrap();

    let listed = project_deps::list(&db, &project, || None).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].enabled);
    assert_eq!(outcome.after, listed);
    let index = saved_settings(&db, &project).script_index_args(None, Path::exists);
    assert_eq!(index.args["dependencies"], json!([dep]));
}

fn project_map(db: &Path, root: &Path) -> Option<serde_json::Value> {
    Store::open(db)
        .unwrap()
        .get_setting(&Scope::Workspace(project_id(root)), PREFS_PROJECT_KEY)
        .unwrap()
}

#[test]
fn adding_keeps_the_other_settings_in_the_project_map() {
    let (project, db) = scratch("deps-keep-keys");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"build.packing.profile": {"Text": "release"}}),
        )
        .unwrap();

    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap();

    let map = project_map(&db, &project).unwrap();
    assert_eq!(map["build.packing.profile"], json!({"Text": "release"}));
    assert!(map.get("launch.mods.test").is_some());
}

#[test]
fn adding_a_folder_with_a_disabled_test_only_row_turns_that_row_into_a_dependency() {
    let (project, db) = scratch("deps-turn-row");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "legacy-row",
                "name": "DepA",
                "source": {"Folder": {"path": dep}},
                "role": "Both",
                "use_as": "TestOnly",
                "enabled": false,
            }]}}),
        )
        .unwrap();

    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap();

    let listed = project_deps::list(&db, &project, || None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "legacy-row");
    assert!(listed[0].enabled);
    let stored = &project_map(&db, &project).unwrap()["launch.mods.test"]["Mods"];
    assert_eq!(stored.as_array().unwrap().len(), 1);
}

#[test]
fn adding_a_folder_that_defaults_already_enable_writes_nothing_and_says_so() {
    let (project, db) = scratch("deps-defaults-note");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Global,
            PREFS_DEFAULTS_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "defaults-row",
                "name": "DepA",
                "source": {"Folder": {"path": dep}},
                "role": "Both",
                "use_as": "Dependency",
                "enabled": true,
            }]}}),
        )
        .unwrap();

    let outcome = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap();

    assert_eq!(
        outcome.note.as_deref(),
        Some("DepA is already in the Defaults mods list")
    );
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn adding_a_folder_under_the_steam_workshop_folder_saves_a_workshop_row() {
    let (project, db) = scratch("deps-workshop");
    let steam = project.parent().unwrap().join("steam");
    let dep = steam.join("steamapps/workshop/content/221100/123456");
    std::fs::create_dir_all(&dep).unwrap();

    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        Some(&steam),
        true,
    )
    .unwrap();

    let listed = project_deps::list(&db, &project, || None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "123456");
    assert_eq!(
        listed[0].source,
        modfoundry_commands::project_settings::ModSource::Workshop {
            id: "123456".to_owned()
        }
    );
}

#[test]
fn adding_a_missing_folder_names_it_and_writes_nothing() {
    let (project, db) = scratch("deps-missing");
    let missing = project.parent().unwrap().join("NoSuchMod");

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(missing.clone()),
        None,
        true,
    )
    .unwrap_err();

    assert!(error.to_string().contains(&missing.display().to_string()));
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn a_preview_shows_the_change_and_writes_nothing() {
    let (project, db) = scratch("deps-preview");
    let dep = mod_folder(project.parent().unwrap(), "DepA");

    let outcome = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        false,
    )
    .unwrap();

    assert_eq!(outcome.after.len(), 1);
    assert!(outcome.before.is_empty());
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn removing_the_only_project_dependency_drops_the_mod_list_key() {
    let (project, db) = scratch("deps-remove-last");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep.clone()),
        None,
        true,
    )
    .unwrap();

    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Remove,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap();

    assert!(project_deps::list(&db, &project, || None)
        .unwrap()
        .is_empty());
    assert!(project_map(&db, &project)
        .unwrap()
        .get("launch.mods.test")
        .is_none());
}

#[test]
fn removing_a_defaults_row_is_refused_and_writes_nothing() {
    let (project, db) = scratch("deps-remove-default");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Global,
            PREFS_DEFAULTS_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "defaults-row",
                "name": "DepA",
                "source": {"Folder": {"path": dep}},
                "role": "Both",
                "use_as": "Dependency",
                "enabled": true,
            }]}}),
        )
        .unwrap();

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Remove,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "DepA is in the Defaults mods list; change it there"
    );
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn removing_an_unknown_dependency_is_refused_by_the_text_given() {
    let (project, db) = scratch("deps-remove-unknown");

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Remove,
        EditTarget::Machine,
        &DependencyRef::Workshop("999".to_owned()),
        None,
        true,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "999 is not a saved dependency");
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn a_dependency_of_digits_is_a_workshop_id_and_anything_else_a_folder() {
    assert_eq!(
        project_deps::parse_dependency("123456"),
        DependencyRef::Workshop("123456".to_owned())
    );
    assert_eq!(
        project_deps::parse_dependency("mods/DepA"),
        DependencyRef::Folder(PathBuf::from("mods/DepA"))
    );
    assert_eq!(
        project_deps::parse_dependency(""),
        DependencyRef::Folder(PathBuf::new())
    );
    assert_eq!(
        project_deps::parse_dependency("workshop:123456"),
        DependencyRef::Workshop("123456".to_owned())
    );
    assert_eq!(
        project_deps::parse_dependency("workshop:abc"),
        DependencyRef::Folder(PathBuf::from("workshop:abc"))
    );
}

#[test]
fn the_dependencies_command_answers_an_empty_list_without_a_database() {
    let (project, _db) = scratch("deps-cmd-none");

    let outcome = common::core()
        .dispatch(
            Actor::CliSession,
            "project.dependencies",
            json!({ "root": project }),
        )
        .unwrap();

    let Outcome::Data(value) = outcome else {
        panic!("expected Data, got {outcome:?}");
    };
    assert_eq!(value, json!({ "dependencies": [] }));
}

#[test]
fn the_edit_command_previews_then_adds_and_the_read_command_lists_the_result() {
    let (project, db) = scratch("deps-cmd-edit");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    let core = common::core_with(&Session::new().with_db(db.clone()));
    let args = json!({ "root": project, "action": "add", "dependency": dep, "where": "machine" });
    let mut preview = args.clone();
    preview[PREVIEW_ARG] = json!(true);

    let Outcome::Preview(plan) = core
        .dispatch(Actor::CliSession, "project.edit_dependencies", preview)
        .unwrap()
    else {
        panic!("expected a preview");
    };
    assert_eq!(plan["after"].as_array().unwrap().len(), 1);
    assert!(project_map(&db, &project).is_none());

    let outcome = core
        .dispatch(Actor::CliSession, "project.edit_dependencies", args)
        .unwrap();
    assert!(matches!(outcome, Outcome::Changed { .. }), "{outcome:?}");
    let Outcome::Data(read) = core
        .dispatch(
            Actor::CliSession,
            "project.dependencies",
            json!({ "root": project }),
        )
        .unwrap()
    else {
        panic!("expected Data");
    };
    assert_eq!(read["dependencies"][0]["enabled"], json!(true));
}

#[test]
fn the_edit_command_fails_on_the_machine_list_without_a_database_to_save_to() {
    let (project, _db) = scratch("deps-cmd-no-db");

    let error = common::core()
        .dispatch(
            Actor::CliSession,
            "project.edit_dependencies",
            json!({ "root": project, "action": "add", "dependency": "123456", "where": "machine" }),
        )
        .unwrap_err();

    assert!(
        matches!(&error, CommandError::Failed(message) if message == "no database to save settings in"),
        "{error}"
    );
}

#[test]
fn adding_a_defaults_dependency_this_project_switched_off_is_refused_and_writes_nothing() {
    let (project, db) = scratch("deps-defaults-off");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    let defaults_row = json!({
        "id": "defaults-row",
        "name": "DepA",
        "source": {"Folder": {"path": dep}},
        "role": "Both",
        "use_as": "Dependency",
        "enabled": true,
    });
    let store = Store::open(&db).unwrap();
    store
        .set_setting(
            &Scope::Global,
            PREFS_DEFAULTS_KEY,
            &json!({"launch.mods.test": {"Mods": [defaults_row.clone()]}}),
        )
        .unwrap();
    let mut switched_off = defaults_row;
    switched_off["enabled"] = json!(false);
    store
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [switched_off]}}),
        )
        .unwrap();

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "DepA is already in the Defaults mods list"
    );
    let stored = &project_map(&db, &project).unwrap()["launch.mods.test"]["Mods"];
    assert_eq!(stored.as_array().unwrap().len(), 1);
    assert_eq!(stored[0]["enabled"], json!(false));
}

#[test]
fn editing_a_project_folder_that_does_not_exist_is_refused() {
    let (project, db) = scratch("deps-no-root");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    let missing = project.parent().unwrap().join("NoSuchProject");

    let error = project_deps::edit(
        Some(&db),
        &missing,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap_err();

    assert!(error.to_string().contains("NoSuchProject"), "{error}");
    assert!(!db.exists());
}

#[test]
fn a_saved_project_value_that_is_not_a_map_is_refused_and_kept() {
    let (project, db) = scratch("deps-not-map");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!("odd"),
        )
        .unwrap();

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap_err();

    assert!(error.to_string().contains("not a map"), "{error}");
    assert_eq!(project_map(&db, &project), Some(json!("odd")));
}

#[test]
fn a_mod_list_that_does_not_parse_is_refused_and_kept() {
    let (project, db) = scratch("deps-bad-list");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    let stored =
        json!({"build.packing.profile": {"Text": "release"}, "launch.mods.test": {"Mods": "bad"}});
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &stored,
        )
        .unwrap();

    let result = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    );

    assert!(result.is_err());
    assert_eq!(project_map(&db, &project), Some(stored));
}

#[test]
fn adding_a_dependency_that_is_already_listed_says_so() {
    let (project, db) = scratch("deps-already");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    let core = common::core_with(&Session::new().with_db(db.clone()));
    let args = json!({ "root": project, "action": "add", "dependency": dep, "where": "machine" });
    core.dispatch(Actor::CliSession, "project.edit_dependencies", args.clone())
        .unwrap();

    let outcome = core
        .dispatch(Actor::CliSession, "project.edit_dependencies", args)
        .unwrap();

    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected Changed, got {outcome:?}");
    };
    assert_eq!(
        summary.description,
        format!("DepA is already a dependency of {}", project.display())
    );
}

#[test]
fn removing_drops_every_project_row_for_that_mod() {
    let (project, db) = scratch("deps-remove-both");
    let steam = project.parent().unwrap().join("steam");
    let dep = steam.join("steamapps/workshop/content/221100/123456");
    std::fs::create_dir_all(&dep).unwrap();
    let row = |id: &str, source: serde_json::Value| {
        json!({"id": id, "name": "DepA", "source": source, "role": "Both",
               "use_as": "Dependency", "enabled": true})
    };
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [
                row("by-folder", json!({"Folder": {"path": dep}})),
                row("123456", json!({"Workshop": {"id": "123456"}})),
            ]}}),
        )
        .unwrap();

    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Remove,
        EditTarget::Machine,
        &DependencyRef::Workshop("123456".to_owned()),
        Some(&steam),
        true,
    )
    .unwrap();

    assert!(project_deps::list(&db, &project, || None)
        .unwrap()
        .is_empty());
}

#[test]
fn listing_a_project_folder_that_does_not_exist_is_refused() {
    let (project, db) = scratch("deps-list-missing");
    let missing = project.join("gone");

    let error = project_deps::list(&db, &missing, || None).unwrap_err();

    assert!(error
        .to_string()
        .starts_with(&missing.display().to_string()));
}

#[test]
fn turning_on_a_disabled_dependency_names_the_mod() {
    let (project, db) = scratch("deps-reenable");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [{
                "id": "legacy-row", "name": "Dep A", "source": {"Folder": {"path": dep}},
                "role": "Both", "use_as": "Dependency", "enabled": false,
            }]}}),
        )
        .unwrap();
    let core = common::core_with(&Session::new().with_db(db.clone()));

    let outcome = core
        .dispatch(
            Actor::CliSession,
            "project.edit_dependencies",
            json!({ "root": project, "action": "add", "dependency": dep, "where": "machine" }),
        )
        .unwrap();

    let Outcome::Changed { summary, .. } = outcome else {
        panic!("expected Changed, got {outcome:?}");
    };
    assert_eq!(
        summary.description,
        format!("added Dep A as a dependency of {}", project.display())
    );
}

#[test]
fn the_edit_command_offers_the_steam_folder_argument() {
    let schema = common::core()
        .command("project.edit_dependencies")
        .unwrap()
        .args_schema();

    assert_eq!(schema["properties"]["steam_dir"]["type"], json!("string"));
}

fn saved_dependency(name: &str, path: &str) -> project_deps::SavedDependency {
    project_deps::SavedDependency {
        id: path.to_owned(),
        name: name.to_owned(),
        source: ModSource::Folder {
            path: PathBuf::from(path),
        },
        enabled: true,
        origin: project_deps::DependencyOrigin::Machine,
        from_defaults: false,
    }
}

#[test]
fn the_edit_summary_says_what_changed_or_why_nothing_did() {
    let dep = saved_dependency("DepA", "/mods/DepA");
    let outcome = |before: Vec<_>, after: Vec<_>, note: Option<&str>| project_deps::EditOutcome {
        before,
        after,
        note: note.map(str::to_owned),
    };
    let cases = [
        (
            outcome(vec![], vec![], Some("DepA is in the Defaults mods list")),
            EditAction::Add,
            "/mods/DepA",
            "DepA is in the Defaults mods list",
        ),
        (
            outcome(vec![dep.clone()], vec![dep.clone()], None),
            EditAction::Add,
            "/mods/DepA",
            "DepA is already a dependency of /projects/Mine",
        ),
        (
            outcome(vec![], vec![dep.clone()], None),
            EditAction::Add,
            "/mods/DepA",
            "added DepA as a dependency of /projects/Mine",
        ),
        (
            outcome(vec![dep], vec![], None),
            EditAction::Remove,
            "/mods/DepA",
            "removed DepA as a dependency of /projects/Mine",
        ),
    ];

    for (outcome, action, dependency, expected) in cases {
        assert_eq!(
            project_deps::describe_edit(
                &outcome,
                action,
                EditTarget::Machine,
                dependency,
                Path::new("/projects/Mine")
            ),
            expected
        );
    }
}

fn workshop_item(steam: &Path, id: &str, meta: Option<&str>) {
    let dir = steam.join("steamapps/workshop/content/221100").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(meta) = meta {
        std::fs::write(dir.join("meta.cpp"), meta).unwrap();
    }
}

#[test]
fn a_workshop_dependency_lists_under_the_name_its_meta_cpp_declares() {
    let (project, db) = scratch("deps-meta-name");
    let steam = project.parent().unwrap().join("steam");
    workshop_item(&steam, "1559212036", Some("name = \"Alpine\";"));
    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Workshop("1559212036".to_owned()),
        Some(&steam),
        true,
    )
    .unwrap();

    let listed = project_deps::list(&db, &project, || Some(steam.clone())).unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "1559212036");
    assert_eq!(listed[0].name, "Alpine");
}

#[test]
fn a_workshop_dependency_without_a_meta_cpp_name_lists_under_its_id() {
    let cases = [
        ("no Steam root", false, Some("name = \"Alpine\";")),
        ("no item folder", true, None),
        (
            "no name in meta.cpp",
            true,
            Some("publisher = \"Foundry\";"),
        ),
    ];
    for (label, has_steam_root, meta) in cases {
        let (project, db) = scratch("deps-meta-fallback");
        let steam = project.parent().unwrap().join("steam");
        std::fs::create_dir_all(&steam).unwrap();
        if let Some(meta) = meta {
            workshop_item(&steam, "1559212036", Some(meta));
        }
        project_deps::edit(
            Some(&db),
            &project,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Workshop("1559212036".to_owned()),
            None,
            true,
        )
        .unwrap();
        let root = has_steam_root.then(|| steam.clone());

        let listed = project_deps::list(&db, &project, || root).unwrap();

        assert_eq!(listed[0].name, "1559212036", "{label}");
    }
}

#[test]
fn listing_folder_dependencies_never_asks_for_the_steam_root() {
    let (project, db) = scratch("deps-no-steam-lookup");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap();

    let listed =
        project_deps::list(&db, &project, || panic!("the Steam root was asked for")).unwrap();

    assert_eq!(listed.len(), 1);
}

#[test]
fn adding_a_workshop_id_reports_the_name_its_meta_cpp_declares() {
    let (project, db) = scratch("deps-add-meta-name");
    let steam = project.parent().unwrap().join("steam");
    workshop_item(&steam, "1559212036", Some("name = \"Alpine\";"));

    let outcome = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Workshop("1559212036".to_owned()),
        Some(&steam),
        true,
    )
    .unwrap();

    assert_eq!(outcome.after.len(), 1);
    assert_eq!(outcome.after[0].id, "1559212036");
    assert_eq!(outcome.after[0].name, "Alpine");
}

#[test]
fn a_workshop_dependency_keeps_its_saved_name_when_steam_is_not_found() {
    let (project, db) = scratch("deps-saved-name");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": [
                {"id": "1559212036", "name": "Community Framework",
                 "source": {"Workshop": {"id": "1559212036"}}, "role": "Both",
                 "use_as": "Dependency", "enabled": true},
            ]}}),
        )
        .unwrap();

    let listed = project_deps::list(&db, &project, || None).unwrap();

    assert_eq!(listed[0].name, "Community Framework");
}

#[test]
fn listing_a_mod_list_that_does_not_parse_is_refused() {
    let (project, db) = scratch("deps-list-bad");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({"launch.mods.test": {"Mods": "bad"}}),
        )
        .unwrap();

    assert!(project_deps::list(&db, &project, || None).is_err());
}

#[test]
fn adding_beside_a_defaults_list_that_does_not_parse_is_refused_and_writes_nothing() {
    let (project, db) = scratch("deps-bad-defaults");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Global,
            PREFS_DEFAULTS_KEY,
            &json!({"launch.mods.test": {"Mods": "bad"}}),
        )
        .unwrap();

    let error = project_deps::edit(
        Some(&db),
        &project,
        EditAction::Add,
        EditTarget::Machine,
        &DependencyRef::Folder(dep),
        None,
        true,
    )
    .unwrap_err();

    assert!(error.to_string().contains("Defaults"), "{error}");
    assert!(project_map(&db, &project).is_none());
}

#[test]
fn listing_settings_the_app_cannot_read_is_refused() {
    let (project, db) = scratch("deps-list-unreadable");
    let dep = mod_folder(project.parent().unwrap(), "DepA");
    Store::open(&db)
        .unwrap()
        .set_setting(
            &Scope::Workspace(project_id(&project)),
            PREFS_PROJECT_KEY,
            &json!({
                "build.packing.profile": {"NoSuchKind": 1},
                "launch.mods.test": {"Mods": [{
                    "id": "row",
                    "name": "DepA",
                    "source": {"Folder": {"path": dep}},
                    "role": "Both",
                    "use_as": "Dependency",
                    "enabled": true,
                }]},
            }),
        )
        .unwrap();

    let error = project_deps::list(&db, &project, || None).unwrap_err();

    assert!(error.to_string().contains("cannot read"), "{error}");
}
