use modfoundry_config::validate_text;

#[test]
fn lowercase_root_classes_are_still_checked() {
    let text = "class cfgpatches { class M { requiredAddons[] = {}; }; };\n\
                class cfgmods { class M { class Defs { class worldscriptmodule {\n\
                    Files[] = { \"M/Scripts/3_Game\" };\n\
                }; }; }; };\n";
    let codes: Vec<String> = validate_text(text, None)
        .diagnostics
        .iter()
        .map(|d| d.code.clone())
        .collect();
    assert!(
        codes.iter().any(|c| c == "MF-C310"),
        "expected MF-C310, got {codes:?}"
    );
    assert!(
        !codes.iter().any(|c| c == "MF-C200"),
        "unexpected MF-C200, got {codes:?}"
    );
}

fn c350(text: &str) -> Vec<String> {
    validate_text(text, None)
        .diagnostics
        .iter()
        .filter(|d| d.code == "MF-C350")
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn new_rows_drive_the_array_lint() {
    let cases = [
        (
            "class CfgVehicles { class X { itemInfo = \"Clothing\"; }; };\n",
            Some("`itemInfo` is an array in DayZ's own configs; write `itemInfo[]`."),
        ),
        (
            "class CfgVehicles { class X { heatIsolation[] = {1}; }; };\n",
            Some("`heatIsolation` is a scalar in DayZ's own configs; write `heatIsolation`."),
        ),
        (
            "class CfgVehicles { class X { lootTag = \"Civilian\"; }; };\n",
            Some("`lootTag` is an array in DayZ's own configs; write `lootTag[]`."),
        ),
        (
            "class CfgWeapons { class X { magazines = \"Mag\"; }; };\n",
            Some("`magazines` is an array in DayZ's own configs; write `magazines[]`."),
        ),
        (
            "class CfgVehicles { class X { color = \"Green\"; }; };\n",
            None,
        ),
        (
            "class CfgVehicles { class X { color[] = {1,1,1,1}; }; };\n",
            None,
        ),
        (
            "class CfgVehicles { class X { hitpoints = 100; }; };\n",
            None,
        ),
        (
            "class CfgVehicles { class X { hitpoints[] = {100}; }; };\n",
            Some("`hitpoints` is a scalar in DayZ's own configs; write `hitpoints`."),
        ),
        (
            "class CfgCloudlets { class Smoke { weight = 5; }; };\n",
            None,
        ),
        (
            "class CfgVehicles { class X { itemInfo[] = {\"Clothing\"}; }; };\n",
            None,
        ),
    ];
    for (text, expected) in cases {
        let messages = c350(text);
        match expected {
            Some(message) => assert_eq!(messages, vec![message.to_owned()], "{text}"),
            None => assert!(messages.is_empty(), "{text}: {messages:?}"),
        }
    }
}
