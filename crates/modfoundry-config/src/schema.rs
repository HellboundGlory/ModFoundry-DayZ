//! DayZ config property schema: groups, units, file types and array-ness
//! for the properties a modder edits, keyed by top-level config root.
//!
//! The root-agnostic rows use the fallback root `*`, which is what lets the
//! same item-property table serve `CfgWeapons`/`CfgMagazines`/`CfgAmmo`; a
//! root-specific row, such as `CfgAmmo` `weight`, wins over the fallback.

/// Grid grouping for a known property.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Group {
    Identity,
    ModelAndTextures,
    Inventory,
    Liquid,
    Damage,
    Sound,
    Script,
    Mod,
    Patch,
    Other,
}

/// One known property: grouping, array-ness, unit, enum labels and file types.
pub struct PropertyInfo {
    pub name: &'static str,
    pub group: Group,
    /// `Some(true)`/`Some(false)` when every corpus occurrence is spelled that
    /// way, `None` when the corpus spells it both ways.
    pub array: Option<bool>,
    pub unit: Option<&'static str>,
    pub enum_labels: &'static [(&'static str, &'static str)],
    pub file_ext: &'static [&'static str],
    pub summary: &'static str,
}

struct Entry {
    root: &'static str,
    info: PropertyInfo,
}

static TABLE: &[Entry] = &[
    Entry {
        root: "CfgPatches",
        info: PropertyInfo {
            name: "units",
            group: Group::Patch,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Class names of the entities this addon adds",
        },
    },
    Entry {
        root: "CfgPatches",
        info: PropertyInfo {
            name: "weapons",
            group: Group::Patch,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Weapon classes the addon owns",
        },
    },
    Entry {
        root: "CfgPatches",
        info: PropertyInfo {
            name: "requiredAddons",
            group: Group::Patch,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Addons that have to load before this one",
        },
    },
    Entry {
        root: "CfgPatches",
        info: PropertyInfo {
            name: "requiredVersion",
            group: Group::Patch,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Minimum game version the addon needs",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "type",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Mod kind; only \"mod\" occurs in the corpus",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "name",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Name shown in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "dir",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Mod folder key",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "author",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Mod author",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "authorID",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Author Steam ID",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "version",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Mod version string",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "credits",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Credits screen entry, read by Community Framework",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "creditsJson",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Path to a credits JSON file, read by Community Framework",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "extra",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Extra numeric field shown in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "hideName",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Hide the mod name in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "hidePicture",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Hide the mod picture in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "picture",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &["paa", "edds"],
            summary: "Picture shown in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "logo",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &["paa", "edds"],
            summary: "Logo drawn in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "logoSmall",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &["paa", "edds"],
            summary: "Small logo drawn in the mod list",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "logoOver",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &["paa", "edds"],
            summary: "Logo drawn while the cursor hovers the mod",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "action",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Link opened from the mod list entry",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "tooltip",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Tooltip for the mod list entry",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "overview",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Overview text for the mod list entry",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "inputs",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Path to the mod's input bindings",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "versionPath",
            group: Group::Mod,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Path to the mod's version file",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "dependencies",
            group: Group::Mod,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Script modules the mod extends, such as Game or World",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "defines",
            group: Group::Mod,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Preprocessor defines passed to the scripts",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "files",
            group: Group::Script,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Files a defs entry loads",
        },
    },
    Entry {
        root: "CfgMods",
        info: PropertyInfo {
            name: "value",
            group: Group::Script,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Value of a script module slot",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "scope",
            group: Group::Identity,
            array: Some(false),
            unit: None,
            enum_labels: &[("0", "Private"), ("1", "Protected"), ("2", "Public")],
            file_ext: &[],
            summary: "Visibility of the class to the engine",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "displayName",
            group: Group::Identity,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Name shown in the inventory and dialogs",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "descriptionShort",
            group: Group::Identity,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "One-line description shown in the inventory",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "model",
            group: Group::ModelAndTextures,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &["p3d"],
            summary: "Model file of the entity",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "hiddenSelections",
            group: Group::ModelAndTextures,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Named model selections that scripts can retexture",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "hiddenSelectionsTextures",
            group: Group::ModelAndTextures,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &["paa", "edds"],
            summary: "Textures applied to the hidden selections",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "hiddenSelectionsMaterials",
            group: Group::ModelAndTextures,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &["rvmat"],
            summary: "Materials applied to the hidden selections",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "weight",
            group: Group::Inventory,
            array: Some(false),
            unit: Some("g"),
            enum_labels: &[],
            file_ext: &[],
            summary: "Item weight in grams",
        },
    },
    Entry {
        root: "CfgAmmo",
        info: PropertyInfo {
            name: "weight",
            group: Group::Other,
            array: Some(false),
            unit: Some("kg"),
            enum_labels: &[],
            file_ext: &[],
            summary: "Projectile mass in kilograms",
        },
    },
    Entry {
        root: "CfgCloudlets",
        info: PropertyInfo {
            name: "weight",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Particle weight among the ones a cloudlet draws",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "itemSize",
            group: Group::Inventory,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Inventory footprint in slots",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "itemsCargoSize",
            group: Group::Inventory,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Cargo grid size of a container item",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "inventorySlot",
            group: Group::Inventory,
            array: None,
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Slots the item occupies or fits",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "rotationFlags",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Bitmask of the inventory rotations the engine allows",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "attachments",
            group: Group::Inventory,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Attachment slot names of the item",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "canBeSplit",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Stack can be split by hand in the inventory",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varStackMax",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Maximum stack size",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "stackedUnit",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Unit shown for a stack's quantity",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "quantityBar",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Show a quantity bar in the inventory",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varQuantityInit",
            group: Group::Inventory,
            array: Some(false),
            unit: Some("ml"),
            enum_labels: &[],
            file_ext: &[],
            summary: "Amount the item starts with",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varQuantityMin",
            group: Group::Inventory,
            array: Some(false),
            unit: Some("ml"),
            enum_labels: &[],
            file_ext: &[],
            summary: "Lowest amount the item can hold",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varQuantityMax",
            group: Group::Inventory,
            array: Some(false),
            unit: Some("ml"),
            enum_labels: &[],
            file_ext: &[],
            summary: "Most the item can hold",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varLiquidTypeInit",
            group: Group::Liquid,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Initial liquid type",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "liquidContainerType",
            group: Group::Liquid,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Liquid class expression of the container, as written",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "destroyOnEmpty",
            group: Group::Liquid,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Delete the item once emptied",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varQuantityDestroyOnMin",
            group: Group::Inventory,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Delete the item when its quantity reaches the minimum",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "repairableWithKits",
            group: Group::Damage,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Repair kit types that can repair the item",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "repairCosts",
            group: Group::Damage,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Kit quantity a repair uses, in the order of repairableWithKits",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "canBeDigged",
            group: Group::Damage,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Container can be buried in the ground with a shovel",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "isMeleeWeapon",
            group: Group::Damage,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Treat the item as a melee weapon",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "soundImpactType",
            group: Group::Sound,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Surface type used for impact sounds",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "simulation",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Simulation handler of the entity",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureInit",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Initial temperature",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureMin",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Minimum temperature",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureMax",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Maximum temperature",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureFreezePoint",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Temperature at which the item freezes",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureFreezeTime",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Seconds the item takes to freeze",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureThawPoint",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Temperature at which the item thaws",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureThawTime",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Seconds the item takes to thaw",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varTemperatureOverheatTime",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Seconds the item takes to overheat",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "absorbency",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "How wet the item gets per rain exposure",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "lootCategory",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Loot category tag",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "storageCategory",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Storage category tag",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "slopeTolerance",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Steepest slope the item can be placed on",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "mapSize",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Size of the entity's icon on the map",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "audible",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "How loud the entity's sound is",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "visibilityModifier",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "How far a worn item makes the wearer stand out",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "itemInfo",
            group: Group::Other,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Category and slot tags such as Clothing, Hips",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "heatIsolation",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Heat insulation of a worn item, 0 to 1",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "varWetMax",
            group: Group::Other,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Highest wetness the item reaches, up to 1",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "vehicleClass",
            group: Group::Identity,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Class group, such as \"Clothing\"",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "lootTag",
            group: Group::Other,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Loot spawn tags such as Civilian",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "hitpoints",
            group: Group::Damage,
            array: Some(false),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Health points of the item or zone",
        },
    },
    Entry {
        root: "*",
        info: PropertyInfo {
            name: "color",
            group: Group::Other,
            array: None,
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Variant colour name on items; lights and ammo use an RGBA array",
        },
    },
    Entry {
        root: "CfgWeapons",
        info: PropertyInfo {
            name: "magazines",
            group: Group::Other,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Magazine classes the weapon accepts",
        },
    },
    Entry {
        root: "CfgWeapons",
        info: PropertyInfo {
            name: "chamberableFrom",
            group: Group::Other,
            array: Some(true),
            unit: None,
            enum_labels: &[],
            file_ext: &[],
            summary: "Ammo classes that can be loaded straight into the chamber",
        },
    },
];

/// The order the grid renders groups in.
pub const GROUP_ORDER: [Group; 10] = [
    Group::Identity,
    Group::ModelAndTextures,
    Group::Inventory,
    Group::Liquid,
    Group::Damage,
    Group::Sound,
    Group::Script,
    Group::Mod,
    Group::Patch,
    Group::Other,
];

/// Looks up `name` under top-level class `root`, case-insensitively, falling
/// back to the root-agnostic `*` rows.
pub fn lookup(root: &str, name: &str) -> Option<&'static PropertyInfo> {
    let mut fallback = None;
    for entry in TABLE {
        if !entry.info.name.eq_ignore_ascii_case(name) {
            continue;
        }
        if entry.root == "*" {
            fallback = Some(&entry.info);
        } else if entry.root.eq_ignore_ascii_case(root) {
            return Some(&entry.info);
        }
    }
    fallback
}

/// Human label for a group, as the grid renders it.
pub fn group_label(group: Group) -> &'static str {
    match group {
        Group::Identity => "Identity",
        Group::ModelAndTextures => "Model and textures",
        Group::Inventory => "Inventory",
        Group::Liquid => "Liquid",
        Group::Damage => "Damage",
        Group::Sound => "Sound",
        Group::Script => "Script",
        Group::Mod => "Mod",
        Group::Patch => "Patch",
        Group::Other => "Other",
    }
}

/// Every table row as `(root, info)`; `*` is the fallback root.
#[doc(hidden)]
pub fn rows() -> impl Iterator<Item = (&'static str, &'static PropertyInfo)> {
    TABLE.iter().map(|entry| (entry.root, &entry.info))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: Option<&'static PropertyInfo>, b: Option<&'static PropertyInfo>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(same(
            lookup("CfgVehicles", "scope"),
            lookup("cfgvehicles", "SCOPE")
        ));
        assert!(same(
            lookup("CFGMODS", "dependencies"),
            lookup("CfgMods", "dependencies")
        ));
        assert!(lookup("CfgVehicles", "scope").is_some());
    }

    #[test]
    fn fallback_root_serves_the_item_roots() {
        let scope = lookup("CfgWeapons", "scope");
        assert!(scope.is_some());
        assert!(same(scope, lookup("CfgMagazines", "scope")));
        assert!(same(scope, lookup("CfgAmmo", "scope")));
        assert!(same(scope, lookup("CfgVehicles", "scope")));
    }

    #[test]
    fn root_specific_rows_do_not_leak_to_other_roots() {
        assert!(lookup("CfgVehicles", "units").is_none());
        assert!(lookup("CfgWeapons", "units").is_none());
        assert!(lookup("CfgVehicles", "dependencies").is_none());
        assert!(lookup("CfgPatches", "dependencies").is_none());
    }

    #[test]
    fn unknown_name_gives_none() {
        assert!(lookup("CfgVehicles", "definitelyNotAProperty").is_none());
        assert!(lookup("CfgNoSuchRoot", "definitelyNotAProperty").is_none());
    }

    #[test]
    fn group_labels_are_sentence_case() {
        for group in GROUP_ORDER {
            let label = group_label(group);
            assert!(!label.is_empty() && !label.contains('_'), "{label:?}");
            assert!(label.chars().next().is_some_and(|c| c.is_uppercase()));
        }
    }

    #[test]
    fn table_rows_are_canonical_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (root, info) in rows() {
            assert!(root == "*" || root.starts_with("Cfg"), "{root}");
            assert!(!info.name.contains('['), "{} carries brackets", info.name);
            assert!(!info.summary.is_empty(), "{} has no summary", info.name);
            assert!(
                seen.insert((root, info.name)),
                "duplicate {root}:{}",
                info.name
            );
        }
    }

    #[test]
    fn vanilla_item_properties_have_rows() {
        let rows = [
            ("CfgCloudlets", "weight", Group::Other, Some(false), None),
            (
                "CfgVehicles",
                "visibilityModifier",
                Group::Other,
                Some(false),
                None,
            ),
            ("CfgVehicles", "itemInfo", Group::Other, Some(true), None),
            (
                "CfgVehicles",
                "heatIsolation",
                Group::Other,
                Some(false),
                None,
            ),
            ("CfgVehicles", "varWetMax", Group::Other, Some(false), None),
            (
                "CfgVehicles",
                "vehicleClass",
                Group::Identity,
                Some(false),
                None,
            ),
            ("CfgVehicles", "lootTag", Group::Other, Some(true), None),
            ("CfgVehicles", "hitpoints", Group::Damage, Some(false), None),
            ("CfgVehicles", "color", Group::Other, None, None),
            ("CfgWeapons", "magazines", Group::Other, Some(true), None),
            (
                "CfgWeapons",
                "chamberableFrom",
                Group::Other,
                Some(true),
                None,
            ),
        ];
        for (root, name, group, array, unit) in rows {
            let info = lookup(root, name).unwrap_or_else(|| panic!("{root}:{name} has no row"));
            assert_eq!(info.group, group, "{root}:{name}");
            assert_eq!(info.array, array, "{root}:{name}");
            assert_eq!(info.unit, unit, "{root}:{name}");
        }
    }

    #[test]
    fn weapon_only_rows_stay_in_cfgweapons() {
        assert!(lookup("CfgWeapons", "magazines").is_some());
        assert!(lookup("CfgWeapons", "chamberableFrom").is_some());
        assert!(lookup("CfgVehicles", "magazines").is_none());
        assert!(lookup("CfgMagazines", "chamberableFrom").is_none());
        assert!(lookup("CfgAmmo", "magazines").is_none());
    }

    #[test]
    fn cloudlet_weight_is_not_in_grams() {
        let cloudlet = lookup("cfgcloudlets", "weight").unwrap();
        assert_eq!(cloudlet.unit, None);
        assert_eq!(cloudlet.group, Group::Other);
        assert_eq!(lookup("CfgWeapons", "weight").unwrap().unit, Some("g"));
        assert_eq!(lookup("CfgAmmo", "weight").unwrap().unit, Some("kg"));
    }

    #[test]
    fn measured_content_is_present() {
        let scope = lookup("CfgVehicles", "scope").unwrap();
        assert_eq!(scope.group, Group::Identity);
        assert_eq!(scope.array, Some(false));
        assert_eq!(
            scope.enum_labels,
            &[("0", "Private"), ("1", "Protected"), ("2", "Public")]
        );
        assert_eq!(
            lookup("CfgVehicles", "displayName").unwrap().group,
            Group::Identity
        );

        let model = lookup("CfgVehicles", "model").unwrap();
        assert_eq!(model.group, Group::ModelAndTextures);
        assert_eq!(model.array, Some(false));
        assert_eq!(model.file_ext, &["p3d"]);

        let textures = lookup("CfgVehicles", "hiddenSelectionsTextures").unwrap();
        assert_eq!(textures.array, Some(true));
        assert_eq!(textures.file_ext, &["paa", "edds"]);
        assert_eq!(
            lookup("CfgVehicles", "hiddenSelectionsMaterials")
                .unwrap()
                .file_ext,
            &["rvmat"]
        );

        let weight = lookup("CfgMagazines", "weight").unwrap();
        assert_eq!(weight.group, Group::Inventory);
        assert_eq!(weight.unit, Some("g"));
        assert_eq!(lookup("CfgAmmo", "weight").unwrap().unit, Some("kg"));
        assert_eq!(lookup("CfgVehicles", "itemSize").unwrap().array, Some(true));
        assert_eq!(lookup("CfgVehicles", "inventorySlot").unwrap().array, None);

        let quantity = lookup("CfgVehicles", "varQuantityMax").unwrap();
        assert_eq!(quantity.group, Group::Inventory);
        assert_eq!(quantity.unit, Some("ml"));
        assert_eq!(
            lookup("CfgVehicles", "liquidContainerType").unwrap().unit,
            None
        );
        assert_eq!(
            lookup("CfgVehicles", "repairableWithKits").unwrap().group,
            Group::Damage
        );
        assert_eq!(
            lookup("CfgWeapons", "soundImpactType").unwrap().group,
            Group::Sound
        );
        assert_eq!(
            lookup("CfgVehicles", "simulation").unwrap().group,
            Group::Other
        );

        let units = lookup("CfgPatches", "units").unwrap();
        assert_eq!(units.group, Group::Patch);
        assert_eq!(units.array, Some(true));
        assert_eq!(lookup("CfgPatches", "weapons").unwrap().array, Some(true));
        assert_eq!(
            lookup("CfgPatches", "requiredAddons").unwrap().array,
            Some(true)
        );
        assert_eq!(
            lookup("CfgPatches", "requiredVersion").unwrap().array,
            Some(false)
        );

        let kind = lookup("CfgMods", "type").unwrap();
        assert_eq!(kind.group, Group::Mod);
        assert!(kind.enum_labels.is_empty());
        assert_eq!(lookup("CfgMods", "dependencies").unwrap().array, Some(true));
        assert_eq!(lookup("CfgMods", "defines").unwrap().array, Some(true));
        assert_eq!(
            lookup("CfgMods", "logo").unwrap().file_ext,
            &["paa", "edds"]
        );

        let files = lookup("CfgMods", "files").unwrap();
        assert_eq!(files.group, Group::Script);
        assert_eq!(files.array, Some(true));
        assert_eq!(lookup("CfgMods", "value").unwrap().array, Some(false));
    }
}
