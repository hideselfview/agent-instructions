use super::*;

#[test]
fn the_platforms_and_folders_are_those_of_the_shots_declared_lines() {
    let text = "step a mac x\nshots-declared mac app/Shots\n\
                shots-declared android app/shots/\nshots-capture mac run\n";
    assert_eq!(
        declared(text).unwrap(),
        [
            (Platform::Mac, "app/Shots".to_string()),
            (Platform::Android, "app/shots".to_string())
        ]
    );
    assert!(declared("shots-capture mac run\n").unwrap().is_empty());
}

#[test]
fn a_platform_lookup_validates_only_its_declarations() {
    let text = "  shots-declared  mac app/Shots/\n\
                shots-declared android app/first\n\
                shots-declared android app/second\n";
    assert_eq!(
        declared_folder(text, Platform::Mac).unwrap(),
        Some("app/Shots".to_string())
    );
    assert_eq!(
        declared_folder(text, Platform::Android).unwrap_err().why,
        "gives a platform a second declare folder"
    );
    assert_eq!(declared_folder(text, Platform::Web).unwrap(), None);
    assert!(declared_folder("shots-declared", Platform::Mac).is_err());
    assert_eq!(
        declared_folder("shots-declared macos app/Shots", Platform::Mac)
            .unwrap_err()
            .why,
        "names a platform other than mac, android or web"
    );
}

#[test]
fn a_shots_declared_line_that_the_shots_program_would_refuse_is_an_error() {
    for (line, why) in [
        ("shots-declared", "names no platform"),
        (
            "shots-declared ios a",
            "names a platform other than mac, android or web",
        ),
        ("shots-declared mac", "names its platform and nothing else"),
        (
            "shots-declared mac a b",
            "gives more than one declare folder",
        ),
        ("shots-declared mac ../a", "names a folder outside the repo"),
        ("shots-declared mac /a", "names a folder outside the repo"),
        (
            "shots-declared mac a\nshots-declared mac b",
            "gives a platform a second declare folder",
        ),
    ] {
        assert_eq!(declared(line).unwrap_err().why, why, "{line}");
    }
}
