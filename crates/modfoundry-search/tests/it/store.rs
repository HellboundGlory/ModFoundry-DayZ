use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::scratch::Scratch;
use modfoundry_search::chunk::{Chunk, Kind};
use modfoundry_search::store::{Index, IndexWriter, Meta, StoreError, StoredHit};

fn index_path(scratch: &Scratch) -> PathBuf {
    scratch.0.join("build").join("index.sqlite")
}

fn meta() -> Meta {
    Meta {
        model: "BAAI/bge-small-en-v1.5".into(),
        revision: "5c38ec7".into(),
        build: "1.30.164289".into(),
        created: "2026-11-07T10:00:00Z".into(),
    }
}

fn chunk(kind: Kind, class: Option<&str>, method: Option<&str>, lines: [usize; 2]) -> Chunk {
    Chunk {
        kind,
        lines,
        class: class.map(String::from),
        method: method.map(String::from),
        text: format!("text of {lines:?}"),
    }
}

pub(crate) fn part_files(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
                .filter(|name| name.ends_with(".part"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn build(scratch: &Scratch) -> Index {
    let mut writer = IndexWriter::create(&index_path(scratch), &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", true).unwrap();
    let file = writer
        .add_file(source, "scripts/3_game/player.c", 120, "9f2a")
        .unwrap();
    writer
        .add_chunks(
            file,
            &[
                (
                    chunk(Kind::Script, Some("PlayerBase"), Some("OnInit"), [10, 20]),
                    "PlayerBase OnInit".into(),
                    vec![1.0, 0.0, 0.0, 0.0],
                ),
                (
                    chunk(Kind::Config, None, None, [1, 5]),
                    "CfgVehicles".into(),
                    vec![0.0, 1.0, 0.0, 0.0],
                ),
                (
                    chunk(Kind::Docs, None, None, [3, 9]),
                    "Getting started".into(),
                    vec![0.0, 0.0, 1.0, 0.0],
                ),
            ],
        )
        .unwrap();
    writer.finish().unwrap();
    Index::open(&index_path(scratch)).unwrap()
}

#[test]
fn round_trip_reads_back_meta_and_hits() {
    let scratch = Scratch::new("round-trip");
    let index = build(&scratch);
    assert_eq!(index.meta(), &meta());
    assert_eq!(index.len(), 3);
    assert!(!index.is_empty());
    assert_eq!(
        index.hit(1).unwrap(),
        StoredHit {
            kind: Kind::Script,
            class: Some("PlayerBase".into()),
            method: Some("OnInit".into()),
            lines: [10, 20],
            snippet: "PlayerBase OnInit".into(),
            text: "text of [10, 20]".into(),
            path: "scripts/3_game/player.c".into(),
            size: 120,
            hash: "9f2a".into(),
            source: "dta".into(),
            root: "/game/dta".into(),
            reference_only: true,
        }
    );
    let config = index.hit(2).unwrap();
    assert_eq!(config.kind, Kind::Config);
    assert_eq!((config.class, config.method), (None, None));
    assert_eq!(config.lines, [1, 5]);
    assert_eq!(index.hit(3).unwrap().kind, Kind::Docs);
}

#[test]
fn nearest_ranks_by_dot_product_then_lower_id() {
    let scratch = Scratch::new("nearest");
    let index = build(&scratch);
    assert_eq!(
        index.nearest(&[0.5, 0.25, 0.0, 0.0], 2).unwrap(),
        vec![(1, 0.5), (2, 0.25)]
    );
    assert_eq!(
        index.nearest(&[1.0, 0.0, 0.0, 0.0], 3).unwrap(),
        vec![(1, 1.0), (2, 0.0), (3, 0.0)]
    );
}

#[test]
fn nearest_refuses_a_query_of_another_length() {
    let scratch = Scratch::new("nearest-dims");
    let index = build(&scratch);
    let err = index
        .nearest(&[1.0, 0.0, 0.0], 2)
        .expect_err("a short query is refused");
    assert!(
        matches!(
            err,
            StoreError::Dims {
                expected: 4,
                found: 3
            }
        ),
        "{err}"
    );
    assert!(index.nearest(&[1.0, 0.0, 0.0, 0.0, 0.0], 2).is_err());
}

#[test]
fn two_writers_for_one_path_both_finish_and_the_later_one_wins() {
    let scratch = Scratch::new("two-writers");
    let path = index_path(&scratch);
    let mut first = IndexWriter::create(&path, &meta()).unwrap();
    let source = first.add_source("dta", "/game/dta", false).unwrap();
    let file = first.add_file(source, "a.c", 1, "h").unwrap();
    first
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 1]),
                "first".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();

    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::a_second_process_writes_the_same_path",
            "--test-threads=1",
        ])
        .env("MF_TEST_INDEX_PATH", &path)
        .output()
        .unwrap();
    let output = String::from_utf8_lossy(&child.stdout);
    assert!(
        child.status.success(),
        "the second process failed: {output}"
    );
    assert!(output.contains("1 passed"), "{output}");
    assert!(path.exists(), "the second process wrote no index");

    first.finish().unwrap();
    let index = Index::open(&path).unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(index.hit(1).unwrap().snippet, "first");
}

#[test]
fn a_second_process_writes_the_same_path() {
    let Some(path) = std::env::var_os("MF_TEST_INDEX_PATH") else {
        return;
    };
    let mut writer = IndexWriter::create(Path::new(&path), &meta()).unwrap();
    let source = writer.add_source("mods", "/mods", false).unwrap();
    let file = writer.add_file(source, "b.c", 1, "h").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Layout, None, None, [2, 2]),
                "second".into(),
                vec![0.0, 1.0],
            )],
        )
        .unwrap();
    writer.finish().unwrap();
    assert_eq!(Index::open(Path::new(&path)).unwrap().len(), 1);
}

#[test]
fn dropped_writer_leaves_no_part_file_behind() {
    let scratch = Scratch::new("dropped-part");
    let path = index_path(&scratch);
    let mut writer = IndexWriter::create(&path, &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 1]),
                "a".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();
    drop(writer);

    assert_eq!(part_files(path.parent().unwrap()), Vec::<String>::new());
    assert!(!path.exists());
}

#[test]
fn finish_that_cannot_replace_the_index_leaves_no_part_file_behind() {
    let scratch = Scratch::new("failed-finish");
    let path = index_path(&scratch);
    let mut writer = IndexWriter::create(&path, &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 1]),
                "a".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("kept"), b"x").unwrap();

    assert!(writer.finish().is_err());
    assert_eq!(part_files(path.parent().unwrap()), Vec::<String>::new());
    assert!(path.join("kept").exists());
}

#[test]
fn a_vector_with_a_non_finite_value_is_refused_and_nothing_from_its_batch_is_kept() {
    let scratch = Scratch::new("non-finite");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 2]),
                "first".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();
    let err = writer
        .add_chunks(
            file,
            &[
                (
                    chunk(Kind::Script, None, None, [3, 4]),
                    "second".into(),
                    vec![0.0, 1.0],
                ),
                (
                    chunk(Kind::Script, None, None, [5, 6]),
                    "nan".into(),
                    vec![f32::NAN, 1.0],
                ),
            ],
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::NonFinite), "{err}");
    assert_eq!(
        err.to_string(),
        "vector holds a value that is not a finite number"
    );
    writer.finish().unwrap();
    assert_eq!(Index::open(&index_path(&scratch)).unwrap().len(), 1);
}

#[test]
fn a_vector_with_an_infinity_is_refused() {
    let scratch = Scratch::new("infinite");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    for value in [f32::INFINITY, f32::NEG_INFINITY] {
        let err = writer
            .add_chunks(
                file,
                &[(
                    chunk(Kind::Script, None, None, [1, 1]),
                    "a".into(),
                    vec![1.0, value],
                )],
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::NonFinite), "{err}");
    }
}

#[test]
fn dropped_writer_leaves_the_earlier_index_in_place() {
    let scratch = Scratch::new("dropped-writer");
    build(&scratch);

    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    writer.add_source("mods", "/mods", false).unwrap();
    drop(writer);
    let index = Index::open(&index_path(&scratch)).unwrap();
    assert_eq!(index.len(), 3);
    assert_eq!(index.meta(), &meta());

    let fresh = scratch.0.join("fresh.sqlite");
    let writer = IndexWriter::create(&fresh, &meta()).unwrap();
    drop(writer);
    assert!(!fresh.exists());
}

#[test]
fn vector_of_another_length_is_refused_and_nothing_from_its_batch_is_kept() {
    let scratch = Scratch::new("dims");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 2]),
                "first".into(),
                vec![1.0, 0.0, 0.0, 0.0],
            )],
        )
        .unwrap();
    let err = writer
        .add_chunks(
            file,
            &[
                (
                    chunk(Kind::Script, None, None, [3, 4]),
                    "second".into(),
                    vec![0.0, 1.0, 0.0, 0.0],
                ),
                (
                    chunk(Kind::Script, None, None, [5, 6]),
                    "short".into(),
                    vec![0.0, 1.0, 0.0],
                ),
            ],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::Dims {
            expected: 4,
            found: 3
        }
    ));
    writer.finish().unwrap();
    assert_eq!(Index::open(&index_path(&scratch)).unwrap().len(), 1);
}

#[test]
fn index_of_another_schema_is_refused_on_open() {
    let scratch = Scratch::new("schema");
    build(&scratch);
    rusqlite::Connection::open(index_path(&scratch))
        .unwrap()
        .execute("UPDATE meta SET value = '2' WHERE key = 'schema'", [])
        .unwrap();
    match Index::open(&index_path(&scratch))
        .err()
        .expect("schema 2 is refused")
    {
        StoreError::Schema { found } => assert_eq!(found, "2"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_index_missing_a_meta_row_is_refused_on_open() {
    let cases = ["schema", "model", "revision", "build", "created"];
    for key in cases {
        let scratch = Scratch::new(&format!("missing-{key}"));
        build(&scratch);
        rusqlite::Connection::open(index_path(&scratch))
            .unwrap()
            .execute("DELETE FROM meta WHERE key = ?1", [key])
            .unwrap();
        let err = Index::open(&index_path(&scratch))
            .err()
            .unwrap_or_else(|| panic!("a missing {key} row is refused"));
        assert!(
            matches!(&err, StoreError::Meta { key: missing } if missing == key),
            "{key}: {err}"
        );
        assert_eq!(
            err.to_string(),
            format!("index has no {key} entry; rebuild it")
        );
    }
}

#[test]
fn texts_lists_every_chunk_in_id_order() {
    let scratch = Scratch::new("texts");
    let index = build(&scratch);
    assert_eq!(
        index.texts().unwrap(),
        vec![
            (1, "text of [10, 20]".to_string()),
            (2, "text of [1, 5]".to_string()),
            (3, "text of [3, 9]".to_string()),
        ]
    );
}

#[test]
fn a_batch_with_mixed_vector_lengths_is_refused() {
    let scratch = Scratch::new("batch-dims");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "h").unwrap();
    let err = writer
        .add_chunks(
            file,
            &[
                (
                    chunk(Kind::Script, None, None, [1, 2]),
                    "long".into(),
                    vec![1.0, 0.0, 0.0, 0.0],
                ),
                (
                    chunk(Kind::Script, None, None, [3, 4]),
                    "short".into(),
                    vec![0.0, 1.0, 0.0],
                ),
            ],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::Dims {
            expected: 4,
            found: 3
        }
    ));
}

#[test]
fn finish_replaces_an_older_index() {
    let scratch = Scratch::new("replace");
    build(&scratch);

    let newer = Meta {
        build: "1.31.0".into(),
        ..meta()
    };
    let mut writer = IndexWriter::create(&index_path(&scratch), &newer).unwrap();
    let source = writer.add_source("dta", "/game/dta", false).unwrap();
    let file = writer.add_file(source, "a.c", 1, "00").unwrap();
    writer
        .add_chunks(
            file,
            &[(
                chunk(Kind::Layout, None, None, [1, 1]),
                "a".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();
    writer.finish().unwrap();

    let index = Index::open(&index_path(&scratch)).unwrap();
    assert_eq!(index.meta().build, "1.31.0");
    assert_eq!((index.len(), index.dims()), (1, 2));
    let hit = index.hit(1).unwrap();
    assert_eq!((hit.kind, hit.reference_only), (Kind::Layout, false));
}

#[test]
fn a_stored_vector_of_another_length_is_refused_on_open() {
    let scratch = Scratch::new("bad-blob");
    build(&scratch);
    for blob in ["X'0000803F'", "X'0000803F000000000000000000'"] {
        rusqlite::Connection::open(index_path(&scratch))
            .unwrap()
            .execute(
                &format!("UPDATE chunks SET vector = {blob} WHERE id = 2"),
                [],
            )
            .unwrap();
        assert!(
            matches!(
                Index::open(&index_path(&scratch)),
                Err(StoreError::Dims { .. })
            ),
            "blob {blob} is refused"
        );
    }
}

#[test]
fn chunks_for_an_unknown_file_are_refused() {
    let scratch = Scratch::new("orphan");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let chunks = [(chunk(Kind::Docs, None, None, [1, 1]), "a".into(), vec![1.0])];
    assert!(writer.add_chunks(99, &chunks).is_err());
}

#[test]
fn ids_where_filters_by_kind_and_source_label() {
    let scratch = Scratch::new("ids-where");
    let mut writer = IndexWriter::create(&index_path(&scratch), &meta()).unwrap();
    let dta = writer.add_source("dta", "/game/dta", false).unwrap();
    let mods = writer.add_source("Mods", "/mods", true).unwrap();
    let a = writer.add_file(dta, "a.c", 1, "h1").unwrap();
    let b = writer.add_file(mods, "b.c", 1, "h2").unwrap();
    let vector = || vec![1.0, 0.0, 0.0, 0.0];
    writer
        .add_chunks(
            a,
            &[
                (
                    chunk(Kind::Script, None, None, [1, 1]),
                    "a".into(),
                    vector(),
                ),
                (
                    chunk(Kind::Config, None, None, [2, 2]),
                    "b".into(),
                    vector(),
                ),
            ],
        )
        .unwrap();
    writer
        .add_chunks(
            b,
            &[
                (chunk(Kind::Docs, None, None, [1, 1]), "c".into(), vector()),
                (
                    chunk(Kind::Script, None, None, [2, 2]),
                    "d".into(),
                    vector(),
                ),
            ],
        )
        .unwrap();
    writer.finish().unwrap();
    let index = Index::open(&index_path(&scratch)).unwrap();

    let cases: [(&[Kind], &[&str], &[i64]); 4] = [
        (&[Kind::Script], &[], &[1, 4]),
        (&[], &["mods"], &[3, 4]),
        (&[Kind::Script], &["MODS"], &[4]),
        (&[], &[], &[1, 2, 3, 4]),
    ];
    for (kinds, sources, expected) in cases {
        let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
        let ids = index.ids_where(kinds, &sources).unwrap();
        let expected: HashSet<i64> = expected.iter().copied().collect();
        assert_eq!(ids, expected, "kinds {kinds:?} sources {sources:?}");
    }
}

#[test]
fn two_writers_in_one_process_keep_their_own_part_files() {
    let scratch = Scratch::new("two-writers-one-process");
    let path = index_path(&scratch);
    let folder = path.parent().unwrap();

    let mut first = IndexWriter::create(&path, &meta()).unwrap();
    let source = first.add_source("dta", "/game/dta", false).unwrap();
    let file = first.add_file(source, "a.c", 1, "h").unwrap();
    first
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [1, 1]),
                "first".into(),
                vec![1.0, 0.0],
            )],
        )
        .unwrap();

    let mut second = IndexWriter::create(&path, &meta()).unwrap();
    let source = second.add_source("dta", "/game/dta", false).unwrap();
    let file = second.add_file(source, "b.c", 1, "h").unwrap();
    second
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [2, 2]),
                "second".into(),
                vec![0.0, 1.0],
            )],
        )
        .unwrap();

    let parts = part_files(folder);
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert_ne!(parts[0], parts[1]);

    first.finish().unwrap();
    second.finish().unwrap();
    let index = Index::open(&path).unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(index.hit(1).unwrap().snippet, "second");
    assert_eq!(part_files(folder), Vec::<String>::new());
}

#[test]
fn a_dropped_writer_leaves_the_other_writers_part_file() {
    let scratch = Scratch::new("one-of-two-dropped");
    let path = index_path(&scratch);
    let first = IndexWriter::create(&path, &meta()).unwrap();
    let mut second = IndexWriter::create(&path, &meta()).unwrap();
    drop(first);

    assert_eq!(part_files(path.parent().unwrap()).len(), 1);

    let source = second.add_source("dta", "/game/dta", false).unwrap();
    let file = second.add_file(source, "b.c", 1, "h").unwrap();
    second
        .add_chunks(
            file,
            &[(
                chunk(Kind::Script, None, None, [2, 2]),
                "second".into(),
                vec![0.0, 1.0],
            )],
        )
        .unwrap();
    second.finish().unwrap();
    assert_eq!(Index::open(&path).unwrap().len(), 1);
}
