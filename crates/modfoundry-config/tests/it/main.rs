//! One integration-test binary per crate: a single link, and every test runs in parallel.

mod bom;
mod common;
mod edit_corpus;
mod edit_errors;
mod edit_extra_ops;
mod edit_fixtures;
mod edit_ops;
mod edit_roundtrip;
mod edit_style;
mod fixes;
mod highlight;
mod lints_corpus;
mod lints_index;
mod lints_index_corpus;
mod rap_fixtures;
mod resolve;
mod resolve_corpus;
mod scaffold;
mod schema_corpus;
mod spans_corpus;
mod validate;
