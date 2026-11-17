//! MCP resources: read-only context, each bound to a registered read command.
//!
//! A resource is the *same* operation a tool call would reach — `vfs.read` for
//! a file, `formats.list` for the format table — so the permission verdict is
//! the resource's existence: a command this session may not run is not a
//! resource here, exactly as it is not a tool.
//!
//! This module owns the table and URI matching only; the server dispatches and
//! gates.

use serde_json::{json, Value};

/// A resource the server can serve: a concrete `uri`, or a template carrying
/// one `{path}` variable. `command` is the registered operation that answers
/// it and therefore what gates it.
pub struct Resource {
    pub name: &'static str,
    pub uri: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
    pub command: &'static str,
}

pub const RESOURCES: &[Resource] = &[
    Resource {
        name: "formats",
        uri: "modfoundry://formats",
        description: "Every file format ModFoundry recognizes.",
        mime_type: "application/json",
        command: "formats.list",
    },
    Resource {
        name: "host",
        uri: "modfoundry://host",
        description: "DayZ game / DayZ Tools / Steam workshop installations on this host.",
        mime_type: "application/json",
        command: "host.doctor",
    },
    Resource {
        name: "ce",
        uri: "modfoundry://ce/{path}",
        description:
            "The records of a DayZ Central Economy XML file: its kind, sections and parse issues.",
        mime_type: "application/json",
        command: "ce.get_tree",
    },
    Resource {
        name: "config",
        uri: "modfoundry://config/{path}",
        description: "The class tree of a DayZ config.cpp (outline + parse issues).",
        mime_type: "application/json",
        command: "config.get_tree",
    },
    Resource {
        name: "file",
        uri: "modfoundry://file/{path}",
        description: "The UTF-8 text of one file.",
        mime_type: "text/plain",
        command: "vfs.read",
    },
    Resource {
        name: "script",
        uri: "modfoundry://script/{path}",
        description: "Enforce Script lint findings (MF-E*/MF-W*) for a file or module tree.",
        mime_type: "application/json",
        command: "script.diagnostics",
    },
];

impl Resource {
    /// The RFC 6570 template this resource is addressed by, when it is not a
    /// concrete URI.
    pub fn template(&self) -> Option<&'static str> {
        self.uri.contains('{').then_some(self.uri)
    }
}

/// The resource a request URI names — an exact concrete match first, then the
/// template whose literal prefix it extends — with the command arguments that
/// request implies. `None` means no such resource.
pub fn resolve(uri: &str) -> Option<(&'static Resource, Value)> {
    if let Some(resource) = RESOURCES.iter().find(|r| r.uri == uri) {
        return Some((resource, json!({})));
    }
    RESOURCES.iter().find_map(|resource| {
        let template = resource.template()?;
        let prefix = &template[..template.find('{')?];
        let rest = uri.strip_prefix(prefix).filter(|rest| !rest.is_empty())?;
        Some((resource, json!({ "path": percent_decode(rest) })))
    })
}

/// Decode the percent-escapes a URI path carries (`%20` for a space). A
/// malformed escape is left as written, so the path reaches the command and
/// fails there rather than being silently rewritten.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = hex_escape(bytes, i) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte a `%XX` escape at `i` names, when the escape is well-formed.
fn hex_escape(bytes: &[u8], i: usize) -> Option<u8> {
    let digits = bytes.get(i + 1..i + 3)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_concrete_uri_resolves_to_its_command_with_no_arguments() {
        let (resource, args) = resolve("modfoundry://formats").expect("a resource");
        assert_eq!(resource.command, "formats.list");
        assert_eq!(args, json!({}));
        assert_eq!(resource.template(), None);
    }

    #[test]
    fn a_path_uri_resolves_to_its_command_and_decoded_path() {
        let rows = [
            (
                "modfoundry://file//tmp/My%20Mod/Scripts/A.c",
                "vfs.read",
                "/tmp/My Mod/Scripts/A.c",
                "modfoundry://file/{path}",
            ),
            (
                "modfoundry://config//tmp/My%20Mod/config.cpp",
                "config.get_tree",
                "/tmp/My Mod/config.cpp",
                "modfoundry://config/{path}",
            ),
            (
                "modfoundry://ce//tmp/My%20Mod/db/types.xml",
                "ce.get_tree",
                "/tmp/My Mod/db/types.xml",
                "modfoundry://ce/{path}",
            ),
        ];
        for (uri, command, path, template) in rows {
            let (resource, args) = resolve(uri).unwrap_or_else(|| panic!("{uri} resolves"));
            assert_eq!(resource.command, command, "{uri}");
            assert_eq!(args["path"], path, "{uri}");
            assert_eq!(resource.template(), Some(template), "{uri}");
        }
    }

    #[test]
    fn an_unknown_uri_and_a_bare_prefix_resolve_to_nothing() {
        assert!(resolve("modfoundry://nope").is_none());
        assert!(resolve("modfoundry://file/").is_none());
        assert!(resolve("modfoundry://file").is_none());
    }

    #[test]
    fn a_malformed_escape_is_not_rewritten() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%zz"), "a%zz");
        assert_eq!(percent_decode("a%+1"), "a%+1");
    }
}
