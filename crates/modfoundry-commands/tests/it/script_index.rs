//! `script_index` loader contract tests: vanilla, dependency and workspace
//! sources into one `modfoundry_script::Index`.

use std::path::PathBuf;

use crate::common::{pack, tmp, write};
use modfoundry_commands::script_index::{
    load_index_with_texts, IndexRequest, Progress, ScriptSource,
};
use modfoundry_script::{Layer, Origin};

fn request(workspace: PathBuf) -> IndexRequest {
    IndexRequest {
        workspace,
        dependencies: Vec::new(),
        game: Some(tmp("no-game")),
    }
}

#[test]
fn script_index_workspace_walk() {
    let root = tmp("workspace");
    let a = write(
        &root,
        "Scripts/4_World/a.c",
        "modded class PlayerBase { void Mine(); }",
    );
    write(&root, ".git/x.c", "class Hidden {}");
    write(&root, "builds/x.c", "class Built {}");

    let (index, report, sources, _) =
        load_index_with_texts(&request(root.clone()), &Progress::new());

    assert_eq!(report.files, 1, "warnings: {:?}", report.warnings);
    let id = a.to_string_lossy().into_owned();
    let (key, _) = index.file(&id).expect("a.c indexed");
    assert_eq!(key.origin, Origin::Workspace);
    assert_eq!(key.layer, Some(Layer::World));
    assert!(sources.contains_key(&id));
}

#[test]
fn script_index_dependency_pbo() {
    let root = tmp("dep-pbo");
    let pbo = root.join("addons/dep.pbo");
    pack(
        &pbo,
        &[("prefix", "JM\\CF\\Scripts")],
        &[("3_Game\\cf.c", b"class CF_X {}")],
    );

    let workspace = tmp("dep-pbo-ws");
    let mut req = request(workspace);
    req.dependencies = vec![root.clone()];
    let (index, report, sources, _) = load_index_with_texts(&req, &Progress::new());

    assert_eq!(report.files, 1, "warnings: {:?}", report.warnings);
    let id = format!("pbo:{}!3_Game/cf.c", pbo.to_string_lossy());
    let (key, _) = index.file(&id).expect("cf.c indexed");
    assert_eq!(key.origin, Origin::Dependency(0));
    assert_eq!(key.layer, Some(Layer::Game));
    let source = sources.get(&id).expect("source recorded");
    assert_eq!(source.display_path(), "JM/CF/Scripts/3_Game/cf.c");
}

#[test]
fn script_index_vanilla_and_workspace_chain() {
    let game = tmp("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[(
            "4_World\\playerbase.c",
            b"class PlayerBase extends ManBase {}",
        )],
    );
    let workspace = tmp("chain-ws");
    write(
        &workspace,
        "Scripts/4_World/a.c",
        "modded class PlayerBase { void Mine(); }",
    );

    let req = IndexRequest {
        workspace,
        dependencies: Vec::new(),
        game: Some(game),
    };
    let (index, report, _, _) = load_index_with_texts(&req, &Progress::new());
    assert_eq!(report.files, 2, "warnings: {:?}", report.warnings);

    let chain = index.class_chain("PlayerBase");
    assert_eq!(chain.len(), 2, "chain length");
    assert_eq!(chain[0].key.origin, Origin::Vanilla);
    assert_eq!(chain[1].key.origin, Origin::Workspace);
    assert!(chain[1].class.modded);
}

#[test]
fn script_index_vanilla_map_scripts() {
    let game = tmp("game-map");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[(
            "4_World\\playerbase.c",
            b"class PlayerBase extends ManBase {}",
        )],
    );
    let map_pbo = game.join("sakhal/Addons/data_sakhal.pbo");
    pack(
        &map_pbo,
        &[("prefix", "DZ\\data_sakhal")],
        &[
            (
                "scripts\\4_World\\RockBase_Sakhal.c",
                b"class RockBase_Sakhal extends RockBase {}",
            ),
            ("tools\\exportFn.c", b"class ExportFn {}"),
        ],
    );
    write(&game, "sakhal/Addons/data_sakhal.pbo.dayz.bisign", "sig");

    let req = IndexRequest {
        workspace: tmp("map-ws"),
        dependencies: Vec::new(),
        game: Some(game.clone()),
    };
    let (index, report, _, _) = load_index_with_texts(&req, &Progress::new());

    let id = format!(
        "pbo:{}!scripts/4_World/RockBase_Sakhal.c",
        map_pbo.display()
    );
    let (key, _) = index
        .file(&id)
        .unwrap_or_else(|| panic!("Sakhal script indexed; warnings: {:?}", report.warnings));
    assert_eq!(key.origin, Origin::Vanilla);
    assert_eq!(key.layer, Some(Layer::World));
    assert!(!index.type_exists("ExportFn"), "export script indexed");

    let vanilla: Vec<_> = report
        .sources
        .iter()
        .filter(|s| s.origin == "vanilla")
        .collect();
    assert_eq!(vanilla.len(), 2, "vanilla sources: {vanilla:?}");
    assert_eq!(vanilla[0].path, game.join("dta/scripts.pbo"));
    assert_eq!(vanilla[0].files, 1);
    assert_eq!(vanilla[1].path, map_pbo);
    assert_eq!(vanilla[1].files, 1);
    assert_eq!(vanilla[1].name.as_deref(), Some("sakhal"));
}

#[test]
fn script_index_missing_sources_warn() {
    let workspace = tmp("missing-ws");
    write(&workspace, "Scripts/4_World/a.c", "class A {}");

    let req = IndexRequest {
        workspace,
        dependencies: vec![tmp("no-dep")],
        game: Some(tmp("no-game-dir")),
    };
    let progress = Progress::new();
    let (index, report, _, _) = load_index_with_texts(&req, &progress);

    assert_eq!(report.files, 1);
    assert!(
        report.warnings.len() >= 2,
        "warnings: {:?}",
        report.warnings
    );
    assert_eq!(index.stats().files, 1);
    assert_eq!(progress.fraction(), 1.0);
}

#[test]
fn script_index_source_id_and_display_path() {
    let file = ScriptSource::File(PathBuf::from("/tmp/mf/a.c"));
    assert_eq!(file.id(), "/tmp/mf/a.c");

    let pbo = ScriptSource::PboEntry {
        pbo: PathBuf::from("/tmp/mf/dep.pbo"),
        prefix: "JM\\CF\\Scripts".into(),
        entry: "3_Game/cf.c".into(),
    };
    assert_eq!(pbo.id(), "pbo:/tmp/mf/dep.pbo!3_Game/cf.c");
    assert_eq!(pbo.display_path(), "JM/CF/Scripts/3_Game/cf.c");
}

#[test]
fn script_index_corpus_vanilla_fast() {
    let Ok(game) = std::env::var("MF_DAYZ_INSTALL") else {
        return;
    };
    let game = PathBuf::from(game);
    let workspace = tmp("corpus-ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let req = IndexRequest {
        workspace,
        dependencies: Vec::new(),
        game: Some(game.clone()),
    };
    let (index, report, _, _) = load_index_with_texts(&req, &Progress::new());
    eprintln!(
        "vanilla corpus: {} files in {} ms",
        index.stats().files,
        report.millis
    );
    assert!(report.files >= 2800, "indexed only {} files", report.files);
    assert!(report.millis < 5000, "took {} ms", report.millis);

    let sakhal = game.join("sakhal/Addons/data_sakhal.pbo");
    if sakhal.is_file() {
        let row = report
            .sources
            .iter()
            .find(|source| source.path == sakhal)
            .unwrap_or_else(|| panic!("no source row for {}", sakhal.display()));
        assert_eq!(row.origin, "vanilla");
        assert!(row.files >= 4, "{row:?}");
    }
}

#[test]
fn script_index_dependency_display_names() {
    let root = tmp("dep-name-ws");
    write(&root, "Scripts/4_World/a.c", "class A {}");

    let named = tmp("dep-named");
    write(
        &named,
        "meta.cpp",
        "protocol = 1;\r\npublishedid = 1559212036;\r\nname = \"CF\";\r\n",
    );
    write(&named, "mod.cpp", "name = \"Community Framework\";\r\n");
    write(&named, "Scripts/3_Game/cf.c", "class CF {}");

    let meta_only = tmp("dep-meta-only");
    write(
        &meta_only,
        "meta.cpp",
        "protocol = 1;\r\nname = \"Expansion\";\r\n",
    );
    write(&meta_only, "Scripts/3_Game/exp.c", "class Exp {}");

    let unnamed = tmp("dep-unnamed");
    write(&unnamed, "Scripts/3_Game/x.c", "class X {}");

    let req = IndexRequest {
        workspace: root,
        dependencies: vec![named, meta_only, unnamed],
        game: Some(tmp("no-game")),
    };
    let (_index, report, _, _) = load_index_with_texts(&req, &Progress::new());

    let find = |origin: &str| {
        report
            .sources
            .iter()
            .find(|s| s.origin == origin)
            .unwrap_or_else(|| panic!("missing {origin}; sources: {:?}", report.sources))
    };
    assert_eq!(
        find("dependency:0").name.as_deref(),
        Some("Community Framework")
    );
    assert_eq!(find("dependency:1").name.as_deref(), Some("Expansion"));
    assert_eq!(find("dependency:2").name, None);

    let json = serde_json::to_value(find("dependency:2")).unwrap();
    assert!(
        json.get("name").is_some(),
        "name key always present: {json}"
    );
    assert!(json["name"].is_null());
}

#[test]
fn script_index_corpus_workshop_display_names() {
    let Ok(workshop) = std::env::var("MF_WORKSHOP_PATH") else {
        return;
    };
    let root = tmp("workshop-name-ws");
    std::fs::create_dir_all(&root).unwrap();

    let mut mods: Vec<PathBuf> = std::fs::read_dir(&workshop)
        .expect("workshop dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    mods.sort();

    let req = IndexRequest {
        workspace: root,
        dependencies: mods.clone(),
        game: Some(tmp("no-game")),
    };
    let (_index, report, _, _) = load_index_with_texts(&req, &Progress::new());

    let named = report
        .sources
        .iter()
        .filter(|s| s.origin.starts_with("dependency:"))
        .filter(|s| s.name.is_some())
        .count();
    eprintln!("workshop corpus: {named}/{} mods have a name", mods.len());
    assert_eq!(
        named,
        mods.len(),
        "unnamed: {:?}",
        report
            .sources
            .iter()
            .filter(|s| s.name.is_none())
            .map(|s| &s.path)
            .collect::<Vec<_>>()
    );
}
