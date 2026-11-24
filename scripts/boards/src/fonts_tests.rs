use super::*;

fn folder(name: &str, files: &[&str]) -> crate::testing::Scratch {
    let dir = crate::testing::Scratch::new(name);
    for file in files {
        std::fs::write(dir.dir.join(file), file.as_bytes()).unwrap();
    }
    dir
}

#[test]
fn each_font_file_of_the_folder_gets_a_rule_with_its_weight_and_style() {
    let dir = folder(
        "fonts-rules",
        &[
            "SF-Mono-Regular.otf",
            "SF-Mono-BoldItalic.otf",
            "SFMono-Terminal.ttf",
        ],
    );
    assert_eq!(
        font_face_css(&dir.dir),
        "@font-face { font-family: \"SF Mono\"; font-weight: 400; font-style: normal; src: url(\"/fonts/sf-mono/SF-Mono-Regular.otf\") format(\"opentype\"); }\n\
         @font-face { font-family: \"SF Mono\"; font-weight: 700; font-style: italic; src: url(\"/fonts/sf-mono/SF-Mono-BoldItalic.otf\") format(\"opentype\"); }"
    );
}

#[test]
fn a_folder_with_no_font_or_no_folder_gives_no_rule() {
    assert_eq!(font_face_css(&folder("fonts-none", &[]).dir), "");
    assert_eq!(font_face_css(Path::new("/no/such/folder")), "");
}

#[test]
fn only_the_family_s_files_are_read() {
    let dir = folder("fonts-read", &["SF-Mono-Regular.otf", "secret.txt"]);
    assert_eq!(
        read_font(&dir.dir, "SF-Mono-Regular.otf").unwrap(),
        Some(b"SF-Mono-Regular.otf".to_vec())
    );
    assert_eq!(read_font(&dir.dir, "secret.txt").unwrap(), None);
    assert_eq!(read_font(&dir.dir, "../secret.txt").unwrap(), None);
    assert_eq!(read_font(&dir.dir, "SF-Mono-Bold.otf").unwrap(), None);
}
