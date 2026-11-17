//! One integration-test binary per crate: a single link, and every test runs in parallel.

mod classes;
mod common;
mod economy;
mod economy_corpus;
mod edit_attrs;
mod edit_elements;
mod fixes;
mod kind_corpus;
mod lints_corpus;
mod lints_schema;
mod lints_small_kinds;
mod lints_types;
mod model;
mod ops_containers;
mod ops_corpus;
mod ops_fields;
mod ops_lists;
mod ops_nested;
mod ops_records;
mod ops_small_kinds;
mod record_files_corpus;
mod schema_corpus;
mod xml_corpus;
mod xml_recovery;
