//! File keys shared by the script integration tests.

use modfoundry_script::{FileKey, Layer, Origin};

pub fn key(id: &str, origin: Origin, layer: Layer) -> FileKey {
    FileKey {
        id: id.to_owned(),
        origin,
        layer: Some(layer),
    }
}

pub fn vanilla_core_key() -> FileKey {
    key("vanilla/1_Core/core.c", Origin::Vanilla, Layer::Core)
}

pub fn dependency_key() -> FileKey {
    key(
        "dependency/4_World/playerbase.c",
        Origin::Dependency(0),
        Layer::World,
    )
}

pub fn workspace_key() -> FileKey {
    key(
        "workspace/4_World/playerbase.c",
        Origin::Workspace,
        Layer::World,
    )
}

pub fn mission_key() -> FileKey {
    key("workspace/5_Mission/m.c", Origin::Workspace, Layer::Mission)
}
