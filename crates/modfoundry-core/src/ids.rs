//! Typed identifiers for ModFoundry domain objects.
//!
//! Newtypes (not raw strings/UUIDs) so the Rust type system prevents mixing up a
//! mod id with a build-profile id at every callsite the GUI/CLI/MCP share.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

id_newtype!(
    /// Identifies a ModFoundry workspace (a collection of projects + settings on disk).
    WorkspaceId
);

id_newtype!(
    /// Identifies a single DayZ mod project inside a workspace.
    ModId
);

id_newtype!(
    /// Identifies a build profile (pack/sign/publish configuration set).
    BuildProfileId
);
