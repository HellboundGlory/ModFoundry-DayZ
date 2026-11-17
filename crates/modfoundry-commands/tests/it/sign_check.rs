//! `sign.check` through the registry: a build's signature findings, with the server's keys.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{self, pack, tmp};
use modfoundry_core::{Actor, CommandError, Outcome};
use modfoundry_sign::{
    bisign_name, generate_keypair, public_key_name, sign_pbo_bytes, KeyBlob, SignVersion, KEY_BITS,
};
use serde_json::{json, Value};

const PAYLOAD: &[u8] = b"class CfgPatches {};";

fn diagnostics(outcome: &Outcome) -> Vec<Value> {
    let Outcome::Data(value) = outcome else {
        panic!("expected data: {outcome:?}");
    };
    value["diagnostics"].as_array().cloned().unwrap_or_default()
}

fn codes(diagnostics: &[Value]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap())
        .collect()
}

/// A `Mod` PBO signed by an `MF` keypair, with both keys under the mod's `keys/`.
fn signed_build(dir: &Path) -> (PathBuf, KeyBlob) {
    let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
    let mod_dir = dir.join("@Mod");
    let pbo = mod_dir.join("addons/Mod.pbo");
    pack(&pbo, &[("prefix", "Mod")], &[("config.cpp", PAYLOAD)]);
    let bisign = sign_pbo_bytes(
        &private.to_bytes(),
        &fs::read(&pbo).unwrap(),
        SignVersion::V3,
    )
    .unwrap();
    fs::create_dir_all(mod_dir.join("keys")).unwrap();
    fs::write(
        mod_dir.join("addons").join(bisign_name("Mod.pbo", "MF")),
        bisign,
    )
    .unwrap();
    fs::write(
        mod_dir.join("keys").join(public_key_name("MF")),
        public.to_bytes(),
    )
    .unwrap();
    (mod_dir, public)
}

#[test]
fn a_signed_build_and_the_servers_key_have_no_findings() {
    let dir = tmp("sign-check-clean");
    let (mod_dir, public) = signed_build(&dir);
    let server = dir.join("server");
    fs::create_dir_all(server.join("keys")).unwrap();
    fs::write(
        server.join("keys").join(public_key_name("MF")),
        public.to_bytes(),
    )
    .unwrap();

    let outcome = common::core()
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": mod_dir, "server_dir": server }),
        )
        .unwrap();

    assert!(diagnostics(&outcome).is_empty(), "{outcome:?}");
}

#[test]
fn a_different_key_on_the_server_is_reported() {
    let dir = tmp("sign-check-server-key");
    let (mod_dir, _) = signed_build(&dir);
    let server = dir.join("server");
    let (_, other) = generate_keypair("MF", KEY_BITS).unwrap();
    fs::create_dir_all(server.join("keys")).unwrap();
    fs::write(
        server.join("keys").join(public_key_name("MF")),
        other.to_bytes(),
    )
    .unwrap();

    let outcome = common::core()
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": mod_dir, "server_dir": server }),
        )
        .unwrap();

    assert_eq!(codes(&diagnostics(&outcome)), ["MF-P017"]);
    assert_eq!(diagnostics(&outcome)[0]["severity"], json!("error"));
}

#[test]
fn findings_come_from_structure_then_signatures_then_the_server() {
    let dir = tmp("sign-check-order");
    let (mod_dir, _) = signed_build(&dir);
    pack(
        &mod_dir.join("addons/Other.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", PAYLOAD)],
    );
    common::write(&mod_dir, "bin/x.dll", "MZ");
    fs::copy(
        mod_dir.join("addons/Mod.pbo.MF.bisign"),
        mod_dir.join("addons/Mod.pbo.Other.bisign"),
    )
    .unwrap();
    fs::create_dir_all(dir.join("server")).unwrap();

    let outcome = common::core()
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": mod_dir, "server_dir": dir.join("server") }),
        )
        .unwrap();

    assert_eq!(
        codes(&diagnostics(&outcome)),
        ["MF-P007", "MF-P008", "MF-P017"]
    );
}

#[test]
fn an_unsigned_pbo_is_reported() {
    let dir = tmp("sign-check-unsigned");
    pack(
        &dir.join("@Mod/addons/Mod.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", PAYLOAD)],
    );

    let outcome = common::core()
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": dir.join("@Mod") }),
        )
        .unwrap();

    assert_eq!(codes(&diagnostics(&outcome)), ["MF-P007"]);
}

#[test]
fn a_mod_dir_that_is_not_there_is_bad_arguments() {
    let err = common::core()
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": tmp("sign-check-missing") }),
        )
        .unwrap_err();

    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
}

#[test]
fn a_folder_with_no_pbo_is_a_finding_and_a_missing_server_is_refused() {
    let dir = tmp("sign-check-empty");
    fs::create_dir_all(&dir).unwrap();
    let core = common::core();
    let outcome = core
        .dispatch(Actor::CliSession, "sign.check", json!({ "mod_dir": dir }))
        .unwrap();
    assert!(codes(&diagnostics(&outcome)).contains(&"MF-P001"));

    let root = tmp("sign-check-project-root");
    pack(
        &root.join("builds/default/@Mod/addons/Mod.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", PAYLOAD)],
    );
    let outcome = core
        .dispatch(Actor::CliSession, "sign.check", json!({ "mod_dir": root }))
        .unwrap();
    assert!(codes(&diagnostics(&outcome)).contains(&"MF-P002"));

    let err = core
        .dispatch(
            Actor::CliSession,
            "sign.check",
            json!({ "mod_dir": dir, "server_dir": dir.join("nope") }),
        )
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidArguments(_)), "{err}");
}
