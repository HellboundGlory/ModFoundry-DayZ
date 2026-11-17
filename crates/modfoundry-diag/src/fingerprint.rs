//! A normalised identity for one logged error: the same fault names itself the
//! same way in another run, whatever its timestamp, address, line or path.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d{1,2}:\d{2}:\d{2}(?:\.\d+)?").unwrap());
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0[xX][0-9a-fA-F]+").unwrap());
static ABSOLUTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(^|[\s"'(=@])(?:[A-Za-z]:[\\/]|\\\\|/)(?:[^\s"'()=@]*[\\/])?([^\s"'()=@\\/]*)"#)
        .unwrap()
});
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+\b").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// What a stack trace's opening line says: the identity sits in the frames.
const GENERIC_EXCEPTION: &str = "Virtual Machine Exception";
const STACK_TRACE: &str = "-- Stack trace --";

/// Strip what changes between runs: the timestamp, addresses, the folders of
/// absolute paths (the file name stays), whole numbers and separator style.
/// Letters, words and digits inside names stay, so two different errors do
/// not collapse into one.
pub fn fingerprint(line: &str) -> String {
    let line = TIMESTAMP.replace(line, "");
    let line = ADDRESS.replace_all(&line, "<addr>");
    let line = ABSOLUTE_PATH.replace_all(&line, "${1}<path>/${2}");
    let line = DIGITS.replace_all(&line, "<n>");
    let line = line.replace('\\', "/");
    SPACES.replace_all(&line, " ").trim().to_lowercase()
}

/// The identity of a mapped error: its message and, when the engine named one,
/// the file it logged. The line number is left out.
pub fn error_fingerprint(message: &str, logged_file: Option<&str>) -> String {
    match logged_file {
        Some(file) => fingerprint(&format!("{message} @ {file}")),
        None => fingerprint(message),
    }
}

/// The first 8 hex characters of the fingerprint's SHA-256, short enough to type.
pub fn short_id(fingerprint: &str) -> String {
    let digest = Sha256::digest(fingerprint.as_bytes());
    format!("{digest:x}")[..8].to_owned()
}

/// True when the message names no fault of its own: a VM exception header or a
/// stack trace whose identity lives in the folded-away frames.
pub fn is_generic(message: &str) -> bool {
    let message = message.trim();
    message == GENERIC_EXCEPTION || message.starts_with(STACK_TRACE)
}

#[cfg(test)]
mod tests {
    use super::{error_fingerprint, fingerprint, is_generic, short_id};

    #[test]
    fn a_timestamp_address_paths_numbers_and_separators_normalise_out() {
        let cases = [
            ("15:52:04.974  SCRIPT    (E): boom", "script (e): boom"),
            (" 3:45:51.567 SCRIPT (E): boom", "script (e): boom"),
            ("15:52:04 SCRIPT (E): boom", "script (e): boom"),
            ("bad address 0x7ff6a1b2 here", "bad address <addr> here"),
            (r"at C:\Users\x\a.c now", "at <path>/a.c now"),
            ("at Z:/mods/a.c now", "at <path>/a.c now"),
            (r"at \\host\share\a.c now", "at <path>/a.c now"),
            (" /home/x/a.c", "<path>/a.c"),
            ("JM/CF/Scripts/a.c", "jm/cf/scripts/a.c"),
            ("and/or", "and/or"),
            ("line 46", "line <n>"),
            (r"a\b", "a/b"),
            ("two    spaces   here", "two spaces here"),
        ];
        for (line, expected) in cases {
            assert_eq!(fingerprint(line), expected, "{line}");
        }
    }

    #[test]
    fn lines_that_differ_only_in_what_the_run_prints_share_a_fingerprint() {
        let first = "15:52:04.974 SCRIPT (E): fail 0x12ab at line 46";
        let second = "16:01:02.100 SCRIPT (E): fail 0x99ff at line 7";
        assert_eq!(fingerprint(first), fingerprint(second));
        assert_ne!(
            fingerprint("SCRIPT (E): one thing"),
            fingerprint("SCRIPT (E): another thing")
        );
    }

    #[test]
    fn digits_inside_a_name_and_the_file_a_path_names_stay_apart() {
        assert_ne!(
            fingerprint("Can't find variable 'Slot1'"),
            fingerprint("Can't find variable 'Slot2'")
        );
        assert_ne!(
            fingerprint("Cannot open /a/x.pbo"),
            fingerprint("Cannot open /b/y.pbo")
        );
        assert_eq!(
            fingerprint(r"Cannot open C:\game\x.pbo"),
            fingerprint("Cannot open Z:/home/me/game/x.pbo")
        );
    }

    #[test]
    fn a_fingerprint_gets_a_stable_eight_hex_character_id() {
        let id = short_id(&fingerprint("SCRIPT (E): boom"));
        assert_eq!(id.len(), 8);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(id, short_id(&fingerprint("SCRIPT (E): boom")));
        assert_ne!(id, short_id(&fingerprint("SCRIPT (E): other")));
    }

    #[test]
    fn the_same_message_in_two_files_differs_and_two_line_numbers_do_not() {
        let message = "Can't find variable 'x'";
        assert_ne!(
            error_fingerprint(message, Some("Mod/scripts/3_Game/a.c")),
            error_fingerprint(message, Some("Mod/scripts/3_Game/b.c"))
        );
        assert_eq!(
            error_fingerprint(message, Some("Mod/scripts/3_Game/a.c")),
            error_fingerprint(message, Some("Mod/scripts/3_Game/a.c"))
        );
    }

    #[test]
    fn a_vm_header_and_a_stack_trace_are_generic() {
        for message in [
            "Virtual Machine Exception",
            "  Virtual Machine Exception  ",
            "-- Stack trace --",
            "   -- Stack trace --  ",
        ] {
            assert!(is_generic(message), "{message}");
        }
        for message in ["JSON ERROR x.c:46", "Virtual Machine Exception: oops"] {
            assert!(!is_generic(message), "{message}");
        }
    }
}
