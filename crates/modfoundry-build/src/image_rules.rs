//! `.pboimages`: folders whose PNG files a build converts to `.edds` or `.paa`.

use crate::build::has_extension;
use anyhow::bail;
use modfoundry_texture::convert::Target;
use std::path::Path;

/// The conversion rules of one source tree, in file order.
#[derive(Debug, Default)]
pub struct ImageRules {
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Rule {
    line: usize,
    folder: Vec<String>,
    target: Target,
}

/// Reads `.pboimages` text: one `<folder> <edds|paa>` rule per line.
pub fn parse(text: &str) -> anyhow::Result<ImageRules> {
    let mut rules = Vec::new();
    for (index, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = index + 1;
        let fields: Vec<&str> = raw.split_whitespace().collect();
        if fields.first().is_none_or(|first| first.starts_with('#')) {
            continue;
        }
        let &[folder, format] = fields.as_slice() else {
            bail!(".pboimages line {line}: expected a folder and a format, like `gui/images edds`");
        };
        let target = match format.to_ascii_lowercase().as_str() {
            "edds" => Target::Edds,
            "paa" => Target::Paa(None),
            _ => bail!(".pboimages line {line}: unknown format \"{format}\"; use edds or paa"),
        };
        let folder: Vec<String> = folder
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        if folder
            .iter()
            .any(|segment| segment == "." || segment == ".." || segment.contains(['*', '?']))
        {
            bail!(".pboimages line {line}: write the folder from the mod root without `.`, `..` or wildcards");
        }
        rules.push(Rule {
            line,
            folder,
            target,
        });
    }
    Ok(ImageRules { rules })
}

/// Reads `<source_root>/.pboimages`; a tree without one has no rules.
pub fn load(source_root: &Path) -> anyhow::Result<ImageRules> {
    match std::fs::read_to_string(source_root.join(".pboimages")) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ImageRules::default()),
        Err(err) => Err(anyhow::anyhow!(
            "cannot read {}: {err}",
            source_root.join(".pboimages").display()
        )),
    }
}

impl ImageRules {
    /// The rule line and target that convert `rel`, a `/`-separated path under the source root.
    pub fn target(&self, rel: &str) -> Option<(usize, Target)> {
        if !has_extension(rel, &["png"]) {
            return None;
        }
        let segments: Vec<&str> = rel.split('/').collect();
        let folders = &segments[..segments.len() - 1];
        self.rules
            .iter()
            .filter(|rule| {
                folders.len() >= rule.folder.len()
                    && rule
                        .folder
                        .iter()
                        .zip(folders)
                        .all(|(want, got)| want.eq_ignore_ascii_case(got))
            })
            .max_by_key(|rule| (rule.folder.len(), rule.line))
            .map(|rule| (rule.line, rule.target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_match_png_files_by_folder_depth_then_line() {
        let rules = parse(
            "\u{feff}# frames and logos\r\n\r\ngui/images edds\r\n\\Data\\ paa\r\ngui/images/deep paa\r\nGUI/IMAGES edds\r\n",
        )
        .unwrap();
        let cases = [
            ("gui/images/frame.png", Some((6, Target::Edds))),
            ("gui/IMAGES/Frame.PNG", Some((6, Target::Edds))),
            ("gui/images/deep/x.png", Some((5, Target::Paa(None)))),
            ("data/sub/logo.PNG", Some((4, Target::Paa(None)))),
            ("gui/images/frame.txt", None),
            ("gui/imagesextra/frame.png", None),
            ("frame.png", None),
        ];
        for (rel, expected) in cases {
            assert_eq!(rules.target(rel), expected, "{rel}");
        }

        let whole_mod = parse("/ edds\n").unwrap();
        assert_eq!(whole_mod.target("frame.png"), Some((1, Target::Edds)));
        assert_eq!(whole_mod.target("a/b/frame.png"), Some((1, Target::Edds)));
    }

    #[test]
    fn bad_lines_name_the_line_and_what_is_wrong() {
        let cases = [
            (
                "gui/images\n",
                ".pboimages line 1: expected a folder and a format, like `gui/images edds`",
            ),
            (
                "gui images edds\n",
                ".pboimages line 1: expected a folder and a format, like `gui/images edds`",
            ),
            (
                "# note\n\ngui/images dds\n",
                ".pboimages line 3: unknown format \"dds\"; use edds or paa",
            ),
            (
                "./gui/images edds\n",
                ".pboimages line 1: write the folder from the mod root without `.`, `..` or wildcards",
            ),
            (
                "gui/* paa\n",
                ".pboimages line 1: write the folder from the mod root without `.`, `..` or wildcards",
            ),
        ];
        for (text, message) in cases {
            let error = parse(text).unwrap_err();

            assert_eq!(error.to_string(), message);
        }
    }
}
