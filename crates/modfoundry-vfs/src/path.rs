//! Virtual path model: normalization, case semantics, legacy forms.

use serde::{Deserialize, Serialize};

/// A normalized virtual path: `root:`-prefixed, `/`-separated, no `.`/`..`,
/// case-preserving. Resolution treats paths case-insensitively against case-folded
/// indexes (DayZ engine semantics); the stored casing is the author's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VirtualPath {
    /// Canonical string form, e.g. `game:DZ/weapons/akm/akm.p3d`.
    canonical: String,
}

impl VirtualPath {
    /// Build from `root:rel/path`. Accepts `\` and `/` separators, collapses
    /// duplicate separators, skips empty and `.` components, rejects `..`.
    pub fn new(root: &str, rel: &str) -> Result<Self, String> {
        if root.is_empty()
            || !root
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!("invalid root `{root}`"));
        }
        let cleaned = Self::clean(rel)?;
        Ok(Self {
            canonical: format!("{root}:{cleaned}"),
        })
    }

    /// Parse `root:path` or a bare relative path (treated as `virtual:` root).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.replace('\\', "/");
        let (root, rest) = match s.split_once(':') {
            Some((r, p)) => (r.to_string(), p.to_string()),
            None => ("virtual".to_string(), s),
        };
        Self::new(&root, &rest)
    }

    /// Normalize a relative path: `\`→`/`, collapse `//`, reject `.`/`..`/empty.
    pub fn clean(rel: &str) -> Result<String, String> {
        let norm = rel.replace('\\', "/");
        if norm.is_empty() {
            return Ok(String::new());
        }
        let mut out = String::new();
        for comp in norm.split('/') {
            match comp {
                "" | "." => continue,
                ".." => return Err(format!("parent traversal not allowed: {rel}")),
                c => {
                    if !out.is_empty() {
                        out.push('/');
                    }
                    out.push_str(c);
                }
            }
        }
        Ok(out)
    }

    /// Root name (`game`, `mod:myslug` if the root itself is namespaced…). Returns
    /// the segment before the first `:`.
    pub fn root(&self) -> &str {
        self.canonical.split_once(':').map(|(r, _)| r).unwrap_or("")
    }

    /// Relative part after the root (`""` for the root itself).
    pub fn rel(&self) -> &str {
        self.canonical.split_once(':').map(|(_, p)| p).unwrap_or("")
    }

    /// Last path component (file/dir name).
    pub fn file_name(&self) -> Option<&str> {
        self.rel().rsplit('/').next().filter(|s| !s.is_empty())
    }

    /// Engine-style rendering for adapters that must emit Windows paths:
    /// `P:\prefix\rest` or `prefix\file` forms are produced by dedicated helpers in
    /// the engine adapter, not here; this returns the plain canonical form.
    pub fn as_str(&self) -> &str {
        &self.canonical
    }
}

impl std::fmt::Display for VirtualPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_backslashes_and_duplicates() {
        let p = VirtualPath::parse("game:DZ\\weapons//akm\\\\akm.p3d").unwrap();
        assert_eq!(p.as_str(), "game:DZ/weapons/akm/akm.p3d");
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(VirtualPath::parse("mod:foo/../../etc").is_err());
        assert!(VirtualPath::parse("game:..").is_err());
    }

    #[test]
    fn file_name_of_root_is_none() {
        let p = VirtualPath::parse("game:").unwrap();
        assert_eq!(p.file_name(), None);
        assert_eq!(p.rel(), "");
    }
}
