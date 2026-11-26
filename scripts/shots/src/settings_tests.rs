use super::*;
use crate::names::Platform;

const APP_BUILD: &str = "\
mac build ./run.sh
shots mac builds shots run --app mac --select {select}
shots-capture mac ./mac/shots-capture.sh --quiet
shots-capture android ./android/shots-capture.sh
shots-declared mac mac/Shots
shots-declared android android/shots
shots-preview .row .row-name .row-cell
shots-preview-calls N W
shots-unshot .keyboard .other
";

#[test]
fn the_shots_lines_are_read_and_the_others_skipped() {
    let settings = Settings::parse(APP_BUILD).unwrap();
    assert_eq!(
        settings.capture,
        [
            (
                App::named_for(Platform::Mac),
                vec!["./mac/shots-capture.sh".to_string(), "--quiet".to_string()]
            ),
            (
                App::named_for(Platform::Android),
                vec!["./android/shots-capture.sh".to_string()]
            ),
        ]
    );
    assert_eq!(
        settings.capturing_apps(),
        [
            App::named_for(Platform::Mac),
            App::named_for(Platform::Android)
        ]
    );
    assert_eq!(
        settings.declared,
        [
            (App::named_for(Platform::Mac), "mac/Shots".to_string()),
            (
                App::named_for(Platform::Android),
                "android/shots".to_string()
            )
        ]
    );
    let preview = settings.preview("MacRow").unwrap();
    assert_eq!(
        (preview.row.as_str(), preview.name.as_str()),
        (".row", ".row-name")
    );
    assert_eq!(preview.cell, ".row-cell");
    assert_eq!(settings.preview_calls, ["N", "W"]);
    assert_eq!(settings.unshot, [".keyboard", ".other"]);
}

#[test]
fn a_marker_lines_selector_is_the_rest_of_the_line() {
    let settings = Settings::parse(
        "shots-slot  .pane .slot \n\
         shots-glyph svg.glyph\n\
         shots-window mac .window, .main > .sheet\n\
         shots-window android .dialog\n\
         shots-body-class android preview-android\n",
    )
    .unwrap();
    let mac = settings.markers(&App::named_for(Platform::Mac));
    assert_eq!(
        (mac.slot, mac.glyph, mac.window, mac.body_class),
        (
            Some(".pane .slot"),
            Some("svg.glyph"),
            Some(".window, .main > .sheet"),
            None
        )
    );
    let android = settings.markers(&App::named_for(Platform::Android));
    assert_eq!(
        (android.slot, android.window, android.body_class),
        (
            Some(".pane .slot"),
            Some(".dialog"),
            Some("preview-android")
        )
    );
}

#[test]
fn a_window_selector_may_hold_its_apps_name() {
    let settings = Settings::parse("shots-window mac mac-window, .mac\n").unwrap();
    assert_eq!(
        settings.markers(&App::named_for(Platform::Mac)).window,
        Some("mac-window, .mac")
    );
}

#[test]
fn a_repo_with_no_shots_lines_has_no_settings() {
    let settings = Settings::parse("mac build ./run.sh\n").unwrap();
    assert_eq!(settings, Settings::default());
    assert!(
        settings
            .preview("MacRow")
            .unwrap_err()
            .to_string()
            .contains("no shots-preview line")
    );
}

#[test]
fn a_design_only_line_names_the_components_a_platform_draws_for_the_design_alone() {
    let settings = Settings::parse(
        "shots-design-only mac AppIcon Sym\nshots-design-only android AppIcon ImportSheep\n",
    )
    .unwrap();
    assert_eq!(
        settings.design_only,
        [
            (App::named_for(Platform::Mac), "AppIcon".to_string()),
            (App::named_for(Platform::Mac), "Sym".to_string()),
            (App::named_for(Platform::Android), "AppIcon".to_string()),
            (App::named_for(Platform::Android), "ImportSheep".to_string()),
        ]
    );
}

#[test]
fn a_design_only_line_names_single_rows_as_component_dot_row() {
    let settings =
        Settings::parse("shots-design-only android Logo.inline Logo.sheep Sym\n").unwrap();
    assert_eq!(
        settings.design_only,
        [
            (App::named_for(Platform::Android), "Logo.inline".to_string()),
            (App::named_for(Platform::Android), "Logo.sheep".to_string()),
            (App::named_for(Platform::Android), "Sym".to_string()),
        ]
    );
}

#[test]
fn a_shots_line_it_cannot_read_is_refused() {
    for (line, why) in [
        ("shots-capture", "names no app"),
        ("shots-capture ios ./x.sh", "names no app"),
        ("shots-capture mac", "nothing else"),
        ("shots-declared mac a b", "more than one declare folder"),
        ("shots-declared mac ../out", "outside the repo"),
        ("shots-preview .row .name", "selectors"),
        ("shots-unshot", "names nothing"),
        ("shots-slot", "names nothing"),
        ("shots-glyph  ", "names nothing"),
        ("shots-slot .a\nshots-slot .b", "repeats shots-slot"),
        ("shots-glyph .a\nshots-glyph .b", "repeats shots-glyph"),
        ("shots-window", "names no app"),
        ("shots-window .window", "names no app"),
        ("shots-window mac", "nothing else"),
        (
            "shots-window mac .a\nshots-window mac .b",
            "second window selector",
        ),
        ("shots-body-class ios a", "names no app"),
        ("shots-body-class android", "nothing else"),
        ("shots-body-class android a b", "more than one class"),
        (
            "shots-body-class android .a",
            "a selector where a class's name goes",
        ),
        (
            "shots-body-class android a\nshots-body-class android b",
            "second body class",
        ),
        ("shots-design-only", "names no app"),
        ("shots-design-only mac", "names no component"),
        ("shots-design-only ios Sym", "names no app"),
        ("shots-design-only android Logo.", "names no row"),
        ("shots-design-only android .inline", "names no component"),
        ("shots-guess x", "no line builds shots knows"),
        ("shots-source app-shots", "no line builds shots knows"),
    ] {
        let error = Settings::parse(line).unwrap_err().to_string();
        assert!(error.contains(why), "{line}: {error}");
    }
    let twice = "shots-capture mac a\nshots-capture mac b\n";
    assert!(
        Settings::parse(twice)
            .unwrap_err()
            .to_string()
            .contains("second command")
    );
}

#[test]
fn app_lines_name_the_apps_the_other_lines_are_keyed_by() {
    let settings = Settings::parse(
        "app windows web\napp linux web\n\
         shots-stage windows ./windows.sh {shot}\nshots-stage linux ./linux.sh {shot}\n\
         shots-declared windows app/windows\nshots-declared linux app/linux\n\
         shots-design-only linux Sym\n",
    )
    .unwrap();
    let windows = settings.app("windows").unwrap();
    let linux = settings.app("linux").unwrap();
    assert_eq!(
        (windows.platform, linux.platform),
        (Platform::Web, Platform::Web)
    );
    assert_eq!(settings.capturing_apps(), [linux.clone(), windows.clone()]);
    assert_eq!(
        settings.declared,
        [
            (windows, "app/windows".to_string()),
            (linux.clone(), "app/linux".to_string())
        ]
    );
    assert_eq!(settings.design_only, [(linux, "Sym".to_string())]);
}

#[test]
fn an_app_is_shot_by_the_command_its_platform_takes() {
    let settings =
        Settings::parse("shots-capture mac ./mac.sh\nshots-stage web ./serve.sh {shot}\n").unwrap();
    assert_eq!(
        settings.stage,
        [(
            settings.app("web").unwrap(),
            vec!["./serve.sh".to_string(), "{shot}".to_string()]
        )]
    );
    assert_eq!(settings.capturing_apps().len(), 2);
    for (line, why) in [
        ("shots-stage mac ./x.sh", "shots-capture"),
        ("shots-capture web ./x.sh", "shots-stage"),
        ("shots-stage web a\nshots-stage web b\n", "second command"),
    ] {
        let error = Settings::parse(line).unwrap_err().to_string();
        assert!(error.contains(why), "{line}: {error}");
    }
}

#[test]
fn a_line_that_names_an_app_no_line_gives_is_refused() {
    for line in [
        "shots-capture windows ./x.sh",
        "shots-declared windows app/x",
        "shots-design-only windows Sym",
    ] {
        let error = Settings::parse(line).unwrap_err().to_string();
        assert!(error.contains("names no app"), "{line}: {error}");
    }
    assert!(Settings::parse("app windows ios\n").is_err());
}
