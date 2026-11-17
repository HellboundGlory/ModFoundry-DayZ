//! One integration-test binary per crate: a single link, and every test runs in parallel.

mod common;
mod complete;
mod corpus;
mod extract;
mod index;
mod infer;
mod layer;
mod lints;
mod outline;
mod resolve;
