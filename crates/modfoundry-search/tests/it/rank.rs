use modfoundry_search::rank::{fuse, tokens, Bm25};

const DOCS: [&str; 4] = [
    "class Toast { void Show() {} }",
    "override void OnScheduledTick() {}",
    "toast toast toast toast",
    "void Tick(float dt) { time += dt; }",
];

#[test]
fn tokens_split_identifiers_and_keep_the_whole_name() {
    let cases: &[(&str, &[&str])] = &[
        (
            "OnScheduledTick",
            &["on", "scheduled", "tick", "onscheduledtick"],
        ),
        ("HTTPServer2", &["http", "server", "httpserver2"]),
        (
            "m_ModifiersManager",
            &["modifiers", "manager", "modifiersmanager"],
        ),
        ("hide toast", &["hide", "toast"]),
        ("a_b", &[]),
        ("parseHTTP", &["parse", "http", "parsehttp"]),
        ("HTTP", &["http"]),
        ("Vec3Add", &["vec", "add", "vec3add"]),
        ("naïveBayes", &["naïve", "bayes", "naïvebayes"]),
        ("aB", &["ab"]),
        ("__init__", &["init"]),
        ("", &[]),
    ];
    for (input, expected) in cases {
        assert_eq!(tokens(input), *expected, "tokens of {input:?}");
    }
}

#[test]
fn bm25_ranks_the_matching_identifier_first_and_leaves_out_the_rest() {
    let index = Bm25::new(&DOCS);
    let found: Vec<usize> = index
        .scores("scheduled tick", 10)
        .iter()
        .map(|&(doc, _)| doc)
        .collect();
    assert_eq!(found, [1, 3]);
}

#[test]
fn bm25_returns_at_most_top_documents() {
    let index = Bm25::new(&DOCS);
    let found: Vec<usize> = index
        .scores("scheduled tick", 1)
        .iter()
        .map(|&(doc, _)| doc)
        .collect();
    assert_eq!(found, [1]);
}

#[test]
fn bm25_scores_follow_the_formula() {
    let index = Bm25::new(&["beta", "beta gamma delta epsilon"]);
    let ranked = index.scores("beta", 10);
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].0, 0);
    assert!((ranked[0].1 - 0.241_631).abs() < 1e-6, "{}", ranked[0].1);
    assert!((ranked[1].1 - 0.146_390).abs() < 1e-6, "{}", ranked[1].1);
}

#[test]
fn bm25_breaks_equal_scores_by_lower_index() {
    let index = Bm25::new(&["tick", "tick"]);
    let found: Vec<usize> = index
        .scores("tick", 10)
        .iter()
        .map(|&(doc, _)| doc)
        .collect();
    assert_eq!(found, [0, 1]);
}

#[test]
fn fuse_prefers_a_document_ranked_well_in_both_lists() {
    let fused = fuse(&[&[0, 1], &[2, 1]], 60.0, 10);
    let expected = [(1, 2.0 / 62.0), (0, 1.0 / 61.0), (2, 1.0 / 61.0)];
    assert_eq!(fused.len(), expected.len());
    for (&(doc, score), (expected_doc, expected_score)) in fused.iter().zip(expected) {
        assert_eq!(doc, expected_doc);
        assert!(
            (score - expected_score).abs() < 1e-6,
            "score {score} for doc {doc}"
        );
    }
}

#[test]
fn fuse_sums_reciprocal_ranks_and_breaks_ties_by_index() {
    let fused = fuse(&[&[0, 1], &[1, 0]], 60.0, 10);
    let both = 1.0 / 61.0 + 1.0 / 62.0;
    assert_eq!(fused.len(), 2);
    for ((doc, score), expected_doc) in fused.iter().zip([0, 1]) {
        assert_eq!(*doc, expected_doc);
        assert!((score - both).abs() < 1e-6, "score {score} for doc {doc}");
    }
}

#[test]
fn fuse_counts_a_chunk_once_per_ranking_at_its_best_position() {
    let repeated = fuse(&[&[3, 3, 5]], 60.0, 10);
    let once = fuse(&[&[3, 5]], 60.0, 10);
    assert_eq!(repeated[0], once[0]);
    assert_eq!(repeated.len(), 2);
}

#[test]
fn fuse_ties_documents_with_the_same_ranks_in_any_order() {
    let fused = fuse(&[&[0, 1, 2], &[1, 2, 0], &[2, 0, 1]], 60.0, 10);
    let docs: Vec<usize> = fused.iter().map(|&(doc, _)| doc).collect();
    assert_eq!(docs, [0, 1, 2]);
    assert_eq!(fused[0].1, fused[2].1);
}
