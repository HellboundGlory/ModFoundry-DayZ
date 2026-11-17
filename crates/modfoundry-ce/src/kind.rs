//! Central Economy file kinds and content-based detection.
//!
//! A CE file's kind is decided by its content, not its name: workshop mods call
//! their files anything, fragments carry no root, and a few `<types>` files are
//! another format entirely. [`detect`] reports the kind, whether the file is a
//! rootless fragment, and a root that disagrees with the content.

use crate::xml::{NodeId, XmlDoc};
use serde::{Deserialize, Serialize};

/// A Central Economy mission-file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CeKind {
    Types,
    SpawnableTypes,
    Events,
    EventSpawns,
    EventGroups,
    RandomPresets,
    Limits,
    LimitsUser,
    EconomyCore,
    Economy,
    Globals,
    Messages,
    IgnoreList,
    Environment,
    PlayerSpawnPoints,
    Weather,
    Territories,
    MapGroupProto,
    MapGroupPos,
}

impl CeKind {
    /// Every kind, in declaration order.
    pub const ALL: [CeKind; 19] = [
        CeKind::Types,
        CeKind::SpawnableTypes,
        CeKind::Events,
        CeKind::EventSpawns,
        CeKind::EventGroups,
        CeKind::RandomPresets,
        CeKind::Limits,
        CeKind::LimitsUser,
        CeKind::EconomyCore,
        CeKind::Economy,
        CeKind::Globals,
        CeKind::Messages,
        CeKind::IgnoreList,
        CeKind::Environment,
        CeKind::PlayerSpawnPoints,
        CeKind::Weather,
        CeKind::Territories,
        CeKind::MapGroupProto,
        CeKind::MapGroupPos,
    ];

    /// The document root element name.
    pub fn root(self) -> &'static str {
        match self {
            CeKind::Types => "types",
            CeKind::SpawnableTypes => "spawnabletypes",
            CeKind::Events => "events",
            CeKind::EventSpawns => "eventposdef",
            CeKind::EventGroups => "eventgroupdef",
            CeKind::RandomPresets => "randompresets",
            CeKind::Limits => "lists",
            CeKind::LimitsUser => "user_lists",
            CeKind::EconomyCore => "economycore",
            CeKind::Economy => "economy",
            CeKind::Globals => "variables",
            CeKind::Messages => "messages",
            CeKind::IgnoreList => "ignore",
            CeKind::Environment => "env",
            CeKind::PlayerSpawnPoints => "playerspawnpoints",
            CeKind::Weather => "weather",
            CeKind::Territories => "territory-type",
            CeKind::MapGroupProto => "prototype",
            CeKind::MapGroupPos => "map",
        }
    }

    /// The human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            CeKind::Types => "Types",
            CeKind::SpawnableTypes => "Spawnable types",
            CeKind::Events => "Events",
            CeKind::EventSpawns => "Event spawns",
            CeKind::EventGroups => "Event groups",
            CeKind::RandomPresets => "Random presets",
            CeKind::Limits => "Limits",
            CeKind::LimitsUser => "User limits",
            CeKind::EconomyCore => "Economy core",
            CeKind::Economy => "Economy",
            CeKind::Globals => "Globals",
            CeKind::Messages => "Messages",
            CeKind::IgnoreList => "Ignore list",
            CeKind::Environment => "Environment",
            CeKind::PlayerSpawnPoints => "Player spawn points",
            CeKind::Weather => "Weather",
            CeKind::Territories => "Territories",
            CeKind::MapGroupProto => "Map group prototypes",
            CeKind::MapGroupPos => "Map group positions",
        }
    }

    /// The vanilla file name.
    pub fn file_name(self) -> &'static str {
        match self {
            CeKind::Types => "types.xml",
            CeKind::SpawnableTypes => "cfgspawnabletypes.xml",
            CeKind::Events => "events.xml",
            CeKind::EventSpawns => "cfgeventspawns.xml",
            CeKind::EventGroups => "cfgeventgroups.xml",
            CeKind::RandomPresets => "cfgrandompresets.xml",
            CeKind::Limits => "cfglimitsdefinition.xml",
            CeKind::LimitsUser => "cfglimitsdefinitionuser.xml",
            CeKind::EconomyCore => "cfgeconomycore.xml",
            CeKind::Economy => "economy.xml",
            CeKind::Globals => "globals.xml",
            CeKind::Messages => "messages.xml",
            CeKind::IgnoreList => "cfgignorelist.xml",
            CeKind::Environment => "cfgenvironment.xml",
            CeKind::PlayerSpawnPoints => "cfgplayerspawnpoints.xml",
            CeKind::Weather => "cfgweather.xml",
            CeKind::Territories => "territories.xml",
            CeKind::MapGroupProto => "mapgroupproto.xml",
            CeKind::MapGroupPos => "mapgrouppos.xml",
        }
    }

    /// The `cfgeconomycore.xml` `<file type="…">` value, if the kind has one.
    pub fn include_type(self) -> Option<&'static str> {
        match self {
            CeKind::Types => Some("types"),
            CeKind::SpawnableTypes => Some("spawnabletypes"),
            CeKind::Events => Some("events"),
            CeKind::RandomPresets => Some("randompresets"),
            CeKind::Economy => Some("economy"),
            CeKind::Globals => Some("globals"),
            CeKind::Messages => Some("messages"),
            _ => None,
        }
    }

    /// The kind a `<file type="…">` value names, if any.
    pub fn from_include_type(name: &str) -> Option<CeKind> {
        CeKind::ALL
            .into_iter()
            .find(|k| k.include_type() == Some(name))
    }
}

/// The result of [`detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub kind: Option<CeKind>,
    pub fragment: bool,
    pub root_mismatch: Option<String>,
}

/// Detects a document's kind from its content, using `file_name` only to break ties.
pub fn detect(file_name: Option<&str>, doc: &XmlDoc) -> Detected {
    let hint = file_name.and_then(file_hint);
    let tops = doc.top_elements();
    if tops.is_empty() {
        return Detected {
            kind: hint,
            fragment: false,
            root_mismatch: None,
        };
    }
    if tops.len() == 1 {
        let id = tops[0];
        let name = elem_name(doc, id).to_string();
        if let Some(kind) = CeKind::ALL.into_iter().find(|k| k.root() == name) {
            if kind == CeKind::Types {
                if let Some(detected) = types_root(doc, id) {
                    return detected;
                }
            }
            return Detected {
                kind: Some(kind),
                fragment: false,
                root_mismatch: None,
            };
        }
        if !is_record(&name) {
            return Detected {
                kind: None,
                fragment: false,
                root_mismatch: None,
            };
        }
    }
    fragment(doc, &tops, hint)
}

const TYPE_FIELDS: [&str; 11] = [
    "nominal", "lifetime", "restock", "min", "quantmin", "quantmax", "cost", "flags", "category",
    "usage", "value",
];
const SPAWNABLE_FIELDS: [&str; 4] = ["attachments", "cargo", "hoarder", "damage"];

fn types_root(doc: &XmlDoc, id: NodeId) -> Option<Detected> {
    let type_children = doc.children_named(id, "type");
    let (types_votes, spawn_votes) = type_vote(doc, &type_children);
    if spawn_votes > types_votes {
        return Some(Detected {
            kind: Some(CeKind::SpawnableTypes),
            fragment: false,
            root_mismatch: Some("types".to_string()),
        });
    }
    let children = doc.child_elements(id);
    if !children.is_empty() {
        let any_vote = type_children.iter().any(|&c| has_any_vote(doc, c));
        let all_childless_type = children
            .iter()
            .all(|&c| elem_name(doc, c) == "type" && doc.child_elements(c).is_empty());
        if !any_vote && !all_childless_type {
            return Some(Detected {
                kind: None,
                fragment: false,
                root_mismatch: None,
            });
        }
    }
    None
}

fn fragment(doc: &XmlDoc, tops: &[NodeId], hint: Option<CeKind>) -> Detected {
    let all = |names: &[&str]| tops.iter().all(|&id| names.contains(&elem_name(doc, id)));
    if all(&["type"]) {
        let (types_votes, spawn_votes) = type_vote(doc, tops);
        let kind = if spawn_votes > types_votes {
            Some(CeKind::SpawnableTypes)
        } else if types_votes > spawn_votes {
            Some(CeKind::Types)
        } else {
            hint.or(Some(CeKind::Types))
        };
        return Detected {
            kind,
            fragment: true,
            root_mismatch: None,
        };
    }
    if all(&["event"]) {
        let mut count = 0;
        let mut only_pos_zone = true;
        for &id in tops {
            for child in doc.child_elements(id) {
                count += 1;
                let name = elem_name(doc, child);
                if name != "pos" && name != "zone" {
                    only_pos_zone = false;
                }
            }
        }
        let kind = if only_pos_zone && count > 0 {
            CeKind::EventSpawns
        } else {
            CeKind::Events
        };
        return Detected {
            kind: Some(kind),
            fragment: true,
            root_mismatch: None,
        };
    }
    if all(&["cargo", "attachments"]) {
        return Detected {
            kind: Some(CeKind::RandomPresets),
            fragment: true,
            root_mismatch: None,
        };
    }
    Detected {
        kind: None,
        fragment: true,
        root_mismatch: None,
    }
}

fn type_vote(doc: &XmlDoc, types: &[NodeId]) -> (u32, u32) {
    let mut types_votes = 0;
    let mut spawn_votes = 0;
    for &id in types {
        for child in doc.child_elements(id) {
            let name = elem_name(doc, child);
            if TYPE_FIELDS.contains(&name) {
                types_votes += 1;
            } else if SPAWNABLE_FIELDS.contains(&name) {
                spawn_votes += 1;
            }
        }
    }
    (types_votes, spawn_votes)
}

fn has_any_vote(doc: &XmlDoc, id: NodeId) -> bool {
    doc.child_elements(id).iter().any(|&child| {
        let name = elem_name(doc, child);
        TYPE_FIELDS.contains(&name) || SPAWNABLE_FIELDS.contains(&name)
    })
}

fn elem_name(doc: &XmlDoc, id: NodeId) -> &str {
    doc.element(id).map_or("", |e| e.name.as_str())
}

fn is_record(name: &str) -> bool {
    matches!(name, "type" | "event" | "cargo" | "attachments")
}

fn file_hint(name: &str) -> Option<CeKind> {
    const HINTS: [(&str, CeKind); 13] = [
        ("spawnabletypes", CeKind::SpawnableTypes),
        ("eventspawns", CeKind::EventSpawns),
        ("eventgroups", CeKind::EventGroups),
        ("randompresets", CeKind::RandomPresets),
        ("limitsdefinitionuser", CeKind::LimitsUser),
        ("limitsdefinition", CeKind::Limits),
        ("economycore", CeKind::EconomyCore),
        ("ignorelist", CeKind::IgnoreList),
        ("types", CeKind::Types),
        ("events", CeKind::Events),
        ("globals", CeKind::Globals),
        ("messages", CeKind::Messages),
        ("economy", CeKind::Economy),
    ];
    let lower = name.to_ascii_lowercase();
    HINTS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, kind)| *kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    fn detect_text(name: Option<&str>, text: &str) -> Detected {
        detect(name, &parse(text))
    }

    #[test]
    fn every_root_gives_its_kind() {
        for kind in CeKind::ALL {
            let text = format!("<{}></{}>", kind.root(), kind.root());
            let got = detect_text(None, &text);
            assert_eq!(got.kind, Some(kind), "root of {kind:?}");
            assert!(!got.fragment, "{kind:?}");
            assert_eq!(got.root_mismatch, None, "{kind:?}");
        }
    }

    #[test]
    fn fragment_detection_by_content() {
        let rows: [(&str, &str, Option<CeKind>, bool); 8] = [
            (
                "fragment of types with nominal votes types",
                "<type name=\"a\"><nominal>1</nominal></type>\
                    <type name=\"b\"><nominal>1</nominal></type>\
                    <type name=\"c\"><nominal>1</nominal></type>",
                Some(CeKind::Types),
                true,
            ),
            (
                "fragment of types with attachments votes spawnable types",
                "<type name=\"a\"><attachments/></type>\
                    <type name=\"b\"><attachments/></type>\
                    <type name=\"c\"><attachments/></type>",
                Some(CeKind::SpawnableTypes),
                true,
            ),
            (
                "lone type is a types fragment",
                "<type name=\"a\"><lifetime>1</lifetime></type>",
                Some(CeKind::Types),
                true,
            ),
            (
                "fragment type without votes is types",
                "<type name=\"a\">text</type>",
                Some(CeKind::Types),
                true,
            ),
            (
                "fragment event with nominal is events",
                "<event name=\"a\"><nominal>1</nominal></event>",
                Some(CeKind::Events),
                true,
            ),
            (
                "fragment event with only pos is event spawns",
                "<event name=\"a\"><pos x=\"1\" z=\"2\"/></event>",
                Some(CeKind::EventSpawns),
                true,
            ),
            (
                "fragment of cargo and attachments is random presets",
                "<cargo name=\"a\"/><attachments name=\"b\"/>",
                Some(CeKind::RandomPresets),
                true,
            ),
            (
                "mixed fragment types are none",
                "<type name=\"a\"><nominal>1</nominal></type>\
                    <event name=\"b\"><nominal>1</nominal></event>",
                None,
                true,
            ),
        ];
        for (label, text, kind, fragment) in rows {
            let got = detect_text(None, text);
            assert_eq!(got.kind, kind, "{label}");
            assert_eq!(got.fragment, fragment, "{label}");
            assert_eq!(got.root_mismatch, None, "{label}");
        }
    }

    #[test]
    fn types_root_holding_spawnable_types_mismatches() {
        let text = "<types><type name=\"A\"><attachments/></type></types>";
        let got = detect_text(None, text);
        assert_eq!(got.kind, Some(CeKind::SpawnableTypes));
        assert!(!got.fragment);
        assert_eq!(got.root_mismatch.as_deref(), Some("types"));
    }

    #[test]
    fn types_root_with_a_foreign_format_is_none() {
        let text = "<types><type name=\"A\"><Health_Points>1</Health_Points></type></types>";
        assert_eq!(detect_text(None, text).kind, None);
    }

    #[test]
    fn empty_and_childless_types_roots_are_types() {
        assert_eq!(
            detect_text(None, "<types></types>").kind,
            Some(CeKind::Types)
        );
        assert_eq!(
            detect_text(None, "<types><type name=\"A\"/></types>").kind,
            Some(CeKind::Types)
        );
    }

    #[test]
    fn empty_documents_use_the_file_name_hint() {
        assert_eq!(
            detect_text(Some("cfgspawnabletypes.xml"), "").kind,
            Some(CeKind::SpawnableTypes)
        );
        assert_eq!(detect_text(Some("notes.xml"), "").kind, None);
    }

    #[test]
    fn a_hint_never_overrides_content() {
        let text = "<types><type name=\"A\"><nominal>1</nominal></type></types>";
        assert_eq!(
            detect_text(Some("events.xml"), text).kind,
            Some(CeKind::Types)
        );
    }

    #[test]
    fn unknown_roots_are_not_ce() {
        assert_eq!(detect_text(None, "<Trader></Trader>").kind, None);
        assert_eq!(
            detect_text(None, "<modded_inputs></modded_inputs>").kind,
            None
        );
    }

    #[test]
    fn kinds_round_trip_through_serde() {
        for kind in CeKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", snake_case(kind)));
            let back: CeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn include_types_round_trip() {
        let mut some = 0;
        for kind in CeKind::ALL {
            if let Some(name) = kind.include_type() {
                some += 1;
                assert_eq!(CeKind::from_include_type(name), Some(kind), "{kind:?}");
            }
        }
        assert!(some > 0);
        assert_eq!(CeKind::from_include_type("nope"), None);
    }

    fn snake_case(kind: CeKind) -> String {
        match kind {
            CeKind::Types => "types",
            CeKind::SpawnableTypes => "spawnable_types",
            CeKind::Events => "events",
            CeKind::EventSpawns => "event_spawns",
            CeKind::EventGroups => "event_groups",
            CeKind::RandomPresets => "random_presets",
            CeKind::Limits => "limits",
            CeKind::LimitsUser => "limits_user",
            CeKind::EconomyCore => "economy_core",
            CeKind::Economy => "economy",
            CeKind::Globals => "globals",
            CeKind::Messages => "messages",
            CeKind::IgnoreList => "ignore_list",
            CeKind::Environment => "environment",
            CeKind::PlayerSpawnPoints => "player_spawn_points",
            CeKind::Weather => "weather",
            CeKind::Territories => "territories",
            CeKind::MapGroupProto => "map_group_proto",
            CeKind::MapGroupPos => "map_group_pos",
        }
        .to_string()
    }
}
