use serde::{Deserialize, Serialize};

use crate::input::Module;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub module: Module,
    pub severity: Severity,
    pub message: String,
    pub fixme: bool,
    pub game_crash: bool,
    // Reported, then the module keeps loading: the destructor bookkeeping. It is
    // how this checker models the module, not a field of the game's output.
    #[serde(skip)]
    pub reports_only: bool,
    pub related: Option<(String, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    pub failed_modules: Vec<Module>,
}

impl Diagnostic {
    pub fn to_game_line(&self) -> String {
        let mut line = format!("@\"{},{}\": ", self.file, self.line);
        if self.fixme {
            line.push_str("FIX-ME: ");
        }
        line.push_str(&self.message);
        if self.game_crash {
            line.push_str(" (the game crashes on this input)");
        }
        line
    }
}

impl CheckResult {
    pub fn to_game_text(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for diagnostic in &self.diagnostics {
            lines.push(diagnostic.to_game_line());
            if let Some((file, line)) = &diagnostic.related {
                lines.push(format!("@\"{file},{line}\": Previously declared here"));
            }
        }
        lines.extend(
            self.failed_modules
                .iter()
                .map(|module| format!("Can't compile \"{}\" script module!", module.as_str())),
        );
        lines.join("\n")
    }
}
