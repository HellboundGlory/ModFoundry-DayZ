#![allow(dead_code)]

use enforce_check::{CheckInput, Diagnostic, Layer, ModInfo, SourceFile, Target, check};

pub const PROBE: &str = "Scripts/3_Game/probe.c";

// A stand-in for what `1_Core` declares: the roots, the containers, the value
// types and `Print`.
pub const ENGINE: &str = "Scripts/1_Core/proto/engine.c";
pub const ENGINE_TEXT: &str = "\
class Class\n{\n\tproto native string ClassName();\n}\n\
class Managed\n{\n}\n\
class array<Class T>\n{\n\tproto native int Count();\n\tproto T Get(int n);\n\tproto int Insert(T value);\n}\n\
class set<Class T>\n{\n}\n\
class map<Class K, Class V>\n{\n}\n\
class int\n{\n\tproto string ToString();\n}\n\
class float\n{\n}\n\
class bool\n{\n}\n\
class string\n{\n\tproto native int Length();\n}\n\
class vector\n{\n\tstatic const vector Zero = \"0 0 0\";\n}\n\
class typename\n{\n}\n\
class ScriptCallQueue\n{\n\tproto void CallLater(func fn, int delay = 0, bool repeat = false, void param1 = NULL);\n}\n\
proto void Print(void var);\n";

pub fn input(target: Target, files: &[(&str, &str)]) -> CheckInput {
    CheckInput {
        target,
        mods: vec![ModInfo {
            name: "probe".into(),
            cfg_class: "Probe_body".into(),
        }],
        files: files
            .iter()
            .map(|(path, text)| {
                let folder = path.split('/').nth(1).unwrap_or("3_Game");
                let layer = Layer::from_folder_name(folder).unwrap_or(Layer::Game);
                SourceFile {
                    path: (*path).into(),
                    layer,
                    mod_index: match layer {
                        Layer::Core => 0,
                        _ => 1,
                    },
                    text: (*text).into(),
                }
            })
            .collect(),
        extra_defines: Vec::new(),
        input_names: Vec::new(),
    }
}

// Joins source pieces into a row's text, which a row holds for the whole run.
pub fn text(parts: &[&str]) -> &'static str {
    Box::leak(parts.concat().into_boxed_str())
}

pub fn diagnostics(target: Target, text: &str) -> Vec<Diagnostic> {
    check(&input(target, &[(ENGINE, ENGINE_TEXT), (PROBE, text)])).diagnostics
}

// Each line the checker printed for the probe, as the game would: a `FIX-ME: `
// prefix marks a rule the retail targets only warn about.
pub fn lines(target: Target, text: &str) -> Vec<(u32, String)> {
    diagnostics(target, text)
        .iter()
        .map(|diagnostic| {
            let prefix = if diagnostic.fixme { "FIX-ME: " } else { "" };
            (diagnostic.line, format!("{prefix}{}", diagnostic.message))
        })
        .collect()
}

pub struct Row {
    pub id: &'static str,
    pub text: &'static str,
    pub expected: &'static [(u32, &'static str)],
}

pub fn row(id: &'static str, text: &'static str, expected: &'static [(u32, &'static str)]) -> Row {
    Row { id, text, expected }
}

// Every row prints exactly its expected lines; the failure lists the rows that
// did not.
pub fn assert_rows(target: Target, rows: &[Row]) {
    let mut failures = Vec::new();
    for row in rows {
        let got = lines(target, row.text);
        let expected: Vec<(u32, String)> = row
            .expected
            .iter()
            .map(|(line, message)| (*line, (*message).to_string()))
            .collect();
        if got != expected {
            failures.push(format!("{}: got {got:?}, want {expected:?}", row.id));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
