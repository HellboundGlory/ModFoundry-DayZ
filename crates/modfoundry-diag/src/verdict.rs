//! Judging a run's logs: expected lines, failing lines, script errors tied to a mod in the run, and
//! crash logs. Pure: callers pass each log's path and text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// One expected or failing line: plain text, or with a `re:` prefix a regular expression.
#[derive(Debug, Clone)]
pub enum LinePattern {
    Text(String),
    Regex { source: String, regex: Regex },
}

impl LinePattern {
    /// Parse a pattern: `re:<regex>` is a regular expression, anything else matches as plain text.
    pub fn parse(s: &str) -> Result<LinePattern, String> {
        match s.strip_prefix("re:") {
            Some(regex) if regex.trim().is_empty() => Err(format!("{s:?}: empty pattern")),
            Some(regex) => Regex::new(regex)
                .map(|regex| LinePattern::Regex {
                    source: s.to_string(),
                    regex,
                })
                .map_err(|error| format!("{s}: {error}")),
            None if s.trim().is_empty() => Err(format!("{s:?}: empty pattern")),
            None => Ok(LinePattern::Text(s.to_string())),
        }
    }

    /// True when `line` matches this pattern.
    pub fn matches(&self, line: &str) -> bool {
        match self {
            LinePattern::Text(text) => line.contains(text.as_str()),
            LinePattern::Regex { regex, .. } => regex.is_match(line),
        }
    }

    /// The pattern as the user wrote it, `re:` included.
    pub fn source(&self) -> &str {
        match self {
            LinePattern::Text(text) => text,
            LinePattern::Regex { source, .. } => source,
        }
    }
}

/// What a run's logs are judged against.
#[derive(Debug, Clone, Default)]
pub struct VerdictRules {
    pub expect: Vec<LinePattern>,
    pub fail: Vec<LinePattern>,
    pub allow: Vec<LinePattern>,
    /// Fingerprints of errors the project's known-error file allows.
    pub allow_fingerprints: Vec<String>,
    /// Names that tie a script error to this run: PBO prefixes and `@` folder names without the `@`.
    pub mods: Vec<String>,
}

/// The outcome of judging a run's logs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub passed: bool,
    pub missing: Vec<String>,
    pub fail_lines: Vec<String>,
    pub mod_errors: Vec<String>,
    pub other_errors: usize,
    pub ignored: usize,
    pub crash_logs: Vec<PathBuf>,
}

/// Judge one run's logs; `logs` holds each log file's path and text.
pub fn judge(rules: &VerdictRules, logs: &[(PathBuf, String)]) -> Verdict {
    let lines: Vec<&str> = logs
        .iter()
        .flat_map(|(_, text)| text.split('\n').map(str::trim_end))
        .collect();

    let missing = missing_expectations(rules, &lines);
    let fail_lines = matching_lines(rules, &lines);
    let crash_logs: Vec<PathBuf> = logs
        .iter()
        .filter(|(path, text)| is_crash_log(path) && !ignored_vm_crash(text, rules))
        .map(|(path, _)| path.clone())
        .collect();

    let mut seen_errors = HashSet::new();
    let mut mod_errors = Vec::new();
    let (mut other_errors, mut ignored) = (0, 0);
    for error in logs
        .iter()
        .flat_map(|(_, text)| script_errors(text))
        .filter(|error| seen_errors.insert(dedup_key(error).to_string()))
    {
        if is_known_vanilla(&error)
            || is_allowed(&error, &rules.allow)
            || is_allowed_fingerprint(&error, &rules.allow_fingerprints)
        {
            ignored += 1;
        } else if names_a_mod(&error, &rules.mods) {
            mod_errors.push(error);
        } else {
            other_errors += 1;
        }
    }

    Verdict {
        passed: missing.is_empty()
            && fail_lines.is_empty()
            && mod_errors.is_empty()
            && crash_logs.is_empty(),
        missing,
        fail_lines,
        mod_errors,
        other_errors,
        ignored,
        crash_logs,
    }
}

// `error_*.log` repeats a trace without the message line before it.
fn dedup_key(error: &str) -> &str {
    error
        .find("-- Stack trace --")
        .map_or(error, |start| &error[start..])
}

fn missing_expectations(rules: &VerdictRules, lines: &[&str]) -> Vec<String> {
    rules
        .expect
        .iter()
        .filter(|pattern| !lines.iter().any(|line| pattern.matches(line)))
        .map(|pattern| pattern.source().to_string())
        .collect()
}

fn matching_lines(rules: &VerdictRules, lines: &[&str]) -> Vec<String> {
    let mut seen = HashSet::new();
    lines
        .iter()
        .filter(|line| rules.fail.iter().any(|pattern| pattern.matches(line)))
        .filter(|line| seen.insert(line_body(line)))
        .map(|line| line.trim().to_string())
        .collect()
}

fn is_crash_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("crash_"))
}

// The game writes a crash log for every script VM exception and keeps running,
// so such a log only fails the verdict when a block in it is anything else.
// Fails closed: any section it cannot read as an ignored VM exception counts.
fn ignored_vm_crash(text: &str, rules: &VerdictRules) -> bool {
    let mut sections = vec![Vec::new()];
    for line in text.split('\n').map(str::trim_end) {
        if is_dash_line(line.trim()) {
            sections.push(Vec::new());
        } else if !line.trim().is_empty() {
            sections.last_mut().expect("never empty").push(line);
        }
    }
    let mut found = false;
    for section in sections.iter().filter(|s| !s.is_empty()) {
        if is_crash_preamble(section) {
            continue;
        }
        let Some(body) = vm_exception_body(section) else {
            return false;
        };
        if !(is_known_vanilla(&body)
            || is_allowed(&body, &rules.allow)
            || is_allowed_fingerprint(&body, &rules.allow_fingerprints))
        {
            return false;
        }
        found = true;
    }
    found
}

fn is_crash_preamble(section: &[&str]) -> bool {
    matches!(section, [line] if line.starts_with("Log ") && line.contains(" started at "))
}

/// A section's exception text without its date and its `Runtime mode` trailer, which
/// names every loaded mod and so must never be what an allow pattern matches.
fn vm_exception_body(section: &[&str]) -> Option<String> {
    let [date, header, rest @ ..] = section else {
        return None;
    };
    if !is_crash_date(date) || *header != "Virtual Machine Exception" {
        return None;
    }
    let end = rest
        .iter()
        .position(|line| *line == "Runtime mode")
        .unwrap_or(rest.len());
    if !rest[end..]
        .iter()
        .skip(1)
        .all(|line| line.starts_with("CLI params:"))
    {
        return None;
    }
    Some(
        std::iter::once(*header)
            .chain(rest[..end].iter().copied())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// `08.11 2026 19:22:18`: the date a crash log prints under each separator.
fn is_crash_date(line: &str) -> bool {
    let digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = line.split_whitespace();
    let (Some(date), Some(year), Some(time), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some((day, month)) = date.split_once('.') else {
        return false;
    };
    let mut clock = time.split(':');
    digits(day, 2)
        && digits(month, 2)
        && digits(year, 4)
        && matches!(
            (clock.next(), clock.next(), clock.next(), clock.next()),
            (Some(h), Some(m), Some(s), None)
                if digits(h, 2) && digits(m, 2) && digits(s, 2)
        )
}

// Vanilla `GetPlugin` as the top frame keeps a mod frame above it from being hidden.
const KNOWN_VANILLA: &[&[&str]] = &[&[
    "-- Stack trace --\nGetPlugin() scripts/4_World/plugins/pluginmanager.c",
    "scripts/4_World/plugins/pluginbase/pluginitemdiagnostic.c",
]];

fn is_known_vanilla(text: &str) -> bool {
    KNOWN_VANILLA
        .iter()
        .any(|entry| entry.iter().all(|part| text.contains(part)))
}

struct ScriptLine<'a> {
    error: bool,
    message: &'a str,
}

fn script_header(line: &str) -> Option<ScriptLine<'_>> {
    let (_, rest) = split_time(line);
    let (word, after) = split_word(rest.trim_start());
    if !word.eq_ignore_ascii_case("SCRIPT") {
        return None;
    }
    let after = after.trim_start();
    if let Some(stripped) = after.strip_prefix('(') {
        let close = stripped.find(')')?;
        if &stripped[..close] != "E" {
            return None;
        }
        let message = stripped[close + 1..].strip_prefix(':')?.trim_start();
        return Some(ScriptLine {
            error: true,
            message,
        });
    }
    let message = after.strip_prefix(':')?.trim_start();
    Some(ScriptLine {
        error: false,
        message,
    })
}

fn split_word(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}

fn split_time(line: &str) -> (Option<&str>, &str) {
    let rest = line.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':' || c == '.'))
        .unwrap_or(rest.len());
    let token = &rest[..end];
    if token.matches(':').count() >= 2 {
        (Some(token), &rest[end..])
    } else {
        (None, rest)
    }
}

fn line_body(line: &str) -> &str {
    match script_header(line) {
        Some(header) => header.message.trim(),
        None => line.trim(),
    }
}

fn push_line(lines: &mut Vec<String>, line: &str) {
    let line = line.trim();
    if !line.is_empty() {
        lines.push(line.to_string());
    }
}

fn is_dash_line(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|b| b == b'-')
}

fn script_errors(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').map(str::trim_end).collect();
    let mut errors = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(header) = script_header(lines[i]).filter(|header| header.error) else {
            i += 1;
            continue;
        };
        let message = header.message.trim();
        let (time, _) = split_time(lines[i]);
        if message.starts_with("-- Stack trace --") {
            let mut body = Vec::new();
            let previous_line = i.checked_sub(1).map(|j| lines[j]);
            if let Some(previous) = previous_line
                .filter(|line| split_time(line).0 == time)
                .and_then(script_header)
            {
                if !previous.error {
                    push_line(&mut body, previous.message);
                }
            }
            push_line(&mut body, message);
            i += 1;
            while let Some(line) = lines.get(i) {
                let body_line = line_body(line);
                if is_dash_line(body_line) {
                    push_line(&mut body, body_line);
                    i += 1;
                    break;
                }
                // The engine prints a whole trace under one log call's time; a
                // later line with another time starts a new error.
                if split_time(line).0 != time {
                    break;
                }
                push_line(&mut body, body_line);
                i += 1;
            }
            errors.push(body.join("\n"));
            continue;
        }
        if message == "Virtual Machine Exception" {
            let mut body = Vec::new();
            push_line(&mut body, message);
            i += 1;
            while let Some(line) = lines.get(i) {
                if split_time(line).0.is_some() || script_header(line).is_some() {
                    break;
                }
                push_line(&mut body, line_body(line));
                i += 1;
            }
            errors.push(body.join("\n"));
            continue;
        }
        errors.push(message.to_string());
        i += 1;
    }
    errors
}

/// A line also counts by its message alone, the text `mf errors` shows, so a
/// pattern anchored to the message matches past an `@"path,line":` prefix.
fn is_allowed(text: &str, allow: &[LinePattern]) -> bool {
    text.split('\n').any(|line| {
        let message = crate::parse::parse_src_ref(line.trim()).map(|(message, _, _)| message);
        allow
            .iter()
            .any(|pattern| pattern.matches(line) || message.is_some_and(|m| pattern.matches(m)))
    })
}

/// True when the error's first line names the same message and file as a listed
/// fingerprint. A generic first line carries no identity, so it never matches.
fn is_allowed_fingerprint(text: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return false;
    }
    let first = text.lines().next().unwrap_or_default().trim();
    if crate::fingerprint::is_generic(first) {
        return false;
    }
    let (message, path) = match crate::parse::parse_src_ref(first) {
        Some((message, path, _)) => (message, Some(path)),
        None => (first, None),
    };
    allowed.contains(&crate::fingerprint::error_fingerprint(
        message,
        path.as_deref(),
    ))
}

fn names_a_mod(text: &str, mods: &[String]) -> bool {
    let names: Vec<String> = mods
        .iter()
        // PBO prefixes are stored with `\`; logs print paths with `/`.
        .map(|name| name.replace('\\', "/").trim_matches('/').to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    text.split('\n').any(|line| {
        let line = line.replace('\\', "/").to_lowercase();
        names.iter().any(|name| line_names_mod(&line, name))
    })
}

fn line_names_mod(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    line.match_indices(name).any(|(start, _)| {
        let before = start.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(start + name.len()).copied();
        // `"` covers the script compiler's @"path,line": attribution.
        matches!(
            before,
            None | Some(b'/' | b'\\' | b' ' | b'(' | b'@' | b'"')
        ) && matches!(after, Some(b'/' | b'\\'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DONE_LINE: &str =
        " 3:46:54.502 SCRIPT       : [SampleModTests] DONE side=server pass=2350 fail=0";
    const FAILED_LINE: &str = " 3:46:54.502 SCRIPT       : [SampleModTests] FAIL SampleModCfg";

    const VANILLA_BLOCK: &str = concat!(
        " 3:45:34.985  SCRIPT       : string s =    'Module PluginConfigDebugProfile is not Registred in PluginManager.c!'\n",
        " 3:45:34.985  SCRIPT    (E):    -- Stack trace --\n",
        " 3:45:34.985  SCRIPT    (E):    GetPlugin() scripts/4_World/plugins/pluginmanager.c : 341\n",
        " 3:45:34.985  SCRIPT    (E):    OnInit() scripts/4_World/plugins/pluginbase/pluginitemdiagnostic.c : 94\n",
        " 3:45:34.985  SCRIPT    (E):    PluginsInit() scripts/4_World/plugins/pluginmanager.c : 136\n",
        " 3:45:34.985  SCRIPT    (E):    PluginManagerInit() scripts/4_World/plugins/pluginmanager.c : 304\n",
        " 3:45:34.985  SCRIPT    (E):    MissionBase#227#1() scripts/5_Mission/mission/missionbase.c : 21\n",
        " 3:45:34.985  SCRIPT    (E):    CreateCustomMission() $CurrentDir:mpmissions\\dayzOffline.chernarusplus\\init.c : 97\n",
        " 3:45:34.985  SCRIPT    (E):    -----------------",
    );

    const JSON_ERROR_LINE: &str = " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46";
    const CFGSOMETHING_LINE: &str = " 3:45:51.567 SCRIPT    (E): oops scripts/cfgsomething.c:12";

    const VM_BLOCK: &str = concat!(
        "15:52:04.974  SCRIPT    (E): Virtual Machine Exception\n",
        "\n",
        "Reason: Function 'SetTransform' not linked\n",
        "\n",
        "Class:      'AA_AreaAnnouncementManager'\n",
        "Function: 'ApplyTextStyle'\n",
        "Stack trace:\n",
        "AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1123 Function ApplyTextStyle\n",
        "AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1140 Function ApplyTitleTextStyleToAll\n",
        "15:52:05.855  ENTITY    (E): Type 'LargeTentClutterCutter' must be inherited from class 'Building'",
    );

    const MOD_STACK_TRACE: &str = concat!(
        " 3:45:34.985  SCRIPT       : some mod message\n",
        " 3:45:34.985  SCRIPT    (E):    -- Stack trace --\n",
        " 3:45:34.985  SCRIPT    (E):    OnCfg() SampleMod/scripts/5_Mission/cfg.c : 12\n",
        " 3:45:34.985  SCRIPT    (E):    -----------------",
    );

    #[test]
    fn text_patterns_match_a_substring_of_the_line() {
        let cases = [
            ("[SampleModTests] DONE", DONE_LINE, true),
            ("[SampleModTests] DONE", FAILED_LINE, false),
            ("pass=2350", DONE_LINE, true),
        ];
        for (source, line, expected) in cases {
            let pattern = LinePattern::parse(source).expect("pattern parses");
            assert_eq!(pattern.matches(line), expected, "{source} on {line}");
            assert_eq!(pattern.source(), source);
        }
    }

    #[test]
    fn a_re_prefix_makes_the_rest_a_regular_expression() {
        let source = r"re:DONE side=server pass=[1-9]\d* fail=0$";
        let pattern = LinePattern::parse(source).expect("pattern parses");
        assert!(pattern.matches(DONE_LINE));
        assert!(!pattern.matches(
            " 3:46:54.502 SCRIPT       : [SampleModTests] DONE side=server pass=0 fail=0"
        ));
        assert!(!pattern.matches(FAILED_LINE));
        assert_eq!(pattern.source(), source);
    }

    #[test]
    fn an_unusable_pattern_names_the_pattern_and_why() {
        let bad = LinePattern::parse("re:[unclosed").unwrap_err();
        assert!(bad.contains("re:[unclosed"), "{bad}");
        assert!(bad.contains("unclosed character class"), "{bad}");

        for empty in ["", "   ", "re:", "re: "] {
            let error = LinePattern::parse(empty).unwrap_err();
            assert!(error.contains("empty"), "{empty}: {error}");
        }
    }

    fn log(name: &str, text: &str) -> (PathBuf, String) {
        (PathBuf::from(name), text.to_string())
    }

    fn rule(source: &str) -> LinePattern {
        LinePattern::parse(source).expect("pattern parses")
    }

    fn rules_with_mods(mods: &[&str]) -> VerdictRules {
        VerdictRules {
            mods: mods.iter().map(|name| name.to_string()).collect(),
            ..VerdictRules::default()
        }
    }

    #[test]
    fn an_expected_line_that_appears_passes() {
        let rules = VerdictRules {
            expect: vec![rule("[SampleModTests] DONE")],
            ..VerdictRules::default()
        };
        let verdict = judge(&rules, &[log("server_2026-11-03.rpt", DONE_LINE)]);
        assert!(verdict.passed);
        assert!(verdict.missing.is_empty());
    }

    #[test]
    fn a_missing_expected_line_is_named() {
        let rules = VerdictRules {
            expect: vec![rule("[SampleModTests] DONE"), rule(r"re:pass=[1-9]\d*")],
            ..VerdictRules::default()
        };
        let verdict = judge(&rules, &[log("server_2026-11-03.rpt", FAILED_LINE)]);
        assert!(!verdict.passed);
        assert_eq!(
            verdict.missing,
            vec![
                "[SampleModTests] DONE".to_string(),
                r"re:pass=[1-9]\d*".to_string(),
            ]
        );
    }

    #[test]
    fn a_fail_line_fails_the_verdict() {
        let rules = VerdictRules {
            fail: vec![rule("[SampleModTests] FAIL")],
            ..VerdictRules::default()
        };
        let verdict = judge(
            &rules,
            &[
                log(
                    "server_2026-11-03.rpt",
                    &format!("{DONE_LINE}\n{FAILED_LINE}"),
                ),
                log(
                    "script_2026-11-03_12-00-00.log",
                    &FAILED_LINE.replacen(" 3:", "03:", 1),
                ),
            ],
        );
        assert!(!verdict.passed);
        assert_eq!(verdict.fail_lines, vec![FAILED_LINE.trim().to_string()]);
    }

    #[test]
    fn a_crash_log_fails_the_verdict() {
        for name in ["crash_2026-11-03.log", "Crash_2026-11-03.rpt"] {
            let verdict = judge(&VerdictRules::default(), &[log(name, "")]);
            assert!(!verdict.passed, "{name}");
            assert_eq!(verdict.crash_logs, vec![PathBuf::from(name)]);
        }
        for name in ["server.rpt", "CrashReporter.log"] {
            let quiet = judge(&VerdictRules::default(), &[log(name, "")]);
            assert!(quiet.passed, "{name}");
        }
    }

    const NULL_REASON: &str =
        "NULL pointer to instance. Variable 'nothing' during call 'Object.GetPosition'";

    const NULL_VM_SCRIPT_LOG: &str = concat!(
        " 3:45:34.985  SCRIPT    (E): Virtual Machine Exception\n",
        "\n",
        "Reason: NULL pointer to instance. Variable 'nothing' during call 'Object.GetPosition'\n",
        "\n",
        "Class:      'CustomMission'\n",
        "Function: 'MissionServer'\n",
        "Stack trace:\n",
        "MFChkB/Scripts/5_Mission/MFChkB.c:7 Function MissionServer\n",
        "$CurrentDir:mpmissions\\dayzOffline.chernarusplus\\init.c:97 Function CreateCustomMission\n",
    );

    const NULL_VM_CRASH_LOG: &str = concat!(
        "---------------------------------------------\n",
        "Log Z:\\home\\u\\DayZ\\mf-diag-profiles\\crash_2026-11-08_19-21-58.log started at 2026-11-08 19:22:18 (2026-11-08 19:22:18 UTC)\n",
        "\n\n\n",
        "------------------------------------\n",
        "08.11 2026 19:22:18\n",
        "Virtual Machine Exception\n",
        "\n",
        "Reason: NULL pointer to instance. Variable 'nothing' during call 'Object.GetPosition'\n",
        "\n",
        "Class:      'CustomMission'\n",
        "Function: 'MissionServer'\n",
        "Stack trace:\n",
        "MFChkB/Scripts/5_Mission/MFChkB.c:7 Function MissionServer\n",
        "$CurrentDir:mpmissions\\dayzOffline.chernarusplus\\init.c:97 Function CreateCustomMission\n",
        "\n",
        "Runtime mode\n",
        "CLI params: config serverDZ_modfoundry.cfg mod @MFChkA;@MFChkB serverMod  profiles mf-diag-profiles port 2302 server  filePatching  freezecheck  adminlog  dologs  nolauncher\n",
    );

    #[test]
    fn a_crash_log_of_an_ignored_vm_exception_does_not_fail_the_verdict() {
        let rules = VerdictRules {
            allow: vec![rule(NULL_REASON)],
            ..VerdictRules::default()
        };
        let verdict = judge(
            &rules,
            &[
                log("script_2026-11-08_19-21-58.log", NULL_VM_SCRIPT_LOG),
                log("crash_2026-11-08_19-21-58.log", NULL_VM_CRASH_LOG),
            ],
        );
        assert!(verdict.crash_logs.is_empty(), "{verdict:?}");
        assert!(verdict.passed, "{verdict:?}");
    }

    #[test]
    fn a_crash_log_of_a_vm_exception_no_rule_ignores_fails_and_is_named() {
        let verdict = judge(
            &VerdictRules::default(),
            &[
                log("script_2026-11-08_19-21-58.log", NULL_VM_SCRIPT_LOG),
                log("crash_2026-11-08_19-21-58.log", NULL_VM_CRASH_LOG),
            ],
        );
        assert!(!verdict.passed);
        assert_eq!(
            verdict.crash_logs,
            vec![PathBuf::from("crash_2026-11-08_19-21-58.log")]
        );
    }

    #[test]
    fn a_crash_log_with_no_vm_exception_fails_even_when_the_rules_allow_the_reason() {
        let rules = VerdictRules {
            allow: vec![rule(NULL_REASON)],
            ..VerdictRules::default()
        };
        let engine_crash = "------------------------------------\n\
                            08.11 2026 19:22:18\n\
                            Engine crash\n\
                            \n\
                            Reason: NULL pointer to instance. Variable 'nothing' during call 'Object.GetPosition'\n\
                            \n\
                            Stack trace:\n\
                            MFChkB/Scripts/5_Mission/MFChkB.c:7 Function MissionServer\n";
        let verdict = judge(
            &rules,
            &[log("crash_2026-11-08_19-21-58.log", engine_crash)],
        );
        assert!(!verdict.passed);
        assert_eq!(verdict.crash_logs.len(), 1, "{verdict:?}");
    }

    #[test]
    fn a_crash_log_with_two_vm_exceptions_fails_when_only_one_is_allowed() {
        let rules = VerdictRules {
            allow: vec![rule(NULL_REASON)],
            ..VerdictRules::default()
        };
        let text = format!(
            "{NULL_VM_CRASH_LOG}------------------------------------\n\
             09.11 2026 10:00:00\n\
             Virtual Machine Exception\n\
             \n\
             Reason: Function 'SetTransform' not linked\n\
             \n\
             Stack trace:\n\
             MFChkB/Scripts/5_Mission/MFChkB.c:9 Function MissionServer\n"
        );
        let verdict = judge(&rules, &[log("crash_2026-11-08_19-21-58.log", &text)]);
        assert!(!verdict.passed, "{verdict:?}");
        assert_eq!(verdict.crash_logs.len(), 1, "{verdict:?}");
    }

    #[test]
    fn the_vanilla_diag_block_is_ignored() {
        let verdict = judge(
            &VerdictRules::default(),
            &[log("diag_2026-11-03.rpt", VANILLA_BLOCK)],
        );
        assert_eq!(verdict.ignored, 1);
        assert_eq!(verdict.other_errors, 0);
        assert!(verdict.mod_errors.is_empty());
    }

    #[test]
    fn the_vanilla_block_is_ignored_even_with_a_mod_frame() {
        let text = VANILLA_BLOCK.replace(
            "    -----------------",
            "    SampleMod/scripts/5_Mission/init.c : 3\n    -----------------",
        );
        let verdict = judge(&rules_with_mods(&["SampleMod"]), &[log("diag.rpt", &text)]);
        assert_eq!(verdict.ignored, 1);
        assert!(verdict.mod_errors.is_empty());
        assert_eq!(verdict.other_errors, 0);
    }

    #[test]
    fn a_mod_frame_above_vanilla_get_plugin_is_a_mod_error() {
        let text = VANILLA_BLOCK.replace(
            "    GetPlugin() scripts/4_World/plugins/pluginmanager.c : 341",
            "    GetPluginByType() SampleMod/scripts/4_World/pluginmanager.c : 9\n 3:45:34.985  SCRIPT    (E):    GetPlugin() scripts/4_World/plugins/pluginmanager.c : 341",
        );
        let verdict = judge(&rules_with_mods(&["SampleMod"]), &[log("diag.rpt", &text)]);
        assert_eq!(verdict.mod_errors.len(), 1, "{verdict:?}");
    }

    #[test]
    fn a_script_error_is_judged_by_the_mod_it_names() {
        #[derive(Debug, PartialEq)]
        enum Outcome {
            ModError,
            Ignored,
            Other,
        }
        let cases: &[(&str, &[&str], Option<&str>, Outcome)] = &[
            (JSON_ERROR_LINE, &["JM/CF"], None, Outcome::ModError),
            (JSON_ERROR_LINE, &[], Some("JSON ERROR"), Outcome::Ignored),
            (JSON_ERROR_LINE, &["SampleMod"], None, Outcome::Other),
            (JSON_ERROR_LINE, &["CF"], None, Outcome::ModError),
            (
                JSON_ERROR_LINE,
                &["JM\\CF\\Scripts\\"],
                None,
                Outcome::ModError,
            ),
            (CFGSOMETHING_LINE, &["CF"], None, Outcome::Other),
        ];
        for (line, mods, allow, expected) in cases {
            let rules = VerdictRules {
                allow: allow.map(|source| vec![rule(source)]).unwrap_or_default(),
                mods: mods.iter().map(|name| name.to_string()).collect(),
                ..VerdictRules::default()
            };
            let verdict = judge(&rules, &[log("server.rpt", line)]);
            let counted = if !verdict.mod_errors.is_empty() {
                Some(Outcome::ModError)
            } else if verdict.ignored > 0 {
                Some(Outcome::Ignored)
            } else if verdict.other_errors > 0 {
                Some(Outcome::Other)
            } else {
                None
            };
            assert_eq!(counted.as_ref(), Some(expected), "{line} with {mods:?}");
            assert_eq!(verdict.passed, *expected != Outcome::ModError, "{line}");
        }
    }

    #[test]
    fn a_mod_stack_trace_carries_its_message_and_frames() {
        let verdict = judge(
            &rules_with_mods(&["SampleMod"]),
            &[log("diag.rpt", MOD_STACK_TRACE)],
        );
        assert_eq!(
            verdict.mod_errors,
            vec!["some mod message\n-- Stack trace --\n\
                  OnCfg() SampleMod/scripts/5_Mission/cfg.c : 12\n-----------------"
                .to_string()]
        );
    }

    #[test]
    fn a_virtual_machine_exception_block_counts_once() {
        let verdict = judge(
            &rules_with_mods(&["AreaAnnouncements"]),
            &[log("diag.rpt", VM_BLOCK)],
        );
        assert_eq!(
            verdict.mod_errors,
            vec![concat!(
                "Virtual Machine Exception\n",
                "Reason: Function 'SetTransform' not linked\n",
                "Class:      'AA_AreaAnnouncementManager'\n",
                "Function: 'ApplyTextStyle'\n",
                "Stack trace:\n",
                "AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1123 Function ApplyTextStyle\n",
                "AreaAnnouncements/scripts/5_Mission/areaannouncements\\aa_areaannouncementmanager.c:1140 Function ApplyTitleTextStyleToAll",
            )
            .to_string()]
        );
        assert_eq!(verdict.other_errors, 0);
    }

    #[test]
    fn a_quoted_attribution_error_is_a_mod_error() {
        let verdict = judge(
            &rules_with_mods(&["MyMod"]),
            &[log(
                "script_2026-11-03.log",
                "03:45:51.567 SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": boom",
            )],
        );
        assert_eq!(
            verdict.mod_errors,
            vec!["@\"MyMod/scripts/3_Game/broken.c,5\": boom".to_string()]
        );
        assert!(!verdict.passed);
    }

    #[test]
    fn a_mod_name_outside_ascii_matches_ignoring_case() {
        let verdict = judge(
            &rules_with_mods(&["Живой"]),
            &[log(
                "server.rpt",
                " 3:45:51.567 SCRIPT    (E): Живой failed; see живой/Scripts/x.c:1",
            )],
        );
        assert_eq!(verdict.mod_errors.len(), 1);
        assert!(!verdict.passed);
    }

    #[test]
    fn an_unterminated_stack_trace_does_not_swallow_the_next_error() {
        let text = concat!(
            " 3:45:34.985  SCRIPT       : a mod message\n",
            " 3:45:34.985  SCRIPT    (E):    -- Stack trace --\n",
            " 3:45:34.985  SCRIPT    (E):    F() Mod/scripts/a.c : 1\n",
            " 3:45:51.567 SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46",
        );
        let verdict = judge(&rules_with_mods(&["JM/CF"]), &[log("server.rpt", text)]);
        assert_eq!(
            verdict.mod_errors,
            vec![
                "JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46".to_string()
            ]
        );
        assert_eq!(verdict.other_errors, 1);
    }

    #[test]
    fn a_trace_takes_its_message_only_from_a_line_of_the_same_time() {
        let text = concat!(
            "03:46:00.100 SCRIPT       : [CF] RPC registered\n",
            "03:46:00.250 SCRIPT    (E):    -- Stack trace --\n",
            "03:46:00.250 SCRIPT    (E):    Foo() SampleMod/scripts/4_World/foo.c : 12\n",
            "03:46:00.250 SCRIPT    (E):    -----------------",
        );
        let rules = VerdictRules {
            allow: vec![rule("[CF]")],
            ..rules_with_mods(&["SampleMod"])
        };
        let verdict = judge(&rules, &[log("server.rpt", text)]);
        assert_eq!(verdict.mod_errors.len(), 1, "{verdict:?}");
    }

    #[test]
    fn a_virtual_machine_block_ends_at_the_next_script_line() {
        let text = concat!(
            " SCRIPT    (E): Virtual Machine Exception\n",
            "Reason: Function 'SetTransform' not linked\n",
            " SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46",
        );
        let verdict = judge(&rules_with_mods(&["JM/CF"]), &[log("script.log", text)]);
        assert_eq!(
            verdict.mod_errors,
            vec![
                "JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46".to_string()
            ]
        );
    }

    #[test]
    fn a_trace_without_its_message_line_in_the_error_log_counts_once() {
        let error_log = concat!(
            "03:45:34.985 GUI (W): some widget warning\n",
            "03:45:34.985 SCRIPT    (E):    -- Stack trace --\n",
            "03:45:34.985 SCRIPT    (E):    Foo() SampleMod/scripts/4_World/foo.c : 12\n",
            "03:45:34.985 SCRIPT    (E):    -----------------",
        );
        let rpt = concat!(
            "03:45:34.985 SCRIPT       : string s = 'broken'\n",
            "03:45:34.985 SCRIPT    (E):    -- Stack trace --\n",
            "03:45:34.985 SCRIPT    (E):    Foo() SampleMod/scripts/4_World/foo.c : 12\n",
            "03:45:34.985 SCRIPT    (E):    -----------------",
        );
        let verdict = judge(
            &rules_with_mods(&["SampleMod"]),
            &[
                log("server.rpt", rpt),
                log("error_2026-11-03.log", error_log),
            ],
        );
        assert_eq!(verdict.mod_errors.len(), 1, "{verdict:?}");
        assert!(verdict.mod_errors[0].starts_with("string s = 'broken'"));
    }

    #[test]
    fn the_same_error_in_two_logs_counts_once() {
        let verdict = judge(
            &rules_with_mods(&["JM/CF"]),
            &[
                log("diag_2026-11-03.rpt", JSON_ERROR_LINE),
                log(
                    "script_2026-11-03_12-00-00.log",
                    " SCRIPT    (E): JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46",
                ),
            ],
        );
        assert_eq!(
            verdict.mod_errors,
            vec![
                "JSON ERROR JM/CF/Scripts/3_Game/communityframework/game/dayzgame.c:46".to_string()
            ]
        );
    }

    #[test]
    fn a_listed_fingerprint_ignores_the_error_but_only_its_own_message_and_file() {
        let quoted =
            " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Can't find variable 'x'";
        let allowed = crate::fingerprint::error_fingerprint(
            "Can't find variable 'x'",
            Some("MyMod/scripts/3_Game/broken.c"),
        );
        let cases: [(&str, bool); 5] = [
            (quoted, true),
            (
                " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,6\": Can't find variable 'x'",
                true,
            ),
            (
                " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,5\": Undefined function 'y'",
                false,
            ),
            (
                " SCRIPT    (E): @\"MyMod/scripts/3_Game/other.c,5\": Can't find variable 'x'",
                false,
            ),
            (" SCRIPT    (E): Virtual Machine Exception", false),
        ];
        for (line, ignored) in cases {
            let rules = VerdictRules {
                allow_fingerprints: vec![allowed.clone()],
                mods: vec!["MyMod".to_owned()],
                ..VerdictRules::default()
            };
            let verdict = judge(&rules, &[log("script.log", line)]);
            assert_eq!(verdict.ignored, usize::from(ignored), "{line}: {verdict:?}");
            assert_eq!(
                verdict.mod_errors.len() + verdict.other_errors,
                usize::from(!ignored),
                "{line}: {verdict:?}"
            );
        }
    }

    #[test]
    fn an_allow_pattern_anchored_to_the_message_skips_the_file_prefix() {
        let rules = VerdictRules {
            allow: vec![LinePattern::parse("re:^Can't find variable").unwrap()],
            mods: vec!["MyMod".to_owned()],
            ..VerdictRules::default()
        };
        let line = " SCRIPT    (E): @\"MyMod/scripts/3_Game/broken.c,6\": Can't find variable 'x'";

        let verdict = judge(&rules, &[log("script.log", line)]);

        assert_eq!(verdict.ignored, 1, "{verdict:?}");
    }

    #[test]
    fn a_generic_error_is_never_ignored_by_a_fingerprint() {
        let generic = crate::fingerprint::error_fingerprint("Virtual Machine Exception", None);
        let rules = VerdictRules {
            allow_fingerprints: vec![generic],
            ..VerdictRules::default()
        };
        let verdict = judge(
            &rules,
            &[log(
                "script.log",
                " SCRIPT    (E): Virtual Machine Exception",
            )],
        );
        assert_eq!(verdict.ignored, 0, "{verdict:?}");
    }

    #[test]
    fn a_listed_fingerprint_changes_neither_a_fail_line_nor_a_missing_expectation() {
        let rules = VerdictRules {
            expect: vec![rule("[SampleModTests] DONE")],
            fail: vec![rule("[SampleModTests] FAIL")],
            allow_fingerprints: vec![crate::fingerprint::fingerprint(
                "SCRIPT (E): [SampleModTests] FAIL",
            )],
            ..VerdictRules::default()
        };
        let verdict = judge(&rules, &[log("script.log", FAILED_LINE)]);
        assert!(!verdict.passed);
        assert_eq!(verdict.fail_lines.len(), 1, "{verdict:?}");
        assert_eq!(verdict.missing.len(), 1, "{verdict:?}");
    }

    #[test]
    fn a_crash_log_clears_only_when_the_allow_matches_the_exception_and_nothing_else_is_in_it() {
        let cases = [
            ("MFChkA", NULL_VM_CRASH_LOG.to_string()),
            (
                NULL_REASON,
                format!(
                    "{NULL_VM_CRASH_LOG}------------------------------------\nEXCEPTION_ACCESS_VIOLATION at 0xdeadbeef\n"
                ),
            ),
            (
                NULL_REASON,
                format!("{NULL_VM_CRASH_LOG}EXCEPTION_ACCESS_VIOLATION at 0x1\n"),
            ),
            (
                NULL_REASON,
                NULL_VM_CRASH_LOG.replace(
                    "Virtual Machine Exception\n",
                    "Virtual Machine Exception handler failed\n",
                ),
            ),
        ];
        for (allow, text) in cases {
            let rules = VerdictRules {
                allow: vec![rule(allow)],
                ..VerdictRules::default()
            };
            let verdict = judge(&rules, &[log("crash_1.log", &text)]);
            assert!(!verdict.passed, "allow {allow}:\n{text}\n{verdict:?}");
        }
    }
}
