use modfoundry_enforce::locals::E_DUPLICATE_LOCAL;
use modfoundry_enforce::returns::E_MISSING_RETURN;
use modfoundry_enforce::{lint_text, parse_tree};
use modfoundry_pbo::read_pbo_file;
use modfoundry_script::extract;
use std::path::{Path, PathBuf};

#[test]
fn extracts_the_whole_vanilla_script_tree() {
    let Ok(root) = std::env::var("MF_DAYZ_INSTALL").map(PathBuf::from) else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let pbo = root.join("dta").join("scripts.pbo");
    let (header, payloads) = read_pbo_file(&pbo).expect("read scripts.pbo");
    let mut files = 0usize;
    let mut classes = 0usize;
    let mut vanilla_player_base = false;
    for (entry, data) in header.entries.iter().zip(&payloads) {
        let name = entry.name_str();
        if !name.to_ascii_lowercase().ends_with(".c") {
            continue;
        }
        files += 1;
        let src = String::from_utf8_lossy(data);
        let items = extract(&src);
        classes += items.classes.len();
        if let Some(c) = items
            .classes
            .iter()
            .find(|c| c.name == "PlayerBase" && !c.modded)
        {
            vanilla_player_base |= c.base.as_ref().is_some_and(|b| b.name == "ManBase");
        }
    }
    assert!(files > 0, "no .c entries in scripts.pbo");
    assert!(
        classes > 1000,
        "only {classes} classes across {files} files"
    );
    assert!(vanilla_player_base, "vanilla PlayerBase extends ManBase");
}

/// Every `.c` entry of the install's `dta/scripts.pbo` as (name, text), or
/// `None` when `MF_DAYZ_INSTALL` is unset.
fn vanilla_sources() -> Option<Vec<(String, String)>> {
    let root = PathBuf::from(std::env::var("MF_DAYZ_INSTALL").ok()?);
    let (header, payloads) =
        read_pbo_file(&root.join("dta").join("scripts.pbo")).expect("read scripts.pbo");
    let sources: Vec<(String, String)> = header
        .entries
        .iter()
        .zip(&payloads)
        .filter(|(entry, _)| entry.name_str().to_ascii_lowercase().ends_with(".c"))
        .map(|(entry, data)| {
            (
                entry.name_str().to_string(),
                String::from_utf8_lossy(data).into_owned(),
            )
        })
        .collect();
    assert!(!sources.is_empty(), "no .c entries in scripts.pbo");
    Some(sources)
}

/// Vanilla compiles, so any of these on it is a false alarm.
#[test]
fn vanilla_scripts_raise_no_compile_rule_errors() {
    let Some(sources) = vanilla_sources() else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let path = Path::new("Scripts/3_Game/x.c");
    let mut hits = Vec::new();
    for (name, src) in &sources {
        for d in lint_text(path, &parse_tree(src), src) {
            if d.code == E_DUPLICATE_LOCAL || d.code == E_MISSING_RETURN {
                hits.push(format!("{name}: {} {}", d.code, d.message));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} compile-rule errors in vanilla scripts:\n{}",
        hits.len(),
        hits.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}
