use std::path::{Path, PathBuf};

use modfoundry_config::mod_display_name;
use modfoundry_core::{Diagnostic, Severity};
use modfoundry_platform::steam::Visibility;
use modfoundry_vfs::write::WriteTxn;
use serde::{Deserialize, Serialize};

use super::meta::published_id;
use super::PREVIEW_EXTENSIONS;
use crate::build::source_root_file;

/// The item's fields, relative to the mod root.
pub const ITEM_FILE: &str = "workshop/item.json";
/// The item's description, relative to the mod root.
pub const DESCRIPTION_FILE: &str = "workshop/description.txt";
/// Where a saved preview goes, relative to the mod root.
pub const PREVIEW_FILE: &str = "workshop/preview.jpg";

/// The fields of `workshop/item.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    pub tags: Vec<String>,
    pub version: String,
    pub website: String,
    pub facebook: String,
    pub youtube: String,
    pub twitter: String,
    pub reddit: String,
}

/// A mod's Workshop details as read from its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Details {
    pub item: ItemDetails,
    pub description: String,
    pub preview: Option<PathBuf>,
    pub has_item_file: bool,
}

/// Reads a mod's Workshop details from its `workshop/` folder.
pub fn read_details(mod_root: &Path) -> anyhow::Result<Details> {
    let item_text = read_if_file(&mod_root.join(ITEM_FILE))?;
    let has_item_file = item_text.is_some();
    let mut item = match item_text {
        Some(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| anyhow::anyhow!("{ITEM_FILE}: {e}"))?,
        None => ItemDetails::default(),
    };
    if !has_item_file {
        item.title = default_title(mod_root)?;
    }
    if item.id.is_none() {
        item.id = meta_cpp_id(mod_root)?;
    }
    let description = read_if_file(&mod_root.join(DESCRIPTION_FILE))?.unwrap_or_default();
    let preview = PREVIEW_EXTENSIONS
        .iter()
        .map(|ext| mod_root.join(format!("workshop/preview.{ext}")))
        .find(|path| path.is_file());
    Ok(Details {
        item,
        description,
        preview,
        has_item_file,
    })
}

fn default_title(mod_root: &Path) -> anyhow::Result<String> {
    let mod_cpp = root_file_text(mod_root, "mod.cpp")?;
    if let Some(name) = mod_display_name(mod_cpp.as_deref(), None) {
        return Ok(name);
    }
    let folder = std::path::absolute(mod_root)?;
    let name = folder
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    Ok(name.strip_prefix('@').unwrap_or(name).to_owned())
}

fn root_file_text(mod_root: &Path, name: &str) -> anyhow::Result<Option<String>> {
    source_root_file(mod_root, name).map_or(Ok(None), |path| read_if_file(&path))
}

/// The item's tags as Publisher sends them, with `Mod` and the review tags added.
pub fn item_tags(chosen: &[String], content_review: bool) -> Vec<String> {
    let mut tags = Vec::new();
    for tag in chosen {
        push_once(&mut tags, tag);
    }
    if content_review {
        push_once(&mut tags, "Content Review");
    }
    push_once(&mut tags, "Mod");
    if tags.len() >= TAG_REVIEW_AT {
        push_once(&mut tags, "Tag Review");
    }
    tags
}

fn push_once(tags: &mut Vec<String>, tag: &str) {
    if !tags.iter().any(|t| t == tag) {
        tags.push(tag.to_owned());
    }
}

/// The tags a Workshop item may carry besides `Mod`.
pub const ALLOWED_TAGS: [&str; 13] = [
    "Animation",
    "Character",
    "Economy",
    "Environment",
    "Equipment",
    "Mechanics",
    "Sound",
    "Props",
    "Terrain",
    "Vehicle",
    "Weapon",
    "Server",
    "Addon",
];
/// From this many tags Steam holds an item for tag review.
pub const TAG_REVIEW_AT: usize = 7;
/// Steam's title limit in bytes.
pub const TITLE_LIMIT: usize = 128;
/// Steam's description limit in bytes.
pub const DESCRIPTION_LIMIT: usize = 8000;

/// The problems with `details` that block or slow an upload; `new_item` also requires a new item's fields.
pub fn details_checks(details: &Details, new_item: bool) -> Vec<Diagnostic> {
    details_checks_for(details, new_item, false)
}

/// [`details_checks`] for a caller that also knows whether the build carries a `Content Review` tag.
pub fn details_checks_for(
    details: &Details,
    new_item: bool,
    content_review: bool,
) -> Vec<Diagnostic> {
    let item = &details.item;
    let title = item.title.trim();
    let mut found = Vec::new();
    for tag in &item.tags {
        if tag != "Mod" && !ALLOWED_TAGS.contains(&tag.as_str()) {
            found.push(Diagnostic::new(
                "MF-P010",
                Severity::Error,
                format!(
                    "unknown tag \"{tag}\"; use one of: {}",
                    ALLOWED_TAGS.join(", ")
                ),
            ));
        }
    }
    if title.is_empty() {
        found.push(Diagnostic::new(
            "MF-P011",
            Severity::Error,
            "the item has no title; set one in workshop/item.json",
        ));
    }
    if new_item && (!details.has_item_file || title.is_empty() || item.visibility.is_none()) {
        found.push(Diagnostic::new(
            "MF-P012",
            Severity::Error,
            "a new item needs title and visibility in workshop/item.json; ask the author and save them with publish.save_details",
        ));
    }
    if details.preview.is_none() {
        found.push(Diagnostic::new(
            "MF-P013",
            Severity::Warning,
            "no preview image; add workshop/preview.jpg",
        ));
    }
    if item.title.len() > TITLE_LIMIT {
        found.push(Diagnostic::new(
            "MF-P014",
            Severity::Error,
            format!(
                "the title is {} bytes; Steam allows {TITLE_LIMIT}",
                item.title.len()
            ),
        ));
    }
    if details.description.len() > DESCRIPTION_LIMIT {
        found.push(Diagnostic::new(
            "MF-P014",
            Severity::Error,
            format!(
                "the description is {} bytes; Steam allows {DESCRIPTION_LIMIT}",
                details.description.len()
            ),
        ));
    }
    let count = item_tags(&item.tags, content_review)
        .iter()
        .filter(|tag| *tag != "Tag Review")
        .count();
    if count >= TAG_REVIEW_AT {
        found.push(Diagnostic::new(
            "MF-P016",
            Severity::Warning,
            format!("{count} tags; Steam holds the item for tag review"),
        ));
    }
    found
}

/// The item id the mod's root `meta.cpp` names, if it names one.
pub fn meta_cpp_id(mod_root: &Path) -> anyhow::Result<Option<u64>> {
    Ok(root_file_text(mod_root, "meta.cpp")?.and_then(|text| published_id(&text)))
}

/// A digest of the saved item file and description, so a save can tell whether either moved since it was read.
pub fn details_revision(mod_root: &Path) -> anyhow::Result<String> {
    let mut bytes = read_or_empty(&mod_root.join(ITEM_FILE))?;
    bytes.push(0);
    bytes.extend(read_or_empty(&mod_root.join(DESCRIPTION_FILE))?);
    Ok(modfoundry_vfs::sha256_hex(&bytes))
}

fn read_or_empty(path: &Path) -> anyhow::Result<Vec<u8>> {
    if path.is_file() {
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    } else {
        Ok(Vec::new())
    }
}

fn read_if_file(path: &Path) -> anyhow::Result<Option<String>> {
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(Some(text))
    } else {
        Ok(None)
    }
}

/// Writes `item` and `description` into the `workshop/` folder of the transaction's root.
pub fn write_details(
    txn: &mut WriteTxn,
    item: &ItemDetails,
    description: &str,
) -> anyhow::Result<()> {
    txn.write(
        ITEM_FILE,
        format!("{}\n", serde_json::to_string_pretty(item)?),
    )?;
    txn.write(DESCRIPTION_FILE, description)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_root(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mfdetails-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn written_details_read_back() {
        let root = mod_root("round-trip");
        let item = ItemDetails {
            id: Some(3_000_000_001),
            title: "Ammo Pack".into(),
            visibility: Some(Visibility::Public),
            tags: vec!["Weapons".into(), "Ammo".into()],
            website: "https://example.com".into(),
            ..ItemDetails::default()
        };
        let mut txn = WriteTxn::new(modfoundry_vfs::root_name("mod", &root), &root);
        write_details(&mut txn, &item, "A pack of rounds.").unwrap();
        txn.finish().unwrap();

        let details = read_details(&root).unwrap();

        assert_eq!(details.item, item);
        assert_eq!(details.description, "A pack of rounds.");
        assert!(details.has_item_file);
        assert_eq!(details.preview, None);
    }

    #[test]
    fn a_mod_without_a_workshop_folder_defaults_only_its_title() {
        let root = mod_root("no-workshop");
        let title = root.file_name().unwrap().to_str().unwrap().to_owned();

        let details = read_details(&root).unwrap();

        assert_eq!(
            details,
            Details {
                item: ItemDetails {
                    title,
                    ..ItemDetails::default()
                },
                description: String::new(),
                preview: None,
                has_item_file: false,
            }
        );
    }

    #[test]
    fn a_partial_item_file_takes_defaults_and_finds_the_preview() {
        let root = mod_root("partial");
        std::fs::create_dir_all(root.join("workshop")).unwrap();
        std::fs::write(root.join(ITEM_FILE), r#"{"title": "X"}"#).unwrap();
        std::fs::write(root.join("workshop/preview.png"), b"png").unwrap();

        let details = read_details(&root).unwrap();

        assert_eq!(
            details.item,
            ItemDetails {
                title: "X".into(),
                ..ItemDetails::default()
            }
        );
        assert!(details.has_item_file);
        assert_eq!(details.preview, Some(root.join("workshop/preview.png")));
    }

    #[test]
    fn an_item_file_saved_with_a_byte_order_mark_reads() {
        let root = mod_root("bom");
        std::fs::create_dir_all(root.join("workshop")).unwrap();
        std::fs::write(root.join(ITEM_FILE), "\u{feff}{\"title\": \"X\"}").unwrap();

        assert_eq!(read_details(&root).unwrap().item.title, "X");
    }

    #[test]
    fn a_bad_item_file_error_names_the_file() {
        let root = mod_root("bad-json");
        std::fs::create_dir_all(root.join("workshop")).unwrap();
        std::fs::write(root.join(ITEM_FILE), "{ nope").unwrap();

        let err = read_details(&root).expect_err("bad JSON must fail");

        assert!(err.to_string().starts_with("workshop/item.json:"), "{err}");
    }

    #[test]
    fn default_title_comes_from_the_mod_cpp_name() {
        let root = mod_root("title-mod-cpp");
        std::fs::write(root.join("mod.cpp"), r#"name = "Gardening Plus";"#).unwrap();

        assert_eq!(read_details(&root).unwrap().item.title, "Gardening Plus");
    }

    #[test]
    fn default_title_matches_the_mod_cpp_name_in_any_case() {
        let root = mod_root("title-any-case");
        std::fs::write(root.join("MOD.CPP"), r#"name = "Gardening Plus";"#).unwrap();

        assert_eq!(read_details(&root).unwrap().item.title, "Gardening Plus");
    }

    #[test]
    fn default_title_falls_back_to_the_folder_name_without_its_at_sign() {
        let root = mod_root("title-folder").join("@Plain");
        std::fs::create_dir_all(&root).unwrap();

        assert_eq!(read_details(&root).unwrap().item.title, "Plain");
    }

    #[test]
    fn default_id_comes_from_the_root_meta_cpp() {
        let item_files = [None, Some(r#"{"title": "X"}"#)];
        for item_file in item_files {
            let root = mod_root("id-meta");
            std::fs::write(root.join("meta.cpp"), "publishedid = 3690290409;\n").unwrap();
            if let Some(text) = item_file {
                std::fs::create_dir_all(root.join("workshop")).unwrap();
                std::fs::write(root.join(ITEM_FILE), text).unwrap();
            }

            assert_eq!(read_details(&root).unwrap().item.id, Some(3690290409));
        }
    }

    fn strings(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn details_revision_treats_missing_files_as_empty() {
        let empty = mod_root("revision-empty");
        std::fs::create_dir_all(empty.join("workshop")).unwrap();
        std::fs::write(empty.join(ITEM_FILE), "").unwrap();
        std::fs::write(empty.join(DESCRIPTION_FILE), "").unwrap();
        let none = mod_root("revision-none");

        let revision = details_revision(&empty).unwrap();

        assert_eq!(details_revision(&none).unwrap(), revision);
        std::fs::create_dir_all(none.join("workshop")).unwrap();
        std::fs::write(none.join(DESCRIPTION_FILE), "only the description").unwrap();
        assert_ne!(details_revision(&none).unwrap(), revision);
    }

    #[test]
    fn meta_cpp_id_reads_a_published_id_and_ignores_zero() {
        let root = mod_root("meta-id");
        std::fs::write(root.join("meta.cpp"), "publishedid = 5;\r\n").unwrap();
        assert_eq!(meta_cpp_id(&root).unwrap(), Some(5));

        std::fs::write(root.join("meta.cpp"), "publishedid = 0;\r\n").unwrap();
        assert_eq!(meta_cpp_id(&root).unwrap(), None);

        std::fs::remove_file(root.join("meta.cpp")).unwrap();
        assert_eq!(meta_cpp_id(&root).unwrap(), None);
    }

    #[test]
    fn item_tags_follow_publisher_rules() {
        let cases = [
            (strings(&["Weapon"]), false, strings(&["Weapon", "Mod"])),
            (
                strings(&["Mod", "Weapon", "Weapon"]),
                false,
                strings(&["Mod", "Weapon"]),
            ),
            (
                strings(&["Weapon"]),
                true,
                strings(&["Weapon", "Content Review", "Mod"]),
            ),
            (
                strings(&[
                    "Animation",
                    "Character",
                    "Economy",
                    "Environment",
                    "Equipment",
                    "Mechanics",
                ]),
                false,
                strings(&[
                    "Animation",
                    "Character",
                    "Economy",
                    "Environment",
                    "Equipment",
                    "Mechanics",
                    "Mod",
                    "Tag Review",
                ]),
            ),
        ];
        for (chosen, content_review, expected) in cases {
            assert_eq!(item_tags(&chosen, content_review), expected, "{chosen:?}");
        }
    }

    #[test]
    fn content_review_counts_toward_the_tag_review_warning() {
        let details_with = |count: usize| Details {
            item: ItemDetails {
                title: "Gardening Plus".into(),
                visibility: Some(Visibility::Public),
                tags: strings(&ALLOWED_TAGS[..count]),
                ..ItemDetails::default()
            },
            description: String::new(),
            preview: Some(PathBuf::from(PREVIEW_FILE)),
            has_item_file: true,
        };
        let cases = [
            (5, false, None),
            (5, true, Some("7 tags")),
            (6, false, Some("7 tags")),
            (6, true, Some("8 tags")),
        ];
        for (count, content_review, expected) in cases {
            let found: Vec<Diagnostic> =
                details_checks_for(&details_with(count), false, content_review)
                    .into_iter()
                    .filter(|d| d.code == "MF-P016")
                    .collect();
            let label = format!("{count} tags, content review {content_review}");
            match expected {
                None => assert!(found.is_empty(), "{label}: {found:?}"),
                Some(start) => {
                    assert_eq!(found.len(), 1, "{label}: {found:?}");
                    assert!(
                        found[0].message.starts_with(start),
                        "{label}: {}",
                        found[0].message
                    );
                }
            }
        }

        let mut chosen = details_with(0);
        chosen.item.tags = strings(&["Tag Review"]);
        let found = details_checks_for(&chosen, false, false);
        assert!(!found.iter().any(|d| d.code == "MF-P016"), "{found:?}");
    }

    #[test]
    fn details_checks_report_each_field_problem() {
        let good = Details {
            item: ItemDetails {
                title: "Gardening Plus".into(),
                visibility: Some(Visibility::Public),
                ..ItemDetails::default()
            },
            description: String::new(),
            preview: Some(PathBuf::from(PREVIEW_FILE)),
            has_item_file: true,
        };
        let six_tags = strings(&[
            "Animation",
            "Character",
            "Economy",
            "Environment",
            "Equipment",
            "Mechanics",
        ]);
        let cases: Vec<(&str, Details, bool, Vec<&str>)> = vec![
            ("good details", good.clone(), true, vec![]),
            (
                "lower-case tag",
                Details {
                    item: ItemDetails {
                        tags: strings(&["weapon"]),
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                false,
                vec!["MF-P010"],
            ),
            (
                "empty title",
                Details {
                    item: ItemDetails {
                        title: String::new(),
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                false,
                vec!["MF-P011"],
            ),
            (
                "empty title of a new item",
                Details {
                    item: ItemDetails {
                        title: String::new(),
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                true,
                vec!["MF-P011", "MF-P012"],
            ),
            (
                "new item without an item file",
                Details {
                    has_item_file: false,
                    ..good.clone()
                },
                true,
                vec!["MF-P012"],
            ),
            (
                "new item without visibility",
                Details {
                    item: ItemDetails {
                        visibility: None,
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                true,
                vec!["MF-P012"],
            ),
            (
                "no preview",
                Details {
                    preview: None,
                    ..good.clone()
                },
                false,
                vec!["MF-P013"],
            ),
            (
                "title over the limit",
                Details {
                    item: ItemDetails {
                        title: "a".repeat(129),
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                false,
                vec!["MF-P014"],
            ),
            (
                "description over the limit",
                Details {
                    description: "a".repeat(8001),
                    ..good.clone()
                },
                false,
                vec!["MF-P014"],
            ),
            (
                "six tags",
                Details {
                    item: ItemDetails {
                        tags: six_tags,
                        ..good.item.clone()
                    },
                    ..good.clone()
                },
                false,
                vec!["MF-P016"],
            ),
        ];
        for (name, details, new_item, expected) in cases {
            let codes: Vec<String> = details_checks(&details, new_item)
                .into_iter()
                .map(|d| d.code)
                .collect();
            assert_eq!(codes, expected, "{name}");
        }
    }
}
