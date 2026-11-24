use super::*;

fn folder(name: &str, files: &[&str]) -> crate::testing::Scratch {
    let dir = crate::testing::Scratch::new(name);
    for file in files {
        std::fs::write(dir.dir.join(file), file.as_bytes()).unwrap();
    }
    dir
}

fn all_files() -> Vec<&'static str> {
    FACES.iter().map(|(file, ..)| *file).collect()
}

#[test]
fn each_font_file_of_the_family_gets_a_rule_with_its_weight_and_style() {
    let mut files = all_files();
    files.push("SFMono-Terminal.ttf");
    let dir = folder("fonts-rules", &files);
    let css = font_face_css(&dir.dir).unwrap();
    assert_eq!(css.lines().count(), 12);
    assert!(css.lines().any(|rule| rule
        == "@font-face { font-family: \"SF Mono\"; font-weight: 400; font-style: normal; src: url(\"/fonts/sf-mono/SF-Mono-Regular.otf\") format(\"opentype\"); }"));
    assert!(css.lines().any(|rule| rule
        == "@font-face { font-family: \"SF Mono\"; font-weight: 700; font-style: italic; src: url(\"/fonts/sf-mono/SF-Mono-BoldItalic.otf\") format(\"opentype\"); }"));
    assert!(!css.contains("Terminal"));
}

#[test]
fn a_folder_that_is_gone_or_missing_a_file_is_an_error() {
    let gone = font_face_css(Path::new("/no/such/folder"))
        .unwrap_err()
        .text();
    assert!(gone.contains("/no/such/folder"), "{gone}");
    let mut files = all_files();
    files.retain(|file| *file != "SF-Mono-Heavy.otf");
    let dir = folder("fonts-missing", &files);
    let error = font_face_css(&dir.dir).unwrap_err().text();
    assert!(error.contains("SF-Mono-Heavy.otf"), "{error}");
}

#[test]
fn only_the_family_s_files_are_named() {
    assert!(is_font_file("SF-Mono-Regular.otf"));
    assert!(!is_font_file("secret.txt"));
    assert!(!is_font_file("../SF-Mono-Regular.otf"));
    assert!(!is_font_file("SFMono-Terminal.ttf"));
}

#[test]
fn a_file_that_cannot_be_opened_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = folder("fonts-unreadable", &all_files());
    let locked = dir.dir.join("SF-Mono-Bold.otf");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let error = font_face_css(&dir.dir).unwrap_err().text();
    assert!(error.contains("SF-Mono-Bold.otf"), "{error}");
}
