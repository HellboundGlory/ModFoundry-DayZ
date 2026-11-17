//! Typed model for a parsed config.cpp (Arma/DayZ C-like superset).
//!
//! This is the *parse* layer: it preserves class/property structure with byte
//! spans but is intentionally generic — it does not know what `CfgMods` means.
//! DayZ semantics (module binding, patch dependencies) are derived from this
//! tree by the validation layer so the parser stays reusable for any config
//! family (CfgPatches/CfgMods, rvmat, CE XML-adjacent configs, etc.).

use modfoundry_core::Span;

/// The whole parsed file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigFile {
    /// Top-level members in source order.
    pub members: Vec<Member>,
    /// Flattened `enum` table, as stored by config.bin. The binary keeps one
    /// name/value map with no table names or grouping, so a binarized file
    /// cannot be attributed back to its `enum` declarations.
    pub enums: Vec<EnumEntry>,
    /// True when a `#include` directive appears anywhere in the file.
    pub has_include: bool,
}

/// One `enum` member (already resolved to its integer value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumEntry {
    pub name: String,
    pub value: i32,
}

/// A class or property statement at any nesting depth.
#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Class(ConfigClass),
    /// `name = value;` or `name[] = { ... };` or a bare `name[];`-style decl.
    Property(ConfigProperty),
    /// `delete Name;` — removes an inherited member. Kept structurally.
    Delete {
        name: String,
        span: Span,
        /// `delete` through the trailing `;`.
        statement: Span,
    },
}

impl Member {
    /// The declared name (`class`/property identifier, or the `delete` target).
    pub fn name(&self) -> &str {
        match self {
            Member::Class(c) => &c.name,
            Member::Property(p) => &p.name,
            Member::Delete { name, .. } => name,
        }
    }

    /// The span of the whole statement.
    pub fn statement(&self) -> Span {
        match self {
            Member::Class(c) => c.spans.statement,
            Member::Property(p) => p.spans.statement,
            Member::Delete { statement, .. } => *statement,
        }
    }

    /// The span of the declared name.
    pub fn name_span(&self) -> Span {
        match self {
            Member::Class(c) => c.spans.name,
            Member::Property(p) => p.spans.name,
            Member::Delete { span, .. } => *span,
        }
    }
}

/// The `Class` members of `members`, in order.
pub fn classes(members: &[Member]) -> impl Iterator<Item = &ConfigClass> {
    members.iter().filter_map(|m| match m {
        Member::Class(c) => Some(c),
        _ => None,
    })
}

/// Every `requiredAddons` property of a class, in order; covers `=` and `+=`.
pub(crate) fn required_addon_properties(
    class: &ConfigClass,
) -> impl Iterator<Item = &ConfigProperty> {
    class.members.iter().filter_map(|member| match member {
        Member::Property(property) if property.name.eq_ignore_ascii_case("requiredAddons") => {
            Some(property)
        }
        _ => None,
    })
}

/// Every string item of a class's `requiredAddons` properties, with the item's
/// span; covers `=` and `+=` arrays and repeats.
pub(crate) fn required_addons(class: &ConfigClass) -> Vec<(&str, Span)> {
    required_addon_properties(class)
        .flat_map(|property| property.spanned_strings())
        .collect()
}

/// Byte spans of one `class` statement, all relative to the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassSpans {
    /// The `class` keyword.
    pub keyword: Span,
    pub name: Span,
    /// `:` through the last parent name.
    pub parents: Option<Span>,
    pub open_brace: Option<Span>,
    pub close_brace: Option<Span>,
    /// `external` when present (else `class`) through the trailing `;`, the
    /// close brace when there is no `;`, or the forward declaration's `;`.
    pub statement: Span,
}

impl Default for ClassSpans {
    fn default() -> Self {
        Self {
            keyword: Span::new(0, 0),
            name: Span::new(0, 0),
            parents: None,
            open_brace: None,
            close_brace: None,
            statement: Span::new(0, 0),
        }
    }
}

/// Byte spans of one property statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertySpans {
    pub name: Span,
    /// `[` through `]` for a `name[]` declaration.
    pub brackets: Option<Span>,
    /// `=` or `+=`; zero-width when the declaration has no value.
    pub operator: Span,
    /// The whole value expression, arrays included.
    pub value: Option<Span>,
    /// Top-level array items in order, each the item's full extent. A nested
    /// array contributes only its outer span.
    pub value_items: Vec<Span>,
    /// The array's opening and closing braces.
    pub array_braces: Option<(Span, Span)>,
    /// The property name through the trailing `;`.
    pub statement: Span,
}

impl Default for PropertySpans {
    fn default() -> Self {
        Self {
            name: Span::new(0, 0),
            brackets: None,
            operator: Span::new(0, 0),
            value: None,
            value_items: Vec::new(),
            array_braces: None,
            statement: Span::new(0, 0),
        }
    }
}

/// A `class Name [: Parent [, Parent2] ] { ... }` definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigClass {
    pub name: String,
    pub span: Span,
    /// Exact spans of the class statement's parts.
    pub spans: ClassSpans,
    /// Optional inheritance list after `:`.
    pub parents: Vec<String>,
    /// `class Name;` — declared without a body (config.bin tag `0x03`).
    pub forward: bool,
    pub members: Vec<Member>,
}

/// A property assignment. `name` is the declared identifier; if the original
/// was written `name[]`, `declared_array` is set (used to disambiguate empty-array
/// declarations from scalar assignment).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigProperty {
    pub name: String,
    pub span: Span,
    /// Exact spans of the property statement's parts.
    pub spans: PropertySpans,
    /// True when declared `name[]`.
    pub declared_array: bool,
    /// True for `name[] += { ... };` (config.bin tag `0x05`).
    pub array_append: bool,
    pub value: Option<Value>,
}

impl ConfigProperty {
    /// Every `Value::Str` item when the value is an array, in order; `None` otherwise.
    pub fn strings(&self) -> Option<Vec<&str>> {
        let Some(Value::Array(items)) = self.value.as_ref() else {
            return None;
        };
        Some(
            items
                .iter()
                .filter_map(|v| match v {
                    Value::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect(),
        )
    }

    /// The array's `Value::Str` items paired with their spans; empty otherwise.
    pub fn spanned_strings(&self) -> Vec<(&str, Span)> {
        let Some(Value::Array(items)) = self.value.as_ref() else {
            return Vec::new();
        };
        items
            .iter()
            .zip(&self.spans.value_items)
            .filter_map(|(value, span)| match value {
                Value::Str(text) => Some((text.as_str(), *span)),
                _ => None,
            })
            .collect()
    }
}

/// The right-hand side of an assignment.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Number(String),
    /// A binary-format float (config.bin tag `0x01`). Separate from
    /// [`Value::Number`] because text spelling cannot recover the type: an
    /// integral float such as `1.0` prints as `1`.
    Float(f32),
    /// A bare identifier / macro reference used as a value (e.g. `type = mod;`
    /// or a `#define`d constant not expanded by v1).
    Ident(String),
    /// `{ item, item, ... }`.
    Array(Vec<Value>),
    /// A value we could not classify as the above (rare expression tails).
    /// Kept verbatim so no data is silently dropped.
    Other(String),
}

impl ConfigFile {
    /// The first top-level class with this name, ignoring case.
    pub fn find_class(&self, name: &str) -> Option<&ConfigClass> {
        classes(&self.members).find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The first top-level `CfgPatches` class.
    pub fn cfg_patches(&self) -> Option<&ConfigClass> {
        self.find_class("CfgPatches")
    }

    /// Every top-level `CfgPatches` class, in file order.
    pub fn cfg_patches_blocks(&self) -> impl Iterator<Item = &ConfigClass> {
        classes(&self.members).filter(|c| c.name.eq_ignore_ascii_case("CfgPatches"))
    }

    /// The patch classes of every `CfgPatches` block, in file order.
    pub(crate) fn patch_classes(&self) -> impl Iterator<Item = &ConfigClass> {
        self.cfg_patches_blocks()
            .flat_map(|block| classes(&block.members))
    }
}

impl ConfigClass {
    /// Direct child class members by name, ignoring case (first match; duplicates
    /// allowed but rare — DayZ uses one definition per sibling).
    pub fn child_class(&self, name: &str) -> Option<&ConfigClass> {
        self.members.iter().find_map(|m| match m {
            Member::Class(c) if c.name.eq_ignore_ascii_case(name) => Some(c),
            _ => None,
        })
    }

    /// Direct property members with the given name, ignoring case.
    pub fn property(&self, name: &str) -> Option<&ConfigProperty> {
        self.members.iter().find_map(|m| match m {
            Member::Property(p) if p.name.eq_ignore_ascii_case(name) => Some(p),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls(name: &str, members: Vec<Member>) -> Member {
        Member::Class(ConfigClass {
            name: name.into(),
            span: Span::new(0, 1),
            spans: ClassSpans::default(),
            parents: vec![],
            forward: false,
            members,
        })
    }
    fn prop(name: &str, value: Value) -> Member {
        Member::Property(ConfigProperty {
            name: name.into(),
            span: Span::new(0, 1),
            spans: PropertySpans::default(),
            declared_array: false,
            array_append: false,
            value: Some(value),
        })
    }

    #[test]
    fn patch_classes_cover_every_cfgpatches_block() {
        let f = ConfigFile {
            members: vec![
                cls("CfgPatches", vec![cls("A", vec![])]),
                cls("CfgVehicles", vec![]),
                cls("cfgpatches", vec![cls("B", vec![]), cls("C", vec![])]),
            ],
            ..Default::default()
        };
        let names: Vec<&str> = f.patch_classes().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
        assert_eq!(f.cfg_patches_blocks().count(), 2);
    }

    #[test]
    fn find_class_and_child() {
        let f = ConfigFile {
            members: vec![cls(
                "CfgPatches",
                vec![cls(
                    "MyMod",
                    vec![prop("requiredAddons", Value::Array(vec![]))],
                )],
            )],
            ..Default::default()
        };
        assert!(f.find_class("CfgPatches").is_some());
        let patches = f.find_class("CfgPatches").unwrap();
        assert!(patches.child_class("MyMod").is_some());
        assert!(patches.child_class("Missing").is_none());
    }
}
