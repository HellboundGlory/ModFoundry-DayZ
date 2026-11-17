//! Opt-in external-corpus verification of the PBO read side. BI content
//! is never committed, so these tests skip unless the env vars point at a real
//! install: `MF_DAYZ_INSTALL` = a DayZ install dir (must contain `Addons/`),
//! `MF_WORKSHOP_PATH` = a workshop `content/221100` tree.

use modfoundry_vfs::{LayerKind, PboBackend, Vfs, VirtualPath};
use std::path::{Path, PathBuf};

fn pbo_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pbo"))
        {
            out.push(path);
        }
    }
}

/// The sanitizer must not cost a single name in BI's own containers: every entry
/// of every vanilla addon/`dta` PBO has to be addressable, with no case-fold
/// collision inside a container. A nonzero count here means the read side is
/// hiding real content.
#[test]
fn mounts_every_vanilla_container_without_dropping_a_name() {
    let Ok(root) = std::env::var("MF_DAYZ_INSTALL").map(PathBuf::from) else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let addons = root.join("Addons");
    assert!(
        addons.is_dir(),
        "MF_DAYZ_INSTALL={} is not a DayZ install (no Addons/)",
        root.display()
    );

    let mut vfs = Vfs::new();
    let mut containers = 0usize;
    let mut entries = 0usize;
    let mut dropped: Vec<(PathBuf, usize)> = Vec::new();
    let mut collisions: Vec<(PathBuf, usize)> = Vec::new();
    for pbo in pbo_files(&addons)
        .into_iter()
        .chain(pbo_files(&root.join("dta")))
    {
        let backend = PboBackend::open(&pbo)
            .unwrap_or_else(|e| panic!("vanilla container {} failed to open: {e}", pbo.display()));
        containers += 1;
        entries += backend.index().len();
        if backend.skipped_names() > 0 {
            dropped.push((pbo.clone(), backend.skipped_names()));
        }
        if backend.case_collisions() > 0 {
            collisions.push((pbo.clone(), backend.case_collisions()));
        }
        vfs.mount(
            "game",
            backend.layer(pbo.display().to_string(), LayerKind::VanillaGame),
        );
    }
    assert!(
        containers > 0,
        "no vanilla containers under {}",
        root.display()
    );
    assert!(entries > 0, "no vanilla entries indexed");
    assert!(
        dropped.is_empty(),
        "sanitizer dropped vanilla names: {dropped:?}"
    );
    assert!(
        collisions.is_empty(),
        "vanilla casing collides inside a container: {collisions:?}"
    );

    let constants = VirtualPath::parse("game:scripts/1_Core/constants.c").unwrap();
    let text = vfs
        .read(&constants)
        .expect("vanilla 1_Core/constants.c reads through the VFS");
    let text = String::from_utf8(text).expect("vanilla Enforce source is UTF-8");
    assert!(
        text.contains("Constants"),
        "unexpected content at {constants}: {:.60}",
        text
    );

    let ai_config = VirtualPath::parse("game:DZ/AI/config.bin").unwrap();
    if vfs.resolve(&ai_config).is_some() {
        let bytes = vfs.read(&ai_config).unwrap();
        assert!(bytes.len() > 1024, "vanilla ai config.bin looks truncated");
    }
}

/// Third-party name manglers pad containers with decoy/garbage names, so a large absolute rejection count is
/// expected — what must NOT happen is exclusions spread across ordinary mods.
/// This asserts the exclusions stay confined to a minority of containers.
#[test]
fn workshop_name_rejections_stay_confined_to_mangled_containers() {
    let Ok(root) = std::env::var("MF_WORKSHOP_PATH").map(PathBuf::from) else {
        eprintln!("skipped: MF_WORKSHOP_PATH not set");
        return;
    };
    let mut containers = 0usize;
    let mut addressable = 0usize;
    let mut dropped = 0usize;
    let mut with_drops: Vec<(PathBuf, usize, usize)> = Vec::new();
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    for pbo in pbo_files(&root) {
        match PboBackend::open(&pbo) {
            Ok(backend) => {
                containers += 1;
                addressable += backend.index().len();
                dropped += backend.skipped_names();
                if backend.skipped_names() > 0 {
                    with_drops.push((pbo, backend.skipped_names(), backend.index().len()));
                }
            }
            Err(e) => failures.push((pbo, e.to_string())),
        }
    }
    assert!(containers > 0, "no containers under {}", root.display());
    assert!(
        failures.is_empty(),
        "{} workshop container(s) failed to open: {failures:?}",
        failures.len()
    );

    with_drops.sort_by_key(|(_, dropped, _)| std::cmp::Reverse(*dropped));
    let top: usize = with_drops.iter().take(10).map(|(_, d, _)| d).sum();
    println!(
        "workshop: {containers} containers, {addressable} addressable entries, {dropped} dropped \
         across {} containers; top 10 hold {top} ({:.0}%)",
        with_drops.len(),
        100.0 * top as f64 / dropped.max(1) as f64
    );

    assert!(
        with_drops.len() * 4 < containers,
        "exclusions reach too many containers ({} of {containers}) — the sanitizer is hiding \
         real mod files",
        with_drops.len()
    );
    assert!(
        (top * 2 >= dropped && !with_drops.is_empty()) || dropped == 0,
        "exclusions are not concentrated in the padded decoy containers (top 10 = {top} of {dropped})"
    );
}
