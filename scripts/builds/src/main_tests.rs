use super::holds_for_life;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// An app build holds a use from its start, as every lane's build does, so an
/// install that moves a store waits for the build to record its run. The
/// forms that read or write in one transaction hold none.
#[test]
fn an_app_build_holds_a_use_for_its_life_and_its_short_forms_hold_none() {
    for build in [
        "bae main",
        "bae main mac-tests",
        "bae main --shots mac.welcome.",
        "bae --lane mac main",
        "bae main --all-shots mac",
    ] {
        assert!(
            holds_for_life("app-build", &words(build)),
            "app-build {build} holds no use"
        );
    }
    for short in [
        "--landing-build bae main",
        "--drop-shots bae main mac.welcome. why",
        "bae --list",
    ] {
        assert!(
            !holds_for_life("app-build", &words(short)),
            "app-build {short} holds a use"
        );
    }
}
