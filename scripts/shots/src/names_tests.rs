use super::*;
use crate::names::Apps;

#[test]
fn a_screen_name_has_an_area_a_screen_and_maybe_a_state() {
    for text in [
        "mac.welcome.create-name",
        "mac.welcome.libraries.none",
        "android.welcome.libraries",
    ] {
        let name = Name::parse(text, &Apps::default()).unwrap();
        assert!(matches!(name, Name::Screen(_)), "{text}");
        assert_eq!(name.to_string(), text);
    }
}

fn preview(root: &std::path::Path, component: &str) {
    let path = root.join(boards::preview::preview_path(component));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "").unwrap();
}

fn component_of(text: &str) -> ComponentName {
    let Name::Component(c) = Name::parse(text, &Apps::default()).unwrap() else {
        panic!("not a component");
    };
    c
}

#[test]
fn a_component_name_names_its_platforms_component_and_state() {
    let root = tempfile::tempdir().unwrap();
    preview(root.path(), "AndroidCastRow");
    preview(root.path(), "MacRow");
    let c = component_of("android.component.cast-row.long-name");
    assert_eq!(
        c.component(&Previews::of(root.path()).unwrap()),
        "AndroidCastRow"
    );
    assert_eq!(c.state(), "long-name");
    let c = component_of("mac.component.row.nested-off");
    assert_eq!(c.component(&Previews::of(root.path()).unwrap()), "MacRow");
}

#[test]
fn a_shared_component_is_named_without_a_platform_on_both_platforms() {
    let root = tempfile::tempdir().unwrap();
    preview(root.path(), "StatusChip");
    for text in [
        "mac.component.status-chip.done",
        "android.component.status-chip.done",
    ] {
        assert_eq!(
            component_of(text).component(&Previews::of(root.path()).unwrap()),
            "StatusChip"
        );
    }
    let mac = Name::parse("mac.component.status-chip.done", &Apps::default()).unwrap();
    let android = Name::parse("android.component.status-chip.done", &Apps::default()).unwrap();
    assert_eq!(
        mac.file_name("png"),
        "mac.component.status-chip.done@macos.png"
    );
    assert_eq!(
        android.file_name("png"),
        "android.component.status-chip.done@android.png"
    );
}

#[test]
fn a_platform_preview_wins_over_a_shared_one_of_the_same_thing() {
    let root = tempfile::tempdir().unwrap();
    preview(root.path(), "Row");
    preview(root.path(), "MacRow");
    assert_eq!(
        component_of("mac.component.row.label").component(&Previews::of(root.path()).unwrap()),
        "MacRow"
    );
    assert_eq!(
        component_of("android.component.row.label").component(&Previews::of(root.path()).unwrap()),
        "Row"
    );
}

#[test]
fn a_component_with_no_preview_is_named_for_its_platform() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        component_of("android.component.nothing.default")
            .component(&Previews::of(root.path()).unwrap()),
        "AndroidNothing"
    );
}

#[test]
fn a_component_s_row_names_its_shot_by_the_component_s_thing() {
    let key = ComponentKey::of("MacQRScanner").unwrap();
    assert_eq!(
        component_shot(&key, &App::named_for(Platform::Mac), "scanning")
            .unwrap()
            .to_string(),
        "mac.component.qr-scanner.scanning"
    );
}

#[test]
fn a_component_shot_finds_a_component_whose_name_holds_an_acronym() {
    let root = tempfile::tempdir().unwrap();
    preview(root.path(), "MacQRScanner");
    preview(root.path(), "MacS3KeyFields");
    assert_eq!(
        component_of("mac.component.qr-scanner.scanning")
            .component(&Previews::of(root.path()).unwrap()),
        "MacQRScanner"
    );
    assert_eq!(
        component_of("mac.component.s3-key-fields.empty")
            .component(&Previews::of(root.path()).unwrap()),
        "MacS3KeyFields"
    );
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
        assert!(
            Name::parse(text, &Apps::default()).is_err(),
            "{text:?} was accepted"
        );
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
    let mac = Name::parse("mac.welcome.libraries.none", &Apps::default()).unwrap();
    assert_eq!(mac.file_name("png"), "mac.welcome.libraries.none@macos.png");
    let android = Name::parse("android.component.button.filled", &Apps::default()).unwrap();
    assert_eq!(
        android.file_name("json"),
        "android.component.button.filled@android.json"
    );
}

#[test]
fn a_shot_file_is_read_back_to_its_name() {
    let name = Name::from_file_name(
        &App::named_for(Platform::Mac),
        &Apps::default(),
        "mac.welcome.libraries@macos.json",
    );
    assert_eq!(
        name.unwrap().unwrap(),
        Name::parse("mac.welcome.libraries", &Apps::default()).unwrap()
    );
    assert!(
        Name::from_file_name(
            &App::named_for(Platform::Mac),
            &Apps::default(),
            "notes.txt"
        )
        .is_none()
    );
    assert!(
        Name::from_file_name(
            &App::named_for(Platform::Mac),
            &Apps::default(),
            "a@android.png"
        )
        .is_none()
    );
    assert!(
        Name::from_file_name(
            &App::named_for(Platform::Mac),
            &Apps::default(),
            "Bad Name@macos.png"
        )
        .unwrap()
        .is_err()
    );
}

#[test]
fn a_prefix_selects_the_names_it_starts() {
    let prefix = Prefix::parse("mac.welcome.").unwrap();
    assert_eq!(prefix.app(), "mac");
    assert!(prefix.matches(&Name::parse("mac.welcome.libraries", &Apps::default()).unwrap()));
    assert!(!prefix.matches(&Name::parse("mac.settings.library", &Apps::default()).unwrap()));
    assert!(!prefix.matches(&Name::parse("android.welcome.libraries", &Apps::default()).unwrap()));
    let component = Prefix::parse("android.component.row.").unwrap();
    assert!(
        component.matches(&Name::parse("android.component.row.title", &Apps::default()).unwrap())
    );
    assert!(
        !component
            .matches(&Name::parse("android.component.row-x.title", &Apps::default()).unwrap())
    );
    for text in ["mac", "Mac.welcome.", "mac.Welcome", "mac.welcome "] {
        assert!(Prefix::parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn previews_hold_each_folder_with_a_preview_and_name_the_folders_that_are_no_component() {
    let root = tempfile::tempdir().unwrap();
    preview(root.path(), "MacRow");
    preview(root.path(), "row");
    preview(root.path(), "Bad-Name");
    std::fs::create_dir_all(root.path().join("design/system/components/Empty")).unwrap();
    std::fs::write(root.path().join("design/system/components/index.d.ts"), "").unwrap();
    let previews = Previews::of(root.path()).unwrap();
    let names: Vec<&str> = previews
        .components
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["MacRow"]);
    assert_eq!(previews.unnamed, ["Bad-Name", "row"]);
    let none = tempfile::tempdir().unwrap();
    let previews = Previews::of(none.path()).unwrap();
    assert!(previews.components.is_empty() && previews.unnamed.is_empty());
}

#[test]
fn a_name_starts_with_an_app_that_an_app_line_or_a_platforms_word_gives() {
    let apps = Apps::read("app windows web\napp linux web\n").unwrap();
    let windows = Name::parse("windows.main.home", &apps).unwrap();
    let linux = Name::parse("linux.main.home", &apps).unwrap();
    assert_eq!(windows.platform(), Platform::Web);
    assert_eq!(linux.platform(), Platform::Web);
    assert_ne!(windows.app(), linux.app());
    assert_eq!(windows.to_string(), "windows.main.home");
    assert_eq!(
        Name::parse("mac.main.home", &apps).unwrap().platform(),
        Platform::Mac
    );
    let error = Name::parse("ios.main.home", &apps).unwrap_err().to_string();
    assert!(error.contains("starts with no app"), "{error}");
    assert!(Name::parse("windows.main.home", &Apps::default()).is_err());
}

#[test]
fn the_shot_file_of_an_app_is_told_from_that_of_another_app_on_its_platform() {
    let apps = Apps::read("app windows web\napp linux web\n").unwrap();
    let windows = apps.get("windows").unwrap();
    assert_eq!(
        Name::from_file_name(&windows, &apps, "windows.main.home@web.png")
            .unwrap()
            .unwrap()
            .to_string(),
        "windows.main.home"
    );
    assert!(Name::from_file_name(&windows, &apps, "linux.main.home@web.png").is_none());
}

#[test]
fn a_prefix_names_its_app_by_its_first_segment() {
    assert_eq!(Prefix::parse("windows.main.").unwrap().app(), "windows");
    assert_eq!(
        Prefix::all_of(&App::named_for(Platform::Android)).as_str(),
        "android."
    );
}

#[test]
fn a_shot_file_of_no_known_app_is_still_a_candidate_of_the_app() {
    let mac = App::named_for(Platform::Mac);
    let stray = Name::from_file_name(&mac, &Apps::default(), "macc.welcome.a@macos.png");
    assert!(stray.unwrap().is_err());
}
