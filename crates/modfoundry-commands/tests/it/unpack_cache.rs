//! The unpack cache: a source is unpacked once and the copy is reused while its PBOs do not change.

use std::fs;

use crate::common::{pack, tmp};
use modfoundry_commands::unpack_cache::unpacked;

#[test]
fn a_mod_folder_is_unpacked_under_the_root_with_every_pbo() {
    let dir = tmp("unpack-cache-folder");
    let mod_dir = dir.join("@Mod");
    let a: &[u8] = b"class A {};";
    let b: &[u8] = b"class B {};";
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", a)],
    );
    pack(
        &mod_dir.join("Addons/b.pbo"),
        &[("prefix", "Other")],
        &[("config.cpp", b)],
    );
    let root = dir.join("cache");

    let copy = unpacked(&root, &mod_dir).unwrap();

    assert!(copy.dir.starts_with(&root));
    assert_eq!(fs::read(copy.dir.join("a/My/Mod/config.cpp")).unwrap(), a);
    assert_eq!(fs::read(copy.dir.join("b/Other/config.cpp")).unwrap(), b);
    assert_eq!(copy.left_out, None);
}

#[test]
fn a_second_call_reuses_the_copy() {
    let dir = tmp("unpack-cache-reuse");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"kept").unwrap();

    let second = unpacked(&root, &mod_dir).unwrap();

    assert_eq!(second.dir, first.dir);
    assert_eq!(fs::read(&marker).unwrap(), b"kept");
}

#[test]
fn a_changed_pbo_is_unpacked_again() {
    let dir = tmp("unpack-cache-changed");
    let mod_dir = dir.join("@Mod");
    let pbo = mod_dir.join("Addons/a.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"stale").unwrap();
    let script: &[u8] = b"class B {};";
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};"), ("Scripts\\new.c", script)],
    );

    let second = unpacked(&root, &mod_dir).unwrap();

    assert!(!marker.exists());
    assert_eq!(
        fs::read(second.dir.join("a/My/Mod/Scripts/new.c")).unwrap(),
        script
    );
}

#[test]
fn a_copy_without_its_stamp_is_unpacked_again() {
    let dir = tmp("unpack-cache-no-stamp");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"unstamped").unwrap();
    let stamp = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "stamp"))
        .unwrap();
    fs::remove_file(stamp).unwrap();

    unpacked(&root, &mod_dir).unwrap();

    assert!(!marker.exists());
}

#[test]
fn a_single_pbo_is_unpacked_under_its_prefix() {
    let dir = tmp("unpack-cache-single");
    let pbo = dir.join("Mod.pbo");
    let config: &[u8] = b"class A {};";
    pack(&pbo, &[("prefix", "My\\Mod")], &[("config.cpp", config)]);

    let copy = unpacked(&dir.join("cache"), &pbo).unwrap();

    assert_eq!(
        fs::read(copy.dir.join("My/Mod/config.cpp")).unwrap(),
        config
    );
    assert_eq!(copy.left_out, None);
}

#[test]
fn a_prefixed_pbo_is_cached_under_its_prefix_with_no_prefix_file() {
    let dir = tmp("unpack-cache-no-prefix-file");
    let pbo = dir.join("Mod.pbo");
    pack(
        &pbo,
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );

    let copy = unpacked(&dir.join("cache"), &pbo).unwrap();

    assert!(copy.dir.join("My/Mod/config.cpp").is_file());
    assert!(!copy.dir.join("$PBOPREFIX$").exists());
}

#[test]
fn a_pbo_that_is_not_a_pbo_is_named_and_not_unpacked_again() {
    let dir = tmp("unpack-cache-bad-pbo");
    let mod_dir = dir.join("@Mod");
    let config: &[u8] = b"class A {};";
    pack(
        &mod_dir.join("Addons/good.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", config)],
    );
    fs::write(mod_dir.join("Addons/bad.pbo"), b"not a pbo").unwrap();
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let left_out = first.left_out.clone().expect("the bad PBO is left out");
    assert!(left_out.contains("bad.pbo"), "{left_out}");
    assert_eq!(
        fs::read(first.dir.join("good/Mod/config.cpp")).unwrap(),
        config
    );
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"kept").unwrap();

    let second = unpacked(&root, &mod_dir).unwrap();

    assert_eq!(second.left_out, Some(left_out));
    assert!(marker.exists());
}

#[test]
fn sources_with_one_folder_name_get_separate_copies() {
    let dir = tmp("unpack-cache-names");
    let first_src = dir.join("one/@Mod");
    let second_src = dir.join("two/@Mod");
    pack(
        &first_src.join("Addons/a.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", b"one")],
    );
    pack(
        &second_src.join("Addons/a.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", b"two")],
    );
    let root = dir.join("cache");

    let one = unpacked(&root, &first_src).unwrap();
    let two = unpacked(&root, &second_src).unwrap();

    assert_ne!(one.dir, two.dir);
    assert!(one
        .dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("_Mod-"));
    assert_eq!(fs::read(one.dir.join("a/Mod/config.cpp")).unwrap(), b"one");
    assert_eq!(fs::read(two.dir.join("a/Mod/config.cpp")).unwrap(), b"two");
}

#[test]
fn a_folder_with_no_pbo_is_refused_and_leaves_nothing_under_the_root() {
    let dir = tmp("unpack-cache-empty");
    let mod_dir = dir.join("@Mod");
    fs::create_dir_all(mod_dir.join("Scripts")).unwrap();
    let root = dir.join("cache");

    assert!(unpacked(&root, &mod_dir).is_err());
    assert!(!root.exists());
}

#[test]
fn a_source_that_is_not_there_is_named_and_leaves_nothing_under_the_root() {
    let dir = tmp("unpack-cache-missing");
    let missing = dir.join("gone.pbo");
    let root = dir.join("cache");

    let error = unpacked(&root, &missing).err().expect("no such source");

    let message = format!("{error:#}");
    let start = format!("cannot read {}: ", missing.display());
    assert!(message.starts_with(&start), "{message}");
    assert!(!root.exists());
}

#[test]
fn a_pbo_added_to_the_folder_is_unpacked_too() {
    let dir = tmp("unpack-cache-added");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "A")],
        &[("config.cpp", b"class A {};")],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    pack(
        &mod_dir.join("Addons/b.pbo"),
        &[("prefix", "B")],
        &[("config.cpp", b"class B {};")],
    );

    let second = unpacked(&root, &mod_dir).unwrap();

    assert_eq!(second.dir, first.dir);
    assert!(second.dir.join("a/A/config.cpp").is_file());
    assert!(second.dir.join("b/B/config.cpp").is_file());
}

#[test]
fn callers_at_the_same_time_all_get_the_whole_copy() {
    let dir = tmp("unpack-cache-together");
    let mod_dir = dir.join("@Mod");
    let files: Vec<(String, Vec<u8>)> = (0..200)
        .map(|n| (format!("data\\f{n}.txt"), vec![b'x'; 4096]))
        .collect();
    let entries: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    pack(&mod_dir.join("Addons/a.pbo"), &[("prefix", "A")], &entries);
    let root = dir.join("cache");
    let start = std::sync::Barrier::new(4);

    let copies: Vec<_> = std::thread::scope(|scope| {
        let callers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    unpacked(&root, &mod_dir)
                })
            })
            .collect();
        callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect()
    });

    for copy in &copies {
        let copy = copy.as_ref().expect("every caller gets the copy");
        assert_eq!(
            fs::read_dir(copy.dir.join("a/A/data")).unwrap().count(),
            200
        );
    }
    let left: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_temp_folder_another_run_left_behind_is_removed() {
    let dir = tmp("unpack-cache-leftover-temp");
    let mod_dir = dir.join("@Mod");
    let config: &[u8] = b"class A {};";
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", config)],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let key = first
        .dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fs::remove_dir_all(&first.dir).unwrap();
    let leftover = root.join(format!("{key}.tmp-4242"));
    fs::create_dir_all(leftover.join("a/My/Mod")).unwrap();
    fs::write(leftover.join("a/My/Mod/config.cpp"), b"stale").unwrap();

    let copy = unpacked(&root, &mod_dir).unwrap();

    assert_eq!(
        fs::read(copy.dir.join("a/My/Mod/config.cpp")).unwrap(),
        config
    );
    assert!(!leftover.exists());
}

#[test]
fn a_left_out_text_with_a_line_break_still_reuses_the_copy() {
    let dir = tmp("unpack-cache-line-break");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/good.pbo"),
        &[("prefix", "Mod")],
        &[("config.cpp", b"class A {};")],
    );
    fs::write(mod_dir.join("Addons/bad\nx.pbo"), b"not a pbo").unwrap();
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"kept").unwrap();

    let second = unpacked(&root, &mod_dir).unwrap();

    assert_eq!(second.left_out, first.left_out);
    assert!(marker.exists());
}

#[test]
fn a_copy_stamped_with_the_previous_format_is_unpacked_again() {
    let dir = tmp("unpack-cache-old-format");
    let mod_dir = dir.join("@Mod");
    pack(
        &mod_dir.join("Addons/a.pbo"),
        &[("prefix", "My\\Mod")],
        &[("config.cpp", b"class A {};")],
    );
    let root = dir.join("cache");
    let first = unpacked(&root, &mod_dir).unwrap();
    let marker = first.dir.join("marker.txt");
    fs::write(&marker, b"made by the previous format").unwrap();
    let stamp = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "stamp"))
        .unwrap();
    let text = fs::read_to_string(&stamp).unwrap();
    let (_, rest) = text.split_once('\n').unwrap();
    fs::write(&stamp, format!("1\n{rest}")).unwrap();

    unpacked(&root, &mod_dir).unwrap();

    assert!(!marker.exists());
}
