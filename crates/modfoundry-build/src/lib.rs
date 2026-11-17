//! ModFoundry deterministic build pipeline.

pub mod build;
pub mod image_rules;
pub mod manifest;
pub mod pack_rules;
pub mod publish;
pub mod strip_comments;

pub use strip_comments::{strip_comments, UnterminatedComment};

pub use build::{
    build_mod, contents, plan_build, remove_unbuilt_addons, BuildOptions, ContentFile, Contents,
    ModMeta, Signing,
};
pub use manifest::BuildReport;
