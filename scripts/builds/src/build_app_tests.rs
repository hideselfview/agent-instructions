use super::*;
use crate::app_config::parse_config;
use crate::testing::Scratch;

#[test]
fn the_copy_replaces_the_test_app() {
    let scratch = Scratch::new("build-app-copy");
    let places = scratch.places();
    let config = parse_config("mac build b\nmac app out/out.app\nmac bundle demo.test\n").unwrap();
    let app = copy_path(&places, "demo", config.mac.as_ref().unwrap()).unwrap();
    assert_eq!(app, places.home.join("Applications/demo-test/out.app"));
    std::fs::create_dir_all(app.join("old")).unwrap();

    let built = scratch.dir.join("built/out.app");
    std::fs::create_dir_all(built.join("new")).unwrap();
    copy_out(&built, &app).unwrap();
    assert!(app.join("old").exists(), "the test app stays until its replacement is ready");
    replace(&app).unwrap();
    assert!(app.join("new").exists());
    assert!(!app.join("old").exists());
    assert!(!with_new(&app).exists());
}

#[test]
fn a_platform_the_config_lacks_is_refused() {
    let config = parse_config("android build ./a.sh\n").unwrap();
    assert_eq!(Platform::Android.build(&config).unwrap(), ["./a.sh".to_string()]);
    assert!(Platform::Mac.build(&config).is_err());
    assert!(Platform::parse("ios").is_err());
}
