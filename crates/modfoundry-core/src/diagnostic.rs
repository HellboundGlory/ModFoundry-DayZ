//! Shared diagnostic model.
//!
//! Every ModFoundry analyzer emits [`Diagnostic`]s shaped identically, so GUI
//! editors, CLI output, MCP tools and automation render/route one vocabulary.
//! Diagnostics carry a namespaced `MF-*` code, a severity tier, an optional
//! source range and a human message. Codes are owned per subsystem.
//!
//! This crate intentionally defines the *shape* only; subsystems add codes.

use serde::{Deserialize, Serialize};

/// Severity tiers, mirroring the DayZ/engine and LSP conventions ModFoundry
/// presents in every frontend. Serialized in lowercase (`"error"`), the
/// vocabulary every frontend and the MCP wire already use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The mildest tier: a suggestion, below a warning.
    Hint,
    Warning,
    Error,
    /// A parse or analysis failure that made the rest of the unit unreliable.
    Fatal,
}

/// Zero-based byte range into the analyzed source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A single analyzer finding. Serializable so it can cross the MCP boundary and
/// be stored for later GUI/MCP querying without a second representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Namespaced code, e.g. `MF-C101` or `MF-E1001`.
    pub code: String,
    pub severity: Severity,
    /// Optional source range. `None` = applies to the whole unit/file.
    pub span: Option<Span>,
    /// Frontend-ready message; one line, specific, action-oriented.
    pub message: String,
}

impl Diagnostic {
    pub fn new(code: impl Into<String>, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity,
            span: None,
            message: message.into(),
        }
    }

    pub fn at(
        code: impl Into<String>,
        severity: Severity,
        span: Span,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            span: Some(span),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_hint_lowest() {
        assert!(Severity::Hint < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Fatal);
    }
}
