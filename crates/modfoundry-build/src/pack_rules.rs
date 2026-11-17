use crate::build::has_extension;
use serde::Serialize;
use std::path::Path;

/// Extensions the engine loads as game content, compared without case.
pub const GAME_EXTENSIONS: &[&str] = &[
    "c", "h", "hpp", "cpp", "inc", "ext", "cfg", "bin", "layout", "imageset", "styles", "edds",
    "paa", "png", "jpg", "jpeg", "tga", "p3d", "rvmat", "ogg", "wss", "wav", "csv", "xml", "json",
    "ptc", "fnt", "pac", "anm", "emat", "rtm", "bisurf", "lip", "bikb", "wrp", "agr", "ast", "asi",
    "aw", "atxt",
];

/// Root-level folders that never hold packed content, compared without case.
pub const SKIPPED_FOLDERS: &[&str] = &["docs", "tests", "tools"];

/// Why a file is packed or left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Reason {
    GameFile,
    NotAGameFile,
    SkippedFolder,
    Rule { line: usize, pattern: String },
    Converted { line: usize, to: String },
    Binarized,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::GameFile => f.write_str("game file"),
            Reason::NotAGameFile => f.write_str("not a game file"),
            Reason::SkippedFolder => f.write_str("skipped folder"),
            Reason::Rule { line, pattern } => write!(f, ".pboignore line {line}: {pattern}"),
            Reason::Converted { line, to } => {
                let name = Path::new(to).file_name().and_then(|name| name.to_str());
                write!(
                    f,
                    "packed as {} (.pboimages line {line})",
                    name.unwrap_or(to)
                )
            }
            Reason::Binarized => f.write_str("packed as config.bin (--binarize)"),
        }
    }
}

/// Whether a file is packed, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub included: bool,
    pub reason: Reason,
}

/// The packing rules of one source tree, in file order.
#[derive(Debug, Default)]
pub struct PackRules {
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Rule {
    line: usize,
    include: bool,
    pattern: String,
}

/// Reads `.pboignore` text: blank lines and `#` comments are skipped, `!` re-includes.
pub fn parse(text: &str) -> PackRules {
    let rules = text
        .trim_start_matches('\u{feff}')
        .lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let line = raw.trim_end();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (include, pattern) = match line.strip_prefix('!') {
                Some(pattern) => (true, pattern),
                None => (false, line),
            };
            Some(Rule {
                line: index + 1,
                include,
                // Rules written with the engine's `\` separators match too.
                pattern: pattern.replace('\\', "/"),
            })
        })
        .collect();
    PackRules { rules }
}

/// Reads `<source_root>/.pboignore`; a tree without one keeps the defaults.
pub fn load(source_root: &Path) -> anyhow::Result<PackRules> {
    match std::fs::read_to_string(source_root.join(".pboignore")) {
        Ok(text) => Ok(parse(&text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PackRules::default()),
        Err(err) => Err(anyhow::anyhow!(
            "cannot read {}: {err}",
            source_root.join(".pboignore").display()
        )),
    }
}

/// The `.pboignore` text after ticking or unticking `rel`; refuses a name no rule can hold.
pub fn set_packed(text: &str, rel: &str, folder: bool, packed: bool) -> anyhow::Result<String> {
    let rel = rel.replace('\\', "/");
    if rel.contains(['*', '?']) {
        anyhow::bail!(
            "* and ? are wildcards in .pboignore, so this name cannot be written as a rule"
        );
    }
    if rel.contains(['\r', '\n']) {
        anyhow::bail!("a name with a line break cannot be written as a .pboignore rule");
    }
    if !folder && rel.ends_with(char::is_whitespace) {
        anyhow::bail!(
            "a name ending in a space or other whitespace would be trimmed when .pboignore is read"
        );
    }
    let rule = if folder {
        format!("/{rel}/")
    } else {
        format!("/{rel}")
    };
    let lowered = rule.to_ascii_lowercase();
    let mut kept: String = text
        .trim_start_matches('\u{feff}')
        .split_inclusive('\n')
        .filter(|line| {
            let body = line.trim_end().replace('\\', "/").to_ascii_lowercase();
            let body = body.strip_prefix('!').unwrap_or(&body);
            let inside_folder = folder && body.starts_with(&lowered) && !body.contains(['*', '?']);
            body != lowered && !inside_folder
        })
        .collect();
    if !folder && parse(&kept).decide(&rel).included == packed {
        return Ok(kept);
    }
    let newline = match text.find('\n') {
        Some(at) if at > 0 && text.as_bytes()[at - 1] == b'\r' => "\r\n",
        _ => "\n",
    };
    if !kept.is_empty() && !kept.ends_with('\n') {
        kept.push_str(newline);
    }
    if packed {
        kept.push('!');
    }
    kept.push_str(&rule);
    kept.push_str(newline);
    Ok(kept)
}

impl PackRules {
    /// Decides whether `rel`, a `/`-separated path under the source root, is packed.
    pub fn decide(&self, rel: &str) -> Decision {
        match self
            .rules
            .iter()
            .rev()
            .find(|rule| pattern_matches(&rule.pattern, rel))
        {
            Some(rule) => Decision {
                included: rule.include,
                reason: Reason::Rule {
                    line: rule.line,
                    pattern: rule.pattern.clone(),
                },
            },
            None => default_decision(rel),
        }
    }
}

/// Whether `pattern` matches `rel` or a folder above it. A pattern with a `/` is
/// anchored at the source root; one without matches a single path segment.
fn pattern_matches(pattern: &str, rel: &str) -> bool {
    let (body, folders_only) = match pattern.strip_suffix('/') {
        Some(folder) => (folder, true),
        None => (pattern, false),
    };
    let anchored = body.contains('/');
    let body = body.trim_start_matches('/');
    let segments: Vec<&str> = rel.split('/').collect();
    let depth = if folders_only {
        segments.len() - 1
    } else {
        segments.len()
    };
    (1..=depth).any(|end| {
        if anchored {
            glob_match(body, &segments[..end].join("/"))
        } else {
            glob_match(body, segments[end - 1])
        }
    })
}

/// Matches `/`-separated segments; a `**` segment stands for any number of them.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let text: Vec<&str> = text.split('/').collect();
    segments_match(&pattern, &text)
}

fn segments_match(pattern: &[&str], text: &[&str]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((&"**", [])) => !text.is_empty(),
        Some((&"**", rest)) => (0..=text.len()).any(|skip| segments_match(rest, &text[skip..])),
        Some((first, rest)) => text.split_first().is_some_and(|(segment, tail)| {
            chars_match(first, segment) && segments_match(rest, tail)
        }),
    }
}

/// Matches one segment: `*` is any run of characters, letters compare without case.
fn chars_match(pattern: &str, text: &str) -> bool {
    let mut pattern_chars = pattern.chars();
    match pattern_chars.next() {
        None => text.is_empty(),
        Some('*') => {
            let rest = pattern_chars.as_str();
            (0..=text.len())
                .filter(|&at| text.is_char_boundary(at))
                .any(|at| chars_match(rest, &text[at..]))
        }
        Some(wanted) => text.chars().next().is_some_and(|got| {
            (wanted == '?' || wanted.eq_ignore_ascii_case(&got))
                && chars_match(pattern_chars.as_str(), &text[got.len_utf8()..])
        }),
    }
}

fn default_decision(rel: &str) -> Decision {
    let folder = rel.split_once('/').map(|(folder, _)| folder);
    if folder.is_some_and(|folder| {
        SKIPPED_FOLDERS
            .iter()
            .any(|skipped| skipped.eq_ignore_ascii_case(folder))
    }) {
        return Decision {
            included: false,
            reason: Reason::SkippedFolder,
        };
    }
    if has_extension(rel, GAME_EXTENSIONS) {
        Decision {
            included: true,
            reason: Reason::GameFile,
        }
    } else {
        Decision {
            included: false,
            reason: Reason::NotAGameFile,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_follow_the_defaults_then_the_last_matching_rule() {
        let rules = parse(
            "# comment\n*.txt\n!stringtable.txt\n/data/raw/\nScripts/**/Debug*.c\n!tests/selftest/config.cpp\n",
        );
        let rule = |line, pattern: &str| Reason::Rule {
            line,
            pattern: pattern.into(),
        };
        let cases = [
            ("config.cpp", true, Reason::GameFile),
            ("README.md", false, Reason::NotAGameFile),
            ("docs/a.c", false, Reason::SkippedFolder),
            (
                "tests/selftest/config.cpp",
                true,
                rule(6, "tests/selftest/config.cpp"),
            ),
            ("notes.txt", false, rule(2, "*.txt")),
            ("Sub/StringTable.txt", true, rule(3, "stringtable.txt")),
            ("data/raw/x.paa", false, rule(4, "/data/raw/")),
            ("x/data/raw/y.paa", true, Reason::GameFile),
            (
                "Scripts/3_Game/sub/DebugTools.c",
                false,
                rule(5, "Scripts/**/Debug*.c"),
            ),
        ];
        for (rel, included, reason) in cases {
            assert_eq!(rules.decide(rel), Decision { included, reason }, "{rel}");
        }
    }

    #[test]
    fn reasons_display_the_labels_the_cli_prints() {
        let cases = [
            (Reason::GameFile, "game file"),
            (Reason::NotAGameFile, "not a game file"),
            (Reason::SkippedFolder, "skipped folder"),
            (
                Reason::Rule {
                    line: 3,
                    pattern: "/data/a.xml".into(),
                },
                ".pboignore line 3: /data/a.xml",
            ),
            (
                Reason::Converted {
                    line: 1,
                    to: "gui/images/frame.edds".into(),
                },
                "packed as frame.edds (.pboimages line 1)",
            ),
            (Reason::Binarized, "packed as config.bin (--binarize)"),
        ];
        for (reason, label) in cases {
            assert_eq!(reason.to_string(), label);
        }
    }

    #[test]
    fn set_packed_edits_the_text_so_the_tick_holds() {
        let cases = [
            ("", "notes.txt", false, true, "!/notes.txt\n"),
            ("", "data/a.xml", false, false, "/data/a.xml\n"),
            ("", "data/a.xml", false, true, ""),
            (
                "# keep\n/data/a.xml\n",
                "data/a.xml",
                false,
                true,
                "# keep\n",
            ),
            ("*.txt", "readme.txt", false, true, "*.txt\n!/readme.txt\n"),
            (
                "/data/a.xml\n!/data/b.txt\n*.bak\n",
                "data",
                true,
                false,
                "*.bak\n/data/\n",
            ),
            ("", "tests", true, true, "!/tests/\n"),
            ("/Data/A.xml\n", "data/a.xml", false, true, ""),
            (
                "\u{feff}/notes.txt\n",
                "notes.txt",
                false,
                true,
                "!/notes.txt\n",
            ),
            ("!/data/b.c\n", "data\\b.c", false, false, "/data/b.c\n"),
        ];
        for (text, rel, folder, packed, expected) in cases {
            let result = set_packed(text, rel, folder, packed).unwrap();
            assert_eq!(result, expected, "{text:?} {rel} {folder} {packed}");
            let rel = rel.replace('\\', "/");
            let probe = if folder { format!("{rel}/file.c") } else { rel };
            assert_eq!(parse(&result).decide(&probe).included, packed, "{probe}");
        }
    }

    #[test]
    fn set_packed_refuses_names_a_rule_cannot_hold() {
        let cases = [
            ("data/a*.xml", false),
            ("data/a?.xml", false),
            ("data/sub*", true),
            ("notes.txt ", false),
            ("notes.txt\t", false),
            ("a\nb.txt", false),
            ("a\r.txt", false),
        ];
        for (rel, folder) in cases {
            for packed in [true, false] {
                assert!(set_packed("", rel, folder, packed).is_err(), "{rel:?}");
            }
        }
    }

    #[test]
    fn a_trailing_tab_is_named_as_whitespace_in_the_refusal() {
        let err = set_packed("", "notes.txt\t", false, true).unwrap_err();

        assert!(err.to_string().contains("whitespace"), "{err}");
    }

    #[test]
    fn a_folder_name_ending_in_a_space_is_still_written() {
        let text = set_packed("", "data ", true, false).unwrap();

        assert_eq!(text, "/data /\n");
        assert!(!parse(&text).decide("data /x.c").included);
    }

    #[test]
    fn set_packed_keeps_crlf_line_endings() {
        assert_eq!(
            set_packed("*.bak\r\n", "notes.txt", false, true).unwrap(),
            "*.bak\r\n!/notes.txt\r\n"
        );
        assert_eq!(
            set_packed("*.bak", "notes.txt", false, true).unwrap(),
            "*.bak\n!/notes.txt\n"
        );
        assert_eq!(
            set_packed("*.bak\r\n/data/", "notes.txt", false, true).unwrap(),
            "*.bak\r\n/data/\r\n!/notes.txt\r\n"
        );
        assert_eq!(
            set_packed("\u{feff}*.bak\r\n", "notes.txt", false, true).unwrap(),
            "*.bak\r\n!/notes.txt\r\n"
        );
    }

    #[test]
    fn patterns_match_wildcards_folders_and_anchors() {
        let cases = [
            ("*.c", "dir/a.c", true),
            ("a*", "dir/abc", true),
            ("a?c", "dir/abc", true),
            ("a?c", "dir/ac", false),
            ("Debug*.c", "x/DebugTools.c", true),
            ("**/x.c", "a/b/x.c", true),
            ("**/x.c", "x.c", true),
            ("a/**/c", "a/b/d/c", true),
            ("a/**/c", "a/c", true),
            ("data/**", "data/a/b.c", true),
            ("data/**", "data", false),
            ("build/", "build/x.c", true),
            ("build/", "x/build/y.c", true),
            ("build/", "build", false),
            ("/a.c", "a.c", true),
            ("/a.c", "x/a.c", false),
            ("x/a.c", "y/x/a.c", false),
            ("a.c", "x/a.c", true),
            ("STRING.TXT", "string.txt", true),
            ("Scripts/3_game/*.c", "scripts/3_Game/a.c", true),
        ];
        for (pattern, rel, expected) in cases {
            assert_eq!(
                pattern_matches(pattern, rel),
                expected,
                "{pattern} vs {rel}"
            );
        }
    }

    #[test]
    fn a_windows_style_rule_after_a_byte_order_mark_still_matches() {
        let rules = parse("\u{feff}Scripts\\3_Game\\skip.c\r\n");
        assert!(!rules.decide("Scripts/3_Game/skip.c").included);
    }

    #[test]
    fn terrain_and_animation_files_are_game_files() {
        let rules = parse("");
        for file in ["world.wrp", "a.agr", "a.ast", "a.asi", "a.aw", "a.atxt"] {
            assert!(rules.decide(file).included, "{file}");
        }
    }
}
