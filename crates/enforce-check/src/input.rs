use serde::{Deserialize, Serialize};

use crate::target::Target;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layer {
    Core,
    GameLib,
    Game,
    World,
    Mission,
}

impl Layer {
    // The order the game compiles in: a later layer sees what the earlier ones
    // declared.
    pub const ALL: [Layer; 5] = [
        Layer::Core,
        Layer::GameLib,
        Layer::Game,
        Layer::World,
        Layer::Mission,
    ];

    pub fn from_folder_name(name: &str) -> Option<Layer> {
        match name {
            "1_Core" => Some(Layer::Core),
            "2_GameLib" => Some(Layer::GameLib),
            "3_Game" => Some(Layer::Game),
            "4_World" => Some(Layer::World),
            "5_Mission" => Some(Layer::Mission),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Module {
    Core,
    GameLib,
    Game,
    World,
    Mission,
}

impl Module {
    pub fn as_str(self) -> &'static str {
        match self {
            Module::Core => "Core",
            Module::GameLib => "GameLib",
            Module::Game => "Game",
            Module::World => "World",
            Module::Mission => "Mission",
        }
    }

    pub fn from_name(name: &str) -> Option<Module> {
        match name {
            "Core" => Some(Module::Core),
            "GameLib" => Some(Module::GameLib),
            "Game" => Some(Module::Game),
            "World" => Some(Module::World),
            "Mission" => Some(Module::Mission),
            _ => None,
        }
    }
}

impl From<Layer> for Module {
    fn from(layer: Layer) -> Module {
        match layer {
            Layer::Core => Module::Core,
            Layer::GameLib => Module::GameLib,
            Layer::Game => Module::Game,
            Layer::World => Module::World,
            Layer::Mission => Module::Mission,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub layer: Layer,
    pub mod_index: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModInfo {
    pub name: String,
    pub cfg_class: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckInput {
    pub target: Target,
    pub mods: Vec<ModInfo>,
    pub files: Vec<SourceFile>,
    pub extra_defines: Vec<String>,
    // The input names the mods register in their `inputs.xml`, which a script
    // may use as a number.
    #[serde(default)]
    pub input_names: Vec<String>,
}
