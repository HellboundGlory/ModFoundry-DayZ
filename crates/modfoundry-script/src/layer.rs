//! Script module layer: the `Scripts/{1..5}_*` load-order dirs.

/// Canonical Enforce script module layer, in engine load order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Core = 1,
    GameLib,
    Game,
    World,
    Mission,
}

impl Layer {
    /// All layers in load order.
    pub const ALL: [Layer; 5] = [
        Layer::Core,
        Layer::GameLib,
        Layer::Game,
        Layer::World,
        Layer::Mission,
    ];

    /// The module directory name, e.g. `"3_Game"`.
    pub fn name(self) -> &'static str {
        match self {
            Layer::Core => "1_Core",
            Layer::GameLib => "2_GameLib",
            Layer::Game => "3_Game",
            Layer::World => "4_World",
            Layer::Mission => "5_Mission",
        }
    }

    /// The layer named by the first path segment that matches a module dir
    /// (case-insensitive; `/` and `\` both separate). `None` when the path is
    /// not inside any `N_*` module.
    pub fn from_path(path: &str) -> Option<Layer> {
        path.split(['/', '\\']).find_map(|seg| {
            Layer::ALL
                .into_iter()
                .find(|l| seg.eq_ignore_ascii_case(l.name()))
        })
    }
}
