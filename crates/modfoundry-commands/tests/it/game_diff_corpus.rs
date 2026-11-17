//! `mf game-diff` on the real game trees, opt-in.

use std::path::PathBuf;
use std::time::Instant;

use modfoundry_commands::game_diff::{game_diff, render_text, GameDiffRequest, Summary};
use modfoundry_commands::game_side::read_side;
use modfoundry_commands::game_snapshot::{load_snapshot, save_snapshot};
use modfoundry_gamediff::snapshot::Snapshot;

use crate::common::tmp;

/// The 164200 -> 164289 hotfix, read from the unpacked reference trees.
#[test]
fn game_diff_corpus_reference_hotfix() {
    let Ok(reference) = std::env::var("MF_REFERENCE_DIR") else {
        eprintln!("skipped: MF_REFERENCE_DIR not set");
        return;
    };
    let reference = PathBuf::from(reference);
    let request = GameDiffRequest {
        old: reference
            .join("dayz-1.30.164200.56")
            .to_string_lossy()
            .into_owned(),
        new: reference
            .join("dayz-1.30.164289.70")
            .to_string_lossy()
            .into_owned(),
        against: Vec::new(),
        snapshots: Some(tmp("game-diff-corpus-reference")),
    };

    let started = Instant::now();
    let report = game_diff(&request).unwrap();
    let elapsed = started.elapsed().as_millis();
    let text = render_text(&report);
    eprintln!(
        "{} script, {} config and {} layout changes in {elapsed} ms",
        report.summary.scripts, report.summary.configs, report.summary.layouts
    );

    assert_eq!(report.summary.scripts, 29, "{text}");
    for line in [
        "method changed  ItemFall  scripts/4_World/Classes/ConstructionBase.c:301  signature, body",
        "method changed  IsBoxCollidingGeometry  scripts/3_Game/Global/Game.c:1366  signature",
        "method removed  CanSimulateItemFalling  ",
        "method removed  GetItemFallingData  ",
        "enum changed  CollisionFlags  ",
        "enum changed  DiagMenuIDs  ",
    ] {
        assert!(text.contains(line), "{line} missing from:\n{text}");
    }
}

/// A snapshot of the install, saved and loaded back, compares with the install as no change.
#[test]
fn game_diff_corpus_install_snapshot_round_trip() {
    let Ok(install) = std::env::var("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let install = PathBuf::from(install);
    let snapshots = tmp("game-diff-corpus-round-trip");

    let started = Instant::now();
    let side = read_side(&install).unwrap();
    eprintln!("read the install in {} ms", started.elapsed().as_millis());
    let build = side.build.expect("the install carries a build number");
    assert!(side.warnings.is_empty(), "{:?}", side.warnings);
    assert!(
        side.scripts.classes.len() > 1000,
        "{} classes",
        side.scripts.classes.len()
    );
    assert!(
        side.configs.classes.len() > 1000,
        "{} config classes",
        side.configs.classes.len()
    );
    assert!(
        side.layouts.files.len() > 100,
        "{} layouts",
        side.layouts.files.len()
    );
    let snapshot = Snapshot::new(build.clone(), side.scripts, side.configs, side.layouts);

    let started = Instant::now();
    let path = save_snapshot(&snapshots, &snapshot).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    eprintln!(
        "saved {} in {} ms, {size} bytes",
        path.display(),
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    assert_eq!(load_snapshot(&path).unwrap(), snapshot);
    eprintln!("loaded it back in {} ms", started.elapsed().as_millis());

    let request = GameDiffRequest {
        old: build,
        new: install.to_string_lossy().into_owned(),
        against: Vec::new(),
        snapshots: Some(snapshots.clone()),
    };
    let started = Instant::now();
    let report = game_diff(&request).unwrap();
    eprintln!("compared it in {} ms", started.elapsed().as_millis());

    assert_eq!(
        report.summary,
        Summary::default(),
        "{}",
        render_text(&report)
    );
    let _ = std::fs::remove_dir_all(&snapshots);
}
