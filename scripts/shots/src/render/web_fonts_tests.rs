use super::*;

#[test]
fn the_stylesheet_names_exactly_the_pinned_files() {
    let mut linked: Vec<&str> = STYLESHEET
        .split("url(/web-fonts/roboto/")
        .skip(1)
        .map(|rest| rest.split(".woff2)").next().unwrap())
        .collect();
    let mut held: Vec<&str> = FILES.iter().map(|(name, _)| *name).collect();
    linked.sort_unstable();
    held.sort_unstable();
    assert_eq!(linked, held);
}

#[test]
fn every_pinned_file_is_a_woff2() {
    for (name, bytes) in FILES {
        assert_eq!(&bytes[..4], b"wOF2", "{name}");
    }
}

#[test]
fn relink_changes_only_google_stylesheet_links() {
    assert_eq!(
        relink(
            "<link href=\"https://fonts.googleapis.com/css2?family=Roboto\"><link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">"
        ),
        "<link href=\"/web-fonts/css2?family=Roboto\"><link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">"
    );
}
