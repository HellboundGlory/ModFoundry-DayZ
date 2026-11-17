//! Reads the top-level `name` property from `mod.cpp`/`meta.cpp`.

use crate::token::read_string;

/// The top-level `name` of `mod.cpp` when present and non-empty, else that of
/// `meta.cpp`, else `None` (the caller falls back to the folder name).
pub fn mod_display_name(mod_cpp: Option<&str>, meta_cpp: Option<&str>) -> Option<String> {
    mod_cpp
        .and_then(top_level_name)
        .or_else(|| meta_cpp.and_then(top_level_name))
}

fn top_level_name(src: &str) -> Option<String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
            }
            b'"' => {
                read_string(src, &mut i);
            }
            c if is_ident_start(c) => {
                let start = i;
                while i < b.len() && is_ident_continue(b[i]) {
                    i += 1;
                }
                if depth == 0 && &src[start..i] == "name" {
                    if let Some(name) = read_assigned_string(src, &mut i) {
                        return Some(name);
                    }
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn read_assigned_string(src: &str, i: &mut usize) -> Option<String> {
    let b = src.as_bytes();
    let mut j = *i;
    while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\r' | b'\n') {
        j += 1;
    }
    if b.get(j) != Some(&b'=') {
        return None;
    }
    j += 1;
    while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\r' | b'\n') {
        j += 1;
    }
    *i = j;
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let name = read_string(src, i)?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

#[cfg(test)]
mod tests {
    use super::mod_display_name;

    const CF_META: &str = "protocol = 1;\r\npublishedid = 1559212036;\r\nname = \"CF\";\r\ntimestamp = 5250757174595880000;\r\n";
    const CF_MOD: &str = "name = \"Community Framework\";\r\n";

    #[test]
    fn meta_name_survives_crlf() {
        assert_eq!(
            mod_display_name(None, Some(CF_META)),
            Some("CF".to_string())
        );
    }

    #[test]
    fn mod_cpp_name_wins() {
        assert_eq!(
            mod_display_name(Some(CF_MOD), Some(CF_META)),
            Some("Community Framework".to_string())
        );
    }

    #[test]
    fn meta_name_used_when_mod_has_none() {
        assert_eq!(
            mod_display_name(Some("dir = \"CF\";\r\n"), Some(CF_META)),
            Some("CF".to_string())
        );
    }

    #[test]
    fn empty_mod_name_falls_back_to_meta() {
        assert_eq!(
            mod_display_name(Some("name = \"\";\r\n"), Some(CF_META)),
            Some("CF".to_string())
        );
    }

    #[test]
    fn no_name_anywhere_is_none() {
        assert_eq!(mod_display_name(None, None), None);
        assert_eq!(
            mod_display_name(Some("name = \"\";"), Some("protocol = 1;")),
            None
        );
    }

    #[test]
    fn comments_whitespace_and_doubled_quotes() {
        let src = "// name = \"nope\";\r\n/* ignored */\nname   =   \"a\"\"b\" ;\r\n";
        assert_eq!(mod_display_name(Some(src), None), Some("a\"b".to_string()));
    }

    #[test]
    fn nested_and_array_names_are_skipped() {
        let src = "class X { name = \"inner\"; };\r\nname[] = { \"a\" };\r\nname = \"top\";";
        assert_eq!(mod_display_name(Some(src), None), Some("top".to_string()));
    }
}
