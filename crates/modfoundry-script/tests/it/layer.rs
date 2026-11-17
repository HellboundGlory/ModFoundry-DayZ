use modfoundry_script::Layer;

#[test]
fn from_path_finds_the_module_folder() {
    for (path, expected) in [
        (r"scripts\4_World\entities\a.c", Some(Layer::World)),
        ("JM/CF/Scripts/1_Core/x.c", Some(Layer::Core)),
        ("3_game/a.c", Some(Layer::Game)),
        ("stray.c", None),
    ] {
        assert_eq!(Layer::from_path(path), expected, "{path}");
    }
}

#[test]
fn load_order() {
    assert!(Layer::Core < Layer::Mission);
    assert!(Layer::GameLib < Layer::Game);
    assert_eq!(Layer::Mission.name(), "5_Mission");
    assert_eq!(Layer::Core.name(), "1_Core");
}
