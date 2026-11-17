//! File-level lints for a clean config.cpp parse: duplicate members, base-class
//! resolution and schema array-ness.
//!
//! `validate_text` calls [`run`] only when the parse produced no issues, so
//! every rule here sees real structure. Codes: `MF-C330` (property set twice),
//! `MF-C331` (class defined twice), `MF-C340` (base unresolved), `MF-C341`
//! (base casing) and `MF-C350` (array/scalar against the schema).

use crate::ast::{
    classes, required_addon_properties, required_addons, ConfigClass, ConfigFile, ConfigProperty,
    Member,
};
use crate::parser::parse;
use crate::resolve::Resolver;
use crate::schema;
use modfoundry_core::{Diagnostic, Severity, Span};
use std::collections::{HashMap, HashSet, VecDeque};

/// Run the file lints over a clean parse of `text`.
pub fn run(file: &ConfigFile, text: &str) -> Vec<Diagnostic> {
    let index = FileIndex::build(file);
    let mut out = Vec::new();
    let mut levels = Vec::new();
    body(
        &mut out,
        text,
        &index,
        None,
        None,
        &file.members,
        &mut levels,
    );
    out
}

/// MF-C410: every `requiredAddons` item this file's `CfgPatches` classes list
/// that no source in `resolver` declares (ignoring `own_source`'s stored patches),
/// no vanilla source requires, and this file does not declare itself.
pub fn index_findings(
    text: &str,
    resolver: &Resolver,
    own_source: Option<&str>,
) -> Vec<Diagnostic> {
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Vec::new();
    }
    let mut known: HashSet<String> = parsed
        .file
        .patch_classes()
        .map(|class| class.name.to_ascii_lowercase())
        .collect();
    known.extend(
        resolver
            .patches()
            .into_iter()
            .filter(|patch| own_source != Some(patch.source.as_str()))
            .map(|patch| patch.name.to_ascii_lowercase()),
    );
    // Vanilla requires addons declared only inside protected `.ebo` packages (`DZ_Worlds_Nasdara`).
    known.extend(resolver.vanilla_required_addons());
    let mut out = Vec::new();
    for (name, span) in parsed.file.patch_classes().flat_map(required_addons) {
        if known.contains(&name.to_ascii_lowercase()) {
            continue;
        }
        out.push(Diagnostic {
            code: "MF-C410".to_string(),
            severity: Severity::Warning,
            span: Some(span),
            message: format!(
                "`{name}` isn't an addon any loaded mod or the game declares. Check the spelling, or add the mod as a dependency."
            ),
        });
    }
    out
}

/// MF-C400: one finding per `dependencies` entry whose patch list is non-empty
/// and has nothing this file's `CfgPatches` lists in `requiredAddons`.
pub fn dependency_findings(text: &str, dependencies: &[(String, Vec<String>)]) -> Vec<Diagnostic> {
    let parsed = parse(text);
    if !parsed.issues.is_empty() {
        return Vec::new();
    }
    let listed: HashSet<String> = parsed
        .file
        .patch_classes()
        .flat_map(required_addons)
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    let span = dependency_span(&parsed.file);
    let mut out = Vec::new();
    for (display, patches) in dependencies {
        if patches.is_empty()
            || patches
                .iter()
                .any(|patch| listed.contains(&patch.to_ascii_lowercase()))
        {
            continue;
        }
        let names: Vec<String> = patches.iter().map(|patch| format!("`{patch}`")).collect();
        out.push(Diagnostic {
            code: "MF-C400".to_string(),
            severity: Severity::Warning,
            span,
            message: format!(
                "`{display}` is marked as a dependency, but none of its addons ({}) are in requiredAddons.",
                names.join(", ")
            ),
        });
    }
    out
}

/// The span a C400 finding points at: the first `requiredAddons` name over every
/// `CfgPatches` block, else the first patch class's name.
fn dependency_span(file: &ConfigFile) -> Option<Span> {
    file.patch_classes()
        .find_map(|class| required_addon_properties(class).next())
        .map(|property| property.spans.name)
        .or_else(|| file.patch_classes().next().map(|class| class.spans.name))
}

/// Every class in the file, keyed by name for the base-chain lookup. A name can
/// repeat across bodies (vanilla declares `SlotCalm` under many parents), so a
/// lookup yields every match.
struct FileIndex<'a> {
    exact: HashMap<&'a str, Vec<&'a ConfigClass>>,
    lower: HashMap<String, Vec<&'a ConfigClass>>,
}

impl<'a> FileIndex<'a> {
    fn build(file: &'a ConfigFile) -> Self {
        let mut index = FileIndex {
            exact: HashMap::new(),
            lower: HashMap::new(),
        };
        index.collect(&file.members);
        index
    }

    fn collect(&mut self, members: &'a [Member]) {
        for member in members {
            let Member::Class(class) = member else {
                continue;
            };
            self.exact
                .entry(class.name.as_str())
                .or_default()
                .push(class);
            self.lower
                .entry(class.name.to_ascii_lowercase())
                .or_default()
                .push(class);
            self.collect(&class.members);
        }
    }

    fn find(&self, name: &str) -> &[&'a ConfigClass] {
        self.exact
            .get(name)
            .or_else(|| self.lower.get(&name.to_ascii_lowercase()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// One enclosing class body, with the position of the class being resolved.
struct Level<'a> {
    members: &'a [Member],
    owner: Option<&'a ConfigClass>,
    up_to: usize,
}

fn body<'a>(
    out: &mut Vec<Diagnostic>,
    text: &str,
    index: &FileIndex<'a>,
    owner: Option<&'a ConfigClass>,
    root: Option<&'a ConfigClass>,
    members: &'a [Member],
    levels: &mut Vec<Level<'a>>,
) {
    if let Some(owner) = owner {
        duplicate_properties(out, owner, members);
        duplicate_classes(out, owner, members);
    }
    if let Some(root) = root {
        schema_arrays(out, &root.name, members);
    }
    for (i, member) in members.iter().enumerate() {
        let Member::Class(class) = member else {
            continue;
        };
        levels.push(Level {
            members,
            owner,
            up_to: i,
        });
        check_bases(out, text, index, class, levels);
        body(
            out,
            text,
            index,
            Some(class),
            Some(root.unwrap_or(class)),
            &class.members,
            levels,
        );
        levels.pop();
    }
}

fn duplicate_properties(out: &mut Vec<Diagnostic>, owner: &ConfigClass, members: &[Member]) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut reported: HashSet<String> = HashSet::new();
    let mut first_repeats: Vec<(String, &ConfigProperty)> = Vec::new();
    let mut plain_repeats: HashSet<String> = HashSet::new();
    for member in members {
        let Member::Property(property) = member else {
            continue;
        };
        let key = property.name.to_ascii_lowercase();
        if seen.insert(key.clone()) {
            continue;
        }
        if !property.array_append {
            plain_repeats.insert(key.clone());
        }
        if reported.insert(key.clone()) {
            first_repeats.push((key, property));
        }
    }
    for (key, property) in first_repeats {
        // Only a repeat that assigns (not `+=`) is known to stop the 1.30 Binarizer.
        let ending = if plain_repeats.contains(&key) {
            "; DayZ Tools 1.30 refuses to binarize a config that sets a member twice."
        } else {
            "."
        };
        out.push(Diagnostic::at(
            "MF-C330",
            Severity::Warning,
            property.spans.name,
            format!(
                "`{}` is set twice in `{}`. Keep one{ending}",
                property.name, owner.name
            ),
        ));
    }
}

fn duplicate_classes(out: &mut Vec<Diagnostic>, owner: &ConfigClass, members: &[Member]) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut reported: HashSet<String> = HashSet::new();
    for member in members {
        let Member::Class(class) = member else {
            continue;
        };
        if class.forward {
            continue;
        }
        let key = class.name.to_ascii_lowercase();
        if !seen.insert(key.clone()) && reported.insert(key) {
            out.push(Diagnostic::at(
                "MF-C331",
                Severity::Warning,
                class.spans.name,
                format!(
                    "class `{}` is defined twice in `{}`. Merge them into one; DayZ \
                     Tools 1.30 refuses to binarize a config that defines a member twice.",
                    class.name, owner.name
                ),
            ));
        }
    }
}

fn schema_arrays(out: &mut Vec<Diagnostic>, root: &str, members: &[Member]) {
    for member in members {
        let Member::Property(property) = member else {
            continue;
        };
        let Some(info) = schema::lookup(root, &property.name) else {
            continue;
        };
        let message = match info.array {
            Some(true) if !property.declared_array => format!(
                "`{}` is an array in DayZ's own configs; write `{}[]`.",
                property.name, property.name
            ),
            Some(false) if property.declared_array => format!(
                "`{}` is a scalar in DayZ's own configs; write `{}`.",
                property.name, property.name
            ),
            _ => continue,
        };
        out.push(Diagnostic::at(
            "MF-C350",
            Severity::Warning,
            property.spans.name,
            message,
        ));
    }
}

fn check_bases(
    out: &mut Vec<Diagnostic>,
    text: &str,
    index: &FileIndex,
    class: &ConfigClass,
    levels: &[Level],
) {
    let spans = parent_spans(text, class);
    for (i, base) in class.parents.iter().enumerate() {
        let span = spans
            .get(i)
            .copied()
            .or(class.spans.parents)
            .unwrap_or(class.spans.name);
        let (code, severity, message) = match resolve(levels, index, base) {
            Some(declared) if declared.name == *base => continue,
            Some(declared) => (
                "MF-C341",
                Severity::Hint,
                format!(
                    "`{base}` is declared as `{}`; match its casing.",
                    declared.name
                ),
            ),
            None => (
                "MF-C340",
                Severity::Error,
                format!(
                    "`{}` extends `{base}`, which isn't declared here. Declare it first: `class {base};`.",
                    class.name
                ),
            ),
        };
        out.push(Diagnostic::at(code, severity, span, message));
    }
}

/// The span of each parent name inside the class's `: A, B` clause.
fn parent_spans(text: &str, class: &ConfigClass) -> Vec<Span> {
    let Some(parents) = class.spans.parents else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = parents.start;
    for name in &class.parents {
        let found = text
            .get(cursor..parents.end)
            .and_then(|slice| slice.find(name.as_str()));
        let span = match found {
            Some(offset) => Span::new(cursor + offset, cursor + offset + name.len()),
            None => parents,
        };
        cursor = span.end;
        out.push(span);
    }
    out
}

fn resolve<'a>(levels: &[Level<'a>], index: &FileIndex<'a>, name: &str) -> Option<&'a ConfigClass> {
    let named = |class: &&ConfigClass| class.name.eq_ignore_ascii_case(name);
    levels.iter().rev().find_map(|level| {
        classes(&level.members[..level.up_to])
            .chain(level.owner)
            .find(named)
            .or_else(|| {
                let owner = level.owner?;
                base_chain_children(owner, index).into_iter().find(named)
            })
    })
}

/// Child classes of every base in `owner`'s base chain.
fn base_chain_children<'a>(owner: &'a ConfigClass, index: &FileIndex<'a>) -> Vec<&'a ConfigClass> {
    let mut out = Vec::new();
    let mut queue: VecDeque<&ConfigClass> = owner
        .parents
        .iter()
        .flat_map(|name| index.find(name).iter().copied())
        .collect();
    let mut seen: HashSet<usize> = HashSet::new();
    seen.insert(owner as *const ConfigClass as usize);
    while let Some(class) = queue.pop_front() {
        if !seen.insert(class as *const ConfigClass as usize) {
            continue;
        }
        for member in &class.members {
            if let Member::Class(child) = member {
                out.push(child);
            }
        }
        queue.extend(
            class
                .parents
                .iter()
                .flat_map(|name| index.find(name).iter().copied()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::validate::validate_text;
    use modfoundry_core::{Diagnostic, Severity};

    const MOD: &str = r#"class CfgPatches { class M { requiredAddons[] = { "DZ_Data" }; }; };
class CfgMods { class M { class defs { class worldScriptModule { files[] = { "M/Scripts/4_World" }; }; }; }; };
"#;

    fn diags(text: &str, code: &str) -> Vec<Diagnostic> {
        validate_text(text, None)
            .diagnostics
            .into_iter()
            .filter(|d| d.code == code)
            .collect()
    }

    fn slice<'a>(text: &'a str, d: &Diagnostic) -> &'a str {
        let span = d.span.expect("diagnostic carries a span");
        &text[span.start..span.end]
    }

    #[test]
    fn clean_mod_is_lint_free() {
        let v = validate_text(MOD, None);
        assert!(v.diagnostics.is_empty(), "{:?}", v.diagnostics);
    }

    #[test]
    fn c330_duplicate_property_warns_on_the_second_name() {
        let text = "class A { x = 1; x = 2; };";
        let d = diags(text, "MF-C330");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(slice(text, &d[0]), "x");
        assert_eq!(d[0].span.unwrap().start, text.rfind("x").unwrap());
        assert_eq!(
            d[0].message,
            "`x` is set twice in `A`. Keep one; DayZ Tools 1.30 refuses to binarize a config \
             that sets a member twice."
        );
    }

    #[test]
    fn c330_case_insensitive_and_array_append_count() {
        let text = "class A { X[] = { 1 }; x[] += { 2 }; };";
        let d = diags(text, "MF-C330");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(slice(text, &d[0]), "x");
        assert_eq!(d[0].message, "`x` is set twice in `A`. Keep one.");

        let text = "class A { x = 1; x[] += { 2 }; x = 3; };";
        let d = diags(text, "MF-C330");
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0]
            .message
            .ends_with("refuses to binarize a config that sets a member twice."));
    }

    #[test]
    fn c330_distinct_names_and_separate_bodies_are_clean() {
        assert!(diags("class A { x = 1; y = 2; };", "MF-C330").is_empty());
        assert!(diags(
            "class A { class B { x = 1; }; class C { x = 2; }; };",
            "MF-C330"
        )
        .is_empty());
    }

    #[test]
    fn c331_two_definitions_warn_on_the_second_name() {
        let text = "class P { class C {}; class C {}; };";
        let d = diags(text, "MF-C331");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(slice(text, &d[0]), "C");
        assert_eq!(
            d[0].span.unwrap().start,
            text.rfind("class C").unwrap() + "class ".len()
        );
        assert_eq!(
            d[0].message,
            "class `C` is defined twice in `P`. Merge them into one; DayZ Tools 1.30 refuses \
             to binarize a config that defines a member twice."
        );
    }

    #[test]
    fn c331_forward_declaration_then_definition_is_clean() {
        assert!(diags("class P { class C; class C {}; };", "MF-C331").is_empty());
        assert!(diags("class P { class C; class C; };", "MF-C331").is_empty());
    }

    #[test]
    fn c340_unresolved_base_is_an_error_on_the_base_name() {
        let text = "class A: Missing {};";
        let d = diags(text, "MF-C340");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(slice(text, &d[0]), "Missing");
        assert!(
            d[0].message
                .contains("`A` extends `Missing`, which isn't declared here. Declare it first: `class Missing;`."),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn c340_earlier_sibling_and_outer_body_resolve() {
        assert!(diags("class A { class Base {}; class B: Base {}; };", "MF-C340").is_empty());
        assert!(diags("class Base; class A { class B: Base {}; };", "MF-C340").is_empty());
        assert!(diags("class Base {}; class A: Base {};", "MF-C340").is_empty());
    }

    #[test]
    fn c340_later_in_the_same_body_is_a_hit() {
        assert!(diags("class A { class B: C {}; class C {}; };", "MF-C340").len() == 1);
    }

    #[test]
    fn c340_inherited_child_resolves_at_any_enclosing_level() {
        let clean =
            "class A { class DamageSystem; }; class B: A { class DamageSystem: DamageSystem {}; };";
        assert!(diags(clean, "MF-C340").is_empty(), "{clean}");
        let hit = "class A { }; class B: A { class DamageSystem: DamageSystem {}; };";
        let d = diags(hit, "MF-C340");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(slice(hit, &d[0]), "DamageSystem");
        let two_up = "class A { class DamageSystem; }; class B: A { class Inner { class DamageSystem: DamageSystem {}; }; };";
        assert!(diags(two_up, "MF-C340").is_empty(), "{two_up}");
    }

    #[test]
    fn c340_a_class_is_never_its_own_base() {
        assert_eq!(
            diags("class DamageSystem: DamageSystem {};", "MF-C340").len(),
            1
        );
    }

    #[test]
    fn c341_case_mismatch_is_a_hint_on_the_base_name() {
        let text = "class Base {}; class A: base {};";
        let d = diags(text, "MF-C341");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Hint);
        assert_eq!(slice(text, &d[0]), "base");
        assert!(
            d[0].message
                .contains("`base` is declared as `Base`; match its casing."),
            "{}",
            d[0].message
        );
        assert!(diags("class Base {}; class A: Base {};", "MF-C341").is_empty());
    }

    #[test]
    fn c341_only_reports_the_case_when_no_exact_match_exists() {
        assert!(diags("class base; class Base {}; class A: base {};", "MF-C341").is_empty());
    }

    #[test]
    fn c350_array_expected_warns_on_the_property_name() {
        let text = r#"class CfgPatches { class M { requiredAddons = { "DZ_Data" }; }; };"#;
        let d = diags(text, "MF-C350");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(slice(text, &d[0]), "requiredAddons");
        assert!(
            d[0].message.contains(
                "`requiredAddons` is an array in DayZ's own configs; write `requiredAddons[]`."
            ),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn c350_scalar_expected_warns() {
        let text = r#"class CfgMods { class M { type[] = { "mod" }; }; };"#;
        let d = diags(text, "MF-C350");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(slice(text, &d[0]), "type");
        assert!(
            d[0].message
                .contains("`type` is a scalar in DayZ's own configs; write `type`."),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn c350_agreement_and_unknown_names_are_clean() {
        assert!(diags(MOD, "MF-C350").is_empty());
        assert!(diags(
            r#"class CfgMods { class M { notASchemaProperty[] = { 1 }; }; };"#,
            "MF-C350"
        )
        .is_empty());
    }
}
