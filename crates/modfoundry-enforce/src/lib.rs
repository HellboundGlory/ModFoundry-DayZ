//! Enforce Script analysis front-end.
//!
//! DayZ script sources (`.c`) are parsed with the MIT `tree-sitter-enforce`
//! grammar, producing syntax findings and a
//! module-bound check — a file outside the standard `Scripts/N_*` and
//! `Scripts/editor` folders compiles only when config.cpp `CfgMods` lists it,
//! so it is flagged early.
//!
//! - [`detect`] — pure path classification: is a file Enforce, and is it
//!   inside a canonical loadable script module dir.
//! - [`parse`] — pure syntax analysis over a source string (tree-sitter):
//!   [`parse_tree`] builds the tree, [`syntax_diagnostics`] walks it.
//! - [`highlight`] — pure syntax spans for editor rendering.
//! - [`lint`] — IO at the boundary: walk a dir / read a file and return
//!   diagnostics; [`read_source`] is the lossy reader.
//!
//! Diagnostics use the shared core [`Diagnostic`] shape (`MF-E*` and `MF-W*` codes).

pub mod detect;
pub mod highlight;
pub mod lint;
pub mod locals;
pub mod loops;
pub mod parse;
pub mod preproc;
pub mod returns;

pub use detect::{classify, FileClass};
pub use highlight::{highlight, HighlightKind, HighlightSpan};
pub use lint::{lint_target, lint_target_with, lint_text, read_source, Finding};
pub use modfoundry_core::{Diagnostic, Severity, Span};
pub use parse::{parse_tree, syntax_diagnostics};
pub use preproc::{first_arms, guards, nested_arm_views, other_arms, Guard};
pub use tree_sitter::Tree;
