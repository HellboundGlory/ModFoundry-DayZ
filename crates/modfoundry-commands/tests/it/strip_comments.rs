//! Real-install check: stripping every vanilla script keeps every token and line.

use std::path::PathBuf;

use modfoundry_build::strip_comments;
use modfoundry_enforce::{parse_tree, Tree};
use modfoundry_pbo::read_pbo_file;

#[test]
fn corpus_strip_comments_keeps_every_token_and_line() {
    let Ok(install) = std::env::var("MF_DAYZ_INSTALL") else {
        return;
    };
    let (header, payloads) = read_pbo_file(&PathBuf::from(install).join("dta").join("scripts.pbo"))
        .expect("read scripts.pbo");
    let mut files = 0usize;
    let mut removed = 0usize;
    for (entry, data) in header.entries.iter().zip(&payloads) {
        let name = entry.name_str();
        if !name.to_ascii_lowercase().ends_with(".c") {
            continue;
        }
        files += 1;
        let stripped = strip_comments(data)
            .unwrap_or_else(|err| panic!("{name}: unterminated comment on line {}", err.line));
        assert_eq!(
            line_breaks(&stripped),
            line_breaks(data),
            "{name}: line breaks changed"
        );
        assert_eq!(
            strip_comments(&stripped).expect("the output has no comment"),
            stripped,
            "{name}: stripping twice changed the bytes"
        );
        removed += data.len() - stripped.len();

        let before_text = String::from_utf8_lossy(data);
        let before: Vec<String> = leaves(&parse_tree(&before_text), &before_text)
            .iter()
            .map(|(kind, text)| token_text(kind, text))
            .collect();
        let after_text = String::from_utf8_lossy(&stripped);
        let after_tree = parse_tree(&after_text);
        assert!(!has_comment(&after_tree), "{name}: a comment survived");
        let after_leaves = leaves(&after_tree, &after_text);
        for (kind, text) in &after_leaves {
            assert!(
                !(kind == "preproc_const" && text.contains("//")),
                "{name}: a preproc_const still holds a comment: {text:?}"
            );
        }
        let after: Vec<String> = after_leaves
            .iter()
            .map(|(kind, text)| token_text(kind, text))
            .collect();
        assert_eq!(after, before, "{name}: tokens changed");
    }
    eprintln!("{files} files, {removed} bytes removed");
    assert!(files > 0, "no .c entries in scripts.pbo");
}

fn line_breaks(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

/// Every comment node the grammar has, including the documented `/*!` and `//!` forms.
const COMMENT_KINDS: &[&str] = &["comment_line", "comment_block", "doc_line", "doc_block"];

/// Leaves are compared by text, not kind: the grammar folds a trailing `//` into `preproc_const`,
/// and a comment before an `if` can change how its `else` is parsed (identifier or keyword).
fn token_text(kind: &str, text: &str) -> String {
    if kind != "preproc_const" {
        return text.to_owned();
    }
    text.find("//")
        .map_or(text, |at| &text[..at])
        .trim_end()
        .to_owned()
}

/// The leaf nodes other than comments, in source order, as (kind, text).
fn leaves(tree: &Tree, text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    if cursor.node().child_count() == 0 {
        return out;
    }
    loop {
        if cursor.goto_first_child() {
            continue;
        }
        let node = cursor.node();
        if !COMMENT_KINDS.contains(&node.kind()) {
            out.push((node.kind().to_owned(), text[node.byte_range()].to_owned()));
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return out;
            }
        }
    }
}

fn has_comment(tree: &Tree) -> bool {
    let mut cursor = tree.walk();
    loop {
        if COMMENT_KINDS.contains(&cursor.node().kind()) {
            return true;
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}
