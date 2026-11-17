//! Search query over a built index: ranking, filters, staleness and errors.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{config_bin, core, pack, tmp, write};
use modfoundry_commands::search_index::{build_index, SearchIndexRequest};
use modfoundry_commands::search_query::{query_index, SearchQuery, SearchQueryReport};
use modfoundry_core::{Actor, CommandError};
use modfoundry_search::chunk::Kind;
use modfoundry_search::embed::FakeEmbed;
use modfoundry_search::model::{MODEL_ID, REVISION};
use modfoundry_search::store::{IndexWriter, Meta};
use serde_json::json;

const ALPHA: &str = "class Alpha\n{\n\tvoid ZorbifyWidget()\n\t{\n\t}\n};\n";
const BETA: &str = "class Beta\n{\n\tvoid QuuxHandler()\n\t{\n\t}\n};\n";
const HELPER: &str = "class Helper\n{\n\tvoid FrobnicateDepot()\n\t{\n\t}\n};\n";

/// A fake game with two script PBOs, a config PBO, a loose dependency and a docs folder; returns its root and index file.
fn fixture(label: &str) -> (PathBuf, PathBuf) {
    let root = tmp(label);
    let game = root.join("game");
    pack(
        &game.join("dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[
            ("3_Game\\Alpha.c", ALPHA.as_bytes()),
            ("4_World\\Beta.c", BETA.as_bytes()),
        ],
    );
    let rifle = config_bin("class CfgVehicles { class Rifle { displayName = \"Rifle\"; }; };");
    pack(
        &game.join("addons/config.pbo"),
        &[],
        &[("x\\config.bin", rifle.as_slice())],
    );
    let dep = root.join("@Dep");
    write(&dep, "Mod/Helper.c", HELPER);
    write(&root, "docs/README.md", "# Guide\n\nHow the mod loads.\n");
    let req = SearchIndexRequest {
        game,
        dependencies: vec![dep],
        docs: vec![root.join("docs")],
        reference_only: Vec::new(),
    };
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &root.join("cache"),
        &mut |_, _| {},
    )
    .unwrap();
    (root, report.index)
}

fn question(text: &str) -> SearchQuery {
    SearchQuery {
        question: text.into(),
        kinds: Vec::new(),
        sources: Vec::new(),
        top: 8,
    }
}

fn answer(index: &Path, query: &SearchQuery) -> SearchQueryReport {
    let report = query_index(index, &FakeEmbed { dims: 8 }, query).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    report
}

#[test]
fn a_rare_identifier_ranks_its_method_first() {
    let (_root, index) = fixture("query-rare");
    let report = answer(&index, &question("ZorbifyWidget"));
    let top = &report.hits[0];
    assert_eq!(top.path, "scripts/3_Game/Alpha.c");
    assert_eq!(top.lines, [3, 5]);
    assert_eq!(top.kind, Kind::Script);
    assert_eq!(top.class.as_deref(), Some("Alpha"));
    assert_eq!(top.method.as_deref(), Some("ZorbifyWidget"));
    assert_eq!((top.source.as_str(), top.reference_only), ("game", false));
    assert_eq!(top.build, report.build);
    assert!(!top.stale);
}

#[test]
fn top_limits_the_number_of_hits() {
    let (_root, index) = fixture("query-top");
    let mut q = question("ZorbifyWidget");
    q.top = 2;
    assert_eq!(answer(&index, &q).hits.len(), 2);
}

#[test]
fn kind_and_source_filters_keep_only_their_hits() {
    let (_root, index) = fixture("query-filters");
    let mut docs = question("guide loads");
    docs.kinds = vec![Kind::Docs];
    let hits = answer(&index, &docs).hits;
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| hit.kind == Kind::Docs));

    let mut dependency = question("FrobnicateDepot");
    dependency.sources = vec!["@dep".into()];
    let hits = answer(&index, &dependency).hits;
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| hit.source == "@Dep"));
}

#[test]
fn a_loose_file_changed_or_removed_since_indexing_is_stale() {
    let (root, index) = fixture("query-stale");
    let helper = root.join("@Dep/Mod/Helper.c");
    let q = question("FrobnicateDepot");
    let hit = answer(&index, &q).hits.remove(0);
    assert_eq!(hit.path, "Mod/Helper.c");
    assert!(!hit.stale);

    fs::write(&helper, format!("{HELPER}// edited\n")).unwrap();
    assert!(answer(&index, &q).hits[0].stale);

    fs::remove_file(&helper).unwrap();
    assert!(answer(&index, &q).hits[0].stale);
}

#[test]
fn a_pbo_entry_changed_since_indexing_is_stale() {
    let (root, index) = fixture("query-pbo-stale");
    let q = question("ZorbifyWidget");
    assert!(!answer(&index, &q).hits[0].stale);

    let edited = "class Alpha\n{\n\tvoid ZorbifyWidget()\n\t{\n\t\tint edited;\n\t}\n};\n";
    pack(
        &root.join("game/dta/scripts.pbo"),
        &[("prefix", "scripts")],
        &[
            ("3_Game\\Alpha.c", edited.as_bytes()),
            ("4_World\\Beta.c", BETA.as_bytes()),
        ],
    );
    assert!(answer(&index, &q).hits[0].stale);
}

#[test]
fn a_query_vector_of_another_length_names_both_lengths() {
    let (_root, index) = fixture("query-dims");
    let err = query_index(&index, &FakeEmbed { dims: 4 }, &question("ZorbifyWidget")).unwrap_err();
    assert_eq!(err, "query vector has 4 values, the index holds 8");
}

#[test]
fn an_index_built_with_another_model_asks_for_a_rebuild() {
    let path = tmp("query-model").join("index.sqlite");
    let meta = Meta {
        model: "other/model".into(),
        revision: "abc123".into(),
        build: "1.30.164289.70".into(),
        created: "0".into(),
    };
    IndexWriter::create(&path, &meta).unwrap().finish().unwrap();
    let err = query_index(&path, &FakeEmbed { dims: 8 }, &question("ZorbifyWidget")).unwrap_err();
    assert!(
        err.contains("other/model") && err.contains(MODEL_ID) && err.contains("mf search index"),
        "{err}"
    );
}

#[test]
fn search_query_command_refuses_a_missing_question_and_an_unknown_kind() {
    let core = core();
    let missing = core
        .dispatch(
            Actor::CliSession,
            "search.query",
            json!({ "game": "/no/such/game" }),
        )
        .unwrap_err();
    assert!(
        matches!(missing, CommandError::InvalidArguments(_)),
        "{missing}"
    );

    let kind = core
        .dispatch(
            Actor::CliSession,
            "search.query",
            json!({ "question": "how", "game": "/no/such/game", "kind": ["widget"] }),
        )
        .unwrap_err();
    assert!(
        matches!(&kind, CommandError::InvalidArguments(msg) if msg.contains("widget")),
        "{kind}"
    );
}

#[test]
fn an_index_with_no_pieces_asks_for_a_rebuild() {
    let path = tmp("query-empty").join("index.sqlite");
    let meta = Meta {
        model: MODEL_ID.into(),
        revision: REVISION.into(),
        build: "1.30.164289.70".into(),
        created: "0".into(),
    };
    IndexWriter::create(&path, &meta).unwrap().finish().unwrap();
    let err = query_index(&path, &FakeEmbed { dims: 8 }, &question("ZorbifyWidget")).unwrap_err();
    assert_eq!(
        err,
        "the index holds no pieces; rebuild with `mf search index`"
    );
}

#[test]
fn a_piece_is_fresh_when_its_pbo_twin_still_matches() {
    let root = tmp("query-twins");
    let dep = root.join("@Twins");
    let first = b"class First { void Alpha() {} }";
    let second = b"class Second { void Omega() {} }";
    pack(&dep.join("addons/a.pbo"), &[], &[("x.c", first.as_slice())]);
    pack(
        &dep.join("addons/b.pbo"),
        &[],
        &[("x.c", second.as_slice())],
    );
    let req = SearchIndexRequest {
        game: root.join("game"),
        dependencies: vec![dep],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    fs::create_dir_all(root.join("game")).unwrap();
    let report = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &root.join("cache"),
        &mut |_, _| {},
    )
    .unwrap();
    let hit = answer(&report.index, &question("Omega")).hits.remove(0);
    assert_eq!(hit.path, "x.c");
    assert!(!hit.stale);
}

#[test]
fn a_docs_hit_stays_a_loose_file_when_its_folder_gains_an_addons_pbo() {
    let (root, index) = fixture("query-docs-addons");
    pack(
        &root.join("docs/addons/extra.pbo"),
        &[],
        &[("a.c", b"class A {}")],
    );
    let mut q = question("guide loads");
    q.kinds = vec![Kind::Docs];
    let hit = answer(&index, &q).hits.remove(0);
    assert_eq!(hit.path, "README.md");
    assert!(!hit.stale);
}

#[test]
fn a_hit_in_a_folder_with_a_prefix_marker_is_fresh_until_its_file_changes() {
    let root = tmp("query-marker-stale");
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    let dep = root.join("@Dep");
    write(&dep, "a/$PBOPREFIX$", "My\\Mod");
    write(&dep, "a/Scripts/Helper.c", HELPER);
    let req = SearchIndexRequest {
        game,
        dependencies: vec![dep.clone()],
        docs: Vec::new(),
        reference_only: Vec::new(),
    };
    let index = build_index(
        &req,
        &FakeEmbed { dims: 8 },
        &root.join("cache"),
        &mut |_, _| {},
    )
    .unwrap()
    .index;
    let q = question("FrobnicateDepot");
    let hit = answer(&index, &q).hits.remove(0);
    assert_eq!(hit.path, "My/Mod/Scripts/Helper.c");
    assert!(!hit.stale);

    fs::write(
        dep.join("a/Scripts/Helper.c"),
        format!("{HELPER}// edited\n"),
    )
    .unwrap();
    assert!(answer(&index, &q).hits[0].stale);
}
