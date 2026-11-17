//! config.cpp lexer/parser/model/validator.
//!
//! DayZ configs are a C-like superset with no dedicated tree-sitter grammar, so
//! ModFoundry reimplements the front-end in Rust. The crate is split so the
//! lexer/AST can be reused for other config families (RVMat, CE XML-adjacent configs):
//!
//! - [`token`] — span-carrying lexer for the C-like superset.
//! - [`highlight`] — pure token-kind spans for editor syntax colouring.
//! - [`ast`] — generic parse tree (class/property/delete; DayZ-agnostic).
//! - [`parser`] — recursive-descent parser with error recovery → `MF-C1xx`.
//! - [`model`] — DayZ view: CfgPatches addons, CfgMods + `defs` script modules.
//! - [`rap`] — config.bin (RaP) reader/writer and CfgConvert-compatible text.
//! - [`validate`] — DayZ-specific diagnostics → `MF-C*` (module binding etc.).
//!
//! Parser, AST and model are pure (no IO) so they run in tests/CI/WASM later;
//! the only IO is the validator's optional on-disk existence check, which takes
//! an explicit source root.

pub mod ast;
pub mod display;
pub mod edit;
pub mod fixes;
pub mod highlight;
pub mod lints;
pub mod model;
pub mod outline;
pub mod parser;
pub mod rap;
pub mod resolve;
pub mod scaffold;
pub mod schema;
pub mod token;
pub mod validate;

pub use display::mod_display_name;
pub use fixes::{add_required_addons, fixable_codes, fixes_for, Fix, FixCheck, FixFacts};
pub use model::{DayZConfig, ModConfig, ModuleSlot, PatchesEntry, ScriptModule};
pub use modfoundry_core::{Diagnostic, Severity, Span};
pub use rap::{read_rap, to_text, write_rap, RapError};
pub use scaffold::{
    is_safe_name, is_safe_prefix, mod_skeleton, FileToWrite, ScaffoldOptions, SCRIPT_MODULE_DIRS,
};
pub use validate::{validate_text, Validation};
