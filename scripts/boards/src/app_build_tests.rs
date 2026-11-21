use super::*;

#[test]
fn the_folders_are_those_of_the_shots_declared_lines() {
    let text = "step a mac x\nshots-declared mac app/Shots\n\
                shots-declared android app/shots/\nshots-capture mac run\n";
    assert_eq!(declared_folders(text).unwrap(), ["app/Shots", "app/shots"]);
    assert!(
        declared_folders("shots-capture mac run\n")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_shots_declared_line_that_names_no_folder_or_a_folder_outside_the_repo_is_an_error() {
    for (line, why) in [
        ("shots-declared", "names no platform"),
        ("shots-declared mac", "names its platform and nothing else"),
        (
            "shots-declared mac a b",
            "gives more than one declare folder",
        ),
        ("shots-declared mac ../a", "names a folder outside the repo"),
        ("shots-declared mac /a", "names a folder outside the repo"),
    ] {
        let error = declared_folders(line).unwrap_err().text();
        assert!(error.ends_with(why), "{error}");
    }
}
