use modfoundry_core::{Diagnostic, Severity};
use modfoundry_sign::{public_key_name, verify, Bisign, KeyBlob, PboDigest};
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Extensions Steam holds an item for content review over.
pub const REVIEW_EXTENSIONS: [&str; 10] = [
    "exe", "dll", "cmd", "com", "bat", "py", "ps", "jar", "lib", "src",
];

/// Layout findings for a build's relative file list, in Publisher's order.
pub fn structure_checks(files: &[String]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let unsigned = unsigned_pbos(files);
    if !files.iter().any(|file| has_extension(file, "pbo")) {
        diagnostics.push(Diagnostic::new(
            "MF-P001",
            Severity::Error,
            "the build has no PBO; there is nothing to publish",
        ));
    }
    if !files.iter().any(|file| in_addons_folder(file)) {
        diagnostics.push(Diagnostic::new(
            "MF-P002",
            Severity::Error,
            "the build has no top-level addons folder",
        ));
    }
    for file in files {
        if has_extension(file, "pbo") && !is_top_level_addon(file) {
            diagnostics.push(Diagnostic::new(
                "MF-P003",
                Severity::Warning,
                format!("{file} is outside the top-level addons folder; the game does not load it"),
            ));
        }
        if unsigned.contains(file) {
            diagnostics.push(Diagnostic::new(
                "MF-P007",
                Severity::Error,
                format!("{file} is not signed; servers refuse unsigned mods. Pick or create a key in Settings › Signing"),
            ));
        }
        if let Some(extension) = review_extension(file) {
            diagnostics.push(Diagnostic::new(
                "MF-P005",
                Severity::Warning,
                format!("{file} is a .{extension} file; Steam holds the item for content review"),
            ));
        }
    }
    diagnostics
}

/// The PBOs in the top-level addons folder with no `.bisign` beside them, in
/// list order; names compare in any case, as the engine reads them.
pub fn unsigned_pbos(files: &[String]) -> Vec<String> {
    let signed: HashSet<String> = files
        .iter()
        .filter(|file| has_extension(file, "bisign"))
        .filter_map(|file| pbo_beside(file, files))
        .map(|pbo| pbo.to_ascii_lowercase())
        .collect();
    files
        .iter()
        .filter(|file| has_extension(file, "pbo") && is_top_level_addon(file))
        .filter(|file| !signed.contains(&file.to_ascii_lowercase()))
        .cloned()
        .collect()
}

pub(super) fn has_extension(file: &str, extension: &str) -> bool {
    Path::new(file)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn in_addons_folder(file: &str) -> bool {
    file.split_once('/')
        .is_some_and(|(first, _)| first.eq_ignore_ascii_case("addons"))
}

fn is_top_level_addon(file: &str) -> bool {
    in_addons_folder(file) && file.matches('/').count() == 1
}

fn review_extension(file: &str) -> Option<String> {
    let extension = Path::new(file).extension()?.to_str()?.to_ascii_lowercase();
    REVIEW_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(extension)
}

/// Every file under `mod_dir` as a `/`-separated path relative to it, sorted.
pub fn build_files(mod_dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(mod_dir, mod_dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<String>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(root, &path, files)?;
        } else if !(kind.is_symlink() && path.is_dir()) {
            let relative = path.strip_prefix(root)?;
            files.push(
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    Ok(())
}

/// Signature findings for each `.bisign` in the build, then one for each PBO signed more than once.
pub fn signature_checks(mod_dir: &Path) -> anyhow::Result<Vec<Diagnostic>> {
    let files = build_files(mod_dir)?;
    let bisigns: Vec<&String> = files
        .iter()
        .filter(|file| has_extension(file, "bisign"))
        .collect();
    let mut diagnostics = Vec::new();
    for &file in &bisigns {
        diagnostics.extend(signature_check(mod_dir, file, &files)?);
    }
    diagnostics.extend(duplicate_signatures(&bisigns, &files));
    Ok(diagnostics)
}

/// One MF-P008 per PBO signed more than once, in the order the PBOs first appear.
fn duplicate_signatures(bisigns: &[&String], files: &[String]) -> Vec<Diagnostic> {
    let mut groups: Vec<(String, Vec<&String>)> = Vec::new();
    for &file in bisigns {
        let Some(pbo) = pbo_beside(file, files) else {
            continue;
        };
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.eq_ignore_ascii_case(&pbo))
        {
            Some((_, group)) => group.push(file),
            None => groups.push((pbo, vec![file])),
        }
    }
    groups
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .map(|(pbo, group)| {
            let names: Vec<&str> = group.iter().map(|file| file.as_str()).collect();
            Diagnostic::new(
                "MF-P008",
                Severity::Error,
                format!(
                    "{pbo} has {} signatures ({}); keep the one for your key",
                    group.len(),
                    names.join(", ")
                ),
            )
        })
        .collect()
}

fn signature_check(
    mod_dir: &Path,
    file: &str,
    files: &[String],
) -> anyhow::Result<Option<Diagnostic>> {
    let bytes = match fs::read(mod_dir.join(file)) {
        Ok(bytes) => bytes,
        Err(e) => return Ok(Some(signature_error(format!("{file} cannot be read: {e}")))),
    };
    let bisign = match Bisign::parse(&bytes) {
        Ok(bisign) => bisign,
        Err(e) => {
            return Ok(Some(signature_error(format!(
                "{file} is not a valid signature: {e}"
            ))))
        }
    };
    let beside = pbo_beside(file, files);
    let found = beside.and_then(|pbo| files.iter().find(|path| path.eq_ignore_ascii_case(&pbo)));
    let Some(pbo) = found else {
        return Ok(Some(signature_error(format!(
            "{file} has no PBO beside it"
        ))));
    };
    let pbo_bytes = match fs::read(mod_dir.join(pbo)) {
        Ok(bytes) => bytes,
        Err(e) => return Ok(Some(signature_error(format!("{pbo} cannot be read: {e}")))),
    };
    let verified = PboDigest::from_pbo_bytes(&pbo_bytes)
        .and_then(|digest| verify(&bisign.key, &digest, &bisign));
    if verified.is_err() || !trailer_matches(&pbo_bytes) {
        return Ok(Some(signature_error(format!(
            "{file} does not match {pbo}; sign the build again"
        ))));
    }
    let key = format!("keys/{}", public_key_name(&bisign.key.authority));
    let Some(installed) = files.iter().find(|path| path.eq_ignore_ascii_case(&key)) else {
        return Ok(Some(Diagnostic::new(
            "MF-P006",
            Severity::Warning,
            format!("no {key} for {file}; servers cannot check this signature"),
        )));
    };
    let same = match read_key(&mod_dir.join(installed)) {
        Ok(key) => same_key(&key, &bisign.key),
        Err(tail) => {
            return Ok(Some(Diagnostic::new(
                "MF-P009",
                Severity::Error,
                format!("{installed} {tail}"),
            )))
        }
    };
    if !same {
        return Ok(Some(Diagnostic::new(
            "MF-P009",
            Severity::Error,
            format!("{key} is not the key that signed {file}; sign the build again"),
        )));
    }
    Ok(None)
}

/// Read and parse a `.bikey`/`.biprivatekey`; the error is the tail of a finding's message.
fn read_key(path: &Path) -> Result<KeyBlob, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot be read: {e}"))?;
    KeyBlob::parse(&bytes).map_err(|e| format!("is not a valid key: {e}"))
}

/// Whether two blobs hold the same key material; their file name and embedded authority need not agree.
fn same_key(a: &KeyBlob, b: &KeyBlob) -> bool {
    a.bits == b.bits && a.exponent == b.exponent && a.components == b.components
}

/// Server findings for each authority that signed the build: a server checking signatures
/// needs that authority's public key to be the one that signed.
pub fn server_key_checks(mod_dir: &Path, server_dir: &Path) -> anyhow::Result<Vec<Diagnostic>> {
    let files = build_files(mod_dir)?;
    let keys_dir = server_dir.join("keys");
    let mut diagnostics: Vec<Diagnostic> = signed_keys(mod_dir, &files)
        .iter()
        .filter_map(|key| server_key_check(&keys_dir, &key.authority, key))
        .collect();
    diagnostics.dedup_by(|a, b| a.message == b.message);
    Ok(diagnostics)
}

/// Every public key the build's signatures hold, in build-file order. An unreadable signature
/// has its own finding from [`signature_checks`].
fn signed_keys(mod_dir: &Path, files: &[String]) -> Vec<KeyBlob> {
    let mut keys: Vec<KeyBlob> = Vec::new();
    for file in files.iter().filter(|file| has_extension(file, "bisign")) {
        let bisign = fs::read(mod_dir.join(file))
            .ok()
            .and_then(|bytes| Bisign::parse(&bytes).ok());
        if let Some(Bisign { key, .. }) = bisign {
            if !keys.iter().any(|seen| {
                seen.authority.eq_ignore_ascii_case(&key.authority) && same_key(seen, &key)
            }) {
                keys.push(key);
            }
        }
    }
    keys
}

fn server_key_check(keys_dir: &Path, authority: &str, signature: &KeyBlob) -> Option<Diagnostic> {
    let name = public_key_name(authority);
    let Some(installed) = server_key(keys_dir, &name) else {
        return Some(Diagnostic::new(
            "MF-P017",
            Severity::Warning,
            format!("the server has no {name}; a server checking signatures refuses this mod"),
        ));
    };
    let same = match read_key(&installed) {
        Ok(key) => same_key(&key, signature),
        Err(tail) => {
            return Some(Diagnostic::new(
                "MF-P017",
                Severity::Error,
                format!("the server's {name} {tail}"),
            ))
        }
    };
    (!same).then(|| {
        Diagnostic::new(
            "MF-P017",
            Severity::Error,
            format!("the server's {name} is a different key; clients will be kicked"),
        )
    })
}

/// The server's copy of `name`, matched in any case.
fn server_key(keys_dir: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(keys_dir)
        .ok()?
        .flatten()
        .find(|entry| entry.file_name().eq_ignore_ascii_case(name))
        .map(|entry| entry.path())
}

/// The digest takes a PBO's `\0` + SHA-1 trailer on trust, so a payload changed after signing still
/// verifies unless the trailer is checked against the bytes before it.
fn trailer_matches(pbo: &[u8]) -> bool {
    let Some(body_len) = pbo.len().checked_sub(21) else {
        return false;
    };
    pbo[body_len] == 0 && Sha1::digest(&pbo[..body_len]).as_slice() == &pbo[body_len + 1..]
}

fn signature_error(message: String) -> Diagnostic {
    Diagnostic::new("MF-P004", Severity::Error, message)
}

/// The PBO a `.bisign` file signs: `x.pbo` for `x.pbo.<key>.bisign`.
fn pbo_beside(bisign: &str, files: &[String]) -> Option<String> {
    let (dir, name) = match bisign.rfind('/') {
        Some(slash) => bisign.split_at(slash + 1),
        None => ("", bisign),
    };
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".bisign")?;
    // An authority may itself hold ".pbo", so every ".pbo" followed by a named authority is a
    // candidate; the one that names a real PBO wins.
    let candidates: Vec<String> = (0..stem.len())
        .filter(|&at| stem.as_bytes()[at..].starts_with(b".pbo."))
        .map(|at| at + ".pbo".len())
        .filter(|&end| end + 1 < stem.len())
        .map(|end| format!("{dir}{}", &name[..end]))
        .collect();
    candidates
        .iter()
        .rev()
        .find(|pbo| files.iter().any(|file| file.eq_ignore_ascii_case(pbo)))
        .or(candidates.last())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
    use modfoundry_sign::{
        bisign_name, generate_keypair, public_key_name, sign_pbo_bytes, KeyBlob, SignVersion,
        KEY_BITS,
    };
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn structure_checks_report_each_layout_problem() {
        type Case<'a> = (Vec<&'a str>, Vec<(&'a str, Severity)>);
        let cases: Vec<Case> = vec![
            (
                vec![],
                vec![("MF-P001", Severity::Error), ("MF-P002", Severity::Error)],
            ),
            (vec!["addons/A.pbo"], vec![("MF-P007", Severity::Error)]),
            (
                vec!["addons/MyMod.pbo", "Addons/mymod.pbo.K.bisign"],
                vec![],
            ),
            (
                vec!["Addons/A.pbo", "mod.cpp"],
                vec![("MF-P007", Severity::Error)],
            ),
            (
                vec!["A.pbo"],
                vec![("MF-P002", Severity::Error), ("MF-P003", Severity::Warning)],
            ),
            (
                vec!["addons/A.pbo", "addons/sub/B.pbo"],
                vec![("MF-P007", Severity::Error), ("MF-P003", Severity::Warning)],
            ),
            (
                vec!["addons/A.pbo", "bin/x.DLL"],
                vec![("MF-P007", Severity::Error), ("MF-P005", Severity::Warning)],
            ),
        ];
        for (files, expected) in cases {
            let files: Vec<String> = files.into_iter().map(str::to_owned).collect();
            assert_eq!(
                pairs(&structure_checks(&files)),
                expected,
                "files: {files:?}"
            );
        }
    }

    fn pairs(diagnostics: &[Diagnostic]) -> Vec<(&str, Severity)> {
        diagnostics
            .iter()
            .map(|d| (d.code.as_str(), d.severity))
            .collect()
    }

    #[test]
    fn a_pbo_is_an_error_until_a_bisign_sits_beside_it() {
        let unsigned = vec!["addons/x.pbo".to_owned()];
        assert_eq!(
            pairs(&structure_checks(&unsigned)),
            [("MF-P007", Severity::Error)]
        );
        let signed = vec![
            "addons/x.pbo".to_owned(),
            "addons/x.pbo.K.bisign".to_owned(),
        ];
        assert!(structure_checks(&signed).is_empty());
    }

    const PAYLOAD: &[u8] = b"class CfgPatches {};";

    fn build_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mf-checks-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn signed_build(label: &str, private: &KeyBlob, public: &KeyBlob) -> PathBuf {
        let dir = build_dir(label);
        let pbo = pack_pbo(
            &[("prefix".to_owned(), "Mod".to_owned())],
            &[PboFileToWrite {
                name: "config.cpp".into(),
                data: PAYLOAD.to_vec(),
                timestamp: 0,
            }],
            &PackOptions::default(),
        )
        .unwrap();
        let bisign = sign_pbo_bytes(&private.to_bytes(), &pbo, SignVersion::V3).unwrap();
        write(&dir, "addons/Mod.pbo", &pbo);
        write(
            &dir,
            &format!("addons/{}", bisign_name("Mod.pbo", "MF")),
            &bisign,
        );
        write(
            &dir,
            &format!("keys/{}", public_key_name("MF")),
            &public.to_bytes(),
        );
        dir
    }

    #[cfg(unix)]
    #[test]
    fn build_files_skip_a_linked_folder() {
        let dir = build_dir("linked");
        write(&dir, "addons/A.pbo", b"x");
        std::os::unix::fs::symlink(dir.join("addons"), dir.join("linked.dll")).unwrap();

        assert_eq!(build_files(&dir).unwrap(), ["addons/A.pbo"]);
    }

    #[test]
    fn signature_checks_follow_the_build_signature() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();

        let signed = signed_build("signed", &private, &public);
        assert!(signature_checks(&signed).unwrap().is_empty());

        let tampered = signed_build("tampered", &private, &public);
        let mut pbo = fs::read(tampered.join("addons/Mod.pbo")).unwrap();
        let at = pbo
            .windows(PAYLOAD.len())
            .position(|w| w == PAYLOAD)
            .unwrap();
        pbo[at] ^= 1;
        fs::write(tampered.join("addons/Mod.pbo"), pbo).unwrap();
        let diagnostics = signature_checks(&tampered).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P004", Severity::Error)]);
        assert_eq!(
            diagnostics[0].message,
            "addons/Mod.pbo.MF.bisign does not match addons/Mod.pbo; sign the build again"
        );

        let no_key = signed_build("no-key", &private, &public);
        fs::remove_file(no_key.join("keys/MF.bikey")).unwrap();
        let diagnostics = signature_checks(&no_key).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P006", Severity::Warning)]);

        let no_pbo = signed_build("no-pbo", &private, &public);
        fs::remove_file(no_pbo.join("addons/Mod.pbo")).unwrap();
        let diagnostics = signature_checks(&no_pbo).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P004", Severity::Error)]);

        let doubled = signed_build("doubled", &private, &public);
        fs::copy(
            doubled.join("addons/Mod.pbo.MF.bisign"),
            doubled.join("addons/Mod.pbo.Other.bisign"),
        )
        .unwrap();
        let diagnostics = signature_checks(&doubled).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P008", Severity::Error)]);
        assert_eq!(
            diagnostics[0].message,
            "addons/Mod.pbo has 2 signatures \
             (addons/Mod.pbo.MF.bisign, addons/Mod.pbo.Other.bisign); keep the one for your key"
        );

        let wrong_key = signed_build("wrong-key", &private, &public);
        let (_, other_public) = generate_keypair("MF", KEY_BITS).unwrap();
        write(&wrong_key, "keys/MF.bikey", &other_public.to_bytes());
        let diagnostics = signature_checks(&wrong_key).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P009", Severity::Error)]);
        assert_eq!(
            diagnostics[0].message,
            "keys/MF.bikey is not the key that signed addons/Mod.pbo.MF.bisign; sign the build again"
        );

        let broken_key = signed_build("broken-key", &private, &public);
        write(&broken_key, "keys/MF.bikey", b"not a key");
        let diagnostics = signature_checks(&broken_key).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P009", Severity::Error)]);
        assert!(
            diagnostics[0]
                .message
                .starts_with("keys/MF.bikey is not a valid key"),
            "{}",
            diagnostics[0].message
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_key_is_reported_as_unreadable() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
        let build = signed_build("unreadable-key", &private, &public);
        fs::remove_file(build.join("keys/MF.bikey")).unwrap();
        std::os::unix::fs::symlink("gone", build.join("keys/MF.bikey")).unwrap();

        let diagnostics = signature_checks(&build).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P009", Severity::Error)]);
        assert!(
            diagnostics[0]
                .message
                .contains("keys/MF.bikey cannot be read"),
            "{}",
            diagnostics[0].message
        );
        assert!(
            !diagnostics[0].message.contains("is not the key"),
            "{}",
            diagnostics[0].message
        );
    }

    #[test]
    fn a_signature_finds_its_pbo_in_any_case_and_an_unreadable_one_is_a_finding() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
        let cased = signed_build("cased-signature", &private, &public);
        fs::rename(
            cased.join("addons/Mod.pbo.MF.bisign"),
            cased.join("addons/mod.pbo.MF.bisign"),
        )
        .unwrap();
        assert!(signature_checks(&cased).unwrap().is_empty());

        let broken = signed_build("broken-signature", &private, &public);
        std::os::unix::fs::symlink("gone", broken.join("addons/Other.pbo.MF.bisign")).unwrap();
        let diagnostics = signature_checks(&broken).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P004", Severity::Error)]);
        assert!(diagnostics[0]
            .message
            .contains("addons/Other.pbo.MF.bisign"));
    }

    #[test]
    fn server_key_checks_compare_every_key_an_authority_signed_with() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
        let (other_private, _) = generate_keypair("MF", KEY_BITS).unwrap();
        let build = signed_build("two-keys", &private, &public);
        let pbo = fs::read(build.join("addons/Mod.pbo")).unwrap();
        write(&build, "addons/Two.pbo", &pbo);
        write(
            &build,
            "addons/Two.pbo.MF.bisign",
            &sign_pbo_bytes(&other_private.to_bytes(), &pbo, SignVersion::V3).unwrap(),
        );
        let server = build_dir("two-keys-server");
        write(&server, "keys/MF.bikey", &public.to_bytes());
        let diagnostics = server_key_checks(&build, &server).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P017", Severity::Error)]);
    }

    #[test]
    fn a_signature_names_the_pbo_before_its_authority_even_when_that_holds_pbo() {
        let cases: [(&str, &[&str], Option<&str>); 8] = [
            (
                "addons/x.pbo.MF.bisign",
                &["addons/x.pbo"],
                Some("addons/x.pbo"),
            ),
            ("addons/Größe.pbo.MF.bisign", &[], Some("addons/Größe.pbo")),
            (
                "addons/a.pbo.pbo.MF.bisign",
                &["addons/a.pbo.pbo"],
                Some("addons/a.pbo.pbo"),
            ),
            (
                "addons/x.pbo.a.pbo.bisign",
                &["addons/x.pbo"],
                Some("addons/x.pbo"),
            ),
            (
                "addons/my.pbo.tools.pbo.MF.bisign",
                &["addons/my.pbo.tools.pbo"],
                Some("addons/my.pbo.tools.pbo"),
            ),
            (
                "addons/my.pbo.tools.pbo.MF.bisign",
                &["addons/my.pbo"],
                Some("addons/my.pbo"),
            ),
            ("Addons/Mod.PBO.K.BISIGN", &[], Some("Addons/Mod.PBO")),
            ("addons/x.pbo.bisign", &["addons/x.pbo"], None),
        ];
        for (bisign, present, expected) in cases {
            let files: Vec<String> = present.iter().map(|file| (*file).to_owned()).collect();
            assert_eq!(pbo_beside(bisign, &files).as_deref(), expected, "{bisign}");
        }
    }

    #[test]
    fn duplicate_signatures_group_a_pbo_name_in_any_case() {
        let files = [
            "addons/Mod.pbo.MF.bisign".to_owned(),
            "addons/mod.pbo.Other.bisign".to_owned(),
        ];
        let bisigns: Vec<&String> = files.iter().collect();

        let diagnostics = duplicate_signatures(&bisigns, &files);

        assert_eq!(pairs(&diagnostics), [("MF-P008", Severity::Error)]);
        assert_eq!(
            diagnostics[0].message,
            "addons/Mod.pbo has 2 signatures \
             (addons/Mod.pbo.MF.bisign, addons/mod.pbo.Other.bisign); keep the one for your key"
        );
    }

    #[test]
    fn server_key_checks_compare_the_servers_key_for_each_authority() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
        let (_, other) = generate_keypair("MF", KEY_BITS).unwrap();
        let signed = signed_build("server-key", &private, &public);

        let no_key = "the server has no MF.bikey; a server checking signatures refuses this mod";
        let different = "the server's MF.bikey is a different key; clients will be kicked";
        let cases = [
            (
                "same-key",
                true,
                Some(("keys/mf.bikey", public.to_bytes())),
                None,
            ),
            ("no-file", true, None, Some((no_key, Severity::Warning))),
            (
                "another-key",
                true,
                Some(("keys/MF.bikey", other.to_bytes())),
                Some((different, Severity::Error)),
            ),
            (
                "unparseable",
                true,
                Some(("keys/MF.bikey", b"not a key".to_vec())),
                Some(("the server's MF.bikey is not a valid key", Severity::Error)),
            ),
            (
                "no-keys-folder",
                false,
                None,
                Some((no_key, Severity::Warning)),
            ),
        ];
        for (label, keys_folder, key, expected) in cases {
            let server = build_dir(&format!("server-{label}"));
            if keys_folder {
                fs::create_dir_all(server.join("keys")).unwrap();
            }
            if let Some((name, bytes)) = key {
                write(&server, name, &bytes);
            }
            let diagnostics = server_key_checks(&signed, &server).unwrap();
            match expected {
                None => assert!(diagnostics.is_empty(), "{label}: {diagnostics:?}"),
                Some((message, severity)) => {
                    assert_eq!(pairs(&diagnostics), [("MF-P017", severity)], "{label}");
                    assert!(
                        diagnostics[0].message.starts_with(message),
                        "{label}: {}",
                        diagnostics[0].message
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_server_key_is_reported_as_unreadable() {
        let (private, public) = generate_keypair("MF", KEY_BITS).unwrap();
        let signed = signed_build("server-unreadable", &private, &public);
        let server = build_dir("server-unreadable-keys");
        fs::create_dir_all(server.join("keys")).unwrap();
        std::os::unix::fs::symlink("gone", server.join("keys/MF.bikey")).unwrap();

        let diagnostics = server_key_checks(&signed, &server).unwrap();
        assert_eq!(pairs(&diagnostics), [("MF-P017", Severity::Error)]);
        assert!(
            diagnostics[0]
                .message
                .starts_with("the server's MF.bikey cannot be read"),
            "{}",
            diagnostics[0].message
        );
    }
}
