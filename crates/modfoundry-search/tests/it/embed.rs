use std::path::Path;

use modfoundry_search::embed::{query_text, BgeSmall, Embed, FakeEmbed};

#[test]
fn a_query_gets_the_retrieval_prefix() {
    assert_eq!(
        query_text("how do I hide a toast?"),
        "Represent this sentence for searching relevant passages: how do I hide a toast?"
    );
}

#[test]
fn fake_vectors_are_unit_length_and_repeat_for_the_same_text() {
    let fake = FakeEmbed { dims: 384 };
    let vectors = fake
        .embed(&["hide the toast", "hide the toast", "apple pie"])
        .unwrap();
    for vector in &vectors {
        assert_eq!(vector.len(), 384);
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "{norm}");
    }
    assert_eq!(vectors[0], vectors[1]);
}

#[test]
fn texts_sharing_words_score_higher_than_unrelated_ones() {
    let fake = FakeEmbed { dims: 384 };
    let vectors = fake
        .embed(&[
            "how do I hide a toast",
            "class ToastManager { void Hide(); }",
            "apple pie recipe",
        ])
        .unwrap();
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    assert!(dot(&vectors[0], &vectors[1]) > dot(&vectors[0], &vectors[2]));
}

#[test]
#[ignore = "set MF_BGE_MODEL_DIR to a bge-small-en-v1.5 folder"]
fn bge_small_scores_the_matching_passage_highest() {
    let Some(dir) = std::env::var_os("MF_BGE_MODEL_DIR") else {
        return;
    };
    let model = BgeSmall::load(Path::new(&dir)).unwrap();
    let vectors = model
        .embed(&[
            "How do I hide a toast?",
            "class ToastManager { void Hide(); }",
            "apple pie recipe",
        ])
        .unwrap();
    assert_eq!(model.dims(), 384);
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    for vector in &vectors {
        assert_eq!(vector.len(), 384);
        let norm = dot(vector, vector).sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "{norm}");
    }
    assert!(dot(&vectors[0], &vectors[1]) > dot(&vectors[0], &vectors[2]));

    let long = "toast ".repeat(600);
    let longer = "toast ".repeat(700);
    let alone = &model.embed(&["How do I hide a toast?"]).unwrap()[0];
    let padded = model
        .embed(&["How do I hide a toast?", long.as_str(), longer.as_str()])
        .unwrap();
    assert!(
        dot(alone, &padded[0]) > 0.9999,
        "padding changed the vector"
    );
    assert!(
        dot(&padded[1], &padded[2]) > 0.9999,
        "text past 512 tokens changed the vector"
    );
}

#[test]
fn an_empty_text_still_gets_a_unit_vector() {
    let vector = &FakeEmbed { dims: 8 }.embed(&[""]).unwrap()[0];
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5, "{norm}");
}
