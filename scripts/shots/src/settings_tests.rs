use super::*;

const APP_BUILD: &str = "\
mac build ./run.sh
shots mac builds shots run --platform mac --select {select}
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
                Platform::Mac,
                vec!["./mac/shots-capture.sh".to_string(), "--quiet".to_string()]
            ),
            (
                Platform::Android,
                vec!["./android/shots-capture.sh".to_string()]
            ),
        ]
    );
    assert_eq!(settings.platforms(), [Platform::Mac, Platform::Android]);
    assert_eq!(
        settings.declared,
        [
            (Platform::Mac, "mac/Shots".to_string()),
            (Platform::Android, "android/shots".to_string())
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
            (Platform::Mac, "AppIcon".to_string()),
            (Platform::Mac, "Sym".to_string()),
            (Platform::Android, "AppIcon".to_string()),
            (Platform::Android, "ImportSheep".to_string()),
        ]
    );
}

#[test]
fn a_shots_line_it_cannot_read_is_refused() {
    for (line, why) in [
        ("shots-capture", "names no platform"),
        ("shots-capture ios ./x.sh", "mac, android or web"),
        ("shots-capture mac", "nothing else"),
        ("shots-declared mac a b", "more than one declare folder"),
        ("shots-declared mac ../out", "outside the repo"),
        ("shots-preview .row .name", "selectors"),
        ("shots-unshot", "names nothing"),
        ("shots-design-only", "names no platform"),
        ("shots-design-only mac", "names no component"),
        ("shots-design-only ios Sym", "mac, android or web"),
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
            .contains("second capture command")
    );
}
