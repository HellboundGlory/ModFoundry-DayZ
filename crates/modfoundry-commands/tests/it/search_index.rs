//! Search index build: the game's version, the build key and the index file.

use std::fs;
use std::path::PathBuf;

use crate::common::{config_bin, core, pack, tmp, write};
use modfoundry_commands::search_index::{
    build_index, build_key, build_key_checked, pick_embedder, product_version, SearchIndexRequest,
};
use modfoundry_core::{Actor, CommandError};
use modfoundry_search::chunk::Kind;
use modfoundry_search::embed::{Embed, FakeEmbed};
use modfoundry_search::store::Index;
use serde_json::json;
use std::path::Path;

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// A `VS_VERSIONINFO` string entry holding `ProductVersion`, with `lead` bytes before it.
fn version_block(lead: usize, version: &str) -> Vec<u8> {
    let key = utf16le("ProductVersion\0");
    let value = utf16le(&format!("{version}\0"));
    let mut block = vec![0xAA; lead];
    let start = block.len();
    block.extend_from_slice(&0u16.to_le_bytes());
    block.extend_from_slice(&((value.len() / 2) as u16).to_le_bytes());
    block.extend_from_slice(&1u16.to_le_bytes());
    block.extend_from_slice(&key);
    while !block.len().is_multiple_of(4) {
        block.push(0);
    }
    block.extend_from_slice(&value);
    let total = (block.len() - start) as u16;
    block[start..start + 2].copy_from_slice(&total.to_le_bytes());
    block
}

#[test]
fn product_version_reads_the_value_at_any_alignment() {
    for lead in [0, 1, 2, 3] {
        let exe = version_block(lead, "1.30.164289.70");
        assert_eq!(
            product_version(&exe).as_deref(),
            Some("1.30.164289.70"),
            "lead {lead}"
        );
    }
}

#[test]
fn product_version_is_none_without_key_or_whole_value() {
    let block = version_block(0, "1.30.164289.70");
    let cut = block[..block.len() - 4].to_vec();
    assert_eq!(product_version(b"ProductName\0 1.30"), None, "no key");
    assert_eq!(product_version(&cut), None, "value cut off");
}

#[test]
fn build_key_is_the_product_version_of_the_game_exe() {
    let game = tmp("key-exe");
    fs::create_dir_all(&game).unwrap();
    fs::write(
        game.join("DayZ_x64.exe"),
        version_block(1, "1.30.164289.70"),
    )
    .unwrap();
    assert_eq!(build_key(&game, &[]), "1.30.164289.70");
}

#[test]
fn build_key_without_the_exe_hashes_the_pbo_sizes() {
    let game = tmp("key-hash");
    let pbo = game.join("addons/a.pbo");
    pack(&pbo, &[], &[("x.c", b"class A {}")]);
    let key = build_key(&game, std::slice::from_ref(&pbo));
    assert!(key.starts_with("unknown-"), "{key}");
    assert_eq!(key, build_key(&game, std::slice::from_ref(&pbo)));
    pack(&pbo, &[], &[("x.c", b"class A { void B() {} }")]);
    assert_ne!(key, build_key(&game, &[pbo]));
}

#[test]
fn build_index_writes_game_dependency_and_docs_sources() {
    let root = tmp("index-fake");
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[
            ("3_Game\\foo.c", b"class Foo { void Bar() {} }"),
            ("4_World\\baz.c", b"class Baz {}"),
        ],
    );
    let rifle = config_bin("class CfgVehicles { class Rifle { displayName = \"Rifle\"; }; };");
    pack(
        &game.join("addons/a.pbo"),
        &[],
        &[("x\\config.bin", rifle.as_slice())],
    );
    let dep = root.join("@Dep");
    pack(
        &dep.join("addons/dep.pbo"),
        &[],
        &[("dep\\helper.c", b"class Helper {}")],
    );
    write(&root, "docs/README.md", "# Guide\n\nHow the mod loads.\n");
    let req = SearchIndexRequest {
        game: game.clone(),
        dependencies: vec![dep],
        docs: vec![root.join("docs")],
        reference_only: vec!["@dep".into()],
    };
    let cache = tmp("index-cache");
    let embed = FakeEmbed { dims: 8 };
    let mut ticks = Vec::new();
    let report = build_index(&req, &embed, &cache, &mut |done, total| {
        ticks.push((done, total))
    })
    .unwrap();

    assert!(report.build.starts_with("unknown-"), "{}", report.build);
    assert_eq!(
        report.index,
        cache
            .join("modfoundry/search")
            .join(&report.build)
            .join("index.sqlite")
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!((report.sources, report.files), (4, 5));
    assert_eq!(ticks.last(), Some(&(report.chunks, report.chunks)));

    let index = Index::open(&report.index).unwrap();
    assert_eq!(index.len(), report.chunks);
    let query = embed.embed(&["anything"]).unwrap().remove(0);
    let hits: Vec<_> = index
        .nearest(&query, index.len())
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap())
        .collect();

    let config = hits
        .iter()
        .find(|hit| hit.kind == Kind::Config && hit.class.as_deref() == Some("CfgVehicles/Rifle"))
        .expect("the Rifle config hit");
    assert_eq!(config.source, "game");
    assert_eq!(config.path, "x/config.bin");
    assert!(!config.reference_only);

    let script = hits
        .iter()
        .find(|hit| hit.kind == Kind::Script && hit.source == "game")
        .expect("a game script hit");
    assert_eq!(script.path, "scripts/3_Game/foo.c");

    let dependency: Vec<_> = hits.iter().filter(|hit| hit.source == "@Dep").collect();
    assert!(!dependency.is_empty());
    assert!(dependency.iter().all(|hit| hit.reference_only));
    assert!(hits
        .iter()
        .any(|hit| hit.kind == Kind::Docs && !hit.reference_only));
}

#[test]
fn build_index_takes_layouts_from_gui_pbo_and_skips_game_scripts_outside_scripts_pbo() {
    let root = tmp("index-layouts");
    let game = root.join("game");
    let layout = b"FrameWidgetClass hud_root {\n visible 1\n {\n  TextWidgetClass Label {\n   text \"Hi\"\n  }\n }\n}\n";
    pack(
        &game.join("dta/gui.pbo"),
        &[],
        &[("gui\\layouts\\hud.layout", layout.as_slice())],
    );
    pack(
        &game.join("dta/scripts.pbo"),
        &[],
        &[("3_Game\\foo.c", b"class Foo { void Bar() {} }")],
    );
    pack(
        &game.join("addons/a.pbo"),
        &[],
        &[("x\\helper.c", b"class Helper {}")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(&req, &embed, &tmp("index-layouts-cache"), &mut |_, _| {}).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    let index = Index::open(&report.index).unwrap();
    let query = embed.embed(&["x"]).unwrap().remove(0);
    let hits: Vec<_> = index
        .nearest(&query, index.len())
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap())
        .collect();
    let mut layout_hits: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == Kind::Layout)
        .map(|hit| (hit.class.as_deref(), hit.path.as_str(), hit.lines))
        .collect();
    layout_hits.sort_by_key(|&(_, path, lines)| (path, lines));
    assert_eq!(
        layout_hits,
        [
            (Some("hud_root"), "gui/layouts/hud.layout", [1, 2]),
            (Some("hud_root/Label"), "gui/layouts/hud.layout", [4, 5]),
        ]
    );
    assert!(!hits
        .iter()
        .any(|hit| hit.kind == Kind::Script && hit.path == "x/helper.c"));
    assert!(hits
        .iter()
        .any(|hit| hit.kind == Kind::Script && hit.path == "3_Game/foo.c"));
}

#[test]
fn build_index_warns_on_a_broken_layout_and_indexes_the_rest() {
    let root = tmp("index-broken-layout");
    let game = root.join("game");
    pack(
        &game.join("dta/gui.pbo"),
        &[],
        &[
            (
                "gui\\layouts\\bad.layout",
                b"FrameWidgetClass root {\n visible 1\n" as &[u8],
            ),
            (
                "gui\\layouts\\good.layout",
                b"FrameWidgetClass hud_root {\n visible 1\n}\n",
            ),
        ],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(
        &req,
        &embed,
        &tmp("index-broken-layout-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("bad.layout") && w.contains("1:1")),
        "{:?}",
        report.warnings
    );
    let index = Index::open(&report.index).unwrap();
    let (id, _) = index.texts().unwrap()[0].clone();
    assert_eq!(index.hit(id).unwrap().class.as_deref(), Some("hud_root"));
}

#[test]
fn build_index_warns_on_a_bad_pbo_and_indexes_the_rest() {
    let root = tmp("index-bad-pbo");
    let game = root.join("game");
    write(&game, "addons/broken.pbo", "not a pbo");
    pack(
        &game.join("addons/a.pbo"),
        &[],
        &[("x\\config.cpp", b"class CfgVehicles { class Rifle {}; };")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-bad-pbo-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(
        report.warnings.iter().any(|w| w.contains("broken.pbo")),
        "{:?}",
        report.warnings
    );
    assert!(report.chunks > 0);
    assert_eq!(report.files, 1);
}

#[test]
fn build_index_refuses_a_game_path_that_is_not_a_folder() {
    let root = tmp("index-no-game");
    let file = write(&root, "game.txt", "not a folder");
    let cache = tmp("index-no-game-cache");
    let req = SearchIndexRequest {
        game: file,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let err = build_index(&req, &FakeEmbed { dims: 8 }, &cache, &mut |_, _| {}).unwrap_err();
    assert!(err.contains("not a folder"), "{err}");
    assert!(!cache.exists());
}

#[test]
fn search_index_command_refuses_a_game_path_that_is_not_a_folder() {
    let root = tmp("command-no-game");
    let file = write(&root, "game.txt", "not a folder");
    let err = core()
        .dispatch(Actor::CliSession, "search.index", json!({ "game": file }))
        .unwrap_err();
    assert!(
        matches!(&err, CommandError::Failed(msg) if msg.contains("not a folder")),
        "{err}"
    );
}

#[test]
#[ignore]
fn real_install_build_key_and_index_counts() {
    let Some(install) = std::env::var_os("MF_DAYZ_INSTALL") else {
        return;
    };
    let game = PathBuf::from(install);
    assert_eq!(build_key(&game, &[]), "1.30.164289.70");
    let cache = tmp("real-index");
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(&req, &FakeEmbed { dims: 8 }, &cache, &mut |_, _| {}).unwrap();
    println!(
        "build {} sources {} files {} chunks {} millis {}",
        report.build, report.sources, report.files, report.chunks, report.millis
    );
    println!("warnings {:?}", report.warnings);
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn game_sources_are_never_reference_only() {
    let root = tmp("index-game-ref");
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("3_Game\\foo.c", b"class Foo {}")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: vec!["game".into()],
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(&req, &embed, &tmp("index-game-ref-cache"), &mut |_, _| {}).unwrap();
    let index = Index::open(&report.index).unwrap();
    let query = embed.embed(&["x"]).unwrap().remove(0);
    let hits: Vec<_> = index
        .nearest(&query, index.len())
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap())
        .collect();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| !hit.reference_only));
    assert!(
        report.warnings.iter().any(|w| w.contains("game")),
        "{:?}",
        report.warnings
    );
}

/// An embedder that answers with no vectors at all.
struct EmptyEmbed;

impl Embed for EmptyEmbed {
    fn dims(&self) -> usize {
        8
    }

    fn embed(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        Ok(Vec::new())
    }
}

#[test]
fn build_index_fails_when_the_embedder_returns_too_few_vectors() {
    let root = tmp("index-short-embed");
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[("3_Game\\foo.c", b"class Foo {}")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let result = build_index(
        &req,
        &EmptyEmbed,
        &tmp("index-short-embed-cache"),
        &mut |_, _| {},
    );
    assert!(result.is_err());
}

#[test]
fn snippets_keep_the_first_five_lines() {
    let root = tmp("index-snippet");
    write(&root, "docs/guide.md", "# Guide\n\n1\n2\n3\n4\n5\n6\n7\n");
    fs::create_dir_all(root.join("game")).unwrap();
    let req = SearchIndexRequest {
        game: root.join("game"),
        dependencies: Vec::new(),
        docs: vec![root.join("docs")],
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-snippet-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    let index = Index::open(&report.index).unwrap();
    let query = FakeEmbed { dims: 8 }.embed(&["x"]).unwrap().remove(0);
    let hit = index.hit(index.nearest(&query, 1).unwrap()[0].0).unwrap();
    assert_eq!(hit.kind, Kind::Docs);
    assert_eq!(hit.snippet.lines().count(), 5, "{:?}", hit.snippet);
    assert!(!hit.snippet.contains('7'));
}

#[test]
fn docs_files_over_four_megabytes_are_skipped_with_a_warning() {
    let root = tmp("index-docs-size");
    fs::create_dir_all(root.join("game")).unwrap();
    write(&root, "docs/a.md", "# Small\n\nIndexed.\n");
    let text = "x\n".repeat(2 * 1024 * 1024) + "x";
    fs::write(root.join("docs/big.md"), text).unwrap();

    let req = SearchIndexRequest {
        game: root.join("game"),
        dependencies: Vec::new(),
        docs: vec![root.join("docs")],
        reference_only: Vec::new(),
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(&req, &embed, &tmp("index-docs-size-cache"), &mut |_, _| {}).unwrap();

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("big.md") && report.warnings[0].contains("larger than 4 MB"),
        "{:?}",
        report.warnings
    );
    let index = Index::open(&report.index).unwrap();
    let paths: Vec<String> = index
        .texts()
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap().path)
        .collect();
    assert_eq!(paths, ["a.md"]);
}

#[test]
fn an_unreadable_pbo_in_the_fallback_key_is_a_warning() {
    let game = tmp("key-missing-pbo");
    fs::create_dir_all(&game).unwrap();
    let missing = game.join("addons/missing.pbo");
    let mut warnings = Vec::new();
    let key = build_key_checked(&game, std::slice::from_ref(&missing), &mut warnings);
    assert!(key.starts_with("unknown-"), "{key}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&missing.display().to_string()) && warnings[0].contains("size 0"),
        "{warnings:?}"
    );

    let pbo = game.join("addons/a.pbo");
    pack(&pbo, &[], &[("x.c", b"class A {}")]);
    let mut warnings = Vec::new();
    build_key_checked(&game, std::slice::from_ref(&pbo), &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn reference_only_warns_for_a_name_that_matches_no_source() {
    let root = tmp("index-ref-missing");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let dep = root.join("@Dep");
    pack(
        &dep.join("addons/dep.pbo"),
        &[],
        &[("helper.c", b"class Helper {}")],
    );
    let build = |reference_only: Vec<String>| {
        let req = SearchIndexRequest {
            game: game.clone(),
            dependencies: vec![dep.clone()],
            docs: Vec::new(),
            reference_only,
        };
        build_index(
            &req,
            &FakeEmbed { dims: 8 },
            &tmp("index-ref-missing-cache"),
            &mut |_, _| {},
        )
        .unwrap()
    };

    let report = build(vec!["@nothing".into()]);
    assert_eq!(
        report.warnings,
        ["reference-only @nothing matches no source"]
    );
    let report = build(vec!["@DEP".into()]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn snippets_are_cut_at_400_characters_and_lose_trailing_spaces() {
    let root = tmp("index-snippet-trim");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let long = format!("{}\n", "é".repeat(450));
    write(&root, "docs/long.md", &long);
    write(
        &root,
        "docs/spaces.txt",
        "name   value   \ntrailing here   \n\nplain\n",
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: vec![root.join("docs")],
        reference_only: Vec::new(),
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(
        &req,
        &embed,
        &tmp("index-snippet-trim-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    let index = Index::open(&report.index).unwrap();
    let query = embed.embed(&["x"]).unwrap().remove(0);
    let hits: Vec<_> = index
        .nearest(&query, index.len())
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap())
        .collect();

    let long = hits.iter().find(|hit| hit.path == "long.md").unwrap();
    assert_eq!(long.snippet.chars().count(), 400, "{}", long.snippet);

    let spaces = hits.iter().find(|hit| hit.path == "spaces.txt").unwrap();
    assert_eq!(spaces.snippet, "name   value\ntrailing here\n\nplain");
}

#[test]
fn a_pbo_file_is_a_dependency_by_itself() {
    let root = tmp("index-pbo-file");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let pbo = root.join("somewhere/Boxed.pbo");
    pack(
        &pbo,
        &[("prefix", "boxed")],
        &[("scripts\\a.c", b"class A {}")],
    );
    let plain = write(&root, "notes.txt", "not a pbo");

    let req = SearchIndexRequest {
        game: game.clone(),
        dependencies: vec![pbo.clone()],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-pbo-file-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let index = Index::open(&report.index).unwrap();
    let (id, _) = index.texts().unwrap()[0].clone();
    let hit = index.hit(id).unwrap();
    assert_eq!(hit.source, "Boxed.pbo");
    assert_eq!(hit.path, "boxed/scripts/a.c");

    let req = SearchIndexRequest {
        game,
        dependencies: vec![plain.clone()],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-pbo-file-cache-2"),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.sources, 0);
    assert_eq!(
        report.warnings,
        [format!("{} is not a .pbo or a folder", plain.display())]
    );
}

#[test]
fn a_loose_folder_dependency_indexes_scripts_and_config_text_but_not_config_bin() {
    let root = tmp("index-loose-folder");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let loose = root.join("Loose");
    write(&loose, "scripts/a.c", "class A { void B() {} }\n");
    write(
        &loose,
        "config.cpp",
        "class CfgPatches { class Loose {}; };\n",
    );
    let bin = config_bin("class CfgVehicles { class BinOnly { displayName = \"Bin\"; }; };");
    fs::write(loose.join("config.bin"), &bin).unwrap();

    let req = SearchIndexRequest {
        game,
        dependencies: vec![loose],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-loose-folder-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let index = Index::open(&report.index).unwrap();
    let mut paths: Vec<String> = index
        .texts()
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap().path)
        .collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths, ["config.cpp", "scripts/a.c"]);
}

#[test]
fn a_loose_folder_with_a_prefix_marker_stores_paths_under_the_prefix() {
    let root = tmp("index-loose-prefix");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let loose = root.join("Loose");
    write(&loose, "$PBOPREFIX$", "My\\Mod");
    write(&loose, "Scripts/3_Game/x.c", "class X { void Y() {} }\n");

    let req = SearchIndexRequest {
        game,
        dependencies: vec![loose],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-loose-prefix-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let index = Index::open(&report.index).unwrap();
    let mut paths: Vec<String> = index
        .texts()
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap().path)
        .collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths, ["My/Mod/Scripts/3_Game/x.c"]);
}

#[test]
fn a_pbo_without_a_prefix_stores_bare_entry_paths() {
    let root = tmp("index-no-prefix");
    let game = root.join("game");
    pack(
        &game.join("addons/a.pbo"),
        &[],
        &[("x\\config.cpp", b"class CfgPatches { class A {}; };")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: Vec::new(),
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &tmp("index-no-prefix-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let index = Index::open(&report.index).unwrap();
    let (id, _) = index.texts().unwrap()[0].clone();
    assert_eq!(index.hit(id).unwrap().path, "x/config.cpp");
}

#[test]
fn product_version_follows_the_declared_value_length() {
    let block = version_block(1, "1.30.164289.70");
    let lead = 1;
    let cases = [(0u16, None), (4, Some("1.30")), (u16::MAX, None)];
    for (units, want) in cases {
        let mut patched = block.clone();
        patched[lead + 2..lead + 4].copy_from_slice(&units.to_le_bytes());
        assert_eq!(product_version(&patched).as_deref(), want, "units {units}");
    }
}

#[test]
fn build_key_falls_back_when_the_version_is_not_a_plain_folder_name() {
    for version in ["/tmp/elsewhere", "../../up", "..", "a b", &"9".repeat(65)] {
        let game = tmp("key-unsafe");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("DayZ_x64.exe"), version_block(0, version)).unwrap();
        let key = build_key(&game, &[]);
        assert!(key.starts_with("unknown-"), "{version:?} gave {key}");
    }
}

#[test]
fn stored_paths_join_a_slash_wrapped_prefix_cleanly() {
    let root = tmp("index-prefix");
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "\\dz\\scripts\\")],
        &[("3_Game\\foo.c", b"class Foo { void Bar() {} }")],
    );
    let req = SearchIndexRequest {
        game,
        dependencies: vec![],
        docs: vec![],
        reference_only: vec![],
    };
    let cache = root.join("cache");
    let report = build_index(&req, &FakeEmbed { dims: 8 }, &cache, &mut |_, _| {}).unwrap();
    let index = Index::open(&report.index).unwrap();
    let (id, _) = index.texts().unwrap()[0].clone();
    assert_eq!(index.hit(id).unwrap().path, "dz/scripts/3_Game/foo.c");
}

fn cpu_fake(_: &Path) -> Result<Box<dyn Embed>, String> {
    Ok(Box::new(FakeEmbed { dims: 4 }))
}

#[test]
fn pick_embedder_uses_the_cpu_when_there_is_no_helper() {
    let missing = tmp("no-helper");
    let cases = [
        (None, "no GPU helper; embedding on the CPU".to_owned()),
        (
            Some(missing.clone()),
            format!(
                "no GPU helper at {}; embedding on the CPU",
                missing.display()
            ),
        ),
    ];
    for (helper, want) in cases {
        let (embedder, note) =
            pick_embedder(helper.as_deref(), Path::new("model"), None, &cpu_fake).unwrap();
        assert_eq!(embedder.dims(), 4);
        assert_eq!(note, want);
    }
}

// Same "text file busy" guard as the search crate's helper tests.
#[cfg(unix)]
static SPAWNS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
fn write_script(label: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = tmp(label);
    fs::write(&script, body).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
#[test]
fn pick_embedder_falls_back_when_the_helper_cannot_load_cuda() {
    let _spawn = SPAWNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let script = write_script(
        "helper-libs",
        "#!/bin/sh\necho 'error while loading shared libraries: libcublasLt.so.13: cannot open shared object file' >&2\nexit 127\n",
    );
    let picked = pick_embedder(Some(&script), Path::new("model"), None, &cpu_fake);
    let _ = fs::remove_file(&script);
    let (embedder, note) = picked.unwrap();
    assert_eq!(embedder.dims(), 4);
    assert!(
        note.contains("GPU helper failed") && note.contains("libcublasLt.so.13"),
        "{note}"
    );
}

#[cfg(unix)]
#[test]
fn pick_embedder_uses_the_gpu_helper_when_it_starts() {
    let _spawn = SPAWNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let script = write_script(
        "helper-ready",
        "#!/bin/sh\nprintf 'MFE1\\004\\000\\000\\000'\ncat > /dev/null\n",
    );
    let picked = pick_embedder(Some(&script), Path::new("model"), None, &cpu_fake);
    let _ = fs::remove_file(&script);
    let (embedder, note) = picked.unwrap();
    assert_eq!(embedder.dims(), 4);
    assert_eq!(
        note,
        format!("embedding on the GPU with {}", script.display())
    );
}

#[test]
fn build_index_takes_layouts_from_dependency_pbos_and_loose_folders() {
    let root = tmp("index-dep-layouts");
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[],
        &[("a.c", b"class A {}")],
    );
    let packed = root.join("@Packed");
    pack(
        &packed.join("addons/p.pbo"),
        &[("prefix", "packed")],
        &[(
            "gui\\menu.layout",
            b"FrameWidgetClass menu_root {\n visible 1\n}\n",
        )],
    );
    let loose = root.join("Loose");
    write(
        &loose,
        "gui/hud.layout",
        "FrameWidgetClass hud_root {\n visible 1\n}\n",
    );
    let req = SearchIndexRequest {
        game,
        dependencies: vec![packed, loose],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let embed = FakeEmbed { dims: 8 };
    let report = build_index(
        &req,
        &embed,
        &tmp("index-dep-layouts-cache"),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let index = Index::open(&report.index).unwrap();
    let mut layouts: Vec<_> = index
        .texts()
        .unwrap()
        .into_iter()
        .map(|(id, _)| index.hit(id).unwrap())
        .filter(|hit| hit.kind == Kind::Layout)
        .map(|hit| (hit.path, hit.class))
        .collect();
    layouts.sort();
    assert_eq!(
        layouts,
        [
            ("gui/hud.layout".to_string(), Some("hud_root".to_string())),
            (
                "packed/gui/menu.layout".to_string(),
                Some("menu_root".to_string())
            ),
        ]
    );
}
