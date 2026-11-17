use serde_json::Value;
use std::path::{Path, PathBuf};

/// The SDK library `steamworks-sys` links for `target`.
pub fn sdk_library(sdk_root: &Path, target: &str) -> PathBuf {
    let folder = if target.contains("i686") {
        "linux32"
    } else if target.contains("aarch64") {
        "linuxarm64"
    } else {
        "linux64"
    };
    sdk_root
        .join("redistributable_bin")
        .join(folder)
        .join("libsteam_api.so")
}

/// The folder of the `steamworks-sys` package's manifest, from `cargo metadata` output.
pub fn steamworks_sys_dir(metadata: &str) -> Result<PathBuf, String> {
    let parsed: Value = serde_json::from_str(metadata)
        .map_err(|e| format!("could not read the cargo metadata: {e}"))?;
    let found = parsed["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|package| package["name"] == "steamworks-sys")
        .collect::<Vec<_>>();
    match found.as_slice() {
        [package] => package["manifest_path"]
            .as_str()
            .and_then(|path| Path::new(path).parent())
            .map(Path::to_path_buf)
            .ok_or_else(|| "the steamworks-sys package has no manifest path".to_owned()),
        [] => Err("no steamworks-sys package in the cargo metadata".to_owned()),
        packages => Err(format!(
            "several steamworks-sys packages in the cargo metadata: {}",
            packages
                .iter()
                .filter_map(|package| package["version"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &str = r#"{
        "workspace_root": "/work",
        "packages": [
            {
                "name": "steamworks",
                "version": "0.13.1",
                "manifest_path": "/crates/steamworks-0.13.1/Cargo.toml"
            },
            {
                "name": "steamworks-sys",
                "version": "0.13.0",
                "manifest_path": "/crates/steamworks-sys-0.13.0/Cargo.toml"
            },
            {
                "name": "steamworks-sys-helper",
                "version": "1.0.0",
                "manifest_path": "/crates/steamworks-sys-helper-1.0.0/Cargo.toml"
            }
        ]
    }"#;

    #[test]
    fn sdk_library_picks_the_folder_for_the_target() {
        let root = Path::new("/sdk");
        let cases = [
            ("x86_64-unknown-linux-gnu", "linux64"),
            ("i686-unknown-linux-gnu", "linux32"),
            ("aarch64-unknown-linux-gnu", "linuxarm64"),
        ];
        for (target, folder) in cases {
            assert_eq!(
                sdk_library(root, target),
                root.join("redistributable_bin")
                    .join(folder)
                    .join("libsteam_api.so"),
                "{target}"
            );
        }
    }

    #[test]
    fn steamworks_sys_dir_finds_the_one_package_beside_lookalikes() {
        assert_eq!(
            steamworks_sys_dir(METADATA),
            Ok(PathBuf::from("/crates/steamworks-sys-0.13.0"))
        );
    }

    #[test]
    fn steamworks_sys_dir_names_why_it_cannot_pick_one() {
        let cases = [
            (
                r#"{"workspace_root": "/work", "packages": [{"name": "steamworks", "version": "0.13.1", "manifest_path": "/crates/steamworks/Cargo.toml"}]}"#,
                "no steamworks-sys package",
            ),
            (
                r#"{"workspace_root": "/work", "packages": [
                    {"name": "steamworks-sys", "version": "0.13.0", "manifest_path": "/crates/steamworks-sys-0.13.0/Cargo.toml"},
                    {"name": "steamworks-sys", "version": "0.12.0", "manifest_path": "/crates/steamworks-sys-0.12.0/Cargo.toml"}
                ]}"#,
                "several steamworks-sys packages",
            ),
            ("not json", "cargo metadata"),
        ];
        for (metadata, expected) in cases {
            let error = steamworks_sys_dir(metadata).unwrap_err();
            assert!(error.contains(expected), "{metadata}: {error}");
        }
    }
}
