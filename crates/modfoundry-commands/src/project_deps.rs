//! The saved dependency mods of a mod project: listing them and adding or removing one.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use modfoundry_config::mod_display_name;
use modfoundry_platform::workshop_mod_dir;
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::project_settings::{
    project_id, stored_mods_strict, ModRole, ModSource, ModUse, ProjectSettings, TestMod, MODS_KEY,
    PREFS_DEFAULTS_KEY, PREFS_PROJECT_KEY,
};

/// A dependency given on the command line: a Workshop id or a mod folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyRef {
    Folder(PathBuf),
    Workshop(String),
}

/// A dependency argument: all ASCII digits, or `workshop:` and digits as `mf deps list` prints
/// it, is a Workshop id; anything else a folder.
pub fn parse_dependency(text: &str) -> DependencyRef {
    let id = text.strip_prefix("workshop:").unwrap_or(text);
    if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) {
        DependencyRef::Workshop(id.to_owned())
    } else {
        DependencyRef::Folder(PathBuf::from(text))
    }
}

/// A saved dependency mod, as the project's list holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDependency {
    pub id: String,
    pub name: String,
    pub source: ModSource,
    pub enabled: bool,
    pub from_defaults: bool,
}

/// Whether an edit adds a dependency or removes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditAction {
    Add,
    Remove,
}

/// The dependency list before and after an edit, and a note when nothing was written.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct EditOutcome {
    pub before: Vec<SavedDependency>,
    pub after: Vec<SavedDependency>,
    pub note: Option<String>,
}

/// The saved dependency mods of the project at `root`; none when the database is not there.
/// `steam_root` is called only when a listed row is a Workshop item.
pub fn list(
    db: &Path,
    root: &Path,
    steam_root: impl FnOnce() -> Option<PathBuf>,
) -> Result<Vec<SavedDependency>> {
    require_folder(root)?;
    let prefs = Prefs::read(db, root)?;
    let steam_root = prefs.has_workshop_dependency().then(steam_root).flatten();
    Ok(prefs.dependencies(steam_root.as_deref()))
}

struct Prefs {
    defaults: Option<Value>,
    project: Option<Value>,
}

impl Prefs {
    fn read(db: &Path, root: &Path) -> Result<Self> {
        if !db.is_file() {
            return Ok(Self {
                defaults: None,
                project: None,
            });
        }
        let store = Store::open(db)?;
        let prefs = Self {
            defaults: store.get_setting(&Scope::Global, PREFS_DEFAULTS_KEY)?,
            project: store.get_setting(&Scope::Workspace(project_id(root)), PREFS_PROJECT_KEY)?,
        };
        stored_mods_strict(prefs.defaults.as_ref())
            .map_err(|error| anyhow!("the Defaults settings cannot be read: {error}"))?;
        stored_mods_strict(prefs.project.as_ref())?;
        Ok(prefs)
    }

    fn merged_rows(&self) -> Vec<TestMod> {
        ProjectSettings::from_values(self.defaults.as_ref(), self.project.as_ref(), None).test_mods
    }

    fn defaults_rows(&self) -> Vec<TestMod> {
        let own = ProjectSettings::from_values(self.defaults.as_ref(), None, None).test_mods;
        self.merged_rows()
            .into_iter()
            .filter(|row| own.iter().any(|default| default.id == row.id))
            .collect()
    }

    fn has_workshop_dependency(&self) -> bool {
        self.merged_rows().iter().any(|row| {
            row.use_as == ModUse::Dependency && matches!(row.source, ModSource::Workshop { .. })
        })
    }

    fn dependencies(&self, steam_root: Option<&Path>) -> Vec<SavedDependency> {
        let defaults = self.defaults_rows();
        self.merged_rows()
            .into_iter()
            .filter(|row| row.use_as == ModUse::Dependency)
            .map(|row| {
                let name = display_name(&row, steam_root);
                SavedDependency {
                    from_defaults: defaults.iter().any(|default| default.id == row.id),
                    id: row.id,
                    name,
                    source: row.source,
                    enabled: row.enabled,
                }
            })
            .collect()
    }
}

/// Adds or removes one dependency of the project at `root`; with `apply` false nothing is written.
pub fn edit(
    db: &Path,
    root: &Path,
    action: EditAction,
    dependency: &DependencyRef,
    steam_root: Option<&Path>,
    apply: bool,
) -> Result<EditOutcome> {
    require_folder(root)?;
    let wanted = Wanted::resolve(dependency, steam_root, action == EditAction::Add)?;
    let prefs = Prefs::read(db, root)?;
    let before = prefs.dependencies(steam_root);
    let mut rows = stored_mods_strict(prefs.project.as_ref())?;
    let note = match action {
        EditAction::Add => add(&mut rows, &prefs.defaults_rows(), &wanted, steam_root)?,
        EditAction::Remove => {
            remove(
                &mut rows,
                &prefs.defaults_rows(),
                &wanted,
                &dependency_text(dependency),
                steam_root,
            )?;
            None
        }
    };
    if let Some(note) = note {
        return Ok(EditOutcome {
            after: before.clone(),
            before,
            note: Some(note),
        });
    }
    let project = with_rows(prefs.project.as_ref(), &rows);
    let after = Prefs {
        defaults: prefs.defaults.clone(),
        project: Some(project.clone()),
    }
    .dependencies(steam_root);
    if apply {
        write(db, root, &project)?;
    }
    Ok(EditOutcome {
        before,
        after,
        note: None,
    })
}

/// The one-line summary of an edit: the note, why nothing changed, or which dependency moved.
pub fn describe_edit(
    outcome: &EditOutcome,
    action: EditAction,
    dependency: &str,
    root: &Path,
) -> String {
    if let Some(note) = &outcome.note {
        return note.clone();
    }
    if outcome.before == outcome.after {
        let name = file_name(Path::new(dependency));
        return format!("{name} is already a dependency of {}", root.display());
    }
    let changed = |from: &[SavedDependency], to: &[SavedDependency]| {
        from.iter().find(|row| !to.contains(row)).cloned()
    };
    let moved = changed(&outcome.after, &outcome.before)
        .or_else(|| changed(&outcome.before, &outcome.after))
        .map_or_else(|| dependency.to_owned(), |row| row.name);
    let verb = match action {
        EditAction::Add => "added",
        EditAction::Remove => "removed",
    };
    format!("{verb} {moved} as a dependency of {}", root.display())
}

struct Wanted {
    id: String,
    name: String,
    source: ModSource,
}

impl Wanted {
    fn resolve(
        dependency: &DependencyRef,
        steam_root: Option<&Path>,
        must_exist: bool,
    ) -> Result<Self> {
        match dependency {
            DependencyRef::Workshop(id) => Ok(Self {
                id: id.clone(),
                name: id.clone(),
                source: ModSource::Workshop { id: id.clone() },
            }),
            DependencyRef::Folder(path) => {
                if must_exist && !path.is_dir() {
                    bail!("{} is not a folder", path.display());
                }
                let path = canonical_or(path);
                match workshop_id(&path, steam_root) {
                    Some(id) => Ok(Self {
                        name: id.clone(),
                        id: id.clone(),
                        source: ModSource::Workshop { id },
                    }),
                    None => Ok(Self {
                        id: path.display().to_string(),
                        name: file_name(&path),
                        source: ModSource::Folder { path },
                    }),
                }
            }
        }
    }
}

fn add(
    rows: &mut Vec<TestMod>,
    defaults: &[TestMod],
    wanted: &Wanted,
    steam_root: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(row) = defaults
        .iter()
        .find(|row| is_same_mod(row, wanted, steam_root))
    {
        let note = format!("{} is already in the Defaults mods list", row.name);
        if row.enabled && row.use_as == ModUse::Dependency {
            return Ok(Some(note));
        }
        bail!("{note}");
    }
    match rows
        .iter_mut()
        .find(|row| is_same_mod(row, wanted, steam_root))
    {
        Some(row) => {
            row.use_as = ModUse::Dependency;
            row.enabled = true;
        }
        None => rows.push(TestMod {
            id: wanted.id.clone(),
            name: wanted.name.clone(),
            source: wanted.source.clone(),
            role: ModRole::Both,
            use_as: ModUse::Dependency,
            enabled: true,
        }),
    }
    Ok(None)
}

fn remove(
    rows: &mut Vec<TestMod>,
    defaults: &[TestMod],
    wanted: &Wanted,
    text: &str,
    steam_root: Option<&Path>,
) -> Result<()> {
    if let Some(row) = defaults
        .iter()
        .find(|row| is_same_mod(row, wanted, steam_root))
    {
        bail!("{} is in the Defaults mods list; change it there", row.name);
    }
    let count = rows.len();
    rows.retain(|row| !(row.use_as == ModUse::Dependency && is_same_mod(row, wanted, steam_root)));
    if rows.len() == count {
        bail!("{text} is not a saved dependency");
    }
    Ok(())
}

fn require_folder(root: &Path) -> Result<()> {
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    Ok(())
}

fn dependency_text(dependency: &DependencyRef) -> String {
    match dependency {
        DependencyRef::Folder(path) => path.display().to_string(),
        DependencyRef::Workshop(id) => id.clone(),
    }
}

fn is_same_mod(row: &TestMod, wanted: &Wanted, steam_root: Option<&Path>) -> bool {
    row.id == wanted.id
        || folder_of(&row.source, steam_root)
            .zip(folder_of(&wanted.source, steam_root))
            .is_some_and(|(row_dir, wanted_dir)| same_folder(&row_dir, &wanted_dir))
}

fn folder_of(source: &ModSource, steam_root: Option<&Path>) -> Option<PathBuf> {
    match source {
        ModSource::Folder { path } => Some(path.clone()),
        ModSource::Workshop { id } => workshop_mod_dir(steam_root?, id),
    }
}

fn workshop_id(path: &Path, steam_root: Option<&Path>) -> Option<String> {
    let id = path.file_name()?.to_str()?.to_owned();
    let installed = workshop_mod_dir(steam_root?, &id)?;
    same_folder(&installed, path).then_some(id)
}

fn same_folder(a: &Path, b: &Path) -> bool {
    canonical_or(a) == canonical_or(b)
}

/// A folder that is gone resolves through its parent, so `../Gone` still matches the saved path.
fn canonical_or(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let parent = match path.parent() {
            Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
            Some(parent) => parent,
            None => return path.to_path_buf(),
        };
        match (parent.canonicalize(), path.file_name()) {
            (Ok(parent), Some(name)) => parent.join(name),
            _ => path.to_path_buf(),
        }
    })
}

fn display_name(row: &TestMod, steam_root: Option<&Path>) -> String {
    let ModSource::Workshop { id } = &row.source else {
        return row.name.clone();
    };
    steam_root
        .and_then(|steam_root| workshop_item_name(steam_root, id))
        .unwrap_or_else(|| row.name.clone())
}

fn workshop_item_name(steam_root: &Path, id: &str) -> Option<String> {
    let meta = std::fs::read_to_string(workshop_mod_dir(steam_root, id)?.join("meta.cpp")).ok()?;
    mod_display_name(None, Some(&meta))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn with_rows(project: Option<&Value>, rows: &[TestMod]) -> Value {
    let mut map = match project {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    if rows.is_empty() {
        map.remove(MODS_KEY);
    } else {
        map.insert(MODS_KEY.to_owned(), json!({ "Mods": rows }));
    }
    Value::Object(map)
}

fn write(db: &Path, root: &Path, project: &Value) -> Result<()> {
    Store::open(db)?.set_setting(
        &Scope::Workspace(project_id(root)),
        PREFS_PROJECT_KEY,
        project,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::canonical_or;

    #[test]
    fn a_missing_folder_resolves_through_its_parent() {
        let base = std::env::temp_dir().join(format!("mf-canonical-or-{}", std::process::id()));
        std::fs::create_dir_all(base.join("inner")).unwrap();
        let base = base.canonicalize().unwrap();

        let resolved = canonical_or(&base.join("inner/../Gone"));

        assert_eq!(resolved, base.join("Gone"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_bare_name_resolves_against_the_working_folder() {
        let name = "mf-missing-dependency-folder";
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();

        assert_eq!(canonical_or(std::path::Path::new(name)), here.join(name));
    }
}
