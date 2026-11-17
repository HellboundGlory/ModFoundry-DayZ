//! Workshop details through the command registry: read, save, preview and prepare.

use crate::common::{self, tmp};
use modfoundry_commands::publish_upload::{upload, PublishStatus, UploadArgs, UploadResult};
use modfoundry_core::{Actor, Outcome};
use modfoundry_platform::steam::{
    CancelToken, SteamAccount, SteamError, SteamEvent, SteamRequest, SteamRun, UploadStage,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

fn dispatch(core: &modfoundry_core::Core, id: &str, args: Value) -> Outcome {
    core.dispatch(Actor::CliSession, id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

/// A 1x1 24-bit BMP: the 14-byte file header, the 40-byte info header, one pixel and its padding.
fn one_pixel_bmp() -> Vec<u8> {
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&58u32.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&1i32.to_le_bytes());
    bmp.extend_from_slice(&1i32.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&[10, 20, 30, 0]);
    bmp
}

fn diagnostic_codes(view: &Value) -> Vec<&str> {
    view["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect()
}

#[test]
fn details_of_a_new_mod_name_its_title_and_the_missing_fields() {
    let dir = tmp("publish-new");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();

    let Outcome::Data(view) = dispatch(
        &common::core(),
        "publish.details",
        json!({ "mod_root": dir }),
    ) else {
        panic!("expected Data");
    };

    assert_eq!(view["item"]["title"], "MyMod");
    assert_eq!(view["has_item_file"], false);
    let codes = diagnostic_codes(&view);
    assert!(codes.contains(&"MF-P012"), "{codes:?}");
    assert!(codes.contains(&"MF-P013"), "{codes:?}");
}

#[test]
fn saved_details_read_back_with_a_jpeg_preview() {
    let dir = tmp("publish-save");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let picture = "cover.bmp";
    let core = common::core();

    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit", "visibility": "private", "tags": ["Weapon"] },
            "description": "A kit for the field.",
            "preview_from": picture,
        }),
    );

    let Outcome::Data(view) = dispatch(&core, "publish.details", json!({ "mod_root": dir })) else {
        panic!("expected Data");
    };
    assert_eq!(view["item"]["title"], "Field Kit");
    assert_eq!(view["item"]["visibility"], "private");
    assert_eq!(view["item"]["tags"], json!(["Weapon"]));
    assert_eq!(view["description"], "A kit for the field.");
    assert_eq!(view["has_item_file"], true);
    let codes = diagnostic_codes(&view);
    assert!(!codes.contains(&"MF-P012"), "{codes:?}");
    assert!(!codes.contains(&"MF-P013"), "{codes:?}");
    let jpeg = std::fs::read(dir.join("workshop/preview.jpg")).unwrap();
    assert!(jpeg.starts_with(&[0xFF, 0xD8]));
}

#[test]
fn a_preview_of_a_save_writes_nothing() {
    let dir = tmp("publish-preview");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let picture = tmp("publish-preview-picture").with_extension("bmp");
    std::fs::write(&picture, one_pixel_bmp()).unwrap();

    let out = dispatch(
        &common::core(),
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit" },
            "preview_from": picture,
            "preview": true,
        }),
    );

    assert!(matches!(out, Outcome::Preview(_)), "{out:?}");
    assert!(!dir.join("workshop").exists());
}

#[test]
fn a_new_preview_replaces_older_pictures_of_other_types() {
    let dir = tmp("publish-preview-replace");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let workshop = dir.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    std::fs::write(workshop.join("preview.png"), b"png").unwrap();
    std::fs::write(workshop.join("preview.jpeg"), b"jpeg").unwrap();
    std::fs::write(workshop.join("preview.bmp"), one_pixel_bmp()).unwrap();
    let core = common::core();
    let save = |preview_from: Option<&str>| {
        let mut args = json!({
            "mod_root": dir,
            "details": { "title": "Field Kit" },
            "description": "A kit for the field.",
        });
        if let Some(picture) = preview_from {
            args["preview_from"] = json!(picture);
        }
        dispatch(&core, "publish.save_details", args)
    };

    save(Some("cover.bmp"));

    let mut names: Vec<String> = std::fs::read_dir(&workshop)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["description.txt", "item.json", "preview.jpg"],
        "{names:?}"
    );
    let jpeg = std::fs::read(workshop.join("preview.jpg")).unwrap();
    assert!(jpeg.starts_with(&[0xFF, 0xD8]));

    save(None);

    assert!(workshop.join("preview.jpg").is_file());
}

#[test]
fn a_preview_of_a_save_removes_nothing() {
    let dir = tmp("publish-preview-removes-nothing");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let workshop = dir.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    for name in ["preview.jpg", "preview.png", "preview.jpeg"] {
        std::fs::write(workshop.join(name), one_pixel_bmp()).unwrap();
    }
    std::fs::write(workshop.join("preview.bmp"), one_pixel_bmp()).unwrap();

    let out = dispatch(
        &common::core(),
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit" },
            "preview_from": "cover.bmp",
            "preview": true,
        }),
    );

    let Outcome::Preview(change_set) = out else {
        panic!("expected Preview: {out:?}");
    };
    let removed: Vec<String> = change_set["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|change| change["op"] == "delete")
        .map(|change| change["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(removed.len(), 3, "{removed:?}");
    for name in ["preview.jpg", "preview.png", "preview.jpeg", "preview.bmp"] {
        assert!(workshop.join(name).is_file(), "{name} was removed");
    }
}

#[test]
fn saving_details_without_an_id_keeps_the_saved_one() {
    let dir = tmp("publish-keep-id");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    let save = |details: Value| {
        dispatch(
            &core,
            "publish.save_details",
            json!({ "mod_root": dir, "details": details }),
        )
    };

    save(json!({ "id": 7, "title": "Field Kit" }));
    save(json!({ "title": "Field Kit 2" }));

    let Outcome::Data(view) = dispatch(&core, "publish.details", json!({ "mod_root": dir })) else {
        panic!("expected Data");
    };
    assert_eq!(view["item"]["id"], 7);
    assert_eq!(view["item"]["title"], "Field Kit 2");
}

fn folder_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folder_size(&path)
            } else {
                path.metadata().unwrap().len()
            }
        })
        .sum()
}

fn kv_tag<'a>(plan: &'a Value, key: &str) -> Option<&'a Value> {
    plan["kv_tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tag| tag["key"] == key)
        .map(|tag| &tag["value"])
}

#[test]
fn prepare_of_a_saved_mod_answers_a_ready_upload_plan() {
    let dir = tmp("publish-prepare");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit", "visibility": "private", "tags": ["Weapon"] },
            "preview_from": "cover.bmp",
        }),
    );

    let Outcome::Data(plan) = dispatch(
        &core,
        "publish.prepare",
        json!({
            "mod_root": dir,
            "name": "MyMod",
            "profile": "pub",
            "authority": "MF",
            "keys_dir": signing_keys(),
        }),
    ) else {
        panic!("expected Data");
    };

    assert_eq!(plan["ready"], true);
    assert_eq!(plan["tags"], json!(["Weapon", "Mod"]));
    assert!(plan["id"].is_null());
    let content_dir = PathBuf::from(plan["content_dir"].as_str().unwrap());
    let meta = content_dir.join("meta.cpp");
    let meta_text = std::fs::read_to_string(&meta).unwrap();
    assert!(meta_text.contains("publishedid = 0;"), "{meta_text}");
    assert!(meta_text.contains("name = \"Field Kit\";"), "{meta_text}");
    let meta_len = std::fs::metadata(&meta).unwrap().len();
    assert_eq!(kv_tag(&plan, "bis_platform"), Some(&json!("-")));
    assert!(kv_tag(&plan, "bis_shortHash").is_some());
    assert!(kv_tag(&plan, "bis_longHash").is_some());
    let size = folder_size(&content_dir) - meta_len;
    assert_eq!(kv_tag(&plan, "bis_size"), Some(&json!(size.to_string())));
    let preview = PathBuf::from(plan["preview"].as_str().unwrap());
    assert_eq!(
        preview,
        std::path::absolute(dir.join("builds/pub/preview.jpg")).unwrap()
    );
    assert!(std::fs::read(&preview).unwrap().starts_with(&[0xFF, 0xD8]));
}

#[test]
fn prepare_of_an_unsaved_unsigned_mod_is_not_ready() {
    let dir = tmp("publish-prepare-unsaved");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();

    let Outcome::Data(plan) = dispatch(
        &common::core(),
        "publish.prepare",
        json!({ "mod_root": dir, "name": "MyMod", "profile": "pub" }),
    ) else {
        panic!("expected Data");
    };

    assert_eq!(plan["ready"], false);
    let codes = diagnostic_codes(&plan);
    assert!(codes.contains(&"MF-P012"), "{codes:?}");
    assert!(
        codes.contains(&"MF-P007"),
        "an unsigned build is not ready: {codes:?}"
    );
}

#[test]
fn content_only_without_an_item_id_is_not_ready() {
    let dir = tmp("publish-prepare-content-only-no-id");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "title": "Field Kit", "visibility": "private" },
            "preview_from": "cover.bmp",
        }),
    );
    let prepare = |content_only: bool| {
        let Outcome::Data(plan) = dispatch(
            &core,
            "publish.prepare",
            json!({
                "mod_root": dir,
                "name": "MyMod",
                "profile": "pub",
                "authority": "MF",
                "keys_dir": signing_keys(),
                "content_only": content_only,
            }),
        ) else {
            panic!("expected Data");
        };
        plan
    };

    let plan = prepare(true);

    assert_eq!(plan["ready"], false);
    assert_eq!(plan["content_only"], false);
    let codes = diagnostic_codes(&plan);
    assert!(codes.contains(&"MF-P018"), "{codes:?}");

    let plan = prepare(false);

    let codes = diagnostic_codes(&plan);
    assert!(!codes.contains(&"MF-P018"), "{codes:?}");
}

#[test]
fn prepare_removes_a_pbo_an_earlier_build_left_and_says_so() {
    let dir = tmp("publish-prepare-clean");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let out = tmp("publish-prepare-clean-out");
    let core = common::core();
    let prepare = |name: &str| {
        let Outcome::Data(plan) = dispatch(
            &core,
            "publish.prepare",
            json!({
                "mod_root": dir,
                "name": name,
                "prefix": "MyMod",
                "output": out,
                "profile": "pub",
                "authority": "MF",
                "keys_dir": signing_keys(),
            }),
        ) else {
            panic!("expected Data");
        };
        plan
    };

    prepare("Old");
    let plan = prepare("MyMod");

    let addons = out.join("addons");
    let mut names: Vec<String> = std::fs::read_dir(&addons)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["MyMod.pbo", "MyMod.pbo.MF.bisign"]);
    let messages: Vec<&str> = plan["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "MF-P020")
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        messages,
        [
            "removed addons/Old.pbo from the build output: this build did not write it",
            "removed addons/Old.pbo.MF.bisign from the build output: this build did not write it"
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn prepare_warns_about_every_config_file_the_build_packs() {
    let with_sub = tmp("publish-prepare-configs");
    modfoundry_commands::scaffold(
        &with_sub,
        &modfoundry_commands::ScaffoldOptions::new("MyMod"),
    )
    .unwrap();
    common::write(&with_sub, "sub/config.cpp", "class CfgPatches {}");
    let without_sub = tmp("publish-prepare-one-config");
    modfoundry_commands::scaffold(
        &without_sub,
        &modfoundry_commands::ScaffoldOptions::new("MyMod"),
    )
    .unwrap();
    let core = common::core();
    let prepare = |dir: &Path| {
        let Outcome::Data(plan) = dispatch(
            &core,
            "publish.prepare",
            json!({
                "mod_root": dir,
                "name": "MyMod",
                "profile": "pub",
                "authority": "MF",
                "keys_dir": signing_keys(),
            }),
        ) else {
            panic!("expected Data");
        };
        plan
    };

    let plan = prepare(&with_sub);
    let warnings: Vec<&Value> = plan["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "MF-P019")
        .collect();
    assert_eq!(warnings.len(), 1, "{}", plan["diagnostics"]);
    assert_eq!(warnings[0]["severity"], "warning");
    let message = warnings[0]["message"].as_str().unwrap();
    assert!(
        message.contains("config.cpp") && message.contains("sub/config.cpp"),
        "{message}"
    );

    let without = prepare(&without_sub);
    assert!(
        !diagnostic_codes(&without).contains(&"MF-P019"),
        "{}",
        without["diagnostics"]
    );
    assert_eq!(
        without["ready"], plan["ready"],
        "a content warning must not change readiness"
    );
    let _ = std::fs::remove_dir_all(&with_sub);
    let _ = std::fs::remove_dir_all(&without_sub);
}

#[test]
fn prepare_counts_the_content_review_tag() {
    let dir = tmp("publish-prepare-content-review");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("cover.bmp"), one_pixel_bmp()).unwrap();
    let extra = dir.join("builds/pub/@MyMod/bin");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("library.dll"), b"dll").unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": {
                "title": "Field Kit",
                "visibility": "private",
                "tags": ["Animation", "Character", "Economy", "Environment", "Equipment"],
            },
            "preview_from": "cover.bmp",
        }),
    );

    let Outcome::Data(plan) = dispatch(
        &core,
        "publish.prepare",
        json!({
            "mod_root": dir,
            "name": "MyMod",
            "profile": "pub",
            "authority": "MF",
            "keys_dir": signing_keys(),
        }),
    ) else {
        panic!("expected Data");
    };

    let codes = diagnostic_codes(&plan);
    assert!(codes.contains(&"MF-P005"), "{codes:?}");
    assert!(codes.contains(&"MF-P016"), "{codes:?}");
    let tags = plan["tags"].as_array().unwrap();
    assert!(tags.contains(&json!("Content Review")), "{tags:?}");
    assert!(tags.contains(&json!("Tag Review")), "{tags:?}");
}

#[test]
fn an_imported_item_without_saved_details_sends_only_its_content() {
    let dir = tmp("publish-prepare-imported");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    std::fs::write(dir.join("meta.cpp"), "publishedid = 55;\n").unwrap();

    let Outcome::Data(plan) = dispatch(
        &common::core(),
        "publish.prepare",
        json!({ "mod_root": dir, "name": "MyMod", "profile": "pub" }),
    ) else {
        panic!("expected Data");
    };

    assert_eq!(plan["id"], 55);
    assert_eq!(plan["content_only"], true);
    assert!(plan["visibility"].is_null(), "{}", plan["visibility"]);
}

/// A Steam helper that answers from a script and records every request it gets.
struct FakeSteam {
    replies: Mutex<VecDeque<Result<SteamEvent, SteamError>>>,
    requests: Mutex<Vec<SteamRequest>>,
}

impl FakeSteam {
    fn new(replies: Vec<Result<SteamEvent, SteamError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<SteamRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn record(&self, request: &SteamRequest) {
        self.requests.lock().unwrap().push(request.clone());
    }
}

impl SteamRun for FakeSteam {
    fn run(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<SteamEvent, SteamError> {
        self.record(request);
        if matches!(request, SteamRequest::Update(_)) {
            on_progress(UploadStage::UploadingContent, 1, 2);
        }
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted reply")
    }
}

fn account_reply() -> Result<SteamEvent, SteamError> {
    Ok(SteamEvent::Account(SteamAccount {
        name: "builder".into(),
        steam_id: 76_561_198_000_000_001,
    }))
}

/// The key the publish tests sign their builds with, made once per test run.
fn signing_keys() -> &'static Path {
    static KEYS: OnceLock<PathBuf> = OnceLock::new();
    KEYS.get_or_init(|| {
        let dir = tmp("publish-signing-keys");
        std::fs::create_dir_all(&dir).unwrap();
        modfoundry_commands::keygen("MF", &dir, None).unwrap();
        dir
    })
}

fn upload_args(dir: &Path, content_only: bool) -> UploadArgs {
    serde_json::from_value(json!({
        "mod_root": dir,
        "name": "MyMod",
        "profile": "pub",
        "authority": "MF",
        "keys_dir": signing_keys(),
        "content_only": content_only,
    }))
    .unwrap()
}

fn send(
    args: &UploadArgs,
    steam: &dyn SteamRun,
    status: &PublishStatus,
) -> anyhow::Result<UploadResult> {
    upload(args, steam, status, Duration::from_secs(1_800_000_000))
}

#[test]
fn an_unsaved_mod_is_not_sent_to_steam() {
    let dir = tmp("publish-upload-unsaved");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let steam = FakeSteam::new(vec![]);

    let err = send(&upload_args(&dir, false), &steam, &PublishStatus::default()).unwrap_err();

    assert!(err.to_string().starts_with("not published:"), "{err}");
    assert!(err.to_string().contains("MF-P012"), "{err}");
    assert!(steam.requests().is_empty());
}

fn submitted_reply(id: u64) -> Result<SteamEvent, SteamError> {
    Ok(SteamEvent::Submitted {
        id,
        needs_legal_agreement: false,
    })
}

#[test]
fn an_existing_item_is_updated_and_the_status_clears() {
    let dir = tmp("publish-upload-existing");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "id": 7, "title": "Field Kit" } }),
    );
    let steam = FakeSteam::new(vec![account_reply(), submitted_reply(7)]);
    let status = PublishStatus::default();

    let result = send(&upload_args(&dir, false), &steam, &status).unwrap();

    assert_eq!(result.id, 7);
    assert!(!result.new_item);
    assert_eq!(status.current(), None);
    let requests = steam.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[0], SteamRequest::Account);
    let SteamRequest::Update(update) = &requests[1] else {
        panic!("{requests:?}")
    };
    assert_eq!(update.id, 7);
    assert_eq!(update.title.as_deref(), Some("Field Kit"));
}

fn created_reply(id: u64) -> Result<SteamEvent, SteamError> {
    Ok(SteamEvent::Created {
        id,
        needs_legal_agreement: false,
    })
}

#[test]
fn a_new_item_is_created_and_its_id_is_written_into_the_mod() {
    let dir = tmp("publish-upload-new");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let steam = FakeSteam::new(vec![
        account_reply(),
        created_reply(42),
        submitted_reply(42),
    ]);

    let result = send(&upload_args(&dir, false), &steam, &PublishStatus::default()).unwrap();

    assert_eq!(result.id, 42);
    assert!(result.new_item);
    let requests = steam.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests[0], SteamRequest::Account);
    assert_eq!(requests[1], SteamRequest::Create);
    let SteamRequest::Update(update) = &requests[2] else {
        panic!("{requests:?}")
    };
    assert_eq!(update.id, 42);
    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert_eq!(item["id"], 42);
    let meta = std::fs::read_to_string(update.content_dir.join("meta.cpp")).unwrap();
    assert!(meta.contains("publishedid = 42;"), "{meta}");
}

#[test]
fn a_failed_update_deletes_the_new_item_and_takes_its_id_back_out() {
    let dir = tmp("publish-upload-failed");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let steam = FakeSteam::new(vec![
        account_reply(),
        created_reply(42),
        Err(SteamError::Steam("disk full".into())),
        Ok(SteamEvent::Deleted { id: 42 }),
    ]);

    let err = send(&upload_args(&dir, false), &steam, &PublishStatus::default()).unwrap_err();

    assert_eq!(
        steam.requests().last(),
        Some(&SteamRequest::Delete { id: 42 })
    );
    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert!(item["id"].is_null(), "{item}");
    let text = err.to_string();
    assert!(text.contains("disk full"), "{text}");
    assert!(text.contains("was deleted"), "{text}");
}

#[test]
fn a_new_item_whose_id_cannot_be_written_is_deleted() {
    let dir = tmp("publish-upload-unwritable");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    std::fs::remove_file(dir.join("workshop/description.txt")).unwrap();
    std::fs::create_dir(dir.join("workshop/description.txt")).unwrap();
    let steam = FakeSteam::new(vec![
        account_reply(),
        created_reply(42),
        Ok(SteamEvent::Deleted { id: 42 }),
    ]);

    let err = send(&upload_args(&dir, false), &steam, &PublishStatus::default()).unwrap_err();

    assert_eq!(
        steam.requests().last(),
        Some(&SteamRequest::Delete { id: 42 })
    );
    assert!(err.to_string().contains("was deleted"), "{err}");
    assert!(
        err.to_string()
            .starts_with("could not record the new item's id"),
        "{err}"
    );
    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert!(item["id"].is_null(), "{item}");
}

#[test]
fn a_new_item_that_cannot_be_deleted_keeps_its_id_in_the_mod() {
    let dir = tmp("publish-upload-undeletable");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let steam = FakeSteam::new(vec![
        account_reply(),
        created_reply(42),
        Err(SteamError::Steam("disk full".into())),
        Err(SteamError::Steam("Steam is not running".into())),
    ]);

    let err = send(&upload_args(&dir, false), &steam, &PublishStatus::default()).unwrap_err();

    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert_eq!(item["id"], 42);
    assert!(err.to_string().contains("could not be deleted"), "{err}");
}

#[test]
fn a_content_only_update_leaves_the_item_fields_to_steam() {
    let dir = tmp("publish-upload-content-only");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({
            "mod_root": dir,
            "details": { "id": 7, "title": "Field Kit", "visibility": "private", "tags": ["Weapon"] },
        }),
    );
    let steam = FakeSteam::new(vec![account_reply(), submitted_reply(7)]);

    send(&upload_args(&dir, true), &steam, &PublishStatus::default()).unwrap();

    let requests = steam.requests();
    let SteamRequest::Update(update) = &requests[1] else {
        panic!("{requests:?}")
    };
    assert_eq!(update.title, None);
    assert_eq!(update.description, None);
    assert_eq!(update.visibility, None);
    assert_eq!(update.tags, None);
    assert_eq!(update.preview, None);
}

/// Starts a second upload from inside the first one's Steam calls and keeps how it ended.
struct Reentrant<'a> {
    status: &'a PublishStatus,
    args: &'a UploadArgs,
    inner: FakeSteam,
    refusal: Mutex<Option<String>>,
}

impl SteamRun for Reentrant<'_> {
    fn run(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<SteamEvent, SteamError> {
        let second = send(self.args, &self.inner, self.status);
        *self.refusal.lock().unwrap() = second.err().map(|e| e.to_string());
        self.inner.run(request, on_progress)
    }
}

/// A Steam helper that cancels the running upload when it gets a request it was told to.
struct CancelsOn<'a> {
    status: &'a PublishStatus,
    on: fn(&SteamRequest) -> bool,
    inner: FakeSteam,
}

impl SteamRun for CancelsOn<'_> {
    fn run(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<SteamEvent, SteamError> {
        if (self.on)(request) {
            assert!(self.status.cancel(), "an upload is running");
        }
        self.inner.run(request, on_progress)
    }

    fn run_cancellable(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
        cancel: &CancelToken,
    ) -> Result<SteamEvent, SteamError> {
        if (self.on)(request) {
            assert!(self.status.cancel(), "an upload is running");
        }
        if cancel.is_cancelled() {
            self.inner.record(request);
            return Err(SteamError::Cancelled);
        }
        self.inner.run(request, on_progress)
    }
}

#[test]
fn a_cancel_during_the_upload_deletes_the_new_item() {
    let dir = tmp("publish-cancel-update");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let status = PublishStatus::default();
    let steam = CancelsOn {
        status: &status,
        on: |request| matches!(request, SteamRequest::Update(_)),
        inner: FakeSteam::new(vec![
            account_reply(),
            created_reply(42),
            Ok(SteamEvent::Deleted { id: 42 }),
        ]),
    };

    let err = send(&upload_args(&dir, false), &steam, &status).unwrap_err();

    let text = err.to_string();
    assert!(text.contains("cancelled"), "{text}");
    assert!(text.contains("was deleted"), "{text}");
    let requests = steam.inner.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_eq!(requests[0], SteamRequest::Account);
    assert_eq!(requests[1], SteamRequest::Create);
    assert!(
        matches!(requests[2], SteamRequest::Update(_)),
        "{requests:?}"
    );
    assert_eq!(requests[3], SteamRequest::Delete { id: 42 });
    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert!(item["id"].is_null(), "{item}");
    assert_eq!(status.current(), None);
}

#[test]
fn a_cancel_before_the_item_is_made_sends_nothing_more() {
    let dir = tmp("publish-cancel-account");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let status = PublishStatus::default();
    let steam = CancelsOn {
        status: &status,
        on: |request| matches!(request, SteamRequest::Account),
        inner: FakeSteam::new(vec![account_reply()]),
    };

    let err = send(&upload_args(&dir, false), &steam, &status).unwrap_err();

    assert!(
        err.to_string().starts_with("not published: cancelled"),
        "{err}"
    );
    assert_eq!(steam.inner.requests(), [SteamRequest::Account]);
    let item: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workshop/item.json")).unwrap())
            .unwrap();
    assert!(item["id"].is_null(), "{item}");
}

#[test]
fn a_cancel_does_not_carry_into_the_next_upload() {
    let dir = tmp("publish-cancel-next");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    let core = common::core();
    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "title": "Field Kit", "visibility": "private" } }),
    );
    let status = PublishStatus::default();
    let cancelling = CancelsOn {
        status: &status,
        on: |request| matches!(request, SteamRequest::Account),
        inner: FakeSteam::new(vec![account_reply()]),
    };
    let cancelled = send(&upload_args(&dir, false), &cancelling, &status).unwrap_err();
    assert!(
        cancelled
            .to_string()
            .starts_with("not published: cancelled"),
        "{cancelled}"
    );

    dispatch(
        &core,
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "id": 7, "title": "Field Kit" } }),
    );
    let steam = FakeSteam::new(vec![account_reply(), submitted_reply(7)]);

    let result = send(&upload_args(&dir, false), &steam, &status).unwrap();

    assert_eq!(result.id, 7);
    let requests = steam.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[0], SteamRequest::Account);
    assert!(
        matches!(requests[1], SteamRequest::Update(_)),
        "{requests:?}"
    );
}

#[test]
fn a_cancelled_update_of_an_existing_item_deletes_nothing() {
    let dir = tmp("publish-cancel-existing");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "id": 7, "title": "Field Kit" } }),
    );
    let status = PublishStatus::default();
    let steam = CancelsOn {
        status: &status,
        on: |request| matches!(request, SteamRequest::Update(_)),
        inner: FakeSteam::new(vec![account_reply()]),
    };

    let err = send(&upload_args(&dir, false), &steam, &status).unwrap_err();

    assert_eq!(err.to_string(), "the upload failed: cancelled");
    let requests = steam.inner.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        matches!(requests[1], SteamRequest::Update(_)),
        "{requests:?}"
    );
    assert!(status.current().is_none());
}

#[test]
fn a_second_upload_is_refused_while_one_runs() {
    let dir = tmp("publish-upload-running");
    modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
        .unwrap();
    dispatch(
        &common::core(),
        "publish.save_details",
        json!({ "mod_root": dir, "details": { "id": 7, "title": "Field Kit" } }),
    );
    let status = PublishStatus::default();
    let args = upload_args(&dir, false);
    let steam = Reentrant {
        status: &status,
        args: &args,
        inner: FakeSteam::new(vec![account_reply(), submitted_reply(7)]),
        refusal: Mutex::new(None),
    };

    let result = send(&args, &steam, &status).unwrap();

    assert_eq!(result.id, 7);
    assert_eq!(
        steam.refusal.lock().unwrap().as_deref(),
        Some("an upload is already running")
    );
}

#[test]
fn the_upload_prompt_says_what_goes_up() {
    let cases = [
        (
            json!({ "title": "Field Kit", "visibility": "private" }),
            false,
            r#"Upload "Field Kit" as a new private item from {root}"#,
        ),
        (
            json!({ "id": 7, "title": "Field Kit", "visibility": "private" }),
            true,
            r#"Update item 7 "Field Kit" (private), content only from {root}"#,
        ),
        (
            json!({ "id": 7, "title": "Field Kit" }),
            false,
            r#"Update item 7 "Field Kit" (visibility unchanged) from {root}"#,
        ),
        (
            json!({ "title": "Field Kit" }),
            false,
            r#"Upload "Field Kit" as a new item (visibility not set) from {root}"#,
        ),
    ];
    for (details, content_only, expected) in cases {
        let dir = tmp("publish-prompt");
        modfoundry_commands::scaffold(&dir, &modfoundry_commands::ScaffoldOptions::new("MyMod"))
            .unwrap();
        dispatch(
            &common::core(),
            "publish.save_details",
            json!({ "mod_root": dir, "details": details }),
        );

        let core = common::core();
        let summary = core
            .command("publish.upload")
            .unwrap()
            .confirm_summary(&json!({ "mod_root": dir, "content_only": content_only }));

        let expected = expected.replace("{root}", &dir.display().to_string());
        assert_eq!(summary.as_deref(), Some(expected.as_str()));
    }
}
