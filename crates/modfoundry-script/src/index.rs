//! Global symbol index over indexed files: modded class chains, member
//! lookup and existence checks.

use crate::items::{ClassItem, EnumItem, FieldItem, FileItems, MethodItem, TypeRef, TypedefItem};
use crate::layer::Layer;
use modfoundry_core::Span;
use std::collections::{HashMap, HashSet};

pub(crate) const MAX_DEPTH: usize = 16;

/// Where an indexed file comes from. Ord orders files within one module
/// layer: vanilla, then dependencies by load index, then the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Vanilla,
    Dependency(u16),
    Workspace,
}

/// Identity of one indexed file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileKey {
    pub id: String,
    pub origin: Origin,
    pub layer: Option<Layer>,
}

/// A resolvable location: one file and a byte span in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub file: String,
    pub span: Span,
    pub origin: Origin,
    pub layer: Option<Layer>,
}

/// One class declaration in a [`Index::class_chain`], with the file it came
/// from.
pub struct ClassLink<'a> {
    pub key: &'a FileKey,
    pub class: &'a ClassItem,
}

/// One field or method definition found by [`Index::find_members`].
#[derive(Clone, Copy)]
pub enum MemberKind<'a> {
    Field(&'a FieldItem),
    Method(&'a MethodItem),
}

/// A member definition with the link it was found on.
pub struct MemberDef<'a> {
    pub key: &'a FileKey,
    pub owner: &'a str,
    pub member: MemberKind<'a>,
}

/// Index-wide counts (status bar chip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexStats {
    pub files: usize,
    pub classes: usize,
}

const PRIMITIVES: &[&str] = &[
    "int", "float", "bool", "string", "vector", "void", "typename", "Class", "auto", "func",
];

pub(crate) fn is_primitive(name: &str) -> bool {
    PRIMITIVES.contains(&name)
}

struct FileEntry {
    key: FileKey,
    items: FileItems,
}

/// Module layer first, files outside a numbered folder last, then origin and
/// id. The maximum is the declaration a later-loaded file overrides with.
fn load_order(key: &FileKey) -> (bool, Option<Layer>, Origin, &str) {
    (key.layer.is_none(), key.layer, key.origin, key.id.as_str())
}

/// Every field of `class` in order, then every method, as `(name, definition)`.
pub(crate) fn class_members<'a>(
    key: &'a FileKey,
    class: &'a ClassItem,
) -> impl Iterator<Item = (&'a str, MemberDef<'a>)> {
    let fields = class.fields.iter().map(move |f| {
        (
            f.name.as_str(),
            MemberDef {
                key,
                owner: class.name.as_str(),
                member: MemberKind::Field(f),
            },
        )
    });
    let methods = class.methods.iter().map(move |m| {
        (
            m.name.as_str(),
            MemberDef {
                key,
                owner: class.name.as_str(),
                member: MemberKind::Method(m),
            },
        )
    });
    fields.chain(methods)
}

/// Name → owning file ids, one map per declaration kind, so a lookup never
/// scans every indexed file (the corpus is ~3,000 files).
#[derive(Default)]
struct NameIndex {
    classes: HashMap<String, HashSet<String>>,
    enums: HashMap<String, HashSet<String>>,
    typedefs: HashMap<String, HashSet<String>>,
    functions: HashMap<String, HashSet<String>>,
    globals: HashMap<String, HashSet<String>>,
}

impl NameIndex {
    fn insert(&mut self, id: &str, items: &FileItems) {
        Self::index(
            &mut self.classes,
            id,
            items.classes.iter().map(|c| c.name.as_str()),
        );
        Self::index(
            &mut self.enums,
            id,
            items.enums.iter().map(|e| e.name.as_str()),
        );
        Self::index(
            &mut self.typedefs,
            id,
            items.typedefs.iter().map(|t| t.name.as_str()),
        );
        Self::index(
            &mut self.functions,
            id,
            items.functions.iter().map(|f| f.name.as_str()),
        );
        Self::index(
            &mut self.globals,
            id,
            items.globals.iter().map(|g| g.name.as_str()),
        );
    }

    fn remove(&mut self, id: &str, items: &FileItems) {
        Self::unindex(
            &mut self.classes,
            id,
            items.classes.iter().map(|c| c.name.as_str()),
        );
        Self::unindex(
            &mut self.enums,
            id,
            items.enums.iter().map(|e| e.name.as_str()),
        );
        Self::unindex(
            &mut self.typedefs,
            id,
            items.typedefs.iter().map(|t| t.name.as_str()),
        );
        Self::unindex(
            &mut self.functions,
            id,
            items.functions.iter().map(|f| f.name.as_str()),
        );
        Self::unindex(
            &mut self.globals,
            id,
            items.globals.iter().map(|g| g.name.as_str()),
        );
    }

    fn index<'a>(
        map: &mut HashMap<String, HashSet<String>>,
        id: &str,
        names: impl Iterator<Item = &'a str>,
    ) {
        for name in names {
            map.entry(name.to_owned())
                .or_default()
                .insert(id.to_owned());
        }
    }

    fn unindex<'a>(
        map: &mut HashMap<String, HashSet<String>>,
        id: &str,
        names: impl Iterator<Item = &'a str>,
    ) {
        for name in names {
            if let Some(ids) = map.get_mut(name) {
                ids.remove(id);
                if ids.is_empty() {
                    map.remove(name);
                }
            }
        }
    }
}

/// In-memory global symbol index: built once at
/// workspace open and updated per-file on edit. Holds signatures only, never
/// method bodies, so an edit inside a body never invalidates it.
#[derive(Default)]
pub struct Index {
    files: HashMap<String, FileEntry>,
    names: NameIndex,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace this id's items; a re-set never leaves stale members behind.
    pub fn set_file(&mut self, key: FileKey, items: FileItems) {
        self.remove_file(&key.id);
        self.names.insert(&key.id, &items);
        self.files.insert(key.id.clone(), FileEntry { key, items });
    }

    pub fn remove_file(&mut self, id: &str) {
        if let Some(entry) = self.files.remove(id) {
            self.names.remove(id, &entry.items);
        }
    }

    pub fn file(&self, id: &str) -> Option<(&FileKey, &FileItems)> {
        self.files.get(id).map(|e| (&e.key, &e.items))
    }

    fn files_named<'a>(
        &'a self,
        ids: Option<&'a HashSet<String>>,
    ) -> impl Iterator<Item = &'a FileEntry> {
        ids.into_iter()
            .flatten()
            .filter_map(|id| self.files.get(id))
    }

    /// The entry that wins for `ids`: the last-loaded declaration. A name set
    /// is an unordered `HashSet`, so picking an arbitrary entry would make
    /// resolution depend on map iteration order.
    fn last_loaded<'a>(&'a self, ids: Option<&'a HashSet<String>>) -> Option<&'a FileEntry> {
        self.files_named(ids)
            .max_by(|a, b| load_order(&a.key).cmp(&load_order(&b.key)))
    }

    fn def_named<'a, T>(
        &'a self,
        ids: Option<&'a HashSet<String>>,
        items: impl Fn(&'a FileItems) -> &'a [T],
        name_of: impl Fn(&T) -> &str,
        name: &str,
    ) -> Option<(&'a FileKey, &'a T)> {
        let entry = self.last_loaded(ids)?;
        items(&entry.items)
            .iter()
            .find(|item| name_of(item) == name)
            .map(|item| (&entry.key, item))
    }

    /// The class chain for `name`: the non-modded definition first (if
    /// indexed), then every `modded` link in load order (see `load_order`).
    pub fn class_chain(&self, name: &str) -> Vec<ClassLink<'_>> {
        let mut all: Vec<ClassLink> = self
            .files_named(self.names.classes.get(name))
            .flat_map(|e| {
                e.items
                    .classes
                    .iter()
                    .filter(|c| c.name == name)
                    .map(move |c| ClassLink {
                        key: &e.key,
                        class: c,
                    })
            })
            .collect();
        all.sort_by(|a, b| load_order(a.key).cmp(&load_order(b.key)));
        let (base, modded): (Vec<_>, Vec<_>) = all.into_iter().partition(|l| !l.class.modded);
        base.into_iter().chain(modded).collect()
    }

    /// The base of the non-modded `class X extends B` definition, typedef
    /// aliases followed, the last-loaded one when `X` is declared more than
    /// once.
    pub fn base_class(&self, name: &str) -> Option<&str> {
        self.base_target(name).map(|base| base.name.as_str())
    }

    /// [`Index::base_ref`] with typedef aliases in the base followed.
    pub(crate) fn base_target(&self, name: &str) -> Option<&TypeRef> {
        self.base_ref(name).map(|b| self.expand_typedef(b))
    }

    /// `ty` with any typedef alias in its name followed to its target, at
    /// most [`MAX_DEPTH`] hops. A class of the same name wins over a typedef
    /// (vanilla has `typedef int[] PhysicsJoint;` beside `class PhysicsJoint`).
    pub(crate) fn expand_typedef<'a>(&'a self, ty: &'a TypeRef) -> &'a TypeRef {
        let mut ty = ty;
        for _ in 0..MAX_DEPTH {
            if !self.class_chain(&ty.name).is_empty() {
                break;
            }
            match self.typedef_def(&ty.name) {
                Some((_, t)) => ty = &t.target,
                None => break,
            }
        }
        ty
    }

    pub(crate) fn base_ref(&self, name: &str) -> Option<&TypeRef> {
        self.files_named(self.names.classes.get(name))
            .filter_map(|entry| {
                entry
                    .items
                    .classes
                    .iter()
                    .find(|c| c.name == name && !c.modded)
                    .map(|class| (entry, class))
            })
            .max_by(|(a, _), (b, _)| load_order(&a.key).cmp(&load_order(&b.key)))
            .and_then(|(_, class)| class.base.as_ref())
    }

    /// Every field/method named `name` visible from `class`: its chain,
    /// last link to first, then the base class's own chain, and so on.
    /// Guards inheritance cycles and depth.
    pub fn find_members(&self, class: &str, name: &str) -> Vec<MemberDef<'_>> {
        let mut out = Vec::new();
        self.walk_members(
            class,
            &mut |member_name, def| {
                if member_name == name {
                    out.push(def);
                }
            },
            &mut HashSet::new(),
            0,
        );
        out
    }

    /// Every field/method visible from `class`'s modded chain (most-derived
    /// link wins per name), then its base class's own chain, recursively
    /// (completion's member listing when a receiver type is known).
    pub fn members_of(&self, class: &str) -> Vec<MemberDef<'_>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk_members(
            class,
            &mut |member_name, def| {
                if seen.insert(member_name) {
                    out.push(def);
                }
            },
            &mut HashSet::new(),
            0,
        );
        out
    }

    fn walk_members<'a>(
        &'a self,
        class: &str,
        visit: &mut impl FnMut(&'a str, MemberDef<'a>),
        visited: &mut HashSet<String>,
        depth: usize,
    ) {
        if depth > 64 || !visited.insert(class.to_owned()) {
            return;
        }
        let chain = self.class_chain(class);
        for link in chain.iter().rev() {
            for (member_name, def) in class_members(link.key, link.class) {
                visit(member_name, def);
            }
        }
        if let Some(base) = self.base_class(class) {
            self.walk_members(base, visit, visited, depth + 1);
        }
    }

    /// Every field/method of every indexed class, deduped by name (the
    /// last-loaded definition wins): the no-inference completion fallback. Never used per keystroke when a receiver type is
    /// known; this scans every indexed class.
    pub fn all_members(&self) -> Vec<MemberDef<'_>> {
        let mut entries: Vec<&FileEntry> = self.files.values().collect();
        entries.sort_unstable_by(|a, b| load_order(&b.key).cmp(&load_order(&a.key)));
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for entry in entries {
            for class in &entry.items.classes {
                for (member_name, def) in class_members(&entry.key, class) {
                    if seen.insert(member_name.to_owned()) {
                        out.push(def);
                    }
                }
            }
        }
        out
    }

    /// Every indexed class name (bare-prefix completion; O(1) per name via
    /// the name index, never a per-file scan).
    pub fn class_names(&self) -> impl Iterator<Item = &str> {
        self.names.classes.keys().map(String::as_str)
    }

    pub fn enum_names(&self) -> impl Iterator<Item = &str> {
        self.names.enums.keys().map(String::as_str)
    }

    pub fn typedef_names(&self) -> impl Iterator<Item = &str> {
        self.names.typedefs.keys().map(String::as_str)
    }

    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.names.functions.keys().map(String::as_str)
    }

    /// True when an indexed file declares a class named `name`.
    pub(crate) fn has_class(&self, name: &str) -> bool {
        self.names.classes.contains_key(name)
    }

    /// True for indexed classes, enums and typedefs (not primitives).
    pub(crate) fn declares_type(&self, name: &str) -> bool {
        self.has_class(name)
            || self.names.enums.contains_key(name)
            || self.names.typedefs.contains_key(name)
    }

    /// True for primitives, indexed classes, enums and typedefs. A class's
    /// own generic parameter names are not global.
    pub fn type_exists(&self, name: &str) -> bool {
        is_primitive(name) || self.declares_type(name)
    }

    pub fn enum_def(&self, name: &str) -> Option<(&FileKey, &EnumItem)> {
        self.def_named(
            self.names.enums.get(name),
            |items| items.enums.as_slice(),
            |x| x.name.as_str(),
            name,
        )
    }

    pub fn typedef_def(&self, name: &str) -> Option<(&FileKey, &TypedefItem)> {
        self.def_named(
            self.names.typedefs.get(name),
            |items| items.typedefs.as_slice(),
            |x| x.name.as_str(),
            name,
        )
    }

    pub fn function(&self, name: &str) -> Option<(&FileKey, &MethodItem)> {
        self.def_named(
            self.names.functions.get(name),
            |items| items.functions.as_slice(),
            |m| m.name.as_str(),
            name,
        )
    }

    pub fn global(&self, name: &str) -> Option<(&FileKey, &FieldItem)> {
        self.def_named(
            self.names.globals.get(name),
            |items| items.globals.as_slice(),
            |f| f.name.as_str(),
            name,
        )
    }

    pub fn stats(&self) -> IndexStats {
        IndexStats {
            files: self.files.len(),
            classes: self.names.classes.len(),
        }
    }
}
