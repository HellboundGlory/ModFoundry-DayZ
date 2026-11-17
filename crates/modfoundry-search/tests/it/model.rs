use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::scratch::Scratch;
use crate::store::part_files;
use modfoundry_search::model::{check_model, ensure_files, Fetch, ModelFile, MODEL_ID, REVISION};

const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

struct FakeFetch {
    served: Vec<(&'static str, &'static [u8])>,
    calls: RefCell<Vec<String>>,
}

impl FakeFetch {
    fn serving(served: &[(&'static str, &'static [u8])]) -> Self {
        Self {
            served: served.to_vec(),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Fetch for FakeFetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<(), String> {
        self.calls.borrow_mut().push(url.to_owned());
        let name = url.rsplit('/').next().unwrap_or_default();
        let (_, bytes) = self
            .served
            .iter()
            .find(|(served, _)| *served == name)
            .ok_or("not served")?;
        out.write_all(bytes).map_err(|err| err.to_string())
    }
}

#[test]
fn a_bad_download_leaves_no_file_and_names_the_file() {
    let cases: [(ModelFile, &[&str]); 2] = [
        (
            ModelFile {
                name: "hello.bin",
                bytes: 6,
                sha256: HELLO_SHA256,
            },
            &["hello.bin", "6", "5"],
        ),
        (
            ModelFile {
                name: "hello.bin",
                bytes: 5,
                sha256: EMPTY_SHA256,
            },
            &["hello.bin", EMPTY_SHA256, HELLO_SHA256],
        ),
    ];
    for (file, mentions) in cases {
        let scratch = Scratch::new("bad-download");
        let fetch = FakeFetch::serving(&[("hello.bin", b"hello")]);
        let err = ensure_files(&scratch.0, &[file], &fetch, &mut |_| {}).unwrap_err();
        let message = err.to_string();
        for text in mentions {
            assert!(message.contains(text), "{message}");
        }
        assert!(!scratch.0.join("hello.bin").exists());
        assert_eq!(part_files(&scratch.0), Vec::<String>::new());
    }
}

#[test]
fn a_file_already_in_place_at_the_right_size_is_not_fetched_again() {
    let scratch = Scratch::new("skip-present");
    fs::create_dir_all(&scratch.0).unwrap();
    fs::write(scratch.0.join("hello.bin"), b"hello").unwrap();
    let fetch = FakeFetch::serving(&[("hello.bin", b"hello")]);
    let mut lines = Vec::new();
    let file = ModelFile {
        name: "hello.bin",
        bytes: 5,
        sha256: HELLO_SHA256,
    };
    ensure_files(&scratch.0, &[file], &fetch, &mut |line| {
        lines.push(line.to_owned())
    })
    .unwrap();
    assert!(fetch.calls.borrow().is_empty());
    assert!(lines.is_empty());
}

#[test]
fn check_model_names_the_first_missing_or_wrong_file() {
    let scratch = Scratch::new("check");
    fs::create_dir_all(&scratch.0).unwrap();
    let err = check_model(&scratch.0).unwrap_err();
    assert!(err.to_string().contains("config.json"), "{err}");
    fs::write(scratch.0.join("config.json"), b"x").unwrap();
    let err = check_model(&scratch.0).unwrap_err();
    assert!(err.to_string().contains("config.json"), "{err}");
    assert!(err.to_string().contains("expected 743"), "{err}");
}

#[test]
fn a_missing_file_is_fetched_from_the_pinned_revision_and_announced() {
    let scratch = Scratch::new("fetch-missing");
    let fetch = FakeFetch::serving(&[("hello.bin", b"hello")]);
    let mut lines = Vec::new();
    let file = ModelFile {
        name: "hello.bin",
        bytes: 5,
        sha256: HELLO_SHA256,
    };
    ensure_files(&scratch.0, &[file], &fetch, &mut |line| {
        lines.push(line.to_owned())
    })
    .unwrap();
    assert_eq!(fs::read(scratch.0.join("hello.bin")).unwrap(), b"hello");
    assert_eq!(part_files(&scratch.0), Vec::<String>::new());
    assert_eq!(
        *fetch.calls.borrow(),
        [format!(
            "https://huggingface.co/{MODEL_ID}/resolve/{REVISION}/hello.bin"
        )]
    );
    assert_eq!(lines, ["downloading hello.bin (5 B)"]);
}

#[test]
fn a_file_of_the_wrong_size_is_replaced() {
    let scratch = Scratch::new("stale-file");
    fs::create_dir_all(&scratch.0).unwrap();
    fs::write(scratch.0.join("hello.bin"), b"hi").unwrap();
    let fetch = FakeFetch::serving(&[("hello.bin", b"hello")]);
    let file = ModelFile {
        name: "hello.bin",
        bytes: 5,
        sha256: HELLO_SHA256,
    };
    ensure_files(&scratch.0, &[file], &fetch, &mut |_| {}).unwrap();
    assert_eq!(fs::read(scratch.0.join("hello.bin")).unwrap(), b"hello");
    assert_eq!(fetch.calls.borrow().len(), 1);
}

#[test]
fn a_failed_fetch_leaves_no_partial_file() {
    struct Broken;
    impl Fetch for Broken {
        fn fetch(&self, _url: &str, out: &mut dyn Write) -> Result<(), String> {
            out.write_all(b"hel").map_err(|err| err.to_string())?;
            Err("connection reset".to_owned())
        }
    }
    let scratch = Scratch::new("broken-fetch");
    let file = ModelFile {
        name: "hello.bin",
        bytes: 5,
        sha256: HELLO_SHA256,
    };
    let err = ensure_files(&scratch.0, &[file], &Broken, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("hello.bin"), "{err}");
    assert!(!scratch.0.join("hello.bin").exists());
    assert_eq!(part_files(&scratch.0), Vec::<String>::new());
}

struct RecordingFetch {
    dir: PathBuf,
    seen: RefCell<Vec<String>>,
}

impl Fetch for RecordingFetch {
    fn fetch(&self, _url: &str, out: &mut dyn Write) -> Result<(), String> {
        self.seen.borrow_mut().extend(part_files(&self.dir));
        out.write_all(b"hello").map_err(|err| err.to_string())
    }
}

#[test]
fn two_downloads_in_one_process_use_different_part_files() {
    let mut seen = Vec::new();
    for label in ["part-name-one", "part-name-two"] {
        let scratch = Scratch::new(label);
        let fetch = RecordingFetch {
            dir: scratch.0.clone(),
            seen: RefCell::new(Vec::new()),
        };
        let file = ModelFile {
            name: "hello.bin",
            bytes: 5,
            sha256: HELLO_SHA256,
        };
        ensure_files(&scratch.0, &[file], &fetch, &mut |_| {}).unwrap();
        seen.extend(fetch.seen.borrow().iter().cloned());
        assert_eq!(part_files(&scratch.0), Vec::<String>::new());
    }

    let prefix = format!("hello.bin.{}.", std::process::id());
    assert_eq!(seen.len(), 2, "{seen:?}");
    for name in &seen {
        assert!(name.starts_with(&prefix), "{name}");
        assert!(name.ends_with(".part"), "{name}");
    }
    assert_ne!(seen[0], seen[1], "{seen:?}");
}

struct EndlessFetch {
    accepted: RefCell<u64>,
}

impl Fetch for EndlessFetch {
    fn fetch(&self, _url: &str, out: &mut dyn Write) -> Result<(), String> {
        for _ in 0..1_000 {
            out.write_all(&[0; 1024]).map_err(|err| err.to_string())?;
            *self.accepted.borrow_mut() += 1024;
        }
        Ok(())
    }
}

#[test]
fn a_download_longer_than_the_file_stops_early() {
    let scratch = Scratch::new("endless");
    let file = ModelFile {
        name: "config.json",
        bytes: 5,
        sha256: HELLO_SHA256,
    };
    let fetch = EndlessFetch {
        accepted: RefCell::new(0),
    };
    let err = ensure_files(&scratch.0, &[file], &fetch, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("config.json"), "{err}");
    assert_eq!(*fetch.accepted.borrow(), 0);
    assert_eq!(part_files(&scratch.0), Vec::<String>::new());
}
