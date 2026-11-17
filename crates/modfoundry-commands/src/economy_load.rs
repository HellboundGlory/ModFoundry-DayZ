//! Finding and loading the economy reference (a mission folder, the DayZ
//! server's mission, or the game's `worlds_chernarusplus_ce.pbo`) and the
//! workspace's CE files. Plain IO over temp dirs and PBOs; nothing here writes.

use std::path::{Path, PathBuf};

use modfoundry_ce::model::{self, Document};
use modfoundry_ce::xml::{NodeId, XmlDoc};
use modfoundry_config::ast::{ConfigClass, Member, Value as AstValue};
use modfoundry_config::parser::parse;
use modfoundry_pbo::read_header_file;

use crate::config_host::read_pbo_entry;
use crate::run_ops::SERVER_CONFIG;
use crate::script_index::find_child_dir;
use crate::sources::{find_child, game_dir};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LIST_DEPTH: usize = 3;

const KNOWN: [&str; 12] = [
    "db/types.xml",
    "db/events.xml",
    "db/economy.xml",
    "db/globals.xml",
    "db/messages.xml",
    "cfgeconomycore.xml",
    "cfglimitsdefinition.xml",
    "cfglimitsdefinitionuser.xml",
    "cfgspawnabletypes.xml",
    "cfgrandompresets.xml",
    "cfgeventspawns.xml",
    "cfgignorelist.xml",
];

/// What `ce.index` and `ce.reference` are given: the workspace source root and
/// optional overrides for the three discovery paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomyIndexRequest {
    pub workspace: PathBuf,
    pub mission: Option<PathBuf>,
    pub server: Option<PathBuf>,
    pub game: Option<PathBuf>,
}

/// Which of the three locations a reference economy was found at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceOrigin {
    Folder,
    Server,
    Game,
}

/// The reference economy and every file it lists.
#[derive(Debug, Clone)]
pub(crate) struct Reference {
    pub name: String,
    pub location: PathBuf,
    pub origin: ReferenceOrigin,
    pub files: Vec<LoadedFile>,
}

/// One file of a reference economy or of the workspace.
#[derive(Debug, Clone)]
pub(crate) struct LoadedFile {
    pub id: String,
    pub rel: String,
    pub path: Option<PathBuf>,
    pub pbo_entry: Option<usize>,
    pub text: Option<String>,
    pub document: Option<Document>,
    pub declared: Option<String>,
}

/// Find the reference economy: the named mission, else the server's mission,
/// else the game's Chernarus mission PBO. Appends one warning per miss.
pub(crate) fn find_reference(
    req: &EconomyIndexRequest,
    warnings: &mut Vec<String>,
) -> Option<Reference> {
    if let Some(mission) = &req.mission {
        if !mission.is_dir() {
            warnings.push(format!("mission folder not found: {}", mission.display()));
            return None;
        }
        return Some(folder_reference(
            mission,
            ReferenceOrigin::Folder,
            file_name(mission),
            warnings,
        ));
    }
    if let Some(server) = req
        .server
        .clone()
        .or_else(modfoundry_platform::find_dayz_server)
    {
        if let Some((name, dir)) = server_mission(&server) {
            return Some(folder_reference(
                &dir,
                ReferenceOrigin::Server,
                name,
                warnings,
            ));
        }
    }
    if let Some(game) = game_dir(req.game.as_ref()) {
        if let Some(pbo) = game_pbo(&game) {
            if let Some(reference) = pbo_reference(&pbo, warnings) {
                return Some(reference);
            }
        }
    }
    warnings.push(
        "No DayZ server or game install found: reference rows and name checks are off.".into(),
    );
    None
}

/// Every CE XML file under `root`, read and modelled, in path order.
pub(crate) fn load_workspace(root: &Path, warnings: &mut Vec<String>) -> Vec<LoadedFile> {
    let mut out = Vec::new();
    for path in crate::ce_ops::walk_ce_files(root) {
        let Ok(bytes) = std::fs::read(&path) else {
            warnings.push(format!("could not read {}", path.display()));
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let document = model::read(&text, Some(&file_name(&path)));
        if document.kind.is_none() {
            continue;
        }
        out.push(LoadedFile {
            id: path.to_string_lossy().into_owned(),
            rel: rel_path(root, &path),
            path: Some(path),
            pbo_entry: None,
            text: Some(text),
            document: Some(document),
            declared: None,
        });
    }
    out
}

/// Read a listed-only reference file on demand, with the 32 MB cap.
pub(crate) fn read_reference_text(
    reference: &Reference,
    file: &LoadedFile,
) -> Result<String, String> {
    let bytes = read_file_bytes(&reference.location, file)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{} is larger than 32 MB", file.rel));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The mission folder `serverDZ.cfg` names, or `None`.
pub fn mission_template(server_cfg: &str) -> Option<String> {
    let file = parse(server_cfg).file;
    let missions = find_class(&file.members, "Missions")?;
    let dayz = find_class(&missions.members, "DayZ")?;
    dayz.members.iter().find_map(|member| match member {
        Member::Property(property) if property.name.eq_ignore_ascii_case("template") => {
            match &property.value {
                Some(AstValue::Str(text)) => Some(text.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

fn find_class<'a>(members: &'a [Member], name: &str) -> Option<&'a ConfigClass> {
    for member in members {
        if let Member::Class(class) = member {
            if class.name.eq_ignore_ascii_case(name) {
                return Some(class);
            }
            if let Some(found) = find_class(&class.members, name) {
                return Some(found);
            }
        }
    }
    None
}

fn folder_reference(
    dir: &Path,
    origin: ReferenceOrigin,
    name: String,
    warnings: &mut Vec<String>,
) -> Reference {
    let mut listed = Vec::new();
    list_folder(dir, dir, 0, &mut listed);
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut files: Vec<LoadedFile> = listed
        .into_iter()
        .map(|(rel, path)| LoadedFile {
            id: format!("mission:{rel}"),
            rel,
            path: Some(path),
            pbo_entry: None,
            text: None,
            document: None,
            declared: None,
        })
        .collect();
    load_reference(dir, &mut files, warnings);
    Reference {
        name,
        location: dir.to_path_buf(),
        origin,
        files,
    }
}

fn pbo_reference(pbo: &Path, warnings: &mut Vec<String>) -> Option<Reference> {
    let (header, _) = read_header_file(pbo).ok()?;
    let mut files: Vec<LoadedFile> = header
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let rel = entry.name_str().replace('\\', "/").to_lowercase();
            LoadedFile {
                id: format!("mission:{rel}"),
                rel,
                path: None,
                pbo_entry: Some(index),
                text: None,
                document: None,
                declared: None,
            }
        })
        .collect();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    load_reference(pbo, &mut files, warnings);
    Some(Reference {
        name: "Chernarus (game files)".to_string(),
        location: pbo.to_path_buf(),
        origin: ReferenceOrigin::Game,
        files,
    })
}

fn server_mission(server: &Path) -> Option<(String, PathBuf)> {
    let mpmissions = find_child_dir(server, "mpmissions")?;
    let template = std::fs::read_to_string(server.join(SERVER_CONFIG))
        .ok()
        .as_deref()
        .and_then(mission_template);
    if let Some(template) = template {
        if let Some(dir) = resolve_rel(&mpmissions, &template) {
            if dir.is_dir() {
                return Some((file_name(&dir), dir));
            }
        }
    }
    first_dir(&mpmissions).map(|dir| (file_name(&dir), dir))
}

fn game_pbo(game: &Path) -> Option<PathBuf> {
    let addons = find_child_dir(game, "addons")?;
    find_child(&addons, "worlds_chernarusplus_ce.pbo", false)
}

fn resolve_rel(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    let mut seen = false;
    for component in rel.split(['/', '\\']).filter(|part| !part.is_empty()) {
        current = find_child_dir(&current, component)?;
        seen = true;
    }
    seen.then_some(current)
}

fn first_dir(dir: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    dirs.sort_by_key(|path| file_name(path));
    dirs.into_iter().next()
}

fn load_reference(location: &Path, files: &mut [LoadedFile], warnings: &mut Vec<String>) {
    if let Some(core) = files
        .iter_mut()
        .find(|file| file.rel == "cfgeconomycore.xml")
    {
        load_file(location, core, None, warnings);
    }
    let includes = files
        .iter()
        .find(|file| file.rel == "cfgeconomycore.xml")
        .and_then(|file| file.text.as_deref())
        .map(economy_core_includes)
        .unwrap_or_default();
    for file in files.iter_mut() {
        if file.text.is_some() {
            continue;
        }
        if KNOWN.contains(&file.rel.as_str()) {
            load_file(location, file, None, warnings);
        } else if let Some(declared) = includes
            .iter()
            .find(|(rel, _)| rel == &file.rel)
            .map(|(_, declared)| declared.clone())
        {
            load_file(location, file, Some(declared), warnings);
        }
    }
}

fn load_file(
    location: &Path,
    file: &mut LoadedFile,
    declared: Option<String>,
    warnings: &mut Vec<String>,
) {
    match read_file_bytes(location, file) {
        Ok(bytes) if bytes.len() as u64 <= MAX_FILE_BYTES => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
            file.document = Some(model::read(&text, Some(name)));
            file.text = Some(text);
            file.declared = declared;
        }
        Ok(_) => {}
        Err(err) => warnings.push(format!("could not read {}: {err}", file.rel)),
    }
}

fn read_file_bytes(location: &Path, file: &LoadedFile) -> Result<Vec<u8>, String> {
    if let Some(index) = file.pbo_entry {
        return read_pbo_entry(location, index);
    }
    match &file.path {
        Some(path) => std::fs::read(path).map_err(|err| err.to_string()),
        None => Err(format!("{} has no path", file.rel)),
    }
}

pub(crate) fn economy_core_includes(text: &str) -> Vec<(String, String)> {
    let doc = modfoundry_ce::xml::parse(text);
    let mut out = Vec::new();
    for ce in descendants_named(&doc, "ce") {
        let folder = attr_value(&doc, ce, "folder");
        for file in doc.children_named(ce, "file") {
            let name = attr_value(&doc, file, "name");
            let declared = attr_value(&doc, file, "type");
            out.push((format!("{folder}/{name}").to_lowercase(), declared));
        }
    }
    out
}

fn attr_value(doc: &XmlDoc, id: NodeId, name: &str) -> String {
    doc.attr(id, name)
        .map(|attr| attr.value.clone())
        .unwrap_or_default()
}

fn descendants_named(doc: &XmlDoc, name: &str) -> Vec<NodeId> {
    fn collect(doc: &XmlDoc, id: NodeId, name: &str, out: &mut Vec<NodeId>) {
        if doc.element(id).is_some_and(|element| element.name == name) {
            out.push(id);
        }
        for child in doc.child_elements(id) {
            collect(doc, child, name, out);
        }
    }
    let mut out = Vec::new();
    for id in doc.top_elements() {
        collect(doc, id, name, &mut out);
    }
    out
}

fn list_folder(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth >= MAX_LIST_DEPTH || name.starts_with('.') || name.starts_with("storage_") {
                continue;
            }
            list_folder(root, &entry.path(), depth + 1, out);
        } else if kind.is_file() {
            out.push((rel_path(root, &entry.path()).to_lowercase(), entry.path()));
        }
    }
}

fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    use modfoundry_ce::kind::CeKind;
    use modfoundry_pbo::{pack_pbo, PackOptions, PboFileToWrite};
    use std::sync::atomic::{AtomicU64, Ordering};

    const TYPES: &str =
        "<types>\n  <type name=\"Canteen\"><nominal>40</nominal></type>\n</types>\n";
    const INIT_C: &str = "void main() {}\n";

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mf-econload-{label}-{}-{n}", std::process::id()))
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn pack(path: &Path, props: &[(&str, &str)], files: &[(&str, &[u8])]) {
        let properties: Vec<(String, String)> = props
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let entries: Vec<PboFileToWrite> = files
            .iter()
            .map(|(n, d)| PboFileToWrite {
                name: (*n).to_string(),
                data: d.to_vec(),
                timestamp: 1,
            })
            .collect();
        let bytes = pack_pbo(&properties, &entries, &PackOptions::default()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn known_mission(dir: &Path) {
        write(&dir.join("db/types.xml"), TYPES);
        write(&dir.join("db/events.xml"), "<events></events>\n");
        write(&dir.join("db/economy.xml"), "<economy></economy>\n");
        write(&dir.join("db/globals.xml"), "<variables></variables>\n");
        write(&dir.join("db/messages.xml"), "<messages></messages>\n");
        write(
            &dir.join("cfgeconomycore.xml"),
            "<economycore><classes></classes><defaults></defaults></economycore>\n",
        );
        write(&dir.join("cfglimitsdefinition.xml"), "<lists></lists>\n");
        write(
            &dir.join("cfglimitsdefinitionuser.xml"),
            "<user_lists></user_lists>\n",
        );
        write(
            &dir.join("cfgspawnabletypes.xml"),
            "<spawnabletypes></spawnabletypes>\n",
        );
        write(
            &dir.join("cfgrandompresets.xml"),
            "<randompresets></randompresets>\n",
        );
        write(
            &dir.join("cfgeventspawns.xml"),
            "<eventposdef></eventposdef>\n",
        );
        write(&dir.join("cfgignorelist.xml"), "<ignore></ignore>\n");
        write(&dir.join("init.c"), INIT_C);
        write(&dir.join("storage_1/backup.xml"), TYPES);
    }

    fn fixture_server(dir: &Path) {
        write(
            &dir.join("serverDZ.cfg"),
            "class Missions { class DayZ { template=\"dayzOffline.chernarusplus\"; }; };\n",
        );
        write(
            &dir.join("mpmissions/dayzOffline.chernarusplus/db/types.xml"),
            TYPES,
        );
    }

    fn fixture_game(dir: &Path) {
        pack(
            &dir.join("Addons/worlds_chernarusplus_ce.pbo"),
            &[("prefix", "DZ\\worlds\\chernarusplus\\ce")],
            &[
                ("db\\types.xml", TYPES.as_bytes()),
                ("init.c", INIT_C.as_bytes()),
            ],
        );
    }

    fn request(
        workspace: &Path,
        mission: Option<&Path>,
        server: Option<&Path>,
        game: Option<&Path>,
    ) -> EconomyIndexRequest {
        EconomyIndexRequest {
            workspace: workspace.to_path_buf(),
            mission: mission.map(Path::to_path_buf),
            server: server.map(Path::to_path_buf),
            game: game.map(Path::to_path_buf),
        }
    }

    fn mission_request(dir: &Path) -> EconomyIndexRequest {
        request(dir, Some(dir), None, None)
    }

    #[test]
    fn an_explicit_mission_folder_is_listed_and_loaded() {
        let dir = tmp("mission");
        known_mission(&dir);
        let mut warnings = Vec::new();
        let reference = find_reference(&mission_request(&dir), &mut warnings).unwrap();
        assert_eq!(reference.origin, ReferenceOrigin::Folder);
        assert_eq!(reference.name, dir.file_name().unwrap().to_string_lossy());
        assert!(warnings.is_empty(), "{warnings:?}");

        let rels: Vec<&str> = reference.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels.len(), 13, "{rels:?}");
        let mut sorted = rels.clone();
        sorted.sort();
        assert_eq!(rels, sorted);
        assert!(!rels.iter().any(|r| r.starts_with("storage_1/")));
        assert_eq!(reference.location, dir);

        let init = reference.files.iter().find(|f| f.rel == "init.c").unwrap();
        assert_eq!(init.id, "mission:init.c");
        assert!(init.text.is_none());
        assert!(init.document.is_none());

        for rel in ["db/types.xml", "cfgeconomycore.xml", "cfgignorelist.xml"] {
            let file = reference.files.iter().find(|f| f.rel == rel).unwrap();
            assert_eq!(file.id, format!("mission:{rel}"));
            assert!(file.text.is_some(), "{rel}");
            assert!(file.document.as_ref().unwrap().kind.is_some(), "{rel}");
        }
    }

    #[test]
    fn an_explicit_missing_mission_folder_never_falls_back() {
        let server = tmp("no-server");
        fixture_server(&server);
        let game = tmp("no-game");
        fixture_game(&game);
        let gone = tmp("gone");
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), Some(&gone), Some(&server), Some(&game)),
            &mut warnings,
        );
        assert!(reference.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("mission folder not found:"),
            "{warnings:?}"
        );
    }

    #[test]
    fn the_server_cfg_names_the_mission() {
        let server = tmp("server");
        write(
            &server.join("serverDZ.cfg"),
            "class Missions { class DayZ { template=\"dayzOffline.enoch\"; }; };\n",
        );
        write(
            &server.join("mpmissions/dayzOffline.chernarusplus/db/types.xml"),
            TYPES,
        );
        write(
            &server.join("mpmissions/dayzOffline.enoch/db/types.xml"),
            TYPES,
        );
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), None, Some(&server), Some(&tmp("no-game"))),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(reference.origin, ReferenceOrigin::Server);
        assert_eq!(reference.name, "dayzOffline.enoch");
        assert_eq!(
            reference.location,
            server.join("mpmissions/dayzOffline.enoch")
        );
    }

    #[test]
    fn without_a_cfg_the_first_mission_in_name_order_is_used() {
        let server = tmp("server-first");
        write(&server.join("mpmissions/zulu/db/types.xml"), TYPES);
        write(&server.join("mpmissions/alpha/db/types.xml"), TYPES);
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), None, Some(&server), Some(&tmp("no-game"))),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(reference.origin, ReferenceOrigin::Server);
        assert_eq!(reference.name, "alpha");
    }

    #[test]
    fn the_mission_folder_is_found_case_insensitively() {
        let server = tmp("server-case");
        write(
            &server.join("serverDZ.cfg"),
            "class Missions { class DayZ { template=\"dayzOffline.enoch\"; }; };\n",
        );
        write(
            &server.join("mpmissions/DAYZOFFLINE.ENOCH/db/types.xml"),
            TYPES,
        );
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), None, Some(&server), Some(&tmp("no-game"))),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(reference.origin, ReferenceOrigin::Server);
        assert_eq!(reference.name, "DAYZOFFLINE.ENOCH");
    }

    #[test]
    fn the_game_pbo_is_the_reference_when_no_server_is_found() {
        let game = tmp("game");
        fixture_game(&game);
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), None, Some(&tmp("no-server")), Some(&game)),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(reference.origin, ReferenceOrigin::Game);
        assert_eq!(reference.name, "Chernarus (game files)");
        assert!(warnings.is_empty(), "{warnings:?}");

        let types = reference
            .files
            .iter()
            .find(|f| f.rel == "db/types.xml")
            .unwrap();
        assert!(types.path.is_none());
        assert_eq!(types.pbo_entry, Some(0));
        assert_eq!(types.id, "mission:db/types.xml");
        let document = types.document.as_ref().unwrap();
        assert_eq!(document.kind, Some(CeKind::Types));
        assert!(document
            .sections
            .iter()
            .any(|s| s.records.iter().any(|r| r.key == "Canteen")));

        let init = reference.files.iter().find(|f| f.rel == "init.c").unwrap();
        assert!(init.text.is_none());
    }

    #[test]
    fn nothing_found_warns_once() {
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(
                &tmp("ws"),
                None,
                Some(&tmp("no-server")),
                Some(&tmp("no-game")),
            ),
            &mut warnings,
        );
        assert!(reference.is_none());
        assert_eq!(
            warnings,
            vec![
                "No DayZ server or game install found: reference rows and name checks are off."
                    .to_string()
            ]
        );
    }

    #[test]
    fn an_economy_core_include_loads_with_its_declared_type() {
        let dir = tmp("include");
        known_mission(&dir);
        write(
            &dir.join("cfgeconomycore.xml"),
            "<economycore><classes></classes><ce folder=\"custom\">\
             <file name=\"my_types.xml\" type=\"types\"/>\
             <file name=\"missing.xml\" type=\"types\"/></ce></economycore>\n",
        );
        write(&dir.join("custom/my_types.xml"), TYPES);
        let mut warnings = Vec::new();
        let reference = find_reference(&mission_request(&dir), &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let included = reference
            .files
            .iter()
            .find(|f| f.rel == "custom/my_types.xml")
            .unwrap();
        assert_eq!(included.declared.as_deref(), Some("types"));
        assert!(included.text.is_some());
        assert!(included.document.as_ref().unwrap().kind.is_some());
        assert!(!reference
            .files
            .iter()
            .any(|f| f.rel == "custom/missing.xml"));
    }

    #[test]
    fn mission_template_reads_the_dayz_class() {
        assert_eq!(
            mission_template(
                "class Missions { class DayZ { template=\"dayzOffline.chernarusplus\"; }; };\n"
            )
            .as_deref(),
            Some("dayzOffline.chernarusplus")
        );
        let with_noise = "// a comment\nclass Other { x=1; };\n\
                          class Missions {\n  class DayZ { template=\"dayzOffline.enoch\"; };\n};\n";
        assert_eq!(
            mission_template(with_noise).as_deref(),
            Some("dayzOffline.enoch")
        );
        assert_eq!(
            mission_template("class MISSIONS { class dayz { TEMPLATE=\"a\"; }; };").as_deref(),
            Some("a")
        );
        assert_eq!(mission_template("class Other { template=\"a\"; };"), None);
        assert_eq!(
            mission_template("class Missions { class DayZ { template=3; }; };"),
            None
        );
    }

    #[test]
    fn load_workspace_keeps_only_detected_kinds() {
        let root = tmp("ws");
        write(&root.join("one/two/three/types.xml"), TYPES);
        write(
            &root.join("frag.xml"),
            "<type name=\"A\"><nominal>1</nominal></type><type name=\"B\"><nominal>2</nominal></type>\n",
        );
        write(&root.join("builds/ignored.xml"), TYPES);
        write(&root.join(".hidden/h.xml"), TYPES);
        write(&root.join("trader.xml"), "<Trader><Item/></Trader>\n");
        let mut warnings = Vec::new();
        let files = load_workspace(&root, &mut warnings);
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(
            rels,
            vec!["frag.xml", "one/two/three/types.xml"],
            "{rels:?}"
        );
        assert_eq!(files[0].id, root.join("frag.xml").to_string_lossy());
        assert!(files[0].document.as_ref().unwrap().fragment);
    }

    #[test]
    fn read_reference_text_reads_a_folder_file_on_demand() {
        let dir = tmp("read-folder");
        known_mission(&dir);
        let mut warnings = Vec::new();
        let reference = find_reference(&mission_request(&dir), &mut warnings).unwrap();
        let init = reference.files.iter().find(|f| f.rel == "init.c").unwrap();
        assert_eq!(read_reference_text(&reference, init).unwrap(), INIT_C);
    }

    #[test]
    fn read_reference_text_reads_a_pbo_entry_on_demand() {
        let game = tmp("read-pbo");
        fixture_game(&game);
        let mut warnings = Vec::new();
        let reference = find_reference(
            &request(&tmp("ws"), None, Some(&tmp("no-server")), Some(&game)),
            &mut warnings,
        )
        .unwrap();
        let init = reference.files.iter().find(|f| f.rel == "init.c").unwrap();
        assert_eq!(read_reference_text(&reference, init).unwrap(), INIT_C);
    }
}
