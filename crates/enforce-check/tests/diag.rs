use enforce_check::{CheckResult, Diagnostic, Module, Severity};

fn plain(message: &str, line: u32) -> Diagnostic {
    Diagnostic {
        file: "Scripts/3_Game/probe.c".into(),
        line,
        module: Module::Game,
        severity: Severity::Error,
        message: message.into(),
        fixme: false,
        game_crash: false,
        reports_only: false,
        related: None,
    }
}

#[test]
fn game_line_format() {
    let cases = [
        (
            Diagnostic {
                severity: Severity::Error,
                ..plain("Multiple declaration of class 'ProbeDup'", 1)
            },
            "@\"Scripts/3_Game/probe.c,1\": Multiple declaration of class 'ProbeDup'",
        ),
        (
            Diagnostic {
                fixme: true,
                severity: Severity::Warning,
                ..plain(
                    "Unsafe down-casting, use 'PrB.Cast' for safe down-casting",
                    12,
                )
            },
            "@\"Scripts/3_Game/probe.c,12\": FIX-ME: Unsafe down-casting, use 'PrB.Cast' for safe down-casting",
        ),
        (
            Diagnostic {
                severity: Severity::Warning,
                ..plain("Forward declarations are deprecated!", 1)
            },
            "@\"Scripts/3_Game/probe.c,1\": Forward declarations are deprecated!",
        ),
        (
            Diagnostic {
                game_crash: true,
                ..plain("Can't find variable 'b'", 3)
            },
            "@\"Scripts/3_Game/probe.c,3\": Can't find variable 'b' (the game crashes on this input)",
        ),
        (
            Diagnostic {
                fixme: true,
                game_crash: true,
                severity: Severity::Warning,
                ..plain("Invalid identifier 'local'", 4)
            },
            "@\"Scripts/3_Game/probe.c,4\": FIX-ME: Invalid identifier 'local' (the game crashes on this input)",
        ),
    ];
    for (diagnostic, expected) in cases {
        assert_eq!(diagnostic.to_game_line(), expected);
    }
}

#[test]
fn game_text_appends_a_line_per_failed_module() {
    let result = CheckResult {
        diagnostics: vec![plain("Multiple declaration of class 'ProbeDup'", 1)],
        failed_modules: vec![Module::Game, Module::World],
    };
    assert_eq!(
        result.to_game_text(),
        "@\"Scripts/3_Game/probe.c,1\": Multiple declaration of class 'ProbeDup'\n\
         Can't compile \"Game\" script module!\n\
         Can't compile \"World\" script module!"
    );
    let empty = CheckResult {
        diagnostics: Vec::new(),
        failed_modules: Vec::new(),
    };
    assert_eq!(empty.to_game_text(), "");
}

#[test]
fn diagnostic_json_round_trip() {
    let diagnostic = Diagnostic {
        related: Some(("Scripts/3_Game/a.c".into(), 1)),
        ..plain("Multiple declaration of class 'ProbeDup'", 1)
    };
    let json = serde_json::to_string(&diagnostic).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut keys: Vec<&str> = parsed
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "file",
            "fixme",
            "game_crash",
            "line",
            "message",
            "module",
            "related",
            "severity"
        ]
    );
    let back: Diagnostic = serde_json::from_str(&json).unwrap();
    assert_eq!(back, diagnostic);
}

#[test]
fn related_location_prints_as_its_own_line() {
    let result = CheckResult {
        diagnostics: vec![Diagnostic {
            related: Some(("Scripts/3_Game/a.c".into(), 1)),
            ..plain("Multiple declaration of class 'ProbeDup'", 1)
        }],
        failed_modules: vec![Module::Game],
    };
    assert_eq!(
        result.to_game_text(),
        "@\"Scripts/3_Game/probe.c,1\": Multiple declaration of class 'ProbeDup'\n\
         @\"Scripts/3_Game/a.c,1\": Previously declared here\n\
         Can't compile \"Game\" script module!"
    );
}
