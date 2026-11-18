use super::*;

#[test]
fn a_screen_name_has_an_area_a_screen_and_maybe_a_state() {
    for text in [
        "mac.welcome.create-name",
        "mac.welcome.libraries.none",
        "android.welcome.libraries",
    ] {
        let name = Name::parse(text).unwrap();
        assert!(matches!(name, Name::Screen(_)), "{text}");
        assert_eq!(name.to_string(), text);
    }
}

#[test]
fn a_component_name_names_its_component_and_state() {
    let Name::Component(c) = Name::parse("android.component.cast-row.long-name").unwrap() else {
        panic!("not a component");
    };
    assert_eq!(c.component(), "AndroidCastRow");
    assert_eq!(c.state(), "long-name");
    let Name::Component(c) = Name::parse("mac.component.row.nested-off").unwrap() else {
        panic!("not a component");
    };
    assert_eq!(c.component(), "MacRow");
}

#[test]
fn names_that_are_not_boards_are_refused() {
    for text in [
        "",
        "appearance",
        "welcome.libraries",
        "windows.welcome.libraries",
        "mac.welcome",
        "mac.a.b.c.d",
        "mac.Welcome.libraries",
        "mac.welcome.-libraries",
        "mac.welcome..libraries",
        "mac.welcome.libraries ",
        "mac.component.row",
        "mac.component.row.a.b",
    ] {
        assert!(Name::parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn a_preview_row_name_becomes_its_state_id() {
    assert_eq!(state_id("Nested, off"), "nested-off");
    assert_eq!(state_id("Opens a page"), "opens-a-page");
    assert_eq!(state_id("It's  on!"), "it-s-on");
    assert_eq!(state_id("Row 2"), "row-2");
}

#[test]
fn a_shot_writes_files_named_after_its_board_and_platform() {
    let mac = Name::parse("mac.welcome.libraries.none").unwrap();
    assert_eq!(mac.file_name("png"), "mac.welcome.libraries.none@macos.png");
    let android = Name::parse("android.component.button.filled").unwrap();
    assert_eq!(
        android.file_name("json"),
        "android.component.button.filled@android.json"
    );
}

#[test]
fn a_shot_file_is_read_back_to_its_name() {
    let name = Name::from_file_name(Platform::Mac, "mac.welcome.libraries@macos.json");
    assert_eq!(
        name.unwrap().unwrap(),
        Name::parse("mac.welcome.libraries").unwrap()
    );
    assert!(Name::from_file_name(Platform::Mac, "notes.txt").is_none());
    assert!(Name::from_file_name(Platform::Mac, "a@android.png").is_none());
    assert!(
        Name::from_file_name(Platform::Mac, "Bad Name@macos.png")
            .unwrap()
            .is_err()
    );
}

#[test]
fn a_prefix_selects_the_names_it_starts() {
    let prefix = Prefix::parse("mac.welcome.").unwrap();
    assert_eq!(prefix.platform(), Platform::Mac);
    assert!(prefix.matches(&Name::parse("mac.welcome.libraries").unwrap()));
    assert!(!prefix.matches(&Name::parse("mac.settings.library").unwrap()));
    assert!(!prefix.matches(&Name::parse("android.welcome.libraries").unwrap()));
    let component = Prefix::parse("android.component.row.").unwrap();
    assert!(component.matches(&Name::parse("android.component.row.title").unwrap()));
    assert!(!component.matches(&Name::parse("android.component.row-x.title").unwrap()));
    for text in ["mac", "ios.welcome.", "mac.Welcome", "mac.welcome "] {
        assert!(Prefix::parse(text).is_err(), "{text:?}");
    }
}
