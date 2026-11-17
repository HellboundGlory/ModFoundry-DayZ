use modfoundry_platform::steam::{SteamError, SteamHelper, SteamRequest, SteamRun};
use modfoundry_platform::{discover_steam_installations, find_dayz_installs, set_host_discovery};

#[test]
fn switching_host_discovery_off_finds_no_steam_and_no_dayz_and_starts_no_helper() {
    set_host_discovery(false);

    assert!(discover_steam_installations().is_empty());
    assert!(find_dayz_installs().0.is_none());
    let helper = SteamHelper::new(std::env::current_exe().unwrap());
    let result = helper.run(&SteamRequest::Account, &mut |_, _, _| {});
    assert!(matches!(result, Err(SteamError::Spawn(_))), "{result:?}");
}
