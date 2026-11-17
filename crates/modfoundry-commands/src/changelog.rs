//! The embedded `CHANGELOG.md`: what changed in each release, optionally only the newest sections.

use serde::Serialize;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// A version as `major.minor.patch` numbers; `v` in front is allowed.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let [major, minor, patch] = <[&str; 3]>::try_from(text.split('.').collect::<Vec<_>>()).ok()?;
    let number = |part: &str| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    Some((number(major)?, number(minor)?, number(patch)?))
}

/// The sections of `text` newer than `since`, or all of it when `since` is `None`.
pub fn sections_since(text: &str, since: Option<(u64, u64, u64)>) -> Changelog {
    let headings = headings(text);
    let Some(since) = since else {
        return Changelog {
            since: None,
            versions: headings.into_iter().map(|(_, token)| token).collect(),
            text: text.to_owned(),
        };
    };
    let mut versions = Vec::new();
    let mut kept = String::new();
    for (index, (start, token)) in headings.iter().enumerate() {
        let newer = parse_version(token).is_none_or(|version| version > since);
        if !newer {
            continue;
        }
        let end = headings
            .get(index + 1)
            .map_or(text.len(), |(next, _)| *next);
        kept.push_str(&text[*start..end]);
        versions.push(token.clone());
    }
    Changelog {
        since: None,
        versions,
        text: kept,
    }
}

/// The embedded changelog.
pub fn changelog(since: Option<&str>) -> Result<Changelog, String> {
    let Some(raw) = since else {
        return Ok(sections_since(CHANGELOG, None));
    };
    let version =
        parse_version(raw).ok_or_else(|| "`since` must be a version like 1.8.0".to_owned())?;
    let mut answer = sections_since(CHANGELOG, Some(version));
    answer.since = Some(raw.to_owned());
    Ok(answer)
}

/// Each `## ` heading's offset and its version token, in file order.
fn headings(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(token) = heading_token(line) {
            found.push((offset, token));
        }
        offset += line.len();
    }
    found
}

/// The first word after a `## ` heading's hashes; `None` for any other line.
fn heading_token(line: &str) -> Option<String> {
    let rest = line.strip_prefix("## ")?;
    Some(rest.split_whitespace().next()?.to_owned())
}

/// The embedded changelog, or the sections newer than `since`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Changelog {
    pub since: Option<String>,
    pub versions: Vec<String>,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nA preamble line.\n\n## 2.0.0 - unreleased\n\n### Added\n\n- a thing\n\n## 1.10.0 - 2026-01-01\n\n### Fixed\n\n- a fix\n\n## 1.9.0 - 2025-01-01\n\n### Changed\n\n- a change\n\n## 1.8.0 - 2024-01-01\n\n### Removed\n\n- a removal\n";

    fn version(text: &str) -> (u64, u64, u64) {
        parse_version(text).unwrap()
    }

    #[test]
    fn sections_since_keeps_only_newer_releases() {
        let cases = [
            ("1.9.0", vec!["2.0.0", "1.10.0"]),
            ("1.10.0", vec!["2.0.0"]),
            ("99.0.0", vec![]),
        ];
        for (from, expected) in cases {
            let answer = sections_since(SAMPLE, Some(version(from)));

            assert_eq!(answer.versions, expected, "since {from}");
            if expected.is_empty() {
                assert_eq!(answer.text, "", "since {from}");
                continue;
            }
            assert!(answer.text.starts_with("## 2.0.0"), "since {from}");
            assert!(!answer.text.contains("A preamble"), "since {from}");
            assert!(answer.text.contains("### Added\n\n- a thing"), "{from}");
            if expected.len() > 1 {
                assert!(answer.text.contains("### Fixed\n\n- a fix"), "{from}");
            } else {
                assert!(!answer.text.contains("## 1.10.0"), "since {from}");
            }
        }

        let all = sections_since(SAMPLE, None);
        assert_eq!(all.text, SAMPLE);
        assert_eq!(all.versions, ["2.0.0", "1.10.0", "1.9.0", "1.8.0"]);
    }

    #[test]
    fn a_heading_without_a_version_is_always_included() {
        let text = "## Unreleased\n\n- new\n\n## 1.0.0\n\n- old\n";

        let answer = sections_since(text, Some((5, 0, 0)));

        assert_eq!(answer.versions, ["Unreleased"]);
        assert!(answer.text.starts_with("## Unreleased"), "{}", answer.text);
        assert!(!answer.text.contains("## 1.0.0"), "{}", answer.text);
    }

    #[test]
    fn a_third_level_heading_does_not_start_a_section() {
        let text = "## 2.0.0\n\n### Added\n\n- x\n";

        let answer = sections_since(text, Some((1, 0, 0)));

        assert_eq!(answer.versions, ["2.0.0"]);
        assert!(answer.text.contains("### Added\n\n- x"), "{}", answer.text);
    }

    #[test]
    fn parse_version_accepts_three_numbers_only() {
        let accepted = [
            ("1.8.0", (1, 8, 0)),
            ("v1.8.0", (1, 8, 0)),
            ("10.20.30", (10, 20, 30)),
        ];
        for (text, expected) in accepted {
            assert_eq!(parse_version(text), Some(expected), "{text}");
        }
        for text in ["1.8", "1.8.0.1", "1.8.0-rc1", "a.b.c", "", "1..0"] {
            assert_eq!(parse_version(text), None, "{text}");
        }
    }

    #[test]
    fn the_shipped_changelog_is_well_formed() {
        let answer = changelog(None).unwrap();

        assert!(answer.text.starts_with("# Changelog"), "{answer:?}");
        let mut versions = Vec::new();
        for (_, token) in headings(&answer.text) {
            match parse_version(&token) {
                Some(version) => versions.push(version),
                None => assert_eq!(token, "Unreleased", "{token}"),
            }
        }
        assert!(!versions.is_empty(), "no release sections");
        assert!(
            versions.windows(2).all(|pair| pair[0] > pair[1]),
            "{versions:?}"
        );
    }
}
