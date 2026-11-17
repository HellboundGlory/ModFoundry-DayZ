//! Static schema tables for the structured Central Economy kinds.
//!
//! One table per structured [`CeKind`]. A table has sections; a section says
//! where its records live and how to read them. The record model, the ops,
//! the generic lints and the GUI all read this and nothing else.

use crate::kind::CeKind;
use modfoundry_core::text::is_c_identifier;

/// The schema of one structured kind.
#[derive(Debug, PartialEq)]
pub struct KindSchema {
    pub kind: CeKind,
    pub sections: &'static [SectionSchema],
}

/// One section: a container element under the root holding the records.
#[derive(Debug, PartialEq)]
pub struct SectionSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub container: Option<&'static str>,
    pub record: RecordSchema,
}

/// How records are recognised, keyed and displayed.
#[derive(Debug, PartialEq)]
pub struct RecordSchema {
    pub elements: &'static [&'static str],
    pub key: Key,
    pub noun: &'static str,
    pub plural: &'static str,
    pub fields: &'static [FieldSchema],
    pub lists: &'static [ListSchema],
    pub order: &'static [&'static str],
}

/// Where a record's identity is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Attr(&'static str),
    ElementName,
    Index,
}

/// One field of a record.
#[derive(Debug, PartialEq)]
pub struct FieldSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub place: Place,
    pub ty: FieldType,
    pub group: &'static str,
    pub column: Option<&'static str>,
    pub template: Option<&'static str>,
    pub summary: &'static str,
}

/// Where a field's value lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    ChildText(&'static str),
    ChildAttr {
        child: &'static str,
        attr: &'static str,
    },
    Attr(&'static str),
    ChildPresent(&'static str),
}

/// How a field's text is validated and rendered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldType {
    Int { min: Option<i64>, max: Option<i64> },
    Seconds,
    Percent,
    Float { min: Option<f64>, max: Option<f64> },
    Flag,
    Enum(&'static [&'static str]),
    Name(Vocab),
    Class,
    Text,
}

/// A name vocabulary the field draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vocab {
    Category,
    Tag,
    Usage,
    Value,
    UserUsage,
    UserValue,
    CargoPreset,
    AttachmentsPreset,
    Event,
}

/// A repeated child element, optionally inside a container.
#[derive(Debug, PartialEq)]
pub struct ListSchema {
    pub id: &'static str,
    pub label: &'static str,
    pub container: Option<&'static str>,
    pub element: &'static str,
    pub attrs: &'static [FieldSchema],
    pub nested: Option<&'static ListSchema>,
    /// Lists checked below each item but not read into the model, edited or shown.
    pub inner: &'static [ListSchema],
    pub column: Option<&'static str>,
    pub chips: bool,
}

/// Why a value does not fit a field; the text completes "`<field>` must be …".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueProblem {
    Invalid(String),
    OutOfRange(String),
}

/// The schema for `kind`, or `None` when the kind is source-only.
pub fn schema(kind: CeKind) -> Option<&'static KindSchema> {
    match kind {
        CeKind::Types => Some(&TYPES),
        CeKind::SpawnableTypes => Some(&SPAWNABLE_TYPES),
        CeKind::Events => Some(&EVENTS),
        CeKind::EventSpawns => Some(&EVENT_SPAWNS),
        CeKind::RandomPresets => Some(&RANDOM_PRESETS),
        CeKind::Limits => Some(&LIMITS),
        CeKind::LimitsUser => Some(&LIMITS_USER),
        CeKind::EconomyCore => Some(&ECONOMY_CORE),
        CeKind::Economy => Some(&ECONOMY),
        CeKind::Globals => Some(&GLOBALS),
        CeKind::Messages => Some(&MESSAGES),
        CeKind::IgnoreList => Some(&IGNORE_LIST),
        _ => None,
    }
}

impl KindSchema {
    /// The section with this id.
    pub fn section(&self, id: &str) -> Option<&SectionSchema> {
        self.sections.iter().find(|section| section.id == id)
    }
}

impl RecordSchema {
    /// The field with this id.
    pub fn field(&self, id: &str) -> Option<&FieldSchema> {
        self.fields.iter().find(|field| field.id == id)
    }

    /// The list with this id.
    pub fn list(&self, id: &str) -> Option<&ListSchema> {
        self.lists.iter().find(|list| list.id == id)
    }

    /// Every child element a record may have, from its fields and lists.
    pub fn known_children(&self) -> Vec<&'static str> {
        let mut known: Vec<&'static str> = Vec::new();
        let mut push = |name: &'static str| {
            if !known.contains(&name) {
                known.push(name);
            }
        };
        for field in self.fields {
            match field.place {
                Place::ChildText(child) | Place::ChildPresent(child) => push(child),
                Place::ChildAttr { child, .. } => push(child),
                Place::Attr(_) => {}
            }
        }
        for list in self.lists {
            push(list.container.unwrap_or(list.element));
        }
        self.order
            .iter()
            .copied()
            .filter(|name| known.contains(name))
            .collect()
    }
}

impl CeKind {
    /// Whether this kind has a schema (a Table view).
    pub fn structured(self) -> bool {
        schema(self).is_some()
    }
}

impl FieldType {
    /// Checks `raw` against the type; the problem text completes "`<field>` must be …".
    pub fn check(&self, raw: &str) -> Result<(), ValueProblem> {
        match *self {
            FieldType::Int { min, max } => {
                let value =
                    whole(raw).ok_or_else(|| ValueProblem::Invalid("a whole number".into()))?;
                if let Some(min) = min {
                    if value < min {
                        return Err(ValueProblem::OutOfRange(format!("{min} or more")));
                    }
                }
                if let Some(max) = max {
                    if value > max {
                        return Err(ValueProblem::OutOfRange(format!("{max} or less")));
                    }
                }
                Ok(())
            }
            FieldType::Seconds => {
                let value =
                    whole(raw).ok_or_else(|| ValueProblem::Invalid("a whole number".into()))?;
                if value < 0 {
                    return Err(ValueProblem::OutOfRange("0 or more".into()));
                }
                Ok(())
            }
            FieldType::Percent => {
                let value =
                    whole(raw).ok_or_else(|| ValueProblem::Invalid("a whole number".into()))?;
                if !(-1..=100).contains(&value) {
                    return Err(ValueProblem::OutOfRange(
                        "a percentage from -1 to 100".into(),
                    ));
                }
                Ok(())
            }
            FieldType::Float { min, max } => {
                let value = float(raw).ok_or_else(|| ValueProblem::Invalid("a number".into()))?;
                if let Some(min) = min {
                    if value < min {
                        return Err(ValueProblem::OutOfRange(format!("{} or more", number(min))));
                    }
                }
                if let Some(max) = max {
                    if value > max {
                        return Err(ValueProblem::OutOfRange(format!("{} or less", number(max))));
                    }
                }
                Ok(())
            }
            FieldType::Flag => {
                if raw == "0" || raw == "1" {
                    Ok(())
                } else {
                    Err(ValueProblem::Invalid("0 or 1".into()))
                }
            }
            FieldType::Enum(values) => {
                if values.contains(&raw) {
                    Ok(())
                } else {
                    let listed = values
                        .iter()
                        .map(|value| format!("`{value}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(ValueProblem::Invalid(format!("one of {listed}")))
                }
            }
            FieldType::Class => {
                if is_class_name(raw) {
                    Ok(())
                } else {
                    Err(ValueProblem::Invalid("a class name".into()))
                }
            }
            FieldType::Name(_) | FieldType::Text => Ok(()),
        }
    }
}

pub(crate) fn whole(raw: &str) -> Option<i64> {
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

fn float(raw: &str) -> Option<f64> {
    let body = raw.strip_prefix('-').unwrap_or(raw);
    let (mut digits, mut dots) = (0, 0);
    for byte in body.bytes() {
        match byte {
            b'0'..=b'9' => digits += 1,
            b'.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    raw.parse().ok()
}

/// The rule a class name must satisfy.
pub const CLASS_NAME_RULE: &str = "A class name has letters, digits and `_` only.";

/// Whether `value` is a valid class name.
pub fn is_class_name(value: &str) -> bool {
    is_c_identifier(value)
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

const fn child_text(
    id: &'static str,
    label: &'static str,
    ty: FieldType,
    group: &'static str,
    column: Option<&'static str>,
    template: Option<&'static str>,
    summary: &'static str,
) -> FieldSchema {
    FieldSchema {
        id,
        label,
        place: Place::ChildText(id),
        ty,
        group,
        column,
        template,
        summary,
    }
}

const fn flag(
    id: &'static str,
    template: Option<&'static str>,
    summary: &'static str,
) -> FieldSchema {
    FieldSchema {
        id,
        label: id,
        place: Place::ChildAttr {
            child: "flags",
            attr: id,
        },
        ty: FieldType::Flag,
        group: "Flags",
        column: None,
        template,
        summary,
    }
}

const fn record_attr(
    id: &'static str,
    label: &'static str,
    ty: FieldType,
    column: Option<&'static str>,
    summary: &'static str,
) -> FieldSchema {
    FieldSchema {
        id,
        label,
        place: Place::Attr(id),
        ty,
        group: "General",
        column,
        template: None,
        summary,
    }
}

const fn list_attr(
    id: &'static str,
    label: &'static str,
    ty: FieldType,
    summary: &'static str,
) -> FieldSchema {
    FieldSchema {
        id,
        label,
        place: Place::Attr(id),
        ty,
        group: "",
        column: None,
        template: None,
        summary,
    }
}

const INT_POSITIVE: FieldType = FieldType::Int {
    min: Some(0),
    max: None,
};

static TYPES_ORDER: [&str; 12] = [
    "nominal", "lifetime", "restock", "min", "quantmin", "quantmax", "cost", "flags", "category",
    "tag", "usage", "value",
];

static TYPES_FIELDS: [FieldSchema; 14] = [
    child_text(
        "nominal",
        "Nominal",
        INT_POSITIVE,
        "Spawning",
        Some("Nominal"),
        Some("0"),
        "How many the economy keeps on the map.",
    ),
    child_text(
        "min",
        "Min",
        INT_POSITIVE,
        "Spawning",
        Some("Min"),
        Some("0"),
        "Below this count the economy restocks toward nominal.",
    ),
    child_text(
        "lifetime",
        "Lifetime",
        FieldType::Seconds,
        "Spawning",
        Some("Lifetime"),
        Some("14400"),
        "Seconds an untouched item stays before cleanup.",
    ),
    child_text(
        "restock",
        "Restock",
        FieldType::Seconds,
        "Spawning",
        Some("Restock"),
        Some("0"),
        "Seconds between respawns while the count is below nominal.",
    ),
    child_text(
        "quantmin",
        "Quantity min",
        FieldType::Percent,
        "Spawning",
        Some("Qty min–max"),
        Some("-1"),
        "Lowest fill percentage on spawn; -1 for items without a quantity.",
    ),
    child_text(
        "quantmax",
        "Quantity max",
        FieldType::Percent,
        "Spawning",
        None,
        Some("-1"),
        "Highest fill percentage on spawn; -1 for items without a quantity.",
    ),
    child_text(
        "cost",
        "Cost",
        INT_POSITIVE,
        "Spawning",
        Some("Cost"),
        Some("100"),
        "Spawn priority; 100 on almost every vanilla type.",
    ),
    FieldSchema {
        id: "category",
        label: "Category",
        place: Place::ChildAttr {
            child: "category",
            attr: "name",
        },
        ty: FieldType::Name(Vocab::Category),
        group: "Spawning",
        column: Some("Category"),
        template: None,
        summary: "The loot category the item spawns under.",
    },
    flag(
        "count_in_cargo",
        Some("0"),
        "Items inside cargo count toward the total.",
    ),
    flag(
        "count_in_hoarder",
        Some("0"),
        "Items inside a hoarder count toward the total.",
    ),
    flag(
        "count_in_map",
        Some("1"),
        "Items placed on the map count toward the total.",
    ),
    flag(
        "count_in_player",
        Some("0"),
        "Items carried by players count toward the total.",
    ),
    flag("crafted", Some("0"), "The item is crafted, not spawned."),
    flag("deloot", Some("0"), "Spawns only at dynamic events."),
];

static USAGE_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "name",
        "Name",
        FieldType::Name(Vocab::Usage),
        "A usage flag the item spawns at.",
    ),
    list_attr(
        "user",
        "User",
        FieldType::Name(Vocab::UserUsage),
        "A mission-defined usage flag.",
    ),
];

static VALUE_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "name",
        "Name",
        FieldType::Name(Vocab::Value),
        "A value tier the item spawns at.",
    ),
    list_attr(
        "user",
        "User",
        FieldType::Name(Vocab::UserValue),
        "A mission-defined value flag.",
    ),
];

static TAG_ATTRS: [FieldSchema; 1] = [list_attr(
    "name",
    "Name",
    FieldType::Name(Vocab::Tag),
    "A tag grouping the item with others.",
)];

static TYPES_LISTS: [ListSchema; 3] = [
    ListSchema {
        id: "usage",
        label: "Usage",
        container: None,
        element: "usage",
        attrs: &USAGE_ATTRS,
        nested: None,
        inner: &[],
        column: Some("Usage"),
        chips: true,
    },
    ListSchema {
        id: "value",
        label: "Value",
        container: None,
        element: "value",
        attrs: &VALUE_ATTRS,
        nested: None,
        inner: &[],
        column: Some("Value"),
        chips: true,
    },
    ListSchema {
        id: "tag",
        label: "Tags",
        container: None,
        element: "tag",
        attrs: &TAG_ATTRS,
        nested: None,
        inner: &[],
        column: None,
        chips: true,
    },
];

static TYPES_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "types",
    label: "Types",
    container: None,
    record: RecordSchema {
        elements: &["type"],
        key: Key::Attr("name"),
        noun: "type",
        plural: "types",
        fields: &TYPES_FIELDS,
        lists: &TYPES_LISTS,
        order: &TYPES_ORDER,
    },
}];

static TYPES: KindSchema = KindSchema {
    kind: CeKind::Types,
    sections: &TYPES_SECTIONS,
};

const NO_FIELDS: &[FieldSchema] = &[];
const NO_LISTS: &[ListSchema] = &[];
const NO_ORDER: &[&str] = &[];

static LIMITS_SECTIONS: [SectionSchema; 4] = [
    SectionSchema {
        id: "categories",
        label: "Categories",
        container: Some("categories"),
        record: RecordSchema {
            elements: &["category"],
            key: Key::Attr("name"),
            noun: "category",
            plural: "categories",
            fields: NO_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
    SectionSchema {
        id: "tags",
        label: "Tags",
        container: Some("tags"),
        record: RecordSchema {
            elements: &["tag"],
            key: Key::Attr("name"),
            noun: "tag",
            plural: "tags",
            fields: NO_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
    SectionSchema {
        id: "usageflags",
        label: "Usage",
        container: Some("usageflags"),
        record: RecordSchema {
            elements: &["usage"],
            key: Key::Attr("name"),
            noun: "usage flag",
            plural: "usage flags",
            fields: NO_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
    SectionSchema {
        id: "valueflags",
        label: "Value",
        container: Some("valueflags"),
        record: RecordSchema {
            elements: &["value"],
            key: Key::Attr("name"),
            noun: "value flag",
            plural: "value flags",
            fields: NO_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
];

static LIMITS: KindSchema = KindSchema {
    kind: CeKind::Limits,
    sections: &LIMITS_SECTIONS,
};

static USER_USAGE_ATTRS: [FieldSchema; 1] = [list_attr(
    "name",
    "Name",
    FieldType::Name(Vocab::Usage),
    "The usage flag this user flag adds to.",
)];

static USER_VALUE_ATTRS: [FieldSchema; 1] = [list_attr(
    "name",
    "Name",
    FieldType::Name(Vocab::Value),
    "The value flag this user flag adds to.",
)];

static USER_USAGE_LISTS: [ListSchema; 1] = [ListSchema {
    id: "usage",
    label: "Usage",
    container: None,
    element: "usage",
    attrs: &USER_USAGE_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Usage"),
    chips: true,
}];

static USER_VALUE_LISTS: [ListSchema; 1] = [ListSchema {
    id: "value",
    label: "Value",
    container: None,
    element: "value",
    attrs: &USER_VALUE_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Value"),
    chips: true,
}];

static LIMITS_USER_SECTIONS: [SectionSchema; 2] = [
    SectionSchema {
        id: "usageflags",
        label: "Usage",
        container: Some("usageflags"),
        record: RecordSchema {
            elements: &["user"],
            key: Key::Attr("name"),
            noun: "user usage flag",
            plural: "user usage flags",
            fields: NO_FIELDS,
            lists: &USER_USAGE_LISTS,
            order: &["usage"],
        },
    },
    SectionSchema {
        id: "valueflags",
        label: "Value",
        container: Some("valueflags"),
        record: RecordSchema {
            elements: &["user"],
            key: Key::Attr("name"),
            noun: "user value flag",
            plural: "user value flags",
            fields: NO_FIELDS,
            lists: &USER_VALUE_LISTS,
            order: &["value"],
        },
    },
];

static LIMITS_USER: KindSchema = KindSchema {
    kind: CeKind::LimitsUser,
    sections: &LIMITS_USER_SECTIONS,
};

static CLASS_FIELDS: [FieldSchema; 2] = [
    record_attr(
        "act",
        "Act",
        FieldType::Enum(&["none", "character", "car"]),
        Some("Act"),
        "How the root class is used: none, character or car.",
    ),
    record_attr(
        "reportMemoryLOD",
        "Report memory LOD",
        FieldType::Enum(&["yes", "no"]),
        Some("Report memory LOD"),
        "Whether the root class reports its memory LOD.",
    ),
];

static DEFAULT_FIELDS: [FieldSchema; 1] = [record_attr(
    "value",
    "Value",
    FieldType::Text,
    Some("Value"),
    "The default's value.",
)];

static FILE_ATTRS: [FieldSchema; 2] = [
    list_attr("name", "Name", FieldType::Text, "The file's name."),
    list_attr(
        "type",
        "Type",
        FieldType::Enum(&[
            "types",
            "spawnabletypes",
            "globals",
            "economy",
            "events",
            "messages",
            "randompresets",
        ]),
        "The kind of file the entry points to.",
    ),
];

static CE_LISTS: [ListSchema; 1] = [ListSchema {
    id: "file",
    label: "Files",
    container: None,
    element: "file",
    attrs: &FILE_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Files"),
    chips: false,
}];

static ECONOMY_CORE_SECTIONS: [SectionSchema; 3] = [
    SectionSchema {
        id: "classes",
        label: "Classes",
        container: Some("classes"),
        record: RecordSchema {
            elements: &["rootclass"],
            key: Key::Attr("name"),
            noun: "root class",
            plural: "root classes",
            fields: &CLASS_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
    SectionSchema {
        id: "defaults",
        label: "Defaults",
        container: Some("defaults"),
        record: RecordSchema {
            elements: &["default"],
            key: Key::Attr("name"),
            noun: "default",
            plural: "defaults",
            fields: &DEFAULT_FIELDS,
            lists: NO_LISTS,
            order: NO_ORDER,
        },
    },
    SectionSchema {
        id: "ce",
        label: "Custom folders",
        container: None,
        record: RecordSchema {
            elements: &["ce"],
            key: Key::Attr("folder"),
            noun: "custom folder",
            plural: "custom folders",
            fields: NO_FIELDS,
            lists: &CE_LISTS,
            order: &["file"],
        },
    },
];

static ECONOMY_CORE: KindSchema = KindSchema {
    kind: CeKind::EconomyCore,
    sections: &ECONOMY_CORE_SECTIONS,
};

const FLOAT_CHANCE: FieldType = FieldType::Float {
    min: Some(0.0),
    max: None,
};

static SPAWNABLE_ORDER: [&str; 5] = ["damage", "tag", "cargo", "attachments", "hoarder"];

static SPAWNABLE_TAG_ATTRS: [FieldSchema; 1] = [list_attr(
    "name",
    "Name",
    FieldType::Name(Vocab::Tag),
    "A tag grouping the type with others.",
)];

static SPAWNABLE_ITEM_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "name",
        "Name",
        FieldType::Class,
        "The class name of an item the container may hold.",
    ),
    list_attr(
        "chance",
        "Chance",
        FLOAT_CHANCE,
        "The item's weight among the container's items.",
    ),
];

static SPAWNABLE_ITEM: ListSchema = ListSchema {
    id: "item",
    label: "Items",
    container: None,
    element: "item",
    attrs: &SPAWNABLE_ITEM_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Items"),
    chips: false,
};

const FLOAT_DAMAGE: FieldType = FieldType::Float {
    min: Some(0.0),
    max: Some(1.0),
};

static ITEM_DAMAGE_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "min",
        "Min",
        FLOAT_DAMAGE,
        "Lowest damage the item spawns with.",
    ),
    list_attr(
        "max",
        "Max",
        FLOAT_DAMAGE,
        "Highest damage the item spawns with.",
    ),
];

static SPAWNABLE_ITEM_CHILDREN: [ListSchema; 2] = [
    ListSchema {
        id: "damage",
        label: "Damage",
        container: None,
        element: "damage",
        attrs: &ITEM_DAMAGE_ATTRS,
        nested: None,
        inner: &[],
        column: None,
        chips: false,
    },
    ListSchema {
        id: "cargo",
        label: "Cargo",
        container: None,
        element: "cargo",
        attrs: &CARGO_ATTRS,
        nested: Some(&SPAWNABLE_ITEM),
        inner: &[],
        column: None,
        chips: false,
    },
];

static SPAWNABLE_CARGO_ITEM: ListSchema = ListSchema {
    id: "item",
    label: "Items",
    container: None,
    element: "item",
    attrs: &SPAWNABLE_ITEM_ATTRS,
    nested: None,
    inner: &SPAWNABLE_ITEM_CHILDREN,
    column: Some("Items"),
    chips: false,
};

static CARGO_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "chance",
        "Chance",
        FLOAT_CHANCE,
        "Chance the cargo is filled at all.",
    ),
    list_attr(
        "preset",
        "Preset",
        FieldType::Name(Vocab::CargoPreset),
        "A named cargo preset to draw the items from.",
    ),
];

static ATTACHMENTS_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "chance",
        "Chance",
        FLOAT_CHANCE,
        "Chance the attachments are fitted at all.",
    ),
    list_attr(
        "preset",
        "Preset",
        FieldType::Name(Vocab::AttachmentsPreset),
        "A named attachments preset to draw the items from.",
    ),
];

static SPAWNABLE_LISTS: [ListSchema; 3] = [
    ListSchema {
        id: "tag",
        label: "Tags",
        container: None,
        element: "tag",
        attrs: &SPAWNABLE_TAG_ATTRS,
        nested: None,
        inner: &[],
        column: None,
        chips: true,
    },
    ListSchema {
        id: "cargo",
        label: "Cargo",
        container: None,
        element: "cargo",
        attrs: &CARGO_ATTRS,
        nested: Some(&SPAWNABLE_CARGO_ITEM),
        inner: &[],
        column: Some("Cargo"),
        chips: false,
    },
    ListSchema {
        id: "attachments",
        label: "Attachments",
        container: None,
        element: "attachments",
        attrs: &ATTACHMENTS_ATTRS,
        nested: Some(&SPAWNABLE_ITEM),
        inner: &[],
        column: Some("Attachments"),
        chips: false,
    },
];

static SPAWNABLE_FIELDS: [FieldSchema; 5] = [
    FieldSchema {
        id: "damage_min",
        label: "Damage min",
        place: Place::ChildAttr {
            child: "damage",
            attr: "min",
        },
        ty: FieldType::Float {
            min: Some(0.0),
            max: Some(1.0),
        },
        group: "Condition",
        column: Some("Damage"),
        template: None,
        summary: "Lowest damage fraction the type spawns with.",
    },
    FieldSchema {
        id: "damage_max",
        label: "Damage max",
        place: Place::ChildAttr {
            child: "damage",
            attr: "max",
        },
        ty: FieldType::Float {
            min: Some(0.0),
            max: Some(1.0),
        },
        group: "Condition",
        column: None,
        template: None,
        summary: "Highest damage fraction the type spawns with.",
    },
    FieldSchema {
        id: "quantmin",
        label: "Quantity min",
        place: Place::Attr("quantmin"),
        ty: FieldType::Percent,
        group: "Condition",
        column: None,
        template: None,
        summary: "Lowest fill percentage on spawn; -1 for items without a quantity.",
    },
    FieldSchema {
        id: "quantmax",
        label: "Quantity max",
        place: Place::Attr("quantmax"),
        ty: FieldType::Percent,
        group: "Condition",
        column: None,
        template: None,
        summary: "Highest fill percentage on spawn; -1 for items without a quantity.",
    },
    FieldSchema {
        id: "hoarder",
        label: "Hoarder",
        place: Place::ChildPresent("hoarder"),
        ty: FieldType::Flag,
        group: "Storage",
        column: Some("Hoarder"),
        template: None,
        summary: "The type is a storage container; items kept in it count as hoarded.",
    },
];

static SPAWNABLE_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "spawnabletypes",
    label: "Spawnable types",
    container: None,
    record: RecordSchema {
        elements: &["type"],
        key: Key::Attr("name"),
        noun: "spawnable type",
        plural: "spawnable types",
        fields: &SPAWNABLE_FIELDS,
        lists: &SPAWNABLE_LISTS,
        order: &SPAWNABLE_ORDER,
    },
}];

static SPAWNABLE_TYPES: KindSchema = KindSchema {
    kind: CeKind::SpawnableTypes,
    sections: &SPAWNABLE_SECTIONS,
};

static PRESET_ITEM_ATTRS: [FieldSchema; 2] = [
    list_attr(
        "name",
        "Name",
        FieldType::Class,
        "The class name of an item the preset may hold.",
    ),
    list_attr(
        "chance",
        "Chance",
        FLOAT_CHANCE,
        "The item's weight among the preset's items.",
    ),
];

static PRESET_LISTS: [ListSchema; 1] = [ListSchema {
    id: "item",
    label: "Items",
    container: None,
    element: "item",
    attrs: &PRESET_ITEM_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Items"),
    chips: false,
}];

static PRESET_FIELDS: [FieldSchema; 1] = [FieldSchema {
    id: "chance",
    label: "Chance",
    place: Place::Attr("chance"),
    ty: FLOAT_CHANCE,
    group: "General",
    column: Some("Chance"),
    template: None,
    summary: "Chance the preset is chosen.",
}];

static PRESET_ORDER: [&str; 1] = ["item"];

static PRESETS_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "presets",
    label: "Presets",
    container: None,
    record: RecordSchema {
        elements: &["cargo", "attachments"],
        key: Key::Attr("name"),
        noun: "preset",
        plural: "presets",
        fields: &PRESET_FIELDS,
        lists: &PRESET_LISTS,
        order: &PRESET_ORDER,
    },
}];

static RANDOM_PRESETS: KindSchema = KindSchema {
    kind: CeKind::RandomPresets,
    sections: &PRESETS_SECTIONS,
};

static ECONOMY_GROUP_FIELDS: [FieldSchema; 4] = [
    record_attr(
        "init",
        "Init",
        FieldType::Flag,
        Some("Init"),
        "Whether the group is initialised when the server starts.",
    ),
    record_attr(
        "load",
        "Load",
        FieldType::Flag,
        Some("Load"),
        "Whether the group loads from the saved storage.",
    ),
    record_attr(
        "respawn",
        "Respawn",
        FieldType::Flag,
        Some("Respawn"),
        "Whether the group respawns over time.",
    ),
    record_attr(
        "save",
        "Save",
        FieldType::Flag,
        Some("Save"),
        "Whether the group persists to storage.",
    ),
];

static ECONOMY_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "economy",
    label: "Groups",
    container: None,
    record: RecordSchema {
        elements: &[
            "dynamic", "animals", "zombies", "vehicles", "randoms", "custom", "building", "player",
        ],
        key: Key::ElementName,
        noun: "group",
        plural: "groups",
        fields: &ECONOMY_GROUP_FIELDS,
        lists: NO_LISTS,
        order: NO_ORDER,
    },
}];

static ECONOMY: KindSchema = KindSchema {
    kind: CeKind::Economy,
    sections: &ECONOMY_SECTIONS,
};

static VARIABLE_FIELDS: [FieldSchema; 2] = [
    record_attr(
        "type",
        "Type",
        FieldType::Enum(&["0", "1", "2"]),
        Some("Type"),
        "0 for an integer variable, 1 for a float one, 2 for a string one.",
    ),
    record_attr(
        "value",
        "Value",
        FieldType::Text,
        Some("Value"),
        "The variable's value.",
    ),
];

static VARIABLES_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "variables",
    label: "Variables",
    container: None,
    record: RecordSchema {
        elements: &["var"],
        key: Key::Attr("name"),
        noun: "variable",
        plural: "variables",
        fields: &VARIABLE_FIELDS,
        lists: NO_LISTS,
        order: NO_ORDER,
    },
}];

static GLOBALS: KindSchema = KindSchema {
    kind: CeKind::Globals,
    sections: &VARIABLES_SECTIONS,
};

static MESSAGE_ORDER: [&str; 6] = [
    "delay",
    "repeat",
    "deadline",
    "onconnect",
    "shutdown",
    "text",
];

static MESSAGE_FIELDS: [FieldSchema; 6] = [
    child_text(
        "delay",
        "Delay",
        INT_POSITIVE,
        "Timing",
        Some("Delay"),
        None,
        "Minutes after a player connects before the message appears; use with On connect.",
    ),
    child_text(
        "repeat",
        "Repeat",
        INT_POSITIVE,
        "Timing",
        Some("Repeat"),
        None,
        "Minutes between repeats; 0 shows it once.",
    ),
    child_text(
        "deadline",
        "Deadline",
        INT_POSITIVE,
        "Timing",
        Some("Deadline"),
        None,
        "Minutes after server start when the countdown ends.",
    ),
    child_text(
        "onconnect",
        "On connect",
        FieldType::Flag,
        "Timing",
        Some("On connect"),
        None,
        "Whether the message shows when a player connects.",
    ),
    child_text(
        "shutdown",
        "Shutdown",
        FieldType::Flag,
        "Timing",
        Some("Shutdown"),
        None,
        "Whether the server shuts down when the countdown ends.",
    ),
    child_text(
        "text",
        "Text",
        FieldType::Text,
        "Message",
        Some("Text"),
        Some(""),
        "The line shown to players.",
    ),
];

static MESSAGES_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "messages",
    label: "Messages",
    container: None,
    record: RecordSchema {
        elements: &["message"],
        key: Key::Index,
        noun: "message",
        plural: "messages",
        fields: &MESSAGE_FIELDS,
        lists: NO_LISTS,
        order: &MESSAGE_ORDER,
    },
}];

static MESSAGES: KindSchema = KindSchema {
    kind: CeKind::Messages,
    sections: &MESSAGES_SECTIONS,
};

static IGNORE_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "ignore",
    label: "Ignored types",
    container: None,
    record: RecordSchema {
        elements: &["type"],
        key: Key::Attr("name"),
        noun: "ignored type",
        plural: "ignored types",
        fields: NO_FIELDS,
        lists: NO_LISTS,
        order: NO_ORDER,
    },
}];

static IGNORE_LIST: KindSchema = KindSchema {
    kind: CeKind::IgnoreList,
    sections: &IGNORE_SECTIONS,
};

static EVENT_ORDER: [&str; 14] = [
    "nominal",
    "min",
    "max",
    "lifetime",
    "restock",
    "saferadius",
    "distanceradius",
    "cleanupradius",
    "secondary",
    "flags",
    "position",
    "limit",
    "active",
    "children",
];

static EVENT_FIELDS: [FieldSchema; 15] = [
    child_text(
        "nominal",
        "Nominal",
        INT_POSITIVE,
        "Spawning",
        Some("Nominal"),
        Some("0"),
        "How many of the event the economy keeps active.",
    ),
    child_text(
        "min",
        "Min",
        INT_POSITIVE,
        "Spawning",
        Some("Min"),
        Some("0"),
        "Below this count the economy starts one.",
    ),
    child_text(
        "max",
        "Max",
        INT_POSITIVE,
        "Spawning",
        Some("Max"),
        Some("0"),
        "The most the economy starts at once.",
    ),
    child_text(
        "lifetime",
        "Lifetime",
        FieldType::Seconds,
        "Spawning",
        Some("Lifetime"),
        Some("300"),
        "Seconds the event stays before cleanup.",
    ),
    child_text(
        "restock",
        "Restock",
        FieldType::Seconds,
        "Spawning",
        Some("Restock"),
        Some("0"),
        "Seconds between restocks.",
    ),
    child_text(
        "saferadius",
        "Safe radius",
        INT_POSITIVE,
        "Radii",
        None,
        Some("0"),
        "Radius kept clear of players.",
    ),
    child_text(
        "distanceradius",
        "Distance radius",
        INT_POSITIVE,
        "Radii",
        None,
        Some("0"),
        "Minimum distance between two spawns of this event.",
    ),
    child_text(
        "cleanupradius",
        "Cleanup radius",
        INT_POSITIVE,
        "Radii",
        None,
        Some("0"),
        "How near a player keeps the event from being cleaned up.",
    ),
    child_text(
        "secondary",
        "Secondary",
        FieldType::Name(Vocab::Event),
        "Spawning",
        None,
        None,
        "An event that spawns alongside this one.",
    ),
    flag(
        "deletable",
        Some("0"),
        "The event's spawns are removed when its lifetime ends.",
    ),
    flag(
        "init_random",
        Some("0"),
        "The event starts at a random position.",
    ),
    flag(
        "remove_damaged",
        Some("0"),
        "Ruined or killed spawns are removed.",
    ),
    child_text(
        "position",
        "Position",
        FieldType::Enum(&["fixed", "player", "uniform"]),
        "Placement",
        Some("Position"),
        Some("fixed"),
        "How the event's position is chosen.",
    ),
    child_text(
        "limit",
        "Limit",
        FieldType::Enum(&["child", "parent", "mixed", "custom"]),
        "Placement",
        Some("Limit"),
        Some("child"),
        "Which min and max the limit counts: each child, the whole event, both, or custom.",
    ),
    child_text(
        "active",
        "Active",
        FieldType::Flag,
        "Placement",
        Some("Active"),
        Some("0"),
        "Whether the event is active.",
    ),
];

static EVENT_CHILD_ATTRS: [FieldSchema; 5] = [
    list_attr(
        "type",
        "Type",
        FieldType::Class,
        "The class name of a child the event spawns.",
    ),
    list_attr("min", "Min", INT_POSITIVE, "Smallest count of the child."),
    list_attr("max", "Max", INT_POSITIVE, "Largest count of the child."),
    list_attr(
        "lootmin",
        "Loot min",
        INT_POSITIVE,
        "Least loot spawned around the child.",
    ),
    list_attr(
        "lootmax",
        "Loot max",
        INT_POSITIVE,
        "Most loot spawned around the child.",
    ),
];

static EVENT_LISTS: [ListSchema; 1] = [ListSchema {
    id: "children",
    label: "Children",
    container: Some("children"),
    element: "child",
    attrs: &EVENT_CHILD_ATTRS,
    nested: None,
    inner: &[],
    column: Some("Children"),
    chips: false,
}];

static EVENTS_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "events",
    label: "Events",
    container: None,
    record: RecordSchema {
        elements: &["event"],
        key: Key::Attr("name"),
        noun: "event",
        plural: "events",
        fields: &EVENT_FIELDS,
        lists: &EVENT_LISTS,
        order: &EVENT_ORDER,
    },
}];

static EVENTS: KindSchema = KindSchema {
    kind: CeKind::Events,
    sections: &EVENTS_SECTIONS,
};

const FLOAT_ANY: FieldType = FieldType::Float {
    min: None,
    max: None,
};

static ZONE_ATTRS: [FieldSchema; 5] = [
    list_attr(
        "smin",
        "S min",
        INT_POSITIVE,
        "Minimum static infected count.",
    ),
    list_attr(
        "smax",
        "S max",
        INT_POSITIVE,
        "Maximum static infected count.",
    ),
    list_attr(
        "dmin",
        "D min",
        INT_POSITIVE,
        "Minimum dynamic infected count.",
    ),
    list_attr(
        "dmax",
        "D max",
        INT_POSITIVE,
        "Maximum dynamic infected count.",
    ),
    list_attr("r", "Radius", INT_POSITIVE, "Radius of the zone."),
];

static POS_ATTRS: [FieldSchema; 5] = [
    list_attr("x", "X", FLOAT_ANY, "X coordinate."),
    list_attr("z", "Z", FLOAT_ANY, "Z coordinate."),
    list_attr("a", "A", FLOAT_ANY, "Angle."),
    list_attr("y", "Y", FLOAT_ANY, "Y coordinate."),
    list_attr(
        "group",
        "Group",
        FieldType::Text,
        "The group the position belongs to.",
    ),
];

static SPAWN_LISTS: [ListSchema; 2] = [
    ListSchema {
        id: "zone",
        label: "Zones",
        container: None,
        element: "zone",
        attrs: &ZONE_ATTRS,
        nested: None,
        inner: &[],
        column: Some("Zones"),
        chips: false,
    },
    ListSchema {
        id: "pos",
        label: "Positions",
        container: None,
        element: "pos",
        attrs: &POS_ATTRS,
        nested: None,
        inner: &[],
        column: Some("Positions"),
        chips: false,
    },
];

static SPAWN_ORDER: [&str; 2] = ["zone", "pos"];

static SPAWNS_SECTIONS: [SectionSchema; 1] = [SectionSchema {
    id: "events",
    label: "Events",
    container: None,
    record: RecordSchema {
        elements: &["event"],
        key: Key::Attr("name"),
        noun: "event",
        plural: "events",
        fields: NO_FIELDS,
        lists: &SPAWN_LISTS,
        order: &SPAWN_ORDER,
    },
}];

static EVENT_SPAWNS: KindSchema = KindSchema {
    kind: CeKind::EventSpawns,
    sections: &SPAWNS_SECTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn structured_kinds() -> Vec<CeKind> {
        CeKind::ALL.into_iter().filter(|k| k.structured()).collect()
    }

    fn sentence_case(label: &str) -> bool {
        label.chars().next().is_none_or(|c| c.is_uppercase())
    }

    fn sanity_list(record: &RecordSchema, list: &ListSchema, top: bool) {
        assert!(sentence_case(list.label), "list label {:?}", list.label);
        if top {
            let child = list.container.unwrap_or(list.element);
            assert!(
                record.order.contains(&child),
                "list {:?} child {:?} is not in order",
                list.id,
                child
            );
        }
        let mut ids: Vec<&str> = list.attrs.iter().map(|a| a.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate list attr id in {:?}", list.id);
        for attr in list.attrs {
            assert!(
                matches!(attr.place, Place::Attr(_)),
                "list {:?} attr {:?} is not Place::Attr",
                list.id,
                attr.id
            );
            assert!(sentence_case(attr.label), "label {:?}", attr.label);
            if let Some(template) = attr.template {
                assert!(
                    attr.ty.check(template).is_ok(),
                    "template {template:?} fails {:?}",
                    attr.id
                );
            }
        }
        if let Some(nested) = list.nested {
            sanity_list(record, nested, false);
        }
        for inner in list.inner {
            sanity_list(record, inner, false);
        }
    }

    #[test]
    fn exactly_twelve_kinds_are_structured() {
        assert_eq!(
            structured_kinds(),
            vec![
                CeKind::Types,
                CeKind::SpawnableTypes,
                CeKind::Events,
                CeKind::EventSpawns,
                CeKind::RandomPresets,
                CeKind::Limits,
                CeKind::LimitsUser,
                CeKind::EconomyCore,
                CeKind::Economy,
                CeKind::Globals,
                CeKind::Messages,
                CeKind::IgnoreList
            ]
        );
    }

    #[test]
    fn the_cargo_item_lists_its_damage_and_inner_cargo() {
        let spawnable = schema(CeKind::SpawnableTypes).unwrap();
        let section = spawnable.section("spawnabletypes").unwrap();
        let cargo = section.record.list("cargo").unwrap();
        let item = cargo.nested.unwrap();
        let ids: Vec<&str> = item.inner.iter().map(|inner| inner.id).collect();
        assert_eq!(ids, ["damage", "cargo"]);
        let inner_cargo = item.inner[1].nested.unwrap();
        assert_eq!(inner_cargo.attrs.len(), 2);
        assert!(inner_cargo.inner.is_empty());
        let attachments = section.record.list("attachments").unwrap();
        assert!(attachments.nested.unwrap().inner.is_empty());
    }

    #[test]
    fn small_kinds_have_their_sections_and_fields() {
        let economy = schema(CeKind::Economy).unwrap();
        let groups = economy.section("economy").unwrap();
        assert_eq!(groups.container, None);
        assert_eq!(groups.record.key, Key::ElementName);
        assert_eq!(groups.record.noun, "group");
        assert_eq!(
            groups.record.elements,
            &[
                "dynamic", "animals", "zombies", "vehicles", "randoms", "custom", "building",
                "player"
            ]
        );
        for (id, column) in [
            ("init", "Init"),
            ("load", "Load"),
            ("respawn", "Respawn"),
            ("save", "Save"),
        ] {
            let field = groups.record.field(id).unwrap();
            assert_eq!(field.place, Place::Attr(id));
            assert_eq!(field.ty, FieldType::Flag);
            assert_eq!(field.column, Some(column));
        }

        let globals = schema(CeKind::Globals).unwrap();
        let variables = globals.section("variables").unwrap();
        assert_eq!(variables.record.elements, &["var"]);
        assert_eq!(variables.record.key, Key::Attr("name"));
        assert_eq!(variables.record.noun, "variable");
        assert_eq!(
            variables.record.field("type").unwrap().ty,
            FieldType::Enum(&["0", "1", "2"])
        );
        assert_eq!(variables.record.field("type").unwrap().column, Some("Type"));
        assert_eq!(variables.record.field("value").unwrap().ty, FieldType::Text);
        assert_eq!(
            variables.record.field("value").unwrap().column,
            Some("Value")
        );

        let messages = schema(CeKind::Messages).unwrap();
        let message = messages.section("messages").unwrap();
        assert_eq!(message.record.elements, &["message"]);
        assert_eq!(message.record.key, Key::Index);
        assert_eq!(message.record.noun, "message");
        assert_eq!(
            message.record.order,
            &[
                "delay",
                "repeat",
                "deadline",
                "onconnect",
                "shutdown",
                "text"
            ]
        );
        assert_eq!(message.record.fields.len(), 6);
        for id in ["delay", "repeat", "deadline"] {
            let field = message.record.field(id).unwrap();
            assert_eq!(field.place, Place::ChildText(id));
            assert_eq!(
                field.ty,
                FieldType::Int {
                    min: Some(0),
                    max: None
                }
            );
        }
        for id in ["onconnect", "shutdown"] {
            assert_eq!(message.record.field(id).unwrap().ty, FieldType::Flag);
        }
        assert_eq!(
            message.record.field("text").unwrap().place,
            Place::ChildText("text")
        );
        assert_eq!(message.record.field("text").unwrap().template, Some(""));

        let ignore = schema(CeKind::IgnoreList).unwrap();
        let types = ignore.section("ignore").unwrap();
        assert_eq!(types.record.elements, &["type"]);
        assert_eq!(types.record.key, Key::Attr("name"));
        assert_eq!(types.record.noun, "ignored type");
        assert!(types.record.fields.is_empty());
        assert!(types.record.lists.is_empty());
    }

    #[test]
    fn lookups_find_sections_fields_and_lists() {
        let types = schema(CeKind::Types).unwrap();
        assert_eq!(types.kind, CeKind::Types);
        assert_eq!(types.section("nope"), None);

        let section = types.section("types").unwrap();
        assert_eq!(section.container, None);
        assert_eq!(section.record.elements, &["type"]);
        assert_eq!(section.record.key, Key::Attr("name"));
        assert_eq!(section.record.noun, "type");
        assert_eq!(section.record.plural, "types");

        let nominal = section.record.field("nominal").unwrap();
        assert_eq!(nominal.label, "Nominal");
        assert_eq!(nominal.place, Place::ChildText("nominal"));
        assert_eq!(nominal.column, Some("Nominal"));
        assert_eq!(nominal.template, Some("0"));
        assert_eq!(section.record.field("nope"), None);

        let usage = section.record.list("usage").unwrap();
        assert_eq!(usage.element, "usage");
        assert!(usage.chips);
        assert_eq!(usage.column, Some("Usage"));
        assert_eq!(section.record.list("nope"), None);

        let limits = schema(CeKind::Limits).unwrap();
        assert_eq!(limits.sections.len(), 4);
        assert_eq!(
            limits.section("categories").unwrap().record.noun,
            "category"
        );
        assert_eq!(limits.section("tags").unwrap().record.elements, &["tag"]);
        assert!(limits
            .section("usageflags")
            .unwrap()
            .record
            .fields
            .is_empty());
        assert_eq!(limits.section("usageflags").unwrap().label, "Usage");

        let user = schema(CeKind::LimitsUser).unwrap();
        let usage_flags = user.section("usageflags").unwrap();
        assert_eq!(usage_flags.record.noun, "user usage flag");
        let flags_list = usage_flags.record.list("usage").unwrap();
        assert_eq!(flags_list.element, "usage");
        assert_eq!(flags_list.attrs.len(), 1);
        assert_eq!(flags_list.attrs[0].place, Place::Attr("name"));

        let core = schema(CeKind::EconomyCore).unwrap();
        assert_eq!(core.section("ce").unwrap().record.key, Key::Attr("folder"));
        assert_eq!(core.section("defaults").unwrap().record.noun, "default");
        assert_eq!(core.section("nope"), None);

        let spawnable = schema(CeKind::SpawnableTypes).unwrap();
        let section = spawnable.section("spawnabletypes").unwrap();
        assert_eq!(section.record.noun, "spawnable type");
        assert_eq!(
            section.record.field("damage_min").unwrap().place,
            Place::ChildAttr {
                child: "damage",
                attr: "min"
            }
        );
        assert_eq!(
            section.record.field("hoarder").unwrap().column,
            Some("Hoarder")
        );
        let cargo = section.record.list("cargo").unwrap();
        assert_eq!(cargo.column, Some("Cargo"));
        let item = cargo.nested.unwrap();
        assert_eq!(item.element, "item");
        assert_eq!(item.attrs.len(), 2);

        let presets = schema(CeKind::RandomPresets).unwrap();
        let section = presets.section("presets").unwrap();
        assert_eq!(section.record.elements, &["cargo", "attachments"]);
        assert_eq!(section.record.noun, "preset");
        assert_eq!(
            section.record.field("chance").unwrap().place,
            Place::Attr("chance")
        );
        assert_eq!(section.record.list("item").unwrap().column, Some("Items"));

        let events = schema(CeKind::Events).unwrap();
        let event = events.section("events").unwrap();
        assert_eq!(event.container, None);
        assert_eq!(event.record.elements, &["event"]);
        assert_eq!(event.record.noun, "event");
        assert_eq!(
            event.record.field("position").unwrap().place,
            Place::ChildText("position")
        );
        assert_eq!(
            event.record.field("position").unwrap().template,
            Some("fixed")
        );
        assert_eq!(event.record.field("limit").unwrap().template, Some("child"));
        assert_eq!(
            event.record.field("deletable").unwrap().place,
            Place::ChildAttr {
                child: "flags",
                attr: "deletable"
            }
        );
        let children = event.record.list("children").unwrap();
        assert_eq!(children.container, Some("children"));
        assert_eq!(children.element, "child");
        assert_eq!(children.column, Some("Children"));
        assert!(!children.chips);
        assert_eq!(children.attrs.len(), 5);
        assert_eq!(children.attrs[0].place, Place::Attr("type"));

        let spawns = schema(CeKind::EventSpawns).unwrap();
        let spawned = spawns.section("events").unwrap();
        assert_eq!(spawned.record.elements, &["event"]);
        assert_eq!(spawned.record.noun, "event");
        assert_eq!(spawned.record.order, &["zone", "pos"]);
        assert_eq!(spawned.record.list("zone").unwrap().column, Some("Zones"));
        assert_eq!(
            spawned.record.list("pos").unwrap().column,
            Some("Positions")
        );
        assert_eq!(spawned.record.list("pos").unwrap().attrs.len(), 5);
    }

    #[test]
    fn types_known_children_are_order() {
        let types = schema(CeKind::Types).unwrap().section("types").unwrap();
        assert_eq!(
            types.record.order,
            &[
                "nominal", "lifetime", "restock", "min", "quantmin", "quantmax", "cost", "flags",
                "category", "tag", "usage", "value"
            ]
        );
        assert_eq!(types.record.known_children(), types.record.order.to_vec());
    }

    #[test]
    fn table_sanity() {
        for kind in CeKind::ALL {
            let Some(kind_schema) = schema(kind) else {
                continue;
            };
            assert_eq!(kind_schema.kind, kind);
            for section in kind_schema.sections {
                assert!(sentence_case(section.label), "section {:?}", section.id);
                let record = &section.record;
                assert!(!record.elements.is_empty(), "section {:?}", section.id);
                let mut ids: Vec<&str> = record
                    .fields
                    .iter()
                    .map(|f| f.id)
                    .chain(record.lists.iter().map(|l| l.id))
                    .collect();
                let total = ids.len();
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(ids.len(), total, "duplicate id in {:?}", section.id);
                for field in record.fields {
                    assert!(
                        field.label == field.id || sentence_case(field.label),
                        "label {:?}",
                        field.label
                    );
                    let child = match field.place {
                        Place::ChildText(c) | Place::ChildPresent(c) => Some(c),
                        Place::ChildAttr { child, .. } => Some(child),
                        Place::Attr(_) => None,
                    };
                    if let Some(child) = child {
                        assert!(
                            record.order.contains(&child),
                            "field {:?} child {:?} is not in order",
                            field.id,
                            child
                        );
                    }
                    if let Some(template) = field.template {
                        assert!(
                            field.ty.check(template).is_ok(),
                            "template {template:?} fails {:?}",
                            field.id
                        );
                    }
                }
                for list in record.lists {
                    sanity_list(record, list, true);
                }
            }
        }
    }

    #[test]
    fn check_accepts_and_refuses_each_type() {
        let int = FieldType::Int {
            min: Some(0),
            max: None,
        };
        assert_eq!(int.check("12"), Ok(()));
        assert_eq!(
            int.check("-1"),
            Err(ValueProblem::OutOfRange("0 or more".into()))
        );
        assert_eq!(
            int.check("x"),
            Err(ValueProblem::Invalid("a whole number".into()))
        );

        assert_eq!(FieldType::Seconds.check("0"), Ok(()));
        assert_eq!(
            FieldType::Seconds.check("1.5"),
            Err(ValueProblem::Invalid("a whole number".into()))
        );
        assert_eq!(
            FieldType::Seconds.check("-1"),
            Err(ValueProblem::OutOfRange("0 or more".into()))
        );

        assert_eq!(FieldType::Percent.check("-1"), Ok(()));
        assert_eq!(FieldType::Percent.check("100"), Ok(()));
        assert_eq!(
            FieldType::Percent.check("101"),
            Err(ValueProblem::OutOfRange(
                "a percentage from -1 to 100".into()
            ))
        );

        let float = FieldType::Float {
            min: Some(0.0),
            max: Some(1.0),
        };
        assert_eq!(float.check(".5"), Ok(()));
        assert_eq!(float.check("1."), Ok(()));
        assert_eq!(
            float.check("1e5"),
            Err(ValueProblem::Invalid("a number".into()))
        );
        assert_eq!(
            float.check("2"),
            Err(ValueProblem::OutOfRange("1 or less".into()))
        );

        assert_eq!(FieldType::Flag.check("1"), Ok(()));
        assert_eq!(
            FieldType::Flag.check("2"),
            Err(ValueProblem::Invalid("0 or 1".into()))
        );

        let position = FieldType::Enum(&["fixed", "player", "uniform"]);
        assert_eq!(position.check("player"), Ok(()));
        assert_eq!(
            position.check("nope"),
            Err(ValueProblem::Invalid(
                "one of `fixed`, `player`, `uniform`".into()
            ))
        );

        assert_eq!(FieldType::Class.check("CfgVehicles"), Ok(()));
        assert_eq!(
            FieldType::Class.check("1x"),
            Err(ValueProblem::Invalid("a class name".into()))
        );

        assert_eq!(FieldType::Name(Vocab::Category).check("anything"), Ok(()));
        assert_eq!(FieldType::Text.check(""), Ok(()));
    }
}
