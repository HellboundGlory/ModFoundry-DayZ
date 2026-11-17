#[path = "src/sdk.rs"]
mod sdk;

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:$ORIGIN/..");
    println!("cargo:rerun-if-env-changed=STEAM_SDK_LOCATION");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("the build folder sits in the profile folder");
    let target = std::env::var("TARGET").expect("cargo sets TARGET");

    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("../../Cargo.lock").display()
    );
    let sdk_root = match std::env::var_os("STEAM_SDK_LOCATION") {
        Some(location) => {
            let location = PathBuf::from(location);
            if location.is_relative() {
                warn(format!(
                    "STEAM_SDK_LOCATION {} is relative; steamworks-sys reads it from its own folder, so give an absolute path",
                    location.display()
                ));
                return;
            }
            location
        }
        None => match cargo_metadata(&manifest_dir, &target)
            .and_then(|metadata| sdk::steamworks_sys_dir(&metadata).map_err(warn).ok())
        {
            Some(dir) => dir.join("lib/steam"),
            None => return,
        },
    };
    let library = sdk::sdk_library(&sdk_root, &target);
    if !library.is_file() {
        println!(
            "cargo:warning=libsteam_api.so not found at {}; the Steam helper will not start",
            library.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", library.display());
    let copy = profile_dir.join("libsteam_api.so");
    if let Err(error) = std::fs::copy(&library, &copy) {
        println!(
            "cargo:warning=could not copy {} to {}: {error}",
            library.display(),
            copy.display()
        );
    }
}

fn cargo_metadata(manifest_dir: &Path, target: &str) -> Option<String> {
    let cargo = std::env::var_os("CARGO")
        .ok_or_else(|| "cargo did not set CARGO".to_owned())
        .map_err(warn)
        .ok()?;
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
            target,
        ])
        .current_dir(manifest_dir)
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))
        .and_then(|output| {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let first = stderr
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("no message");
                return Err(format!("cargo metadata failed: {first}"));
            }
            String::from_utf8(output.stdout)
                .map_err(|e| format!("could not read cargo metadata: {e}"))
        });
    output.map_err(warn).ok()
}

fn warn(message: String) -> String {
    println!("cargo:warning={message}");
    message
}
