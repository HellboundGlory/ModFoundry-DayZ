//! Enforce Script semantics: per-file declaration extraction, the global symbol
//! index and completion inference.

pub mod complete;
pub mod extract;
pub mod index;
pub mod infer;
pub mod items;
mod keywords;
pub mod layer;
pub mod lints;
pub mod outline;
pub mod resolve;

pub use complete::{complete_at, Completion, CompletionItem, CompletionKind};
pub use extract::extract;
pub use index::{ClassLink, FileKey, Index, IndexStats, Location, MemberDef, MemberKind, Origin};
pub use infer::{infer_receiver, Receiver};
pub use items::{
    ClassItem, EnumItem, EnumMember, FieldItem, FileItems, MethodItem, ParamItem, TypeRef,
    TypedefItem,
};
pub use layer::Layer;
pub use lints::semantic_diagnostics;
pub use outline::{outline, FileOutline};
pub use resolve::{resolve_at, Resolution, SymbolInfo, SymbolKind};
