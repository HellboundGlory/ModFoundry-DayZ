//! Central Economy XML support: a lossless parser, a schema-driven record model,
//! an edit engine, and `MF-X*` diagnostics.

pub mod classes;
pub mod display;
pub mod economy;
pub mod edit;
pub mod fixes;
pub mod highlight;
pub mod kind;
pub mod lints;
pub mod model;
pub mod ops;
pub mod schema;
pub mod xml;

pub use modfoundry_core::{Diagnostic, Severity, Span};
