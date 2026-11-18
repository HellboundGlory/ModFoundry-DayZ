use modfoundry_layout_render::normalize_path;

#[test]
fn normalize_path_turns_backslashes_into_slashes_and_drops_a_guid_and_leading_slash() {
    assert_eq!(
        normalize_path(r"{5A1B2C}gui\Textures\X.edds"),
        "gui/Textures/X.edds"
    );
    assert_eq!(normalize_path("/gui/x.edds"), "gui/x.edds");
}
