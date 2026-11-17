use std::fs;
use std::path::PathBuf;

use enforce_check::Target;
use enforce_check::loader::load_input;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name)
}

#[test]
fn each_disk_path_reads_back_the_text_of_its_file() {
    let cases = [
        ("textmod", None, vec![("gear-mod".to_string(), fixture("loader/textmod"))]),
        ("loose", None, vec![("loose".to_string(), fixture("loader/loose"))]),
        (
            "vanilla with textmod",
            Some(fixture("probes/vanilla")),
            vec![("gear-mod".to_string(), fixture("loader/textmod"))],
        ),
    ];
    for (name, vanilla, mods) in cases {
        let loaded = load_input(Target::RetailServer, vanilla.as_deref(), &mods);
        assert_eq!(loaded.disk.len(), loaded.input.files.len(), "{name}");
        assert!(!loaded.input.files.is_empty(), "{name}");
        let has_vanilla = loaded.input.files.iter().any(|file| file.path == "Scripts/1_Core/base.c");
        assert_eq!(has_vanilla, vanilla.is_some(), "{name}");
        for (file, disk) in loaded.input.files.iter().zip(&loaded.disk) {
            let text = String::from_utf8_lossy(&fs::read(disk).unwrap()).into_owned();
            assert_eq!(text, file.text, "{name}: {}", file.path);
        }
    }
}
