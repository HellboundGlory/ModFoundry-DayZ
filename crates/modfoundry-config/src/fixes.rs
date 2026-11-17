//! Quick fixes for config.cpp diagnostics: a fix is a titled list of
//! [`EditOp`]s plus the checks the GUI resolves after planning it. Pure: the
//! text, one diagnostic and optional external facts in, ops out.

use crate::ast::{
    classes, required_addon_properties, required_addons, ConfigClass, ConfigFile, ConfigProperty,
    Member, Value,
};
use crate::edit::{ConfigPath, EditError, EditOp, NewValue, Position, Target};
use crate::model::ModuleSlot;
use crate::parser::parse;
use crate::schema;
use modfoundry_core::text::{edit_distance_ignore_ascii_case, line_col};
use modfoundry_core::{Diagnostic, Span};

/// One offered fix: its title, the ops that apply it, and its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub ops: Vec<EditOp>,
    pub checks: Vec<FixCheck>,
}

/// A post-condition the GUI verifies after planning a fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixCheck {
    /// A source-root-relative directory the fix points at.
    DirExists(String),
    /// A diagnostic code the fix should also clear.
    Clears(String),
}

/// External facts a fix may need, from `config.resolve` and `config.patches`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixFacts {
    pub owner_patch_of_base: Option<(String, String)>,
    pub dependency_patches: Vec<(String, Vec<String>)>,
}

/// The diagnostic codes [`fixes_for`] can answer.
pub fn fixable_codes() -> &'static [&'static str] {
    &[
        "MF-C300", "MF-C310", "MF-C330", "MF-C340", "MF-C341", "MF-C350", "MF-C400",
    ]
}

/// The fixes for one diagnostic; empty when the code has no fix or the span no
/// longer points at the construct the fix rewrites.
pub fn fixes_for(text: &str, diagnostic: &Diagnostic, facts: &FixFacts) -> Vec<Fix> {
    if diagnostic.code == "MF-C400" {
        return c400(text, diagnostic, facts);
    }
    let Some(span) = diagnostic.span else {
        return Vec::new();
    };
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Vec::new();
    }
    let file = &parsed.file;
    match diagnostic.code.as_str() {
        "MF-C300" => c300(file, span),
        "MF-C310" => c310(file, span),
        "MF-C330" => c330(text, file, span),
        "MF-C340" => c340(text, file, span, facts),
        "MF-C341" => c341(text, file, span),
        "MF-C350" => c350(file, span),
        _ => None,
    }
    .unwrap_or_default()
}

/// Appends `names` to `patch`'s `requiredAddons[]`, creating the array when it
/// is missing; names already listed (case-insensitive) are skipped.
pub fn add_required_addons(
    text: &str,
    patch: Option<&str>,
    names: &[String],
) -> Result<Vec<EditOp>, EditError> {
    let mut wanted: Vec<&String> = Vec::new();
    for name in names {
        if !wanted.iter().any(|w| w.eq_ignore_ascii_case(name)) {
            wanted.push(name);
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Err(EditError::Unparseable);
    }
    let class = patch_class(&parsed.file, patch)?;
    let class_path = ConfigPath(vec!["CfgPatches".to_owned(), class.name.clone()]);
    let existing = required_addon_properties(class).next();
    let listed: Vec<&str> = required_addons(class)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let todo: Vec<&String> = wanted
        .into_iter()
        .filter(|n| !listed.iter().any(|l| l.eq_ignore_ascii_case(n)))
        .collect();
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    match existing {
        None => Ok(vec![EditOp::AddProperty {
            class: class_path,
            name: "requiredAddons".to_owned(),
            value: NewValue::Array(todo.into_iter().map(|n| NewValue::Str(n.clone())).collect()),
            position: Position::Last,
        }]),
        Some(property) => {
            let len = property.spans.value_items.len();
            Ok(todo
                .into_iter()
                .enumerate()
                .map(|(i, name)| EditOp::ArrayInsert {
                    property: Target::Property {
                        class: class_path.clone(),
                        name: property.name.clone(),
                    },
                    index: len + i,
                    value: NewValue::Str(name.clone()),
                })
                .collect())
        }
    }
}

fn c300(file: &ConfigFile, span: Span) -> Option<Vec<Fix>> {
    let located = property_at(file, span)?;
    let (index, path) = files_item(&located)?;
    if path.contains('\\') {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    let leaf = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let dirs = canonical_dirs();
    if dirs.contains(&leaf) {
        return None;
    }
    let mut dir = None;
    for candidate in dirs {
        if !within_one(candidate, leaf) {
            continue;
        }
        if dir.is_some() {
            return None;
        }
        dir = Some(candidate);
    }
    let dir = dir?;
    let new_path = replace_leaf(trimmed, dir);
    Some(vec![Fix {
        title: format!("Use `{dir}`"),
        ops: array_set(&located, index, &new_path),
        checks: Vec::new(),
    }])
}

fn c310(file: &ConfigFile, span: Span) -> Option<Vec<Fix>> {
    let located = property_at(file, span)?;
    let (index, path) = files_item(&located)?;
    if path.contains('\\') {
        return None;
    }
    let dir = located
        .class
        .last()
        .and_then(|name| ModuleSlot::from_defs_name(name))
        .map(ModuleSlot::canonical_dir)?;
    let new_path = replace_leaf(path.trim_end_matches('/'), dir);
    if new_path == path {
        return None;
    }
    Some(vec![Fix {
        title: format!("Point files[] at `{new_path}`"),
        ops: array_set(&located, index, &new_path),
        checks: vec![
            FixCheck::DirExists(new_path),
            FixCheck::Clears("MF-C320".to_owned()),
        ],
    }])
}

fn c330(text: &str, file: &ConfigFile, span: Span) -> Option<Vec<Fix>> {
    let located = property_at(file, span)?;
    if located.item.is_some() {
        return None;
    }
    let members = body(file, &located.class)?;
    let siblings: Vec<&Member> = members
        .iter()
        .filter(|m| {
            !matches!(m, Member::Delete { .. })
                && m.name().eq_ignore_ascii_case(&located.property.name)
        })
        .collect();
    let this = siblings.iter().position(|m| m.name_span() == span)?;
    let other = (0..siblings.len()).find(|i| *i != this)?;
    let class = ConfigPath(located.class.clone());
    let remove = |occurrence: usize| {
        vec![EditOp::RemoveAt {
            class: class.clone(),
            name: located.property.name.clone(),
            occurrence,
        }]
    };
    Some(vec![
        Fix {
            title: format!(
                "Remove this `{}` (line {})",
                siblings[this].name(),
                line_col(text, siblings[this].name_span().start).0
            ),
            ops: remove(this),
            checks: Vec::new(),
        },
        Fix {
            title: format!(
                "Remove the other `{}` (line {})",
                siblings[other].name(),
                line_col(text, siblings[other].name_span().start).0
            ),
            ops: remove(other),
            checks: Vec::new(),
        },
    ])
}

fn c340(text: &str, file: &ConfigFile, span: Span, facts: &FixFacts) -> Option<Vec<Fix>> {
    let reference = base_at(text, file, span)?;
    if body(file, &reference.parent).is_some_and(|m| declares(m, &reference.base)) {
        return None;
    }
    let declare = vec![EditOp::AddClass {
        parent: ConfigPath(reference.parent.clone()),
        name: reference.base.clone(),
        base: None,
        forward: true,
        position: Position::Before(reference.class.name.clone()),
    }];
    let first = Fix {
        title: format!(
            "Declare `class {};` above `{}`",
            reference.base, reference.class.name
        ),
        ops: declare,
        checks: Vec::new(),
    };
    let Some((base, patch)) = &facts.owner_patch_of_base else {
        return Some(vec![first]);
    };
    if !base.eq_ignore_ascii_case(&reference.base)
        || listed_addons(file)
            .iter()
            .any(|a| a.eq_ignore_ascii_case(patch))
    {
        return Some(vec![first]);
    }
    let Ok(addons) = add_required_addons(text, None, std::slice::from_ref(patch)) else {
        return Some(vec![first]);
    };
    if addons.is_empty() {
        return Some(vec![first]);
    }
    let mut ops = first.ops.clone();
    ops.extend(addons);
    Some(vec![
        first,
        Fix {
            title: format!(
                "Declare `{}` and add `{patch}` to requiredAddons",
                reference.base
            ),
            ops,
            checks: Vec::new(),
        },
    ])
}

fn c341(text: &str, file: &ConfigFile, span: Span) -> Option<Vec<Fix>> {
    let reference = base_at(text, file, span)?;
    if reference.class.parents.len() != 1 {
        return None;
    }
    let mut declared: Vec<&str> = Vec::new();
    for class in flatten(file) {
        if class.name != reference.base
            && class.name.eq_ignore_ascii_case(&reference.base)
            && !declared.contains(&class.name.as_str())
        {
            declared.push(&class.name);
        }
    }
    if declared.len() != 1 {
        return None;
    }
    let mut class = reference.parent.clone();
    class.push(reference.class.name.clone());
    Some(vec![Fix {
        title: format!("Use `{}`", declared[0]),
        ops: vec![EditOp::SetBase {
            class: ConfigPath(class),
            base: Some(declared[0].to_owned()),
        }],
        checks: Vec::new(),
    }])
}

fn c350(file: &ConfigFile, span: Span) -> Option<Vec<Fix>> {
    let located = property_at(file, span)?;
    if located.item.is_some() {
        return None;
    }
    let root = located.class.first()?;
    let want = schema::lookup(root, &located.property.name).and_then(|info| info.array)?;
    if located.property.declared_array == want {
        return None;
    }
    let fits = match located.property.value.as_ref() {
        Some(Value::Array(_)) => want,
        Some(_) => !want,
        None => false,
    };
    if !fits {
        return None;
    }
    let name = &located.property.name;
    let title = if want {
        format!("Write `{name}[]`")
    } else {
        format!("Write `{name}` without []")
    };
    Some(vec![Fix {
        title,
        ops: vec![EditOp::SetArray {
            target: prop_target(&located),
            array: want,
        }],
        checks: Vec::new(),
    }])
}

fn c400(text: &str, diagnostic: &Diagnostic, facts: &FixFacts) -> Vec<Fix> {
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Vec::new();
    }
    if parsed.file.cfg_patches_blocks().next().is_none() {
        return Vec::new();
    }
    let required = listed_addons(&parsed.file);
    let named = diagnostic_mod(&diagnostic.message);
    let mut chosen: Vec<&(String, Vec<String>)> = Vec::new();
    if let Some(name) = named {
        chosen.extend(
            facts
                .dependency_patches
                .iter()
                .filter(|(mod_name, _)| mod_name.eq_ignore_ascii_case(&name)),
        );
    }
    if chosen.is_empty() {
        chosen.extend(facts.dependency_patches.iter().filter(|(_, patches)| {
            !patches.is_empty() && patches.iter().all(|p| !listed(&required, p))
        }));
    }
    let mut fixes = Vec::new();
    for (_, patches) in chosen {
        let missing: Vec<&String> = patches.iter().filter(|p| !listed(&required, p)).collect();
        for name in &missing {
            if let Ok(ops) = add_required_addons(text, None, std::slice::from_ref(*name)) {
                fixes.push(Fix {
                    title: format!("Add `{name}` to requiredAddons"),
                    ops,
                    checks: Vec::new(),
                });
            }
        }
        if missing.len() > 1 {
            let names: Vec<String> = missing.iter().map(|n| (*n).clone()).collect();
            if let Ok(ops) = add_required_addons(text, None, &names) {
                fixes.push(Fix {
                    title: format!("Add all {}", missing.len()),
                    ops,
                    checks: Vec::new(),
                });
            }
        }
    }
    fixes
}

struct Property<'a> {
    class: Vec<String>,
    property: &'a ConfigProperty,
    item: Option<usize>,
}

struct BaseRef<'a> {
    parent: Vec<String>,
    class: &'a ConfigClass,
    base: String,
}

fn property_at<'a>(file: &'a ConfigFile, span: Span) -> Option<Property<'a>> {
    fn walk<'a>(members: &'a [Member], path: &mut Vec<String>, span: Span) -> Option<Property<'a>> {
        for member in members {
            match member {
                Member::Property(property) => {
                    if property.spans.name == span {
                        return Some(Property {
                            class: path.clone(),
                            property,
                            item: None,
                        });
                    }
                    if let Some(item) = property.spans.value_items.iter().position(|s| *s == span) {
                        return Some(Property {
                            class: path.clone(),
                            property,
                            item: Some(item),
                        });
                    }
                }
                Member::Class(class) => {
                    path.push(class.name.clone());
                    if let Some(found) = walk(&class.members, path, span) {
                        return Some(found);
                    }
                    path.pop();
                }
                Member::Delete { .. } => {}
            }
        }
        None
    }
    walk(&file.members, &mut Vec::new(), span)
}

fn base_at<'a>(text: &str, file: &'a ConfigFile, span: Span) -> Option<BaseRef<'a>> {
    fn walk<'a>(
        text: &str,
        members: &'a [Member],
        path: &mut Vec<String>,
        span: Span,
    ) -> Option<BaseRef<'a>> {
        for member in members {
            let Member::Class(class) = member else {
                continue;
            };
            let inside = class
                .spans
                .parents
                .is_some_and(|p| p.start <= span.start && span.end <= p.end);
            if inside {
                let base = text.get(span.start..span.end)?;
                if class.parents.iter().any(|p| p.eq_ignore_ascii_case(base)) {
                    return Some(BaseRef {
                        parent: path.clone(),
                        class,
                        base: base.to_owned(),
                    });
                }
            }
            path.push(class.name.clone());
            if let Some(found) = walk(text, &class.members, path, span) {
                return Some(found);
            }
            path.pop();
        }
        None
    }
    walk(text, &file.members, &mut Vec::new(), span)
}

fn body<'a>(file: &'a ConfigFile, path: &[String]) -> Option<&'a [Member]> {
    let mut members = file.members.as_slice();
    for name in path {
        let class = members.iter().find_map(|m| match m {
            Member::Class(c) if c.name.eq_ignore_ascii_case(name) => Some(c),
            _ => None,
        })?;
        members = &class.members;
    }
    Some(members)
}

fn flatten(file: &ConfigFile) -> Vec<&ConfigClass> {
    fn collect<'a>(members: &'a [Member], out: &mut Vec<&'a ConfigClass>) {
        for member in members {
            if let Member::Class(class) = member {
                out.push(class);
                collect(&class.members, out);
            }
        }
    }
    let mut out = Vec::new();
    collect(&file.members, &mut out);
    out
}

fn files_item(located: &Property) -> Option<(usize, String)> {
    if !located.property.name.eq_ignore_ascii_case("files") {
        return None;
    }
    let index = located.item?;
    match located.property.value.as_ref() {
        Some(Value::Array(items)) => match items.get(index) {
            Some(Value::Str(path)) => Some((index, path.clone())),
            _ => None,
        },
        _ => None,
    }
}

fn array_set(located: &Property, index: usize, value: &str) -> Vec<EditOp> {
    vec![EditOp::ArraySet {
        property: prop_target(located),
        index,
        value: NewValue::Str(value.to_owned()),
    }]
}

fn prop_target(located: &Property) -> Target {
    Target::Property {
        class: ConfigPath(located.class.clone()),
        name: located.property.name.clone(),
    }
}

fn canonical_dirs() -> [&'static str; 5] {
    [
        ModuleSlot::Engine,
        ModuleSlot::GameLib,
        ModuleSlot::Game,
        ModuleSlot::World,
        ModuleSlot::Mission,
    ]
    .map(ModuleSlot::canonical_dir)
}

fn replace_leaf(path: &str, leaf: &str) -> String {
    match path.rsplit_once('/') {
        Some((head, _)) => format!("{head}/{leaf}"),
        None => leaf.to_owned(),
    }
}

fn within_one(a: &str, b: &str) -> bool {
    a.len().abs_diff(b.len()) <= 1 && edit_distance_ignore_ascii_case(a, b) <= 1
}

fn declares(members: &[Member], name: &str) -> bool {
    members.iter().any(|m| {
        matches!(m, Member::Class(_) | Member::Property(_)) && m.name().eq_ignore_ascii_case(name)
    })
}

fn patch_class<'a>(
    file: &'a ConfigFile,
    patch: Option<&str>,
) -> Result<&'a ConfigClass, EditError> {
    let patches = file
        .cfg_patches()
        .ok_or_else(|| EditError::NotFound("Class `CfgPatches`".into()))?;
    match patch {
        Some(wanted) => classes(&patches.members)
            .find(|c| c.name.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| EditError::NotFound(format!("Patch class `{wanted}`"))),
        None => classes(&patches.members)
            .next()
            .ok_or_else(|| EditError::NotFound("CfgPatches entry".into())),
    }
}

fn listed_addons(file: &ConfigFile) -> Vec<&str> {
    file.patch_classes()
        .flat_map(required_addons)
        .map(|(name, _)| name)
        .collect()
}

fn listed(required: &[&str], name: &str) -> bool {
    required.iter().any(|r| r.eq_ignore_ascii_case(name))
}

fn diagnostic_mod(message: &str) -> Option<String> {
    let rest = &message[message.find('`')? + 1..];
    let end = rest.find('`')?;
    Some(rest[..end].to_owned())
}
