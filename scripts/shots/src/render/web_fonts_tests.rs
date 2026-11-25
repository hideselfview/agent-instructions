use super::*;

#[test]
fn each_stylesheet_names_exactly_the_files_of_its_family() {
    for (name, sheet) in FAMILIES {
        let slug = name.to_lowercase().replace(' ', "-");
        let prefix = format!("url({URL_PREFIX}{slug}/");
        let mut linked: Vec<String> = sheet
            .split(&prefix)
            .skip(1)
            .map(|rest| format!("{slug}/{}", rest.split(".woff2)").next().unwrap()))
            .collect();
        let mut held: Vec<String> = FILES
            .iter()
            .filter(|(file, _)| file.starts_with(&format!("{slug}/")))
            .map(|(file, _)| file.to_string())
            .collect();
        linked.sort_unstable();
        held.sort_unstable();
        assert!(!held.is_empty(), "{name}");
        assert_eq!(linked, held, "{name}");
    }
}

#[test]
fn every_pinned_file_is_a_woff2_of_a_family() {
    for (file, bytes) in FILES {
        assert_eq!(&bytes[..4], b"wOF2", "{file}");
    }
    let held: usize = FAMILIES
        .iter()
        .map(|(_, sheet)| sheet.matches("@font-face").count())
        .sum();
    assert_eq!(held, FILES.len());
}

#[test]
fn relink_changes_only_google_stylesheet_links() {
    assert_eq!(
        relink("<link href=\"https://fonts.googleapis.com/css2?family=Roboto:wght@400&amp;display=swap\"><link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">").unwrap(),
        "<link href=\"/web-fonts/css2?family=Roboto:wght@400&amp;display=swap\"><link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">"
    );
    assert_eq!(
        relink("@import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;500&family=JetBrains+Mono:wght@400&display=swap');").unwrap(),
        "@import url('/web-fonts/css2?family=Inter:wght@400;500&family=JetBrains+Mono:wght@400&display=swap');"
    );
}

#[test]
fn a_link_to_what_the_checker_holds_no_copy_of_is_refused() {
    let other = relink("<link href=\"https://fonts.googleapis.com/css2?family=Oswald:wght@400\">")
        .unwrap_err();
    assert!(other.contains("Oswald"), "{other}");
    let italic =
        relink("@import url(https://fonts.googleapis.com/css2?family=Roboto:ital,wght@1,400);")
            .unwrap_err();
    assert!(italic.contains("ital,wght@1,400"), "{italic}");
    let none =
        relink("<link href=\"https://fonts.googleapis.com/css2?display=swap\">").unwrap_err();
    assert!(none.contains("no family"), "{none}");
}

#[test]
fn the_stylesheet_of_two_families_holds_both() {
    let sheet = stylesheet("family=Inter:wght@400&family=JetBrains+Mono:wght@400").unwrap();
    assert!(sheet.contains("font-family: 'Inter'"));
    assert!(sheet.contains("font-family: 'JetBrains Mono'"));
    assert!(!sheet.contains("font-family: 'Roboto'"));
}
