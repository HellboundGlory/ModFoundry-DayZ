//! Pure RPT / script.log parser.
//!
//! DayZ engine logs are plaintext but line-format-typed. This module turns a
//! RPT or script.log body into structured [`LogRecord`]s with a channel, a
//! severity and — where the engine reports one — a source file:line the error
//! can be mapped back to. No filesystem IO: callers read the file, we parse.

/// Severity of one engine log line. `Info` covers both the `(I)` marker and
/// markerless info lines (the engine emits them interchangeably).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// The engine subsystem that wrote a log line. `Other` is the fallback for
/// channels not in the small known set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    Engine,
    Gui,
    Script,
    Entity,
    World,
    Resources,
    Animation,
    Render,
    Physics,
    Material,
    Particles,
    Network,
    Player,
    Other(String),
}

/// One structured log line. `file`/`line` carry the source location the engine
/// attributed to the message, when it did (e.g. `@"path,line":` in script
/// warnings or the top VM-exception stack frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// Wall-clock timestamp as printed (e.g. `15:52:04.974`), when present.
    pub time: Option<String>,
    pub channel: Channel,
    pub level: Level,
    /// Full message text (file-reference prefix stripped into `file`/`line`).
    pub message: String,
    /// Engine-reported source path when derivable (PBO/virtual path).
    pub file: Option<String>,
    /// 1-based engine line number when derivable.
    pub line: Option<u32>,
}

/// Channel tokens are matched case-insensitively and may contain `_`.
fn channel(s: &str) -> Channel {
    let t = s.trim_end();
    let lower = t.to_ascii_lowercase();
    match lower.as_str() {
        "engine" => Channel::Engine,
        "gui" => Channel::Gui,
        "script" => Channel::Script,
        "entity" => Channel::Entity,
        "world" => Channel::World,
        "resources" => Channel::Resources,
        "animation" => Channel::Animation,
        "render" => Channel::Render,
        "physics" => Channel::Physics,
        "material" => Channel::Material,
        "particles" => Channel::Particles,
        "network" => Channel::Network,
        "player" => Channel::Player,
        other => Channel::Other(other.to_string()),
    }
}

/// Split a header line's leading timestamp (if any) from the rest.
/// Returns `(time, remainder_after_time)`.
fn strip_time(line: &str) -> (Option<String>, &str) {
    // Timestamps look like `HH:MM:SS.mmm` or `HH:MM:SS`.
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let start = i;
    let mut digits = 0;
    let mut colons = 0;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':' || b[i] == b'.') {
        if b[i].is_ascii_digit() {
            digits += 1;
        } else if b[i] == b':' {
            colons += 1;
        }
        i += 1;
    }
    // Time is only a timestamp if shaped HH:MM:SS[.mmm] (>=2 colons, some digits).
    if colons >= 2 && digits >= 2 {
        (Some(line[start..i].to_string()), &line[i..])
    } else {
        (None, line)
    }
}

/// Header: optional `HH:MM:SS(.mmm)` timestamp, padded channel token, then
/// either `(E|W|I):` or a bare `:`, then the message. Both RPT lines
/// (`15:52:04.974  SCRIPT    (W): …`) and script.log lines
/// (` SCRIPT    (W): …`, no timestamp) match.
///
/// Parse one header line.
/// Returns `None` if the line is not a header.
fn parse_header(line: &str) -> Option<LogRecord> {
    let (time, rest) = strip_time(line);
    let rest = rest.trim_start();
    let (tok, after_tok) = split_token(rest)?;
    let channel = channel(tok);
    let after = after_tok.trim_start();
    let (level, msg) = if let Some(stripped) = after.strip_prefix('(') {
        let close = stripped.find(')')?;
        let code = &stripped[..close];
        let level = match code {
            "E" => Level::Error,
            "W" => Level::Warning,
            _ => Level::Info,
        };
        let rest = stripped[close + 1..].strip_prefix(':')?;
        (level, rest.trim_start())
    } else {
        let rest = after.strip_prefix(':')?;
        (Level::Info, rest.trim_start())
    };
    let mut rec = LogRecord {
        time,
        channel,
        level,
        message: msg.to_string(),
        file: None,
        line: None,
    };
    // `@"path,line":` source-reference prefix in the message.
    if let Some((msg, path, line)) = parse_src_ref(&rec.message) {
        rec.message = msg.to_string();
        rec.file = Some(path);
        rec.line = Some(line);
    }
    Some(rec)
}

fn split_token(s: &str) -> Option<(&str, &str)> {
    let end = s
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_alphanumeric() || *ch == '_'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((&s[..end], &s[end..]))
}

/// `@"<path>,<line>":` prefix — the script compiler's file attribution.
/// Returns `(message_without_prefix, normalized_path, line)`.
pub(crate) fn parse_src_ref(msg: &str) -> Option<(&str, String, u32)> {
    let m = msg.trim_start().strip_prefix("@\"")?;
    let close = m.find('"')?;
    // The path may contain commas; the line number follows the last one.
    let (path, line) = m[..close].rsplit_once(',')?;
    let line: u32 = line.trim().parse().ok()?;
    let msg = m[close + 1..].strip_prefix(':')?.trim_start();
    Some((msg, path.replace('\\', "/"), line))
}

/// True if `line` looks like a fresh record header (started a new engine line)
/// rather than continuation content inside a multi-line block. We treat a line
/// as a header when it begins with a timestamp OR begins (after optional
/// spaces) with an all-caps/known channel token followed by `:`/`(X):`.
fn is_header(line: &str) -> bool {
    let (time, rest) = strip_time(line);
    if time.is_some() {
        return true;
    }
    let rest = rest.trim_start();
    let (tok, after) = match split_token(rest) {
        Some(v) => v,
        None => return false,
    };
    // Channel tokens are engine subsystems; require uppercase-ish token shape
    // to avoid misreading continuation prose lines ("Reason:", "Class:").
    let upper = tok
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit());
    if !upper || tok.len() < 3 {
        return false;
    }
    let after = after.trim_start();
    after.starts_with('(') || after.starts_with(':')
}

/// Parse an RPT or script.log body into records. Header lines are
/// channel/severity-typed; a multi-line "Virtual Machine Exception" block is
/// folded into its opening `SCRIPT (E)` record (location = innermost frame).
pub fn parse_log(text: &str) -> Vec<LogRecord> {
    let mut out: Vec<LogRecord> = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');
        if !is_header(line) {
            i += 1;
            continue;
        }
        let mut rec = match parse_header(line) {
            Some(r) => r,
            None => {
                i += 1;
                continue;
            }
        };
        let mut frame_site: Option<(String, u32)> = None;
        let is_vm = rec.level == Level::Error
            && rec.channel == Channel::Script
            && rec.message.trim() == "Virtual Machine Exception";
        if is_vm {
            i += 1;
            let mut in_stack = false;
            while i < lines.len() {
                let l = lines[i].trim_end_matches('\r');
                if is_header(l) {
                    break;
                }
                let t = l.trim();
                if t.starts_with("Stack trace:") {
                    in_stack = true;
                } else if in_stack {
                    // frame: `<path>:<line> Function <name>`; the first (top,
                    // innermost) is the throw site we want.
                    if frame_site.is_none() {
                        if let Some((path, line)) = parse_frame(t) {
                            frame_site = Some((path, line));
                        }
                    }
                }
                i += 1;
            }
            if let Some((path, line)) = frame_site {
                rec.file = Some(path);
                rec.line = Some(line);
            }
        } else {
            i += 1;
        }
        out.push(rec);
    }
    out
}

/// Parse a VM stack frame line `path:line Function name` → `(normalized_path,
/// line)`. Paths may contain `\` separators and arbitrary prefixes.
fn parse_frame(t: &str) -> Option<(String, u32)> {
    // Find the LAST `:N ` before " Function" so paths containing ':' still work.
    let fun_idx = t.find(" Function")?;
    let head = &t[..fun_idx];
    let colon = head.rfind(':')?;
    let path = &head[..colon];
    let line: u32 = head[colon + 1..].trim().parse().ok()?;
    if path.is_empty() || path.contains(' ') {
        return None;
    }
    Some((path.replace('\\', "/"), line))
}

#[cfg(test)]
mod tests {
    use super::{parse_log, Channel, Level};

    /// Real-shaped single-line warning with a `@"path,line":` source prefix.
    const SCRIPT_WARN_RPT: &str = "15:52:04.285  SCRIPT    (W): @\"SearchInventory/Scripts/5_Mission/uihelper\\ui\\uisearchinventorywindow.c,338\": FIX-ME: Undefined function 'DayZGame.GetMousePos'\r\n";

    /// Real-shaped GUI error, no source reference.
    const GUI_ERR_RPT: &str = "14:56:29.490 GUI       (E): LoadImageSet 'HelicopterModScripts/GUI/Textures.imageset' file not exist\r\n";

    /// Real-shaped multi-line VM-exception block (header + blank + reason +
    /// class/function + stack trace of file:line frames).
    const VM_BLOCK_RPT: &str = "15:52:04.974  SCRIPT    (E): Virtual Machine Exception\r\n\
\r\n\
Reason: Function 'SetTransform' not linked\r\n\
\r\n\
Class:      'AA_AreaAnnouncementManager'\r\n\
Function: 'ApplyTextStyle'\r\n\
Stack trace:\r\n\
AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1123 Function ApplyTextStyle\r\n\
AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1140 Function ApplyTitleTextStyleToAll\r\n\
15:52:05.855  ENTITY    (E): Type 'LargeTentClutterCutter' must be inherited from class 'Building'\r\n";

    /// Real-shaped script.log header (leading-space, no wall-clock time).
    const SCRIPT_LOG_LINE: &str = " SCRIPT    (W): @\"NoRelogHere/Scripts/3_Game/nrhconfig.c,25\": FIX-ME: Unexpected statement 'ref'\r\n";

    #[test]
    fn parses_a_script_warning_with_source_location() {
        let recs = parse_log(SCRIPT_WARN_RPT);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.channel, Channel::Script);
        assert_eq!(r.level, Level::Warning);
        assert_eq!(r.time.as_deref(), Some("15:52:04.285"));
        assert_eq!(
            r.file.as_deref(),
            Some("SearchInventory/Scripts/5_Mission/uihelper/ui/uisearchinventorywindow.c")
        );
        assert_eq!(r.line, Some(338));
        assert!(r
            .message
            .contains("Undefined function 'DayZGame.GetMousePos'"));
    }

    #[test]
    fn parses_an_error_without_source_location() {
        let recs = parse_log(GUI_ERR_RPT);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.channel, Channel::Gui);
        assert_eq!(r.level, Level::Error);
        assert!(r.file.is_none());
        assert!(r.message.contains("file not exist"));
    }

    #[test]
    fn folds_vm_exception_block_into_opening_record_with_innermost_frame() {
        let recs = parse_log(VM_BLOCK_RPT);
        assert_eq!(recs.len(), 2);
        let vm = &recs[0];
        assert_eq!(vm.level, Level::Error);
        assert_eq!(vm.message, "Virtual Machine Exception");
        assert_eq!(
            vm.file.as_deref(),
            Some("AreaAnnouncements/scripts/5_Mission/areaannouncements/aa_areaannouncementmanager.c")
        );
        assert_eq!(vm.line, Some(1123));
        // A following real header still parses as its own record.
        let ent = &recs[1];
        assert_eq!(ent.channel, Channel::Entity);
        assert_eq!(ent.level, Level::Error);
    }

    #[test]
    fn script_log_header_without_timestamp_parses() {
        let recs = parse_log(SCRIPT_LOG_LINE);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.time, None);
        assert_eq!(r.channel, Channel::Script);
        assert_eq!(r.level, Level::Warning);
        assert_eq!(
            r.file.as_deref(),
            Some("NoRelogHere/Scripts/3_Game/nrhconfig.c")
        );
        assert_eq!(r.line, Some(25));
    }

    #[test]
    fn empty_and_headerless_input_yields_no_records() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("==============\n== banner ==\nwall time: 0.00 seconds\n").is_empty());
    }

    #[test]
    fn a_comma_in_the_source_path_keeps_the_location() {
        let recs = parse_log(" SCRIPT    (W): @\"My,Mod/Scripts/3_Game/x.c,25\": FIX-ME: oops\r\n");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].file.as_deref(), Some("My,Mod/Scripts/3_Game/x.c"));
        assert_eq!(recs[0].line, Some(25));
    }
}
