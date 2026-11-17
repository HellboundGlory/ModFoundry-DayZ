//! One game build's three models as a saved file.

use serde::{Deserialize, Serialize};

use crate::config::ConfigModel;
use crate::layout::LayoutModel;
use crate::script::ScriptModel;

/// The snapshot format this mf writes and the newest it reads.
pub const SNAPSHOT_SCHEMA: u32 = 1;

/// Every model of one game build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub build: String,
    pub scripts: ScriptModel,
    pub configs: ConfigModel,
    pub layouts: LayoutModel,
}

impl Snapshot {
    /// A snapshot of `build` in the format this mf writes.
    pub fn new(
        build: String,
        scripts: ScriptModel,
        configs: ConfigModel,
        layouts: LayoutModel,
    ) -> Self {
        Snapshot {
            schema: SNAPSHOT_SCHEMA,
            build,
            scripts,
            configs,
            layouts,
        }
    }

    /// The compact JSON form, fields in struct order.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|err| err.to_string())
    }

    /// A snapshot from its JSON form, after checking the format number.
    pub fn from_json(text: &str) -> Result<Snapshot, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .ok_or("not a game snapshot")?;
        if schema > SNAPSHOT_SCHEMA.into() {
            return Err(format!(
                "snapshot schema {schema} is newer than this mf reads ({SNAPSHOT_SCHEMA})"
            ));
        }
        serde_json::from_value(value).map_err(|err| err.to_string())
    }
}
