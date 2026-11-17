mod config;
mod rap;
mod tree;

pub use tree::{load_tree, load_tree_with_disk};

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::input::{CheckInput, Layer, ModInfo, SourceFile};
use crate::target::Target;

use config::ConfigClass;

// One pack of a mod: the scripts one config names. The game compiles packs in
// the order of their paths inside their PBOs, each after the packs it requires.
#[derive(Clone, Debug)]
pub struct Pack {
    pub path: String,
    pub addons: Vec<String>,
    pub required: Vec<String>,
    pub files: Vec<SourceFile>,
    pub disk: Vec<PathBuf>,
}

// One mod as read from disk: its packs, and what its configs say about defines
// and the addons it provides and needs.
#[derive(Clone, Debug)]
pub struct LoadedMod {
    pub info: ModInfo,
    pub packs: Vec<Pack>,
    // The `CfgMods` class names and their `defines[]`: each is a define the
    // game sets while the mod is loaded.
    pub defines: Vec<String>,
    pub addons: Vec<String>,
    pub required: Vec<String>,
    // The input names its `inputs.xml` files register.
    pub inputs: Vec<String>,
}

impl LoadedMod {
    // The mod's scripts, pack by pack.
    pub fn files(&self) -> Vec<SourceFile> {
        self.packs.iter().flat_map(|pack| pack.files.iter().cloned()).collect()
    }
}

// The config class that holds each script module's folders.
const MODULES: [(&str, Layer); 5] = [
    ("engineScriptModule", Layer::Core),
    ("gameLibScriptModule", Layer::GameLib),
    ("gameScriptModule", Layer::Game),
    ("worldScriptModule", Layer::World),
    ("missionScriptModule", Layer::Mission),
];

// Loads a mod folder: an unpacked mod with one folder per pack, or a source
// tree. The configs name the script folders; a tree with no config is read by
// its `1_Core` .. `5_Mission` folder names.
pub fn load_mod_dir(dir: &Path, name: &str) -> LoadedMod {
    let mut loaded = LoadedMod {
        info: ModInfo { name: name.to_string(), cfg_class: name.to_string() },
        packs: Vec::new(),
        defines: Vec::new(),
        addons: Vec::new(),
        required: Vec::new(),
        inputs: Vec::new(),
    };
    // A folder may be listed twice in one module, and is then read once; the
    // same folder in two modules is read in both.
    let mut seen: HashSet<(Layer, PathBuf)> = HashSet::new();
    let mut packs = Vec::new();
    for (config_dir, config) in configs(dir) {
        let mut pack = Pack { path: pack_path(dir, &config_dir), addons: Vec::new(), required: Vec::new(), files: Vec::new(), disk: Vec::new() };
        let mut defines = Vec::new();
        if let Some(patches) = config.class("CfgPatches") {
            for patch in &patches.classes {
                push_new(&mut pack.addons, &patch.name);
                for required in patch.list("requiredAddons") {
                    push_new(&mut pack.required, required);
                }
            }
        }
        let mods = config.class("CfgMods").map(|mods| mods.classes.as_slice()).unwrap_or_default();
        for class in mods {
            push_new(&mut defines, &class.name);
            for define in class.list("defines") {
                push_new(&mut defines, define);
            }
            for file in class.list("inputs") {
                let text = locate(dir, &config_dir, file).and_then(|(_, found)| fs::read(found).ok());
                for name in input_names(&String::from_utf8_lossy(&text.unwrap_or_default())) {
                    push_new(&mut loaded.inputs, name);
                }
            }
            let Some(defs) = class.class("defs") else {
                continue;
            };
            for (module, layer) in MODULES {
                let folders = defs.class(module).map(|module| module.list("files")).unwrap_or_default();
                for folder in folders {
                    if let Some((root, found)) = locate(dir, &config_dir, folder) {
                        collect(&root, &found, layer, &mut seen, &mut pack);
                    }
                }
            }
        }
        packs.push((defines, pack));
    }
    for (defines, pack) in in_load_order(packs, |(_, pack)| (pack, 0)) {
        for define in &defines {
            push_new(&mut loaded.defines, define);
        }
        for addon in &pack.addons {
            push_new(&mut loaded.addons, addon);
        }
        for required in &pack.required {
            push_new(&mut loaded.required, required);
        }
        loaded.packs.push(pack);
    }
    loaded.required.retain(|required| !loaded.addons.iter().any(|addon| addon.eq_ignore_ascii_case(required)));
    if let Some(first) = loaded.defines.first() {
        loaded.info.cfg_class = first.clone();
    }
    if loaded.packs.iter().all(|pack| pack.files.is_empty()) {
        let mut pack = Pack { path: file_name(dir), addons: Vec::new(), required: Vec::new(), files: Vec::new(), disk: Vec::new() };
        by_layer_folders(dir, dir, None, &mut pack);
        loaded.packs.push(pack);
    }
    loaded
}

// Puts vanilla and the mods together the way the game compiles them: module
// by module, vanilla's files first, then every mod's packs in load order. The
// order the mods are given in decides nothing.
pub fn assemble(target: Target, vanilla: Vec<SourceFile>, mods: Vec<LoadedMod>) -> CheckInput {
    let vanilla = vanilla.into_iter().map(|file| (file, PathBuf::new())).collect();
    assemble_with_disk(target, vanilla, mods).0
}

// The check input, and the disk path of each of its files in the same order.
fn assemble_with_disk(target: Target, vanilla: Vec<(SourceFile, PathBuf)>, mods: Vec<LoadedMod>) -> (CheckInput, Vec<PathBuf>) {
    let all = mods.iter().enumerate().flat_map(|(index, loaded)| loaded.packs.iter().map(move |pack| (index, pack)));
    let packs = in_load_order(all.collect(), |(index, pack)| (*pack, *index));
    let mut files = Vec::new();
    for layer in Layer::ALL {
        files.extend(vanilla.iter().filter(|(file, _)| file.layer == layer).cloned());
        for (index, pack) in &packs {
            let own = pack.files.iter().zip(&pack.disk).filter(|(file, _)| file.layer == layer);
            files.extend(own.map(|(file, disk)| (SourceFile { mod_index: index + 1, ..file.clone() }, disk.clone())));
        }
    }
    let (files, disk) = files.into_iter().unzip();
    let mut extra_defines = Vec::new();
    for loaded in &mods {
        for define in loaded.defines.iter().filter(|define| **define != loaded.info.cfg_class) {
            push_new(&mut extra_defines, define);
        }
    }
    let mut input_names = Vec::new();
    for name in mods.iter().flat_map(|loaded| &loaded.inputs) {
        push_new(&mut input_names, name);
    }
    // The game lists its own `DayZ` define after the mods' classes.
    let mut infos: Vec<ModInfo> = mods.into_iter().map(|loaded| loaded.info).collect();
    infos.push(ModInfo { name: "DayZ".into(), cfg_class: "DayZ".into() });
    (CheckInput { target, mods: infos, files, extra_defines, input_names }, disk)
}

// A check input read from disk, with what the reader wants said about it.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub input: CheckInput,
    pub notes: Vec<String>,
    // The disk path of each entry of `input.files`, in the same order.
    pub disk: Vec<PathBuf>,
}

// Loads the vanilla tree and the mod folders, in load order, into one input.
// A folder that yields no script gets a note, so a mistyped path does not pass
// for a clean check.
pub fn load_input(target: Target, vanilla: Option<&Path>, mods: &[(String, PathBuf)]) -> Loaded {
    let mut notes = Vec::new();
    let mut note_empty = |dir: &Path| notes.push(format!("no scripts found under {}", dir.display()));
    let vanilla_files = vanilla.map(|dir| load_tree_with_disk(dir, Layer::from_folder_name)).unwrap_or_default();
    if let (Some(dir), true) = (vanilla, vanilla_files.is_empty()) {
        note_empty(dir);
    }
    let mut loaded = Vec::with_capacity(mods.len());
    for (name, dir) in mods {
        let one = load_mod_dir(dir, name);
        if one.packs.iter().all(|pack| pack.files.is_empty()) {
            note_empty(dir);
        }
        loaded.push(one);
    }
    let provided: Vec<String> = loaded.iter().flat_map(|one| one.addons.iter().cloned()).collect();
    for pack in loaded.iter().flat_map(|one| &one.packs) {
        for addon in unprovided(pack, &provided) {
            notes.push(format!(
                "pack {} requires addon {addon}, which no mod given provides: it is compiled after the mods listed before it",
                pack.path
            ));
        }
    }
    let (input, disk) = assemble_with_disk(target, vanilla_files, loaded);
    Loaded { input, notes, disk }
}

fn push_new(list: &mut Vec<String>, item: &str) {
    if !item.is_empty() && !list.iter().any(|have| have == item) {
        list.push(item.to_string());
    }
}

// The `name` of each `<input ...>` element of an inputs file.
fn input_names(xml: &str) -> Vec<&str> {
    let mut names = Vec::new();
    // What follows a `-->` up to the next `<!--` is outside a comment.
    let live = xml.split("<!--").enumerate().filter_map(|(index, part)| match index {
        0 => Some(part),
        _ => part.split_once("-->").map(|(_, after)| after),
    });
    for element in live.flat_map(|part| part.split("<input").skip(1)) {
        // `<inputs>` and the like share the prefix; an element's name is
        // followed by a space.
        let Some(attributes) = element.strip_prefix(char::is_whitespace) else {
            continue;
        };
        let attributes = attributes.split('>').next().unwrap_or_default();
        let name = attributes.split("name=\"").nth(1).and_then(|rest| rest.split('"').next());
        names.extend(name);
    }
    names
}

// Every config under a folder with the folder it sits in, in path order. A
// text config wins over a binarized one beside it.
fn configs(dir: &Path) -> Vec<(PathBuf, ConfigClass)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let mut text = None;
        let mut binary = None;
        for path in entries(&folder) {
            let name = file_name(&path).to_ascii_lowercase();
            if path.is_dir() {
                pending.push(path);
            } else if name == "config.cpp" {
                text = fs::read(&path).ok().map(|bytes| config::parse_text(&String::from_utf8_lossy(&bytes)));
            } else if name == "config.bin" {
                binary = fs::read(&path).ok().and_then(|bytes| rap::parse_rap(&bytes));
            }
        }
        if let Some(config) = text.or(binary) {
            found.push((folder, config));
        }
    }
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found
}

// Packs in the order the game compiles them: by path, letter case aside, with
// a pack held back until the packs it requires have gone. Holding one back
// moves nothing else: the next pack in path order goes first. `pack` gives
// each item's pack and the place of its mod among the mods given.
fn in_load_order<T>(mut pending: Vec<T>, pack: impl Fn(&T) -> (&Pack, usize)) -> Vec<T> {
    pending.sort_by_key(|item| pack(item).0.path.to_ascii_lowercase());
    let provided: Vec<String> = pending.iter().flat_map(|item| pack(item).0.addons.iter().cloned()).collect();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let waiting: Vec<&str> = pending.iter().flat_map(|item| &pack(item).0.addons).map(String::as_str).collect();
        let ready = pending.iter().position(|item| {
            let (own, place) = pack(item);
            let needs_waiting = |required: &String| {
                waiting.iter().any(|name| name.eq_ignore_ascii_case(required))
                    && !own.addons.iter().any(|addon| addon.eq_ignore_ascii_case(required))
            };
            // What a missing addon requires in turn is unknown, so the pack
            // keeps the place its mod was given.
            let behind_earlier_mod = !unprovided(own, &provided).is_empty()
                && pending.iter().any(|other| pack(other).1 < place);
            !own.required.iter().any(needs_waiting) && !behind_earlier_mod
        });
        // Packs that require each other in a circle are taken in path order.
        ordered.push(pending.remove(ready.unwrap_or(0)));
    }
    ordered
}

// The addons a pack requires that neither the game nor a given pack provides.
fn unprovided<'a>(pack: &'a Pack, provided: &[String]) -> Vec<&'a str> {
    let missing = |required: &&String| {
        let own_to_the_game = required.get(..3).is_some_and(|start| start.eq_ignore_ascii_case("DZ_"));
        !own_to_the_game && !provided.iter().any(|addon| addon.eq_ignore_ascii_case(required))
    };
    pack.required.iter().filter(missing).map(String::as_str).collect()
}

// The path of a config inside its PBO, which starts with the PBO's prefix. An
// unpacked mod holds one folder per PBO; a config in the mod folder itself is a
// source tree, packed under the folder's name.
fn pack_path(mod_dir: &Path, config_dir: &Path) -> String {
    let inside: Vec<String> = config_dir
        .strip_prefix(mod_dir)
        .map(|relative| relative.iter().skip(1).map(|part| part.to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    match inside.is_empty() {
        true => file_name(config_dir),
        false => inside.join("/"),
    }
}

// A script folder is named by its path inside the pack, so it sits under the
// pack's root: an ancestor of the config's folder, or another pack of the mod.
fn locate(mod_dir: &Path, config_dir: &Path, folder: &str) -> Option<(PathBuf, PathBuf)> {
    let relative: Vec<&str> = folder.split(['/', '\\']).filter(|part| !part.is_empty()).collect();
    // A config in the mod folder itself names its folders from the folder
    // above; any other stays inside the mod.
    let top = match config_dir == mod_dir {
        true => mod_dir.parent().unwrap_or(mod_dir),
        false => mod_dir,
    };
    let ancestors = config_dir.ancestors().take_while(|ancestor| ancestor.starts_with(top));
    let packs = entries(mod_dir).into_iter().filter(|path| path.is_dir());
    let roots = ancestors.map(Path::to_path_buf).chain(packs);
    for root in roots {
        if let Some(found) = descend(&root, &relative) {
            return Some((root, found));
        }
    }
    None
}

// Follows path parts written for a file system that ignores case.
fn descend(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for part in parts {
        current = entries(&current).into_iter().find(|path| file_name(path).eq_ignore_ascii_case(part))?;
    }
    Some(current)
}

// The scripts under one module folder, each named by its path from the pack
// root, which is how the game prints it.
fn collect(root: &Path, at: &Path, layer: Layer, seen: &mut HashSet<(Layer, PathBuf)>, out: &mut Pack) {
    if at.is_dir() {
        for path in entries(at) {
            collect(root, &path, layer, seen, out);
        }
        return;
    }
    if !is_script(at) || !seen.insert((layer, at.to_path_buf())) {
        return;
    }
    let Ok(bytes) = fs::read(at) else {
        return;
    };
    out.files.push(SourceFile {
        path: game_path(root, at),
        layer,
        mod_index: 0,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    });
    out.disk.push(at.to_path_buf());
}

// A tree with no usable config: a script belongs to the nearest folder above
// it that is named like a layer.
fn by_layer_folders(root: &Path, at: &Path, layer: Option<Layer>, out: &mut Pack) {
    for path in entries(at) {
        if path.is_dir() {
            let named = Layer::from_folder_name(&file_name(&path)).or(layer);
            by_layer_folders(root, &path, named, out);
        } else if let (Some(layer), true, Ok(bytes)) = (layer, is_script(&path), fs::read(&path)) {
            out.files.push(SourceFile {
                path: game_path(root, &path),
                layer,
                mod_index: 0,
                text: String::from_utf8_lossy(&bytes).into_owned(),
            });
            out.disk.push(path);
        }
    }
}

fn game_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = relative.iter().map(|part| part.to_string_lossy().into_owned()).collect();
    parts.join("/")
}

// A folder's folders and regular files in name order; none when it cannot be
// read. A link is left out, so a tree that links back into itself ends, and so
// is anything that is not a plain file.
pub(crate) fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            let plain = entries.flatten().filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir() || kind.is_file())
            });
            plain.map(|entry| entry.path()).collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

pub(crate) fn is_script(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("c"))
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}
