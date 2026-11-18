//! Mod project skeleton generation for `mf new` and `project.scaffold`.
//!
//! Pure: fills the bundled `ModTemplate` with the mod's options and returns the
//! files to write, so callers (CLI, MCP, GUI) share one skeleton. `files[]`
//! entries carry the PBO prefix because that is how `mf build` addresses a
//! packed entry: an unprefixed `Scripts/3_Game` would resolve to nothing and
//! bind no scripts.

use crate::model::ModuleSlot;
use std::fmt::Write as _;

/// Module dir names in engine load order (compile order ascending).
pub const SCRIPT_MODULE_DIRS: [&str; 5] = ["1_Core", "2_GameLib", "3_Game", "4_World", "5_Mission"];

/// Why a PBO prefix is rejected, after the prefix itself.
pub const UNSAFE_PREFIX: &str =
    "is not a safe PBO prefix; use letters, digits, `_` and `-`, with `/` between folder names";
/// The rule every class name follows, in its refusal message.
const NAME_RULE: &str = "start with a letter or `_` and use only letters, digits and `_`";
/// Why a mod with no script modules is rejected.
pub const NO_MODULES: &str = "Pick at least one script module.";

/// A file the skeleton wants created: path is relative to the mod root.
#[derive(Debug, Clone, PartialEq)]
pub struct FileToWrite {
    pub path: String,
    pub contents: String,
}

/// What [`mod_skeleton`] fills the template with.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaffoldOptions {
    pub name: String,
    pub display_name: Option<String>,
    pub description: String,
    pub author: String,
    pub version: String,
    pub prefix: Option<String>,
    /// The CfgPatches class name; defaults to `<name>_Scripts`.
    pub patch_class: Option<String>,
    /// CfgPatches names listed in requiredAddons after DZ_Data.
    pub required_addons: Vec<String>,
    pub modules: Vec<String>,
    pub starter: bool,
}

impl ScaffoldOptions {
    /// The default options for a mod named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: None,
            description: String::new(),
            author: String::new(),
            version: "0.1.0".into(),
            prefix: None,
            patch_class: None,
            required_addons: Vec::new(),
            modules: vec!["3_Game".into(), "4_World".into(), "5_Mission".into()],
            starter: true,
        }
    }
}

const MOD_CPP: &str = include_str!("../templates/ModTemplate/mod.cpp");
const CONFIG_CPP: &str = include_str!("../templates/ModTemplate/config.cpp");
const GITIGNORE: &str = include_str!("../templates/ModTemplate/gitignore");
const STARTER: &str = include_str!("../templates/ModTemplate/Scripts/5_Mission/Mission.c");

/// The only characters allowed in a mod name, slug or PBO prefix.
/// A mod name is also a config class name: a letter or `_` first, then letters, digits and `_`.
pub fn is_safe_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A PBO prefix: one or more `/`-separated folder names of letters, digits, `_` and `-`.
pub fn is_safe_prefix(prefix: &str) -> bool {
    prefix.split('/').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// Build the skeleton a mod's options describe.
///
/// Returns an error for an unsafe name or prefix, an unknown module, or no
/// modules, so callers can prompt before any write.
pub fn mod_skeleton(options: &ScaffoldOptions) -> Result<Vec<FileToWrite>, String> {
    let name = options.name.as_str();
    if !is_safe_name(name) {
        return Err(format!("`{name}` is not a safe mod name; {NAME_RULE}"));
    }
    let prefix = options.prefix.as_deref().unwrap_or(name);
    if !is_safe_prefix(prefix) {
        return Err(format!("`{prefix}` {UNSAFE_PREFIX}"));
    }
    let patch_class = options
        .patch_class
        .clone()
        .unwrap_or_else(|| format!("{name}_Scripts"));
    if let Some(class) = std::iter::once(&patch_class)
        .chain(&options.required_addons)
        .find(|class| !is_safe_name(class))
    {
        return Err(format!("`{class}` is not a safe class name; {NAME_RULE}"));
    }
    if options.modules.is_empty() {
        return Err(NO_MODULES.to_owned());
    }
    if let Some(unknown) = options
        .modules
        .iter()
        .find(|m| !SCRIPT_MODULE_DIRS.contains(&m.as_str()))
    {
        return Err(format!(
            "`{unknown}` is not a script module; use one of {}",
            SCRIPT_MODULE_DIRS.join(", ")
        ));
    }
    let modules: Vec<&str> = SCRIPT_MODULE_DIRS
        .iter()
        .copied()
        .filter(|dir| options.modules.iter().any(|m| m.as_str() == *dir))
        .collect();

    let display_name = options.display_name.as_deref().unwrap_or(name);
    let mut out = vec![
        FileToWrite {
            path: "mod.cpp".into(),
            contents: fill(
                MOD_CPP,
                &[
                    ("{{DisplayName}}", escape(display_name)),
                    ("{{Tooltip}}", escape(first_line(&options.description))),
                    ("{{Overview}}", escape(&options.description)),
                    ("{{Author}}", escape(&options.author)),
                    ("{{Version}}", escape(&options.version)),
                ],
            ),
        },
        FileToWrite {
            path: "config.cpp".into(),
            contents: fill(
                CONFIG_CPP,
                &[
                    ("{{PatchClass}}", patch_class),
                    (
                        "{{RequiredAddons}}",
                        required_addons_list(&options.required_addons),
                    ),
                    ("{{ModName}}", name.to_owned()),
                    ("{{Version}}", escape(&options.version)),
                    ("{{ScriptModuleDefs}}", module_defs(prefix, &modules)),
                ],
            ),
        },
        FileToWrite {
            path: ".gitignore".into(),
            contents: GITIGNORE.to_owned(),
        },
    ];
    for dir in modules {
        out.push(if dir == "5_Mission" && options.starter {
            FileToWrite {
                path: format!("Scripts/{dir}/MissionServer.c"),
                contents: fill(STARTER, &[("{{ModName}}", name.to_owned())]),
            }
        } else {
            FileToWrite {
                path: format!("Scripts/{dir}/.gitkeep"),
                contents: String::new(),
            }
        });
    }
    Ok(out)
}

/// One pass over the template, so placeholder text inside a value stays literal.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        match values
            .iter()
            .find(|(placeholder, _)| rest.starts_with(placeholder))
        {
            Some((placeholder, value)) => {
                out.push_str(value);
                rest = &rest[placeholder.len()..];
            }
            None => {
                out.push_str("{{");
                rest = &rest[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One line, with each quote written as two, as a DayZ config string needs.
fn escape(value: &str) -> String {
    one_line(value).replace('"', "\"\"")
}

fn first_line(value: &str) -> &str {
    value.split(['\n', '\r']).next().unwrap_or("")
}

fn one_line(value: &str) -> String {
    value.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

fn required_addons_list(addons: &[String]) -> String {
    let mut listed: Vec<&str> = Vec::new();
    for addon in std::iter::once("DZ_Data").chain(addons.iter().map(String::as_str)) {
        if !listed.contains(&addon) {
            listed.push(addon);
        }
    }
    listed
        .iter()
        .map(|addon| format!("\"{addon}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn module_defs(prefix: &str, modules: &[&str]) -> String {
    let mut out = String::new();
    for (i, dir) in modules.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let _ = write!(
            out,
            "            class {}\n            {{\n                files[] = {{ \"{}/Scripts/{dir}\" }};\n            }};",
            defs_name(dir),
            escape(prefix),
        );
    }
    out
}

/// Map a `Scripts/N_Name` dir back to its `defs` slot name (`3_Game` →
/// `gameScriptModule`).
fn defs_name(dir: &str) -> String {
    let slot = match dir {
        "1_Core" => ModuleSlot::Engine,
        "2_GameLib" => ModuleSlot::GameLib,
        "3_Game" => ModuleSlot::Game,
        "4_World" => ModuleSlot::World,
        "5_Mission" => ModuleSlot::Mission,
        _ => unreachable!("only built-in module dirs are scaffolded"),
    };
    let prefix = match slot {
        ModuleSlot::Engine => "engine",
        ModuleSlot::GameLib => "gameLib",
        ModuleSlot::Game => "game",
        ModuleSlot::World => "world",
        ModuleSlot::Mission => "mission",
    };
    format!("{prefix}ScriptModule")
}
