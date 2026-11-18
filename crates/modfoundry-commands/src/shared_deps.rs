//! A mod's shared dependency list, read from `.modfoundry/dependencies.json` in the mod folder.

use std::io::ErrorKind;
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::{canonical_or, file_name, same_folder};
use crate::project_settings::{ModRole, ModSource, ModUse, TestMod};

/// Where a mod keeps its committed dependency list, relative to the mod folder.
pub const SHARED_DEPS_FILE: &str = ".modfoundry/dependencies.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedFile {
    #[serde(default)]
    dependencies: Vec<Entry>,
}

/// One entry as the file holds it: a Workshop id, or a folder relative to the mod folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    workshop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
}

/// The file's entries, and the dependencies they name.
#[derive(Clone)]
pub(crate) struct Shared {
    pub entries: Vec<Entry>,
    pub rows: Vec<TestMod>,
}

impl Shared {
    pub(crate) fn from_entries(root: &Path, entries: Vec<Entry>) -> Result<Self> {
        let rows = entries
            .iter()
            .map(|entry| dependency(root, entry))
            .collect::<Result<Vec<_>>>()
            .with_context(|| root.join(SHARED_DEPS_FILE).display().to_string())?;
        Ok(Self { entries, rows })
    }
}

/// The shared file of the mod at `root`; an empty list when there is no file.
pub(crate) fn load(root: &Path) -> Result<Shared> {
    Shared::from_entries(root, read_entries(root)?)
}

/// The dependencies the mod's shared file lists; none when there is no file.
pub(crate) fn read(root: &Path) -> Result<Vec<TestMod>> {
    Ok(load(root)?.rows)
}

fn read_entries(root: &Path) -> Result<Vec<Entry>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let path = root.join(SHARED_DEPS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| path.display().to_string()),
    };
    let file: SharedFile =
        serde_json::from_str(&text).with_context(|| path.display().to_string())?;
    Ok(file.dependencies)
}

/// Writes the shared file of the mod at `root` with these entries, creating its folder.
pub(crate) fn write(root: &Path, entries: &[Entry]) -> Result<()> {
    let file = SharedFile {
        dependencies: entries.to_vec(),
    };
    let mut text = serde_json::to_string_pretty(&file)?;
    text.push('\n');
    let mut txn = crate::dir_txn(root, true);
    txn.write(SHARED_DEPS_FILE, text)?;
    txn.finish()?;
    Ok(())
}

/// The entry that names `source` for the mod at `root`.
pub(crate) fn entry_for(root: &Path, source: &ModSource) -> Entry {
    match source {
        ModSource::Workshop { id } => Entry {
            workshop: Some(id.clone()),
            folder: None,
        },
        ModSource::Folder { path } => Entry {
            workshop: None,
            folder: Some(relative_folder(&canonical_or(root), path)),
        },
    }
}

/// `folder` as the file stores it: relative to `root` with `/` separators, or absolute when the two
/// share no leading part (another drive).
fn relative_folder(root: &Path, folder: &Path) -> String {
    let from: Vec<Component> = root.components().collect();
    let to: Vec<Component> = folder.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if shared == 0 {
        return folder.display().to_string();
    }
    let parts: Vec<String> = std::iter::repeat_n("..".to_owned(), from.len() - shared)
        .chain(
            to[shared..]
                .iter()
                .map(|part| part.as_os_str().to_string_lossy().into_owned()),
        )
        .collect();
    match parts.join("/") {
        text if text.is_empty() => ".".to_owned(),
        text => text,
    }
}

fn dependency(root: &Path, entry: &Entry) -> Result<TestMod> {
    match (&entry.workshop, &entry.folder) {
        (Some(id), None) if is_workshop_id(id) => Ok(workshop_dependency(id.clone())),
        (Some(id), None) => bail!("workshop id {id} must be digits only"),
        (None, Some(folder)) if folder.is_empty() => bail!("folder entry must name a folder"),
        (None, Some(folder)) => Ok(folder_dependency(root, folder)),
        _ => bail!("an entry needs exactly one of workshop or folder"),
    }
}

/// One dependency of the merged list, and whether the shared file supplied it.
pub(crate) struct MergedMod {
    pub entry: TestMod,
    pub shared: bool,
}

/// The shared rows in file order, each taken over by this machine's row for the same mod (which
/// stays a dependency, as the file lists it); the machine's remaining rows follow.
pub(crate) fn merge(shared: Vec<TestMod>, machine: Vec<TestMod>) -> Vec<MergedMod> {
    let mut merged: Vec<MergedMod> = shared
        .iter()
        .map(
            |entry| match machine.iter().find(|row| same_shared_mod(row, entry)) {
                Some(row) => MergedMod {
                    entry: TestMod {
                        use_as: ModUse::Dependency,
                        ..row.clone()
                    },
                    shared: false,
                },
                None => MergedMod {
                    entry: entry.clone(),
                    shared: true,
                },
            },
        )
        .collect();
    merged.extend(
        machine
            .into_iter()
            .filter(|row| !shared.iter().any(|entry| same_shared_mod(row, entry)))
            .map(|entry| MergedMod {
                entry,
                shared: false,
            }),
    );
    merged
}

/// Matches by id or folder only: a Workshop row never matches by folder.
pub(crate) fn same_shared_mod(a: &TestMod, b: &TestMod) -> bool {
    a.id == b.id
        || matches!(
            (folder_of(&a.source), folder_of(&b.source)),
            (Some(a), Some(b)) if same_folder(a, b)
        )
}

fn folder_of(source: &ModSource) -> Option<&Path> {
    match source {
        ModSource::Folder { path } => Some(path),
        ModSource::Workshop { .. } => None,
    }
}

/// A Workshop id: one or more ASCII digits.
pub(crate) fn is_workshop_id(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn workshop_dependency(id: String) -> TestMod {
    TestMod {
        name: id.clone(),
        source: ModSource::Workshop { id: id.clone() },
        id,
        role: ModRole::Both,
        use_as: ModUse::Dependency,
        enabled: true,
    }
}

fn folder_dependency(root: &Path, folder: &str) -> TestMod {
    let path = canonical_or(&root.join(folder));
    TestMod {
        id: path.display().to_string(),
        name: file_name(&path),
        source: ModSource::Folder { path },
        role: ModRole::Both,
        use_as: ModUse::Dependency,
        enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{load, merge, read};
    use crate::project_settings::{ModRole, ModSource, ModUse, TestMod};
    use std::path::{Path, PathBuf};

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mf-shared-deps-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_shared_file(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        std::fs::write(root.join(super::SHARED_DEPS_FILE), text).unwrap();
    }

    #[test]
    fn a_workshop_entry_is_a_dependency_named_by_its_id() {
        let root = scratch("workshop");
        write_shared_file(&root, r#"{"dependencies":[{"workshop":"1559212036"}]}"#);

        let rows = read(&root).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1559212036");
        assert_eq!(rows[0].name, "1559212036");
        assert_eq!(
            rows[0].source,
            ModSource::Workshop {
                id: "1559212036".to_owned()
            }
        );
    }

    #[test]
    fn a_file_that_breaks_the_rules_is_an_error_naming_the_file() {
        let cases = [
            (
                r#"{"dependencies":[{"folder":"a","workshop":"1"}]}"#,
                "exactly one",
            ),
            (r#"{"dependencies":[{}]}"#, "exactly one"),
            (r#"{"dependencies":[{"fodler":"a"}]}"#, "fodler"),
            (r#"{"dependencies":[{"workshop":"15a"}]}"#, "digits"),
            ("{\"dependencies\": [", "at line 1 column"),
        ];
        for (text, expected) in cases {
            let root = scratch("broken");
            write_shared_file(&root, text);

            let message = format!("{:#}", load(&root).err().unwrap());

            assert!(message.contains(super::SHARED_DEPS_FILE), "{message}");
            assert!(message.contains(expected), "{message}");
        }
    }

    fn workshop_row(id: &str, enabled: bool) -> TestMod {
        TestMod {
            id: id.to_owned(),
            name: id.to_owned(),
            source: ModSource::Workshop { id: id.to_owned() },
            role: ModRole::Both,
            use_as: ModUse::Dependency,
            enabled,
        }
    }

    #[test]
    fn a_machine_row_for_a_shared_mod_takes_its_place() {
        let shared = vec![workshop_row("111", true), workshop_row("222", true)];
        let machine = vec![workshop_row("222", false), workshop_row("333", true)];

        let merged: Vec<(String, bool, bool)> = merge(shared, machine)
            .into_iter()
            .map(|row| (row.entry.id, row.entry.enabled, row.shared))
            .collect();

        assert_eq!(
            merged,
            [
                ("111".to_owned(), true, true),
                ("222".to_owned(), false, false),
                ("333".to_owned(), true, false),
            ]
        );
    }

    #[test]
    fn a_machine_row_that_is_only_a_test_mod_still_makes_a_listed_mod_a_dependency() {
        let shared = vec![workshop_row("111", true)];
        let mut machine = workshop_row("111", true);
        machine.use_as = ModUse::TestOnly;

        let merged = merge(shared, vec![machine]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].entry.use_as, ModUse::Dependency);
        assert!(!merged[0].shared);
    }

    #[test]
    fn a_mod_without_the_file_has_no_shared_dependencies() {
        let root = scratch("no-file");

        assert!(read(&root).unwrap().is_empty());
    }

    #[test]
    fn a_folder_entry_has_the_id_a_saved_row_for_that_folder_has() {
        use crate::project_deps::{self, DependencyRef, EditAction, EditTarget};

        let base = scratch("folder-id").canonicalize().unwrap();
        let root = base.join("Mod");
        let dep = base.join("Dep");
        let other = base.join("Other");
        for dir in [&root, &dep, &other] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_shared_file(&root, r#"{"dependencies":[{"folder":"../Dep"}]}"#);
        let db = base.join("agents.db");
        project_deps::edit(
            Some(&db),
            &other,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();
        let saved = project_deps::list(&db, &other, || None).unwrap();

        assert_eq!(read(&root).unwrap()[0].id, saved[0].id);
    }

    #[test]
    fn a_root_that_is_a_file_has_no_shared_dependencies() {
        let base = scratch("root-is-file");
        let file = base.join("loose.c");
        std::fs::write(&file, "").unwrap();

        assert!(read(&file).unwrap().is_empty());
    }

    #[test]
    fn an_empty_folder_entry_is_an_error() {
        let root = scratch("empty-folder");
        write_shared_file(&root, r#"{"dependencies":[{"folder":""}]}"#);

        assert!(load(&root).is_err());
    }
}
