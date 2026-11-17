//! Keyword ranking (BM25) and the merge of several rankings into one.

use std::collections::{HashMap, HashSet};

/// Splits text into lowercased keyword tokens.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let pieces = word_pieces(word);
        out.extend(
            pieces
                .iter()
                .map(|p| p.to_lowercase())
                .filter(|t| t.chars().count() >= 2),
        );
        if pieces.len() > 1 {
            out.push(word.to_lowercase());
        }
    }
    out
}

fn word_pieces(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut starts = vec![0];
    for i in 1..chars.len() {
        let next = chars.get(i + 1).map(|&(_, c)| c);
        if starts_piece(chars[i - 1].1, chars[i].1, next) {
            starts.push(chars[i].0);
        }
    }
    let ends = starts
        .iter()
        .skip(1)
        .copied()
        .chain(std::iter::once(word.len()));
    starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| &word[start..end])
        .collect()
}

fn starts_piece(prev: char, cur: char, next: Option<char>) -> bool {
    let case_change = prev.is_lowercase() && cur.is_uppercase();
    let acronym_end =
        prev.is_uppercase() && cur.is_uppercase() && next.is_some_and(char::is_lowercase);
    let digit_edge = prev.is_numeric() != cur.is_numeric();
    case_change || acronym_end || digit_edge
}

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// A BM25 index over a fixed set of documents.
pub struct Bm25 {
    postings: HashMap<String, Vec<(u32, u32)>>,
    lengths: Vec<u32>,
    avg_len: f32,
}

impl Bm25 {
    /// Tokenises each document once and records what scoring needs.
    pub fn new(docs: &[&str]) -> Self {
        let mut postings: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        let mut lengths = Vec::with_capacity(docs.len());
        for (doc, text) in docs.iter().enumerate() {
            let doc_tokens = tokens(text);
            lengths.push(doc_tokens.len() as u32);
            for token in doc_tokens {
                let list = postings.entry(token).or_default();
                match list.last_mut() {
                    Some((last, freq)) if *last == doc as u32 => *freq += 1,
                    _ => list.push((doc as u32, 1)),
                }
            }
        }
        let total: u64 = lengths.iter().map(|&l| u64::from(l)).sum();
        let avg_len = total as f32 / docs.len().max(1) as f32;
        Self {
            postings,
            lengths,
            avg_len,
        }
    }

    /// The best documents for a query as (index, score), best first, ties by lower index.
    pub fn scores(&self, query: &str, top: usize) -> Vec<(usize, f32)> {
        let mut terms = tokens(query);
        terms.sort_unstable();
        terms.dedup();
        let n = self.lengths.len() as f32;
        let mut totals: HashMap<usize, f32> = HashMap::new();
        for list in terms.iter().filter_map(|term| self.postings.get(term)) {
            let df = list.len() as f32;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            for &(doc, freq) in list {
                *totals.entry(doc as usize).or_default() += idf * self.saturated(doc, freq);
            }
        }
        let mut ranked: Vec<(usize, f32)> = totals.into_iter().collect();
        sort_best_first(&mut ranked);
        ranked.truncate(top);
        ranked
    }

    fn saturated(&self, doc: u32, freq: u32) -> f32 {
        let norm = K1 * (1.0 - B + B * self.lengths[doc as usize] as f32 / self.avg_len);
        let f = freq as f32;
        f * (K1 + 1.0) / (f + norm)
    }
}

/// Reciprocal rank fusion of best-first rankings: the `top` best (index, score), ties by lower index.
pub fn fuse(rankings: &[&[usize]], k: f32, top: usize) -> Vec<(usize, f32)> {
    // Summed in f64 so equal sets of ranks tie whatever order the rankings come in.
    let mut fused: HashMap<usize, f64> = HashMap::new();
    let mut counted: HashSet<usize> = HashSet::new();
    for ranking in rankings {
        counted.clear();
        for (position, &doc) in ranking.iter().enumerate() {
            if counted.insert(doc) {
                *fused.entry(doc).or_default() += 1.0 / (f64::from(k) + position as f64 + 1.0);
            }
        }
    }
    let mut ranked: Vec<(usize, f32)> = fused.into_iter().map(|(doc, s)| (doc, s as f32)).collect();
    sort_best_first(&mut ranked);
    ranked.truncate(top);
    ranked
}

fn sort_best_first(ranked: &mut [(usize, f32)]) {
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
}
