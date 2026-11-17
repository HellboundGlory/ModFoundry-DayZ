//! One integration-test binary per crate: a single link, and every test runs in parallel.

mod common;
mod handler;
mod policy;
mod surface;
mod transport;
mod update;
mod update_notice;
