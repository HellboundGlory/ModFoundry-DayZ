use enforce_check::target::{defines, layer_defines};
use enforce_check::{Layer, ModInfo, Module, Target};

fn vanilla_mods() -> Vec<ModInfo> {
    vec![
        ModInfo {
            name: "Probe".into(),
            cfg_class: "Probe_x".into(),
        },
        ModInfo {
            name: "DayZ".into(),
            cfg_class: "DayZ".into(),
        },
    ]
}

fn list(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn defines_per_target() {
    let cases: Vec<(Target, &[&str])> = vec![
        (
            Target::RetailServer,
            &[
                "DAYZ_1_30",
                "SERVER_FOR_WINDOWS",
                "SERVER",
                "PLATFORM_LINUX",
                "RELEASE",
                "NO_GUI",
                "NO_GUI_INGAME",
                "sakhal",
                "nasdara",
                "Probe_x",
                "DayZ",
                "FEATURE_CURSOR",
                "FEATURE_NETWORK_RECONCILIATION",
                "FEATURE_NETWORK_RECONCILATION",
            ],
        ),
        (
            Target::DiagServer,
            &[
                "DAYZ_1_30",
                "SERVER_FOR_WINDOWS",
                "SERVER",
                "PLATFORM_WINDOWS",
                "DIAG",
                "DIAG_DEVELOPER",
                "NO_GUI",
                "NO_GUI_INGAME",
                "ENABLE_LOGGING",
                "ROBOCLIENT",
                "INPUT_OVERRIDE",
                "sakhal",
                "nasdara",
                "Probe_x",
                "DayZ",
                "FEATURE_CURSOR",
                "FEATURE_NETWORK_RECONCILIATION",
                "FEATURE_NETWORK_RECONCILATION",
                "LOG_TO_FILE",
                "LOG_TO_SCRIPT",
            ],
        ),
        (
            Target::DiagClient,
            &[
                "DAYZ_1_30",
                "PLATFORM_WINDOWS",
                "DIAG",
                "DIAG_DEVELOPER",
                "ENABLE_LOGGING",
                "ROBOCLIENT",
                "INPUT_OVERRIDE",
                "sakhal",
                "nasdara",
                "Probe_x",
                "DayZ",
                "FEATURE_CURSOR",
                "FEATURE_NETWORK_RECONCILIATION",
                "FEATURE_NETWORK_RECONCILATION",
                "LOG_TO_FILE",
                "LOG_TO_SCRIPT",
            ],
        ),
    ];
    for (target, expected) in cases {
        assert_eq!(
            defines(target, &vanilla_mods(), &[]),
            list(expected),
            "{target:?}"
        );
    }
}

#[test]
fn extra_defines_follow_the_target_list() {
    let extra = list(&["EXTRA_ONE"]);
    let got = defines(Target::DiagClient, &[], &extra);
    assert_eq!(got.last().map(String::as_str), Some("EXTRA_ONE"));
    assert_eq!(got.len(), defines(Target::DiagClient, &[], &[]).len() + 1);
}

#[test]
fn mission_extras_per_layer() {
    assert_eq!(
        layer_defines(Target::RetailServer, Layer::Mission),
        &["ENABLE_CAROUSEL"]
    );
    assert_eq!(
        layer_defines(Target::DiagServer, Layer::Mission),
        &["ENABLE_CAROUSEL", "BROADCAST_BUNKER_SP_VIS_ENABLED"]
    );
    assert_eq!(
        layer_defines(Target::DiagClient, Layer::Mission),
        &["ENABLE_CAROUSEL", "BROADCAST_BUNKER_SP_VIS_ENABLED"]
    );
    for layer in [Layer::Core, Layer::GameLib, Layer::Game, Layer::World] {
        assert!(
            layer_defines(Target::DiagServer, layer).is_empty(),
            "{layer:?}"
        );
    }
}

#[test]
fn printed_module_names_round_trip() {
    let cases = [
        (Module::Core, "Core"),
        (Module::GameLib, "GameLib"),
        (Module::Game, "Game"),
        (Module::World, "World"),
        (Module::Mission, "Mission"),
    ];
    for (module, printed) in cases {
        assert_eq!(module.as_str(), printed);
        assert_eq!(Module::from_name(printed), Some(module));
    }
    // module_failed values with no module behind them.
    assert_eq!(Module::from_name("none"), None);
    assert_eq!(Module::from_name("CRASH"), None);
    assert_eq!(
        format!("Can't compile \"{}\" script module!", Module::Game.as_str()),
        "Can't compile \"Game\" script module!"
    );
}

#[test]
fn layer_folder_names() {
    let cases = [
        ("1_Core", Some(Layer::Core)),
        ("2_GameLib", Some(Layer::GameLib)),
        ("3_Game", Some(Layer::Game)),
        ("4_World", Some(Layer::World)),
        ("5_Mission", Some(Layer::Mission)),
        ("3_Game/", None),
        ("scripts", None),
    ];
    for (folder, expected) in cases {
        assert_eq!(Layer::from_folder_name(folder), expected, "{folder}");
    }
    assert_eq!(Module::from(Layer::World), Module::World);
}

#[test]
fn severity_adjustment() {
    use enforce_check::target::adjust;
    use enforce_check::{RuleKind, Severity};

    let cases = [
        (
            Target::RetailServer,
            RuleKind::Error,
            Severity::Error,
            false,
        ),
        (
            Target::RetailServer,
            RuleKind::Warning,
            Severity::Warning,
            false,
        ),
        (
            Target::RetailServer,
            RuleKind::Soft,
            Severity::Warning,
            true,
        ),
        (Target::DiagServer, RuleKind::Error, Severity::Error, false),
        (
            Target::DiagServer,
            RuleKind::Warning,
            Severity::Warning,
            false,
        ),
        (Target::DiagServer, RuleKind::Soft, Severity::Error, false),
        (Target::DiagClient, RuleKind::Error, Severity::Error, false),
        (
            Target::DiagClient,
            RuleKind::Warning,
            Severity::Warning,
            false,
        ),
        (Target::DiagClient, RuleKind::Soft, Severity::Error, false),
    ];
    for (target, kind, severity, fixme) in cases {
        assert_eq!(
            adjust(target, kind),
            (severity, fixme),
            "{target:?} {kind:?}"
        );
    }
}
