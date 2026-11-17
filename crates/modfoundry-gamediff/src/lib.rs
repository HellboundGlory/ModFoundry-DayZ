//! What changed between two DayZ game builds: script classes, config classes and layouts.

pub mod config;
pub mod hits;
pub mod hits_data;
pub mod hits_script;
pub mod layout;
pub mod script;
pub mod script_diff;
pub mod snapshot;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// One named value present in either build; `None` on the side where it is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueChange {
    pub name: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The first 16 hex characters of the bytes' SHA-256.
pub fn short_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..16].to_owned()
}

/// The non-empty parts joined with `separator`.
pub fn join_non_empty(parts: &[&str], separator: &str) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(separator)
}

/// The entries of `map` whose key `other` lacks, in key order.
pub(crate) fn only_in<'a, K: Ord, V, W>(
    map: &'a BTreeMap<K, V>,
    other: &'a BTreeMap<K, W>,
) -> impl Iterator<Item = (&'a K, &'a V)> + 'a {
    map.iter().filter(move |(key, _)| !other.contains_key(*key))
}

pub(crate) fn optional_change(
    name: &str,
    old: Option<String>,
    new: Option<String>,
) -> Option<ValueChange> {
    (old != new).then(|| ValueChange {
        name: name.to_owned(),
        old,
        new,
    })
}

/// One change per key whose rendered value differs or exists on one side only, in key order.
pub fn map_changes<V>(
    old: &BTreeMap<String, V>,
    new: &BTreeMap<String, V>,
    name: impl Fn(&String, &V) -> String,
    render: impl Fn(&V) -> String,
) -> Vec<ValueChange> {
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    keys.into_iter()
        .filter_map(|key| {
            let before = old.get(key);
            let after = new.get(key);
            let old_text = before.map(&render);
            let new_text = after.map(&render);
            if old_text == new_text {
                return None;
            }
            let named = after.or(before)?;
            Some(ValueChange {
                name: name(key, named),
                old: old_text,
                new: new_text,
            })
        })
        .collect()
}
