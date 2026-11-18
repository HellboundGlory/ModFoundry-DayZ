//! The saved dependency mods of a mod project: listing them and adding or removing one.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use modfoundry_config::mod_display_name;
use modfoundry_platform::workshop_mod_dir;
use modfoundry_store::settings::Scope;
use modfoundry_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::paths::{canonical_or, file_name, same_folder};
use crate::project_settings::{
    project_id, stored_mods_strict, ModRole, ModSource, ModUse, ProjectSettings, TestMod, MODS_KEY,
    PREFS_DEFAULTS_KEY, PREFS_PROJECT_KEY,
};
use crate::shared_deps::{self, is_workshop_id, Entry, MergedMod, Shared, SHARED_DEPS_FILE};

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
    if is_workshop_id(id) {
        DependencyRef::Workshop(id.to_owned())
    } else {
        DependencyRef::Folder(PathBuf::from(text))
    }
}

/// Where a saved dependency comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyOrigin {
    Project,
    Machine,
    Defaults,
}

/// A saved dependency mod, as the project's list holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDependency {
    pub id: String,
    pub name: String,
    pub source: ModSource,
    pub enabled: bool,
    pub origin: DependencyOrigin,
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

/// The saved dependency mods of the project at `root`: the shared file's, then this machine's.
/// `steam_root` is called only when a listed row is a Workshop item.
pub fn list(
    db: &Path,
    root: &Path,
    steam_root: impl FnOnce() -> Option<PathBuf>,
) -> Result<Vec<SavedDependency>> {
    require_folder(root)?;
    Ok(listed(&Prefs::read(Some(db), root)?, steam_root))
}

/// The dependency mods of the project at `root` when no database is set: the shared file's only.
pub fn list_shared(
    root: &Path,
    steam_root: impl FnOnce() -> Option<PathBuf>,
) -> Result<Vec<SavedDependency>> {
    require_folder(root)?;
    let prefs = Prefs {
        defaults: None,
        project: None,
        shared: shared_deps::load(root)?,
    };
    Ok(listed(&prefs, steam_root))
}

fn listed(prefs: &Prefs, steam_root: impl FnOnce() -> Option<PathBuf>) -> Vec<SavedDependency> {
    let steam_root = prefs.has_workshop_dependency().then(steam_root).flatten();
    prefs.dependencies(steam_root.as_deref())
}

#[derive(Clone)]
struct Prefs {
    defaults: Option<Value>,
    project: Option<Value>,
    shared: Shared,
}

impl Prefs {
    fn read(db: Option<&Path>, root: &Path) -> Result<Self> {
        let shared = shared_deps::load(root)?;
        let Some(db) = db.filter(|db| db.is_file()) else {
            return Ok(Self {
                defaults: None,
                project: None,
                shared,
            });
        };
        let store = Store::open(db)?;
        let prefs = Self {
            defaults: store.get_setting(&Scope::Global, PREFS_DEFAULTS_KEY)?,
            project: store.get_setting(&Scope::Workspace(project_id(root)), PREFS_PROJECT_KEY)?,
            shared,
        };
        stored_mods_strict(prefs.defaults.as_ref())
            .map_err(|error| anyhow!("the Defaults settings cannot be read: {error}"))?;
        stored_mods_strict(prefs.project.as_ref())?;
        Ok(prefs)
    }

    fn merged_rows(&self) -> Vec<MergedMod> {
        let machine =
            ProjectSettings::from_values(self.defaults.as_ref(), self.project.as_ref(), None)
                .test_mods;
        shared_deps::merge(self.shared.rows.clone(), machine)
    }

    fn defaults_rows(&self) -> Vec<TestMod> {
        let own = ProjectSettings::from_values(self.defaults.as_ref(), None, None).test_mods;
        self.merged_rows()
            .into_iter()
            .map(|merged| merged.entry)
            .filter(|row| own.iter().any(|default| default.id == row.id))
            .collect()
    }

    fn has_workshop_dependency(&self) -> bool {
        self.merged_rows().iter().any(|merged| {
            merged.entry.use_as == ModUse::Dependency
                && matches!(merged.entry.source, ModSource::Workshop { .. })
        })
    }

    fn dependencies(&self, steam_root: Option<&Path>) -> Vec<SavedDependency> {
        let defaults = self.defaults_rows();
        self.merged_rows()
            .into_iter()
            .filter(|merged| merged.entry.use_as == ModUse::Dependency)
            .map(|merged| {
                let origin = dependency_origin(&merged, &defaults);
                let row = merged.entry;
                let name = display_name(&row, steam_root);
                SavedDependency {
                    from_defaults: origin == DependencyOrigin::Defaults,
                    origin,
                    id: row.id,
                    name,
                    source: row.source,
                    enabled: row.enabled,
                }
            })
            .collect()
    }
}

fn dependency_origin(merged: &MergedMod, defaults: &[TestMod]) -> DependencyOrigin {
    if merged.shared {
        DependencyOrigin::Project
    } else if defaults.iter().any(|default| default.id == merged.entry.id) {
        DependencyOrigin::Defaults
    } else {
        DependencyOrigin::Machine
    }
}

/// Which saved list an edit changes: the mod's shared file, or this machine's list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditTarget {
    #[default]
    Project,
    Machine,
}

impl EditTarget {
    pub(crate) fn writes_file(self) -> bool {
        self == Self::Project
    }
}

/// What an edit does to the lists: a note when nothing changes, else the list it leaves.
enum Edit {
    Note(String),
    Machine(Vec<TestMod>),
    Shared(Vec<Entry>),
}

/// Adds or removes one dependency of the project at `root`; with `apply` false nothing is written.
pub fn edit(
    db: Option<&Path>,
    root: &Path,
    action: EditAction,
    target: EditTarget,
    dependency: &DependencyRef,
    steam_root: Option<&Path>,
    apply: bool,
) -> Result<EditOutcome> {
    require_folder(root)?;
    if target == EditTarget::Machine && db.is_none() {
        bail!("no database to save settings in");
    }
    let wanteds = wanted_for(dependency, root, action, steam_root)?;
    let prefs = Prefs::read(db, root)?;
    let before = prefs.dependencies(steam_root);
    let change = match target {
        EditTarget::Machine => machine_edit(&prefs, action, &wanteds, dependency, steam_root)?,
        EditTarget::Project => shared_edit(&prefs, root, action, &wanteds, dependency, steam_root)?,
    };
    let (after, note) = match change {
        Edit::Note(note) => (before.clone(), Some(note)),
        Edit::Machine(rows) => {
            let project = with_rows(prefs.project.as_ref(), &rows);
            let after = Prefs {
                project: Some(project.clone()),
                ..prefs.clone()
            }
            .dependencies(steam_root);
            if let (true, Some(db)) = (apply, db) {
                write(db, root, &project)?;
            }
            (after, None)
        }
        Edit::Shared(entries) => {
            let shared = Shared::from_entries(root, entries.clone())?;
            let after = Prefs {
                shared,
                ..prefs.clone()
            }
            .dependencies(steam_root);
            if apply {
                shared_deps::write(root, &entries)?;
            }
            (after, None)
        }
    };
    Ok(EditOutcome {
        before,
        after,
        note,
    })
}

/// The arguments a dependency names: as given (from the working folder), and for a folder remove
/// also under the mod folder, as the shared file stores it.
fn wanted_for(
    dependency: &DependencyRef,
    root: &Path,
    action: EditAction,
    steam_root: Option<&Path>,
) -> Result<Vec<Wanted>> {
    let mut wanteds = vec![Wanted::resolve(
        dependency,
        steam_root,
        action == EditAction::Add,
    )?];
    if let (EditAction::Remove, DependencyRef::Folder(path)) = (action, dependency) {
        wanteds.push(Wanted::resolve(
            &DependencyRef::Folder(root.join(path)),
            steam_root,
            false,
        )?);
    }
    Ok(wanteds)
}

fn machine_edit(
    prefs: &Prefs,
    action: EditAction,
    wanteds: &[Wanted],
    dependency: &DependencyRef,
    steam_root: Option<&Path>,
) -> Result<Edit> {
    let defaults = prefs.defaults_rows();
    let mut rows = stored_mods_strict(prefs.project.as_ref())?;
    if action == EditAction::Add {
        return Ok(match add(&mut rows, &defaults, &wanteds[0], steam_root)? {
            Some(note) => Edit::Note(note),
            None => Edit::Machine(rows),
        });
    }
    let text = dependency_text(dependency);
    let hits = removal(
        prefs,
        &rows,
        EditTarget::Machine,
        &text,
        wanteds,
        steam_root,
    )?;
    Ok(Edit::Machine(without(rows, &hits)))
}

fn shared_edit(
    prefs: &Prefs,
    root: &Path,
    action: EditAction,
    wanteds: &[Wanted],
    dependency: &DependencyRef,
    steam_root: Option<&Path>,
) -> Result<Edit> {
    let wanted = &wanteds[0];
    if action == EditAction::Add {
        if prefs
            .shared
            .rows
            .iter()
            .any(|row| is_same_mod(row, wanted, steam_root))
        {
            let note = format!("{} is already in the mod's {SHARED_DEPS_FILE}", wanted.name);
            return Ok(Edit::Note(note));
        }
        let mut entries = prefs.shared.entries.clone();
        entries.push(shared_deps::entry_for(root, &wanted.source));
        return Ok(Edit::Shared(entries));
    }
    let text = dependency_text(dependency);
    let machine = stored_mods_strict(prefs.project.as_ref())?;
    let hits = removal(
        prefs,
        &machine,
        EditTarget::Project,
        &text,
        wanteds,
        steam_root,
    )?;
    Ok(Edit::Shared(without(prefs.shared.entries.clone(), &hits)))
}

fn is_dependency(row: &TestMod) -> bool {
    row.use_as == ModUse::Dependency
}

/// The indices, in the list `target` edits, of the rows a dependency argument names. The argument is
/// matched against what `list` prints, so a name two listed rows share is refused. A row the edited
/// list lacks is reported with where it is.
fn removal(
    prefs: &Prefs,
    machine: &[TestMod],
    target: EditTarget,
    text: &str,
    wanteds: &[Wanted],
    steam_root: Option<&Path>,
) -> Result<Vec<usize>> {
    let listed: Vec<TestMod> = prefs
        .merged_rows()
        .into_iter()
        .map(|merged| merged.entry)
        .collect();
    let hits = named(&listed, text, wanteds, steam_root, is_dependency)?;
    let (own, other, elsewhere) = match target {
        EditTarget::Project => (
            &prefs.shared.rows[..],
            machine,
            "is on this machine's list; remove it with --machine (where: machine)".to_owned(),
        ),
        EditTarget::Machine => (
            machine,
            &prefs.shared.rows[..],
            format!(
                "is in the mod's {SHARED_DEPS_FILE}; remove it from there (leave out --machine, or where: project)"
            ),
        ),
    };
    let on = |rows: &[TestMod], listed: &TestMod| {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| is_dependency(row) && shared_deps::same_shared_mod(row, listed))
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };
    let own_hits: Vec<usize> = hits.iter().flat_map(|&hit| on(own, &listed[hit])).collect();
    if !own_hits.is_empty() {
        return Ok(own_hits);
    }
    let Some(&hit) = hits.first() else {
        bail!("{text} is not a saved dependency");
    };
    if !on(other, &listed[hit]).is_empty() {
        bail!("{text} {elsewhere}");
    }
    bail!(
        "{} is in the Defaults mods list; change it there",
        listed[hit].name
    );
}

/// The indices of the eligible rows a dependency argument names: the mod itself, by id or folder,
/// else the one row whose listed name is `text`.
fn named(
    rows: &[TestMod],
    text: &str,
    wanteds: &[Wanted],
    steam_root: Option<&Path>,
    eligible: impl Fn(&TestMod) -> bool,
) -> Result<Vec<usize>> {
    let eligible_rows = || {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| eligible(row))
            .map(|(index, _)| index)
    };
    let by_mod: Vec<usize> = eligible_rows()
        .filter(|&index| {
            wanteds
                .iter()
                .any(|wanted| is_same_mod(&rows[index], wanted, steam_root))
        })
        .collect();
    if !by_mod.is_empty() {
        return Ok(by_mod);
    }
    let by_name: Vec<usize> = eligible_rows()
        .filter(|&index| display_name(&rows[index], steam_root) == text)
        .collect();
    if by_name.len() > 1 {
        let places: Vec<String> = by_name
            .iter()
            .map(|&index| location(&rows[index]))
            .collect();
        bail!(
            "{text} is the name of {}; remove one by its path or Workshop id",
            places.join(" and ")
        );
    }
    Ok(by_name)
}

fn location(row: &TestMod) -> String {
    match &row.source {
        ModSource::Folder { path } => path.display().to_string(),
        ModSource::Workshop { id } => format!("workshop:{id}"),
    }
}

fn without<T>(items: Vec<T>, dropped: &[usize]) -> Vec<T> {
    items
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !dropped.contains(index))
        .map(|(_, item)| item)
        .collect()
}

/// The one-line summary of an edit: the note, why nothing changed, or which dependency moved.
pub fn describe_edit(
    outcome: &EditOutcome,
    action: EditAction,
    target: EditTarget,
    dependency: &str,
    root: &Path,
) -> String {
    if let Some(note) = &outcome.note {
        return note.clone();
    }
    let name = file_name(Path::new(dependency));
    if outcome.before == outcome.after {
        return match (target, action) {
            (EditTarget::Project, EditAction::Add) => format!(
                "added {name} to the mod's {SHARED_DEPS_FILE}; {} still takes its place",
                winning_row(&outcome.after, dependency, &name)
            ),
            (EditTarget::Project, EditAction::Remove) => format!(
                "removed {name} from the mod's {SHARED_DEPS_FILE}; this machine's list still has it"
            ),
            (EditTarget::Machine, _) => {
                format!("{name} is already a dependency of {}", root.display())
            }
        };
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

/// Names the listed row for `dependency` that wins over the file's: this machine's or a Defaults row.
fn winning_row(after: &[SavedDependency], dependency: &str, name: &str) -> String {
    let id = dependency.strip_prefix("workshop:").unwrap_or(dependency);
    let row = after
        .iter()
        .find(|row| row.id == id || row.name == dependency || row.name == name);
    match row {
        Some(row) if row.origin == DependencyOrigin::Defaults => {
            format!("the Defaults row for {}", row.name)
        }
        Some(row) => format!("this machine's row for {}", row.name),
        None => "this machine's list".to_owned(),
    }
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

fn defaults_check(
    defaults: &[TestMod],
    wanted: &Wanted,
    steam_root: Option<&Path>,
) -> Result<Option<String>> {
    let Some(row) = defaults
        .iter()
        .find(|row| is_same_mod(row, wanted, steam_root))
    else {
        return Ok(None);
    };
    let note = format!("{} is already in the Defaults mods list", row.name);
    if row.enabled && row.use_as == ModUse::Dependency {
        return Ok(Some(note));
    }
    bail!("{note}");
}

fn add(
    rows: &mut Vec<TestMod>,
    defaults: &[TestMod],
    wanted: &Wanted,
    steam_root: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(note) = defaults_check(defaults, wanted, steam_root)? {
        return Ok(Some(note));
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
    use super::{
        edit, list, parse_dependency, DependencyOrigin, DependencyRef, EditAction, EditTarget,
    };
    use crate::project_settings::{project_id, MODS_KEY, PREFS_DEFAULTS_KEY, PREFS_PROJECT_KEY};
    use modfoundry_store::settings::Scope;
    use modfoundry_store::Store;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    const SHARED_FILE: &str = ".modfoundry/dependencies.json";

    /// A base folder with a mod folder `Mod` in it and a database path that is not created yet.
    fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("mf-deps-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let base = std::fs::create_dir_all(&base)
            .and_then(|()| base.canonicalize())
            .unwrap();
        let root = base.join("Mod");
        std::fs::create_dir_all(&root).unwrap();
        (base.clone(), root, base.join("agents.db"))
    }

    fn dep_folder(base: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn a_folder_added_to_the_project_is_written_to_the_shared_file() {
        let (base, root, db) = fixture("file-add");
        let dep = dep_folder(&base, "Dep");

        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let text = std::fs::read_to_string(root.join(SHARED_FILE)).unwrap();
        assert_eq!(
            text,
            "{\n  \"dependencies\": [\n    {\n      \"folder\": \"../Dep\"\n    }\n  ]\n}\n"
        );
        assert!(!db.exists());
    }

    #[test]
    fn a_workshop_id_added_to_the_project_is_removed_by_its_prefixed_form() {
        let (_base, root, db) = fixture("file-workshop");
        let add = |action, dependency: DependencyRef| {
            edit(
                Some(&db),
                &root,
                action,
                EditTarget::Project,
                &dependency,
                None,
                true,
            )
            .unwrap()
        };

        add(
            EditAction::Add,
            DependencyRef::Workshop("1559212036".to_owned()),
        );
        let listed = list(&db, &root, || None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].origin, DependencyOrigin::Project);

        add(EditAction::Remove, parse_dependency("workshop:1559212036"));
        assert!(list(&db, &root, || None).unwrap().is_empty());
    }

    #[test]
    fn a_folder_in_the_project_file_is_removed_by_the_name_list_prints() {
        let (base, root, db) = fixture("file-remove-name");
        let dep = dep_folder(&base, "Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &parse_dependency("Dep"),
            None,
            true,
        )
        .unwrap();

        assert!(list(&db, &root, || None).unwrap().is_empty());
    }

    fn folder_row(path: &Path) -> serde_json::Value {
        json!({
            "id": path.display().to_string(),
            "name": path.file_name().unwrap().to_string_lossy(),
            "source": {"Folder": {"path": path}},
            "role": "Both",
            "use_as": "Dependency",
            "enabled": true,
        })
    }

    #[test]
    fn a_name_two_listed_dependencies_share_is_refused_with_both_places() {
        let (base, root, db) = fixture("file-same-name");
        for side in ["A", "B"] {
            let core = dep_folder(&base, &format!("{side}/Core"));
            edit(
                Some(&db),
                &root,
                EditAction::Add,
                EditTarget::Project,
                &DependencyRef::Folder(core),
                None,
                true,
            )
            .unwrap();
        }

        let error = edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &parse_dependency("Core"),
            None,
            true,
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.contains(&base.join("A/Core").display().to_string()),
            "{error}"
        );
        assert!(
            error.contains(&base.join("B/Core").display().to_string()),
            "{error}"
        );
        assert_eq!(list(&db, &root, || None).unwrap().len(), 2);
    }

    #[test]
    fn removing_a_machine_only_dependency_from_the_file_points_to_machine() {
        let (base, root, db) = fixture("machine-only-remove");
        let dep = dep_folder(&base, "Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let error = edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &parse_dependency("Dep"),
            None,
            true,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--machine"), "{error}");
        assert_eq!(list(&db, &root, || None).unwrap().len(), 1);
    }

    #[test]
    fn a_machine_add_saves_to_the_database_and_leaves_the_file_alone() {
        let (base, root, db) = fixture("machine-add");
        let dep = dep_folder(&base, "Dep");

        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        assert!(db.is_file());
        assert!(!root.join(SHARED_FILE).exists());
        assert_eq!(list(&db, &root, || None).unwrap().len(), 1);
    }

    #[test]
    fn a_shared_file_that_does_not_parse_is_refused_and_left_as_it_is() {
        let (base, root, db) = fixture("file-broken");
        let dep = dep_folder(&base, "Dep");
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        let broken = "{\"dependencies\": [";
        std::fs::write(root.join(SHARED_FILE), broken).unwrap();

        let result = edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(root.join(SHARED_FILE)).unwrap(),
            broken
        );
    }

    #[test]
    fn a_dependency_already_in_the_file_is_reported_and_the_file_is_not_rewritten() {
        let (base, root, db) = fixture("file-already");
        let dep = dep_folder(&base, "Dep");
        let add = || {
            edit(
                Some(&db),
                &root,
                EditAction::Add,
                EditTarget::Project,
                &DependencyRef::Folder(dep.clone()),
                None,
                true,
            )
            .unwrap()
        };
        add();
        let written = std::fs::read_to_string(root.join(SHARED_FILE)).unwrap();
        std::fs::write(root.join(SHARED_FILE), format!("{written}\n\n")).unwrap();
        let hand_edited = std::fs::read_to_string(root.join(SHARED_FILE)).unwrap();

        let outcome = add();

        assert_eq!(
            outcome.note.as_deref(),
            Some("Dep is already in the mod's .modfoundry/dependencies.json")
        );
        assert_eq!(
            std::fs::read_to_string(root.join(SHARED_FILE)).unwrap(),
            hand_edited
        );
    }

    #[test]
    fn adding_a_machine_dependency_to_the_file_says_this_machine_still_wins() {
        let (base, root, db) = fixture("file-shadowed");
        let dep = dep_folder(&base, "Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Folder(dep.clone()),
            None,
            true,
        )
        .unwrap();

        let outcome = edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let text =
            super::describe_edit(&outcome, EditAction::Add, EditTarget::Project, "Dep", &root);
        assert_eq!(
            text,
            "added Dep to the mod's .modfoundry/dependencies.json; this machine's row for Dep still takes its place"
        );
        assert!(root.join(SHARED_FILE).is_file());
    }

    #[test]
    fn a_file_dependency_a_test_only_default_also_holds_is_still_listed_and_removable() {
        let (base, root, db) = fixture("file-test-only");
        let dep = dep_folder(&base, "Dep");
        let mut default_row = folder_row(&dep);
        default_row["use_as"] = json!("TestOnly");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [default_row]}}),
            )
            .unwrap();
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        std::fs::write(
            root.join(SHARED_FILE),
            r#"{"dependencies":[{"folder":"../Dep"}]}"#,
        )
        .unwrap();

        let listed = list(&db, &root, || None).unwrap();
        let names: Vec<&str> = listed.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["Dep"]);

        let removed = edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();
        assert!(removed.after.is_empty());
    }

    #[test]
    fn the_file_add_summary_names_a_defaults_row_that_takes_its_place() {
        let (base, root, db) = fixture("file-defaults-winner");
        let dep = dep_folder(&base, "Dep");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&dep)]}}),
            )
            .unwrap();

        let outcome = edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let text =
            super::describe_edit(&outcome, EditAction::Add, EditTarget::Project, "Dep", &root);
        assert_eq!(
            text,
            "added Dep to the mod's .modfoundry/dependencies.json; the Defaults row for Dep still takes its place"
        );
    }

    #[test]
    fn a_test_only_machine_row_for_a_file_dependency_is_removed_from_the_file_not_the_machine() {
        let (base, root, db) = fixture("file-test-only-machine");
        let dep = dep_folder(&base, "Dep");
        let mut machine_row = folder_row(&dep);
        machine_row["use_as"] = json!("TestOnly");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Workspace(project_id(&root)),
                PREFS_PROJECT_KEY,
                &json!({MODS_KEY: {"Mods": [machine_row]}}),
            )
            .unwrap();
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        std::fs::write(
            root.join(SHARED_FILE),
            r#"{"dependencies":[{"folder":"../Dep"}]}"#,
        )
        .unwrap();

        let error = edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Machine,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap_err();

        assert!(error.to_string().contains("is in the mod's"), "{error}");
    }

    #[test]
    fn a_preview_of_a_file_edit_writes_nothing() {
        let (base, root, db) = fixture("file-preview");
        let dep = dep_folder(&base, "Dep");

        let outcome = edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            false,
        )
        .unwrap();

        assert_eq!(outcome.after.len(), 1);
        assert!(!root.join(SHARED_FILE).exists());
    }

    #[test]
    fn a_folder_in_the_file_is_removed_by_a_path_relative_to_the_mod_folder() {
        let (base, root, db) = fixture("file-remove-relative");
        let dep = dep_folder(&base, "Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &parse_dependency("../Dep"),
            None,
            true,
        )
        .unwrap();

        assert!(list(&db, &root, || None).unwrap().is_empty());
    }

    #[test]
    fn a_name_a_defaults_row_shares_with_a_file_entry_is_refused_and_the_path_removes_it() {
        let (base, root, db) = fixture("file-remove-defaults-name");
        let dep = dep_folder(&base, "Dep");
        let other = dep_folder(&base, "Other/Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep.clone()),
            None,
            true,
        )
        .unwrap();
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&other)]}}),
            )
            .unwrap();

        let error = edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &parse_dependency("Dep"),
            None,
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(&dep.display().to_string()), "{error}");
        assert!(error.contains(&other.display().to_string()), "{error}");

        edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        assert!(list(&db, &root, || None)
            .unwrap()
            .iter()
            .all(|row| row.origin == DependencyOrigin::Defaults));
    }

    #[test]
    fn a_file_entry_is_removed_even_when_the_defaults_hold_the_same_mod() {
        let (base, root, db) = fixture("file-remove-defaults-same");
        let dep = dep_folder(&base, "Dep");
        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep.clone()),
            None,
            true,
        )
        .unwrap();
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&dep)]}}),
            )
            .unwrap();

        edit(
            Some(&db),
            &root,
            EditAction::Remove,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let text = std::fs::read_to_string(root.join(SHARED_FILE)).unwrap();
        assert!(!text.contains("Dep"), "{text}");
    }

    #[test]
    fn a_file_edit_needs_no_database_but_a_machine_edit_does() {
        let (base, root, _db) = fixture("no-database");
        let dep = dep_folder(&base, "Dep");

        edit(
            None,
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep.clone()),
            None,
            true,
        )
        .unwrap();
        let error = edit(
            None,
            &root,
            EditAction::Add,
            EditTarget::Machine,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap_err();

        assert!(root.join(SHARED_FILE).is_file());
        assert!(error.to_string().contains("no database"), "{error}");
    }

    #[test]
    fn a_mod_in_the_defaults_can_still_be_added_to_the_shared_file() {
        let (base, root, db) = fixture("file-add-defaults");
        let dep = dep_folder(&base, "Dep");
        Store::open(&db)
            .unwrap()
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&dep)]}}),
            )
            .unwrap();

        edit(
            Some(&db),
            &root,
            EditAction::Add,
            EditTarget::Project,
            &DependencyRef::Folder(dep),
            None,
            true,
        )
        .unwrap();

        let text = std::fs::read_to_string(root.join(SHARED_FILE)).unwrap();
        assert!(text.contains("../Dep"), "{text}");
    }

    #[test]
    fn each_dependency_says_whether_the_file_this_machine_or_the_defaults_gave_it() {
        let dir = std::env::temp_dir().join(format!("mf-deps-origin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = std::fs::create_dir_all(&dir)
            .and_then(|()| dir.canonicalize())
            .unwrap();
        let root = dir.join("Mod");
        let folders: [PathBuf; 3] =
            ["FileDep", "MachineDep", "DefaultDep"].map(|name| dir.join(name));
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        for folder in &folders {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(
            root.join(".modfoundry/dependencies.json"),
            r#"{"dependencies":[{"folder":"../FileDep"}]}"#,
        )
        .unwrap();
        let db = dir.join("agents.db");
        let store = Store::open(&db).unwrap();
        store
            .set_setting(
                &Scope::Global,
                PREFS_DEFAULTS_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&folders[2])]}}),
            )
            .unwrap();
        store
            .set_setting(
                &Scope::Workspace(project_id(&root)),
                PREFS_PROJECT_KEY,
                &json!({MODS_KEY: {"Mods": [folder_row(&folders[1])]}}),
            )
            .unwrap();

        let listed = list(&db, &root, || None).unwrap();
        let origin_of = |name: &str| {
            listed
                .iter()
                .find(|row| row.name == name)
                .map(|row| row.origin)
        };

        assert_eq!(origin_of("FileDep"), Some(DependencyOrigin::Project));
        assert_eq!(origin_of("MachineDep"), Some(DependencyOrigin::Machine));
        assert_eq!(origin_of("DefaultDep"), Some(DependencyOrigin::Defaults));
    }

    #[test]
    fn an_edit_summarises_the_shared_rows_too() {
        let dir = std::env::temp_dir().join(format!("mf-deps-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = std::fs::create_dir_all(&dir)
            .and_then(|()| dir.canonicalize())
            .unwrap();
        let root = dir.join("Mod");
        let shared_dep = dir.join("FileDep");
        let new_dep = dir.join("NewDep");
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        std::fs::create_dir_all(&shared_dep).unwrap();
        std::fs::create_dir_all(&new_dep).unwrap();
        std::fs::write(
            root.join(".modfoundry/dependencies.json"),
            r#"{"dependencies":[{"folder":"../FileDep"}]}"#,
        )
        .unwrap();
        let db = dir.join("agents.db");

        let outcome = super::edit(
            Some(&db),
            &root,
            super::EditAction::Add,
            super::EditTarget::Project,
            &super::DependencyRef::Folder(new_dep),
            None,
            false,
        )
        .unwrap();

        let names = |rows: &[super::SavedDependency]| -> Vec<String> {
            rows.iter().map(|row| row.name.clone()).collect()
        };
        assert_eq!(names(&outcome.before), ["FileDep"]);
        assert_eq!(names(&outcome.after), ["FileDep", "NewDep"]);
    }

    #[test]
    fn listing_without_a_database_still_shows_the_shared_file() {
        let dir = std::env::temp_dir().join(format!("mf-deps-nodb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = std::fs::create_dir_all(&dir)
            .and_then(|()| dir.canonicalize())
            .unwrap();
        let root = dir.join("Mod");
        std::fs::create_dir_all(root.join(".modfoundry")).unwrap();
        std::fs::create_dir_all(dir.join("FileDep")).unwrap();
        std::fs::write(
            root.join(".modfoundry/dependencies.json"),
            r#"{"dependencies":[{"folder":"../FileDep"}]}"#,
        )
        .unwrap();

        let listed = super::list_shared(&root, || None).unwrap();

        let names: Vec<&str> = listed.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["FileDep"]);
    }
}
