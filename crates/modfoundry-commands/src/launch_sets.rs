//! Named launch sets: the mod lists a project keeps in one file so every
//! frontend launches the same set.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::build_ops::default_name;
use crate::diag_stage::{TestModArg, TestModRole};
use crate::project_settings::folder_link_name;

/// Where a project's named launch sets live, relative to the source root.
pub const SETS_FILE: &str = "server/launch-sets.json";

/// The build profile a set builds and launches with when it names none.
const DEFAULT_PROFILE: &str = "default";

/// A project's named launch sets.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSets {
    pub sets: BTreeMap<String, LaunchSet>,
}

/// One named set: the mods both halves load, the server-only mods, and its
/// optional server config and build profile.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSet {
    pub mods: Vec<SetMod>,
    #[serde(default)]
    pub server_mods: Vec<SetMod>,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
}

/// One mod of a set.
#[derive(Debug, Clone, PartialEq)]
pub enum SetMod {
    /// A mod source folder, relative to the source root or absolute; `name` overrides the build name.
    Local {
        local: PathBuf,
        name: Option<String>,
    },
    /// A built `@Mod` folder used as it is.
    Folder { folder: PathBuf },
    /// A Workshop item id.
    Workshop { workshop: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalMod {
    local: PathBuf,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderMod {
    folder: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkshopMod {
    workshop: String,
}

impl<'de> Deserialize<'de> for SetMod {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Ok(local) = LocalMod::deserialize(&value) {
            return Ok(SetMod::Local {
                local: local.local,
                name: local.name,
            });
        }
        if let Ok(folder) = FolderMod::deserialize(&value) {
            return Ok(SetMod::Folder {
                folder: folder.folder,
            });
        }
        if let Ok(workshop) = WorkshopMod::deserialize(&value) {
            return Ok(SetMod::Workshop {
                workshop: workshop.workshop,
            });
        }
        Err(serde::de::Error::custom(format!(
            "a mod is {{\"local\": \"...\"}} (optionally with \"name\"), {{\"folder\": \"...\"}} or {{\"workshop\": \"...\"}}; got {value}"
        )))
    }
}

/// A project's launch sets, or `None` when it has no sets file.
pub fn read_sets(source_root: &Path) -> Result<Option<LaunchSets>, String> {
    let path = source_root.join(SETS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{SETS_FILE}: {e}")),
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|e| format!("{SETS_FILE}: {e}"))
}

/// A set resolved into the paths a launch loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSet {
    /// The mods both halves load, in the file's order.
    pub mods: Vec<ResolvedMod>,
    /// The server-only mods, in the file's order.
    pub server_mods: Vec<ResolvedMod>,
    /// The server config the set launches with, under `<source root>/server/`.
    pub config: Option<PathBuf>,
}

/// A mod ready for staging, and the source folder a local mod was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMod {
    /// The mod directory and the link name it is staged under.
    pub arg: TestModArg,
    /// The source folder a local mod was built from; `None` for the others.
    pub local_root: Option<PathBuf>,
}

/// The set a project keeps under `name`, or what the file does have.
pub(crate) fn named_set<'a>(sets: &'a LaunchSets, name: &str) -> Result<&'a LaunchSet, String> {
    sets.sets.get(name).ok_or_else(|| {
        let has = if sets.sets.is_empty() {
            "it has no sets".to_owned()
        } else {
            format!(
                "it has: {}",
                sets.sets.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        };
        format!("no launch set \"{name}\" in {SETS_FILE}; {has}")
    })
}

/// Resolves the set `name` into the mods and config a launch loads.
pub fn resolve_set(
    source_root: &Path,
    name: &str,
    steam_root: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Result<ResolvedSet, String> {
    let path = source_root.join(SETS_FILE);
    let path = std::path::absolute(&path).unwrap_or(path);
    let sets = read_sets(source_root)?.ok_or_else(|| format!("{} is missing", path.display()))?;
    let set = named_set(&sets, name)?;
    check_local_names(set)?;
    let resolver = Resolver {
        source_root,
        set_name: name,
        profile: set.profile.as_deref().unwrap_or(DEFAULT_PROFILE),
        steam_root,
    };
    let resolve_all = |entries: &[SetMod], role: TestModRole| {
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for entry in entries {
            match resolver.entry(entry, role, &exists) {
                Ok(resolved) => found.push(resolved),
                Err(problem) => missing.push(problem),
            }
        }
        (found, missing)
    };
    let (mods, mut missing) = resolve_all(&set.mods, TestModRole::Both);
    let (server_mods, server_missing) = resolve_all(&set.server_mods, TestModRole::ServerOnly);
    missing.extend(server_missing);
    let mut config = None;
    if let Some(file) = &set.config {
        let path = source_root.join("server").join(file);
        if exists(&path) {
            config = Some(path);
        } else {
            missing.push(missing_path(&path));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "launch set \"{name}\" is missing:\n  {}",
            missing.join("\n  ")
        ));
    }
    Ok(ResolvedSet {
        mods,
        server_mods,
        config,
    })
}

/// Refuses a local `name` that cannot be a `-mod=` link name.
pub(crate) fn check_local_names(set: &LaunchSet) -> Result<(), String> {
    for entry in set.mods.iter().chain(&set.server_mods) {
        if let SetMod::Local {
            name: Some(name), ..
        } = entry
        {
            if name.is_empty() || name.starts_with('@') || name.contains(['/', '\\']) {
                return Err(format!(
                    "{SETS_FILE}: mod name \"{name}\" must be a bare folder name without a leading @"
                ));
            }
        }
    }
    Ok(())
}

/// What a set says when a path it names is not there.
fn missing_path(path: &Path) -> String {
    format!("{} does not exist", path.display())
}

/// What every entry of one set resolves against.
struct Resolver<'a> {
    source_root: &'a Path,
    set_name: &'a str,
    profile: &'a str,
    steam_root: Option<&'a Path>,
}

impl Resolver<'_> {
    fn entry(
        &self,
        entry: &SetMod,
        role: TestModRole,
        exists: &dyn Fn(&Path) -> bool,
    ) -> Result<ResolvedMod, String> {
        match entry {
            SetMod::Local { local, name } => {
                let local_root = self.source_root.join(local);
                let local_root = std::path::absolute(&local_root).unwrap_or(local_root);
                let name = name.clone().unwrap_or_else(|| default_name(&local_root));
                let path = local_root
                    .join("builds")
                    .join(self.profile)
                    .join(format!("@{name}"));
                if !exists(&path) {
                    return Err(format!(
                        "{} is not built; build it first: mf build --set {}",
                        path.display(),
                        self.set_name
                    ));
                }
                Ok(ResolvedMod {
                    arg: TestModArg {
                        path,
                        link_name: name,
                        role,
                    },
                    local_root: Some(local_root),
                })
            }
            SetMod::Folder { folder } => {
                if !exists(folder) {
                    return Err(missing_path(folder));
                }
                Ok(ResolvedMod {
                    arg: TestModArg {
                        path: folder.clone(),
                        link_name: folder_link_name(folder),
                        role,
                    },
                    local_root: None,
                })
            }
            SetMod::Workshop { workshop } => {
                let root = self
                    .steam_root
                    .ok_or_else(|| format!("workshop item {workshop}: Steam not found"))?;
                let path = modfoundry_platform::workshop_mod_dir(root, workshop)
                    .ok_or_else(|| format!("workshop item {workshop} is not a numeric id"))?;
                if !exists(&path) {
                    return Err(missing_path(&path));
                }
                Ok(ResolvedMod {
                    arg: TestModArg {
                        path,
                        link_name: workshop.clone(),
                        role,
                    },
                    local_root: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-launch-sets-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sets(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join("server")).unwrap();
        std::fs::write(root.join(SETS_FILE), text).unwrap();
    }

    #[test]
    fn the_file_reads_into_the_set_and_mod_shapes() {
        let root = tmp("shape");
        write_sets(
            &root,
            r#"{
                "sets": {
                    "session": {
                        "mods": [
                            {"workshop": "1559212036"},
                            {"workshop": "2545327648"},
                            {"local": "../../tests/SampleModTests"},
                            {"local": "."}
                        ],
                        "server_mods": [{"folder": "/home/me/@AdminTools"}],
                        "config": "serverDZ_expansion.cfg",
                        "profile": "default"
                    },
                    "bare": {"mods": []}
                }
            }"#,
        );

        let sets = read_sets(&root).unwrap().expect("the file is there");
        let session = &sets.sets["session"];
        assert_eq!(
            session.mods,
            [
                SetMod::Workshop {
                    workshop: "1559212036".into()
                },
                SetMod::Workshop {
                    workshop: "2545327648".into()
                },
                SetMod::Local {
                    local: PathBuf::from("../../tests/SampleModTests"),
                    name: None
                },
                SetMod::Local {
                    local: PathBuf::from("."),
                    name: None
                },
            ]
        );
        assert_eq!(
            session.server_mods,
            [SetMod::Folder {
                folder: PathBuf::from("/home/me/@AdminTools")
            }]
        );
        assert_eq!(session.config.as_deref(), Some("serverDZ_expansion.cfg"));
        assert_eq!(session.profile.as_deref(), Some("default"));
        let bare = &sets.sets["bare"];
        assert_eq!(
            (
                bare.server_mods.len(),
                bare.config.as_deref(),
                bare.profile.as_deref()
            ),
            (0, None, None)
        );
    }

    #[test]
    fn an_unknown_key_is_an_error_that_names_the_file() {
        let root = tmp("unknown-key");
        let cases = [
            r#"{"sets": {"session": {"mods": [{"workshop": "1", "nmae": "x"}]}}}"#,
            r#"{"sets": {"session": {"mods": [{"folder": "/m", "local": "/m2"}]}}}"#,
            r#"{"sets": {"session": {"mods": [], "configs": "x"}}}"#,
            r#"{"sets": {"session": {"mods": []}}, "other": 1}"#,
        ];
        for text in cases {
            write_sets(&root, text);
            let err = read_sets(&root).expect_err(text);
            assert!(err.starts_with(SETS_FILE), "{err}");
        }
    }

    #[test]
    fn a_mod_entry_that_fits_no_kind_says_what_a_mod_is() {
        let root = tmp("bad-entry");
        write_sets(&root, r#"{"sets": {"session": {"mods": [{"local": 5}]}}}"#);

        let err = read_sets(&root).expect_err("no kind fits");

        assert!(err.contains("\"workshop\""), "{err}");
        assert!(err.contains(r#"{"local":5}"#), "{err}");
    }

    #[test]
    fn a_local_name_that_cannot_be_a_link_name_is_refused() {
        let root = tmp("bad-name");
        for name in ["@Custom", "a/b", r"a\b", ""] {
            let json = serde_json::to_string(name).unwrap();
            write_sets(
                &root,
                &format!(
                    r#"{{"sets": {{"session": {{"mods": [{{"local": ".", "name": {json}}}]}}}}}}"#
                ),
            );

            let err = resolve_set(&root, "session", None, |_| true).expect_err(name);

            assert!(err.contains(&format!("\"{name}\"")), "{err}");
        }
    }

    #[test]
    fn a_file_saved_with_a_byte_order_mark_reads() {
        let root = tmp("bom");
        write_sets(&root, "\u{feff}{\"sets\": {\"session\": {\"mods\": []}}}");

        let sets = read_sets(&root).unwrap().expect("the file is there");

        assert!(sets.sets.contains_key("session"));
    }

    #[test]
    fn a_set_keeps_its_order_and_gives_local_mods_their_build() {
        let root = tmp("resolve");
        write_sets(
            &root,
            r#"{"sets": {"session": {
                "mods": [
                    {"workshop": "1559212036"},
                    {"local": "../../tests/SampleModTests"},
                    {"local": ".", "name": "Custom"},
                    {"folder": "/mods/@AdminTools"}
                ],
                "server_mods": [{"folder": "/mods/@ServerTools"}, {"local": "../../tests/SampleServer"}],
                "profile": "release"
            }}}"#,
        );
        let steam = Path::new("/steam");
        let found = [
            Path::new("/steam/steamapps/workshop/content/221100/1559212036").to_path_buf(),
            root.join("../../tests/SampleModTests/builds/release/@SampleModTests"),
            root.join("./builds/release/@Custom"),
            PathBuf::from("/mods/@AdminTools"),
            PathBuf::from("/mods/@ServerTools"),
            root.join("../../tests/SampleServer/builds/release/@SampleServer"),
        ];
        let exists = |path: &Path| found.contains(&path.to_path_buf());

        let set = resolve_set(&root, "session", Some(steam), exists).unwrap();

        let arg = |path: PathBuf, link_name: &str| TestModArg {
            path,
            link_name: link_name.to_owned(),
            role: TestModRole::Both,
        };
        assert_eq!(
            set.mods,
            [
                ResolvedMod {
                    arg: arg(found[0].clone(), "1559212036"),
                    local_root: None
                },
                ResolvedMod {
                    arg: arg(found[1].clone(), "SampleModTests"),
                    local_root: Some(root.join("../../tests/SampleModTests")),
                },
                ResolvedMod {
                    arg: arg(found[2].clone(), "Custom"),
                    local_root: Some(root.join(".")),
                },
                ResolvedMod {
                    arg: arg(found[3].clone(), "AdminTools"),
                    local_root: None
                },
            ]
        );
        assert_eq!(
            set.server_mods,
            [
                ResolvedMod {
                    arg: TestModArg {
                        path: found[4].clone(),
                        link_name: "ServerTools".to_owned(),
                        role: TestModRole::ServerOnly,
                    },
                    local_root: None,
                },
                ResolvedMod {
                    arg: TestModArg {
                        path: found[5].clone(),
                        link_name: "SampleServer".to_owned(),
                        role: TestModRole::ServerOnly,
                    },
                    local_root: Some(root.join("../../tests/SampleServer")),
                },
            ]
        );
    }

    #[test]
    fn an_unknown_set_lists_the_sets_the_file_has() {
        let root = tmp("unknown-set");
        let cases = [
            (
                r#"{"sets": {"session": {"mods": []}, "server": {"mods": []}}}"#,
                "server, session",
            ),
            (r#"{"sets": {}}"#, "it has no sets"),
        ];
        for (text, expected) in cases {
            write_sets(&root, text);
            let err = resolve_set(&root, "sesion", None, |_| true).unwrap_err();
            assert!(err.contains(expected), "{err}");
        }
    }

    #[test]
    fn a_project_without_the_file_says_where_it_looked() {
        let root = tmp("no-file");
        let missing = root.join("server/launch-sets.json");

        let err = resolve_set(&root, "session", None, |_| true).unwrap_err();

        assert!(err.contains(&missing.display().to_string()), "{err}");
    }

    #[test]
    fn a_workshop_mod_that_cannot_be_located_says_why() {
        let root = tmp("workshop");
        let cases = [
            (None, "1559212036", "Steam not found"),
            (Some(Path::new("/steam")), "1559212036x", "not a numeric id"),
        ];
        for (steam_root, id, expected) in cases {
            write_sets(
                &root,
                &format!(r#"{{"sets": {{"session": {{"mods": [{{"workshop": "{id}"}}]}}}}}}"#),
            );

            let err = resolve_set(&root, "session", steam_root, |_| true).unwrap_err();

            assert!(err.contains(expected), "{err}");
        }
    }

    #[test]
    fn every_missing_path_is_reported_in_one_error() {
        let root = tmp("missing");
        write_sets(
            &root,
            r#"{"sets": {"session": {"mods": [
                {"local": "../not-built"},
                {"folder": "/mods/@Gone"},
                {"workshop": "9999"}
            ]}}}"#,
        );

        let err = resolve_set(&root, "session", Some(Path::new("/steam")), |_| false)
            .expect_err("nothing is there");

        let local = root.join("../not-built/builds/default/@not-built");
        assert!(err.contains(&local.display().to_string()), "{err}");
        assert!(
            err.contains("build it first: mf build --set session"),
            "{err}"
        );
        assert!(err.contains("/mods/@Gone does not exist"), "{err}");
        assert!(
            err.contains("/steam/steamapps/workshop/content/221100/9999 does not exist"),
            "{err}"
        );
    }

    #[test]
    fn a_config_resolves_under_the_projects_server_folder() {
        let root = tmp("config");
        write_sets(
            &root,
            r#"{"sets": {"session": {"mods": [], "config": "serverDZ_expansion.cfg"}}}"#,
        );
        let config = root.join("server/serverDZ_expansion.cfg");

        let set = resolve_set(&root, "session", None, |path| path == config).unwrap();

        assert_eq!(set.config, Some(config));
    }

    #[test]
    fn a_missing_config_is_an_error_naming_it() {
        let root = tmp("config-missing");
        write_sets(
            &root,
            r#"{"sets": {"session": {"mods": [], "config": "nope.cfg"}}}"#,
        );

        let err = resolve_set(&root, "session", None, |_| false).unwrap_err();

        let config = root.join("server/nope.cfg");
        assert!(err.contains(&config.display().to_string()), "{err}");
    }
}
