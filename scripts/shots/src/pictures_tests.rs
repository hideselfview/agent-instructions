use super::*;

#[test]
fn a_kept_board_picture_is_taken_by_its_hash_under_any_shots_name() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"));
    let drawn = dir.path().join("drawn");
    std::fs::create_dir_all(&drawn).unwrap();
    std::fs::write(drawn.join("mac.a.b.png"), "png").unwrap();
    std::fs::write(drawn.join("mac.a.b.json"), "{}").unwrap();
    let out = dir.path().join("out");
    assert!(!pictures.take_board("h1", "mac.a.b", &out).unwrap());
    pictures.keep_board("h1", "mac.a.b", &drawn).unwrap();
    assert!(pictures.take_board("h1", "mac.a.b", &out).unwrap());
    assert_eq!(
        std::fs::read_to_string(out.join("mac.a.b.png")).unwrap(),
        "png"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("mac.a.b.json")).unwrap(),
        "{}"
    );
    assert!(!pictures.take_board("h2", "mac.a.b", &out).unwrap());
    let left: Vec<String> = std::fs::read_dir(dir.path().join("pictures/boards"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "no temporary file is left: {left:?}");
}

#[test]
fn a_picture_whose_element_list_is_missing_is_not_taken() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().to_path_buf());
    std::fs::create_dir_all(dir.path().join("boards")).unwrap();
    std::fs::write(dir.path().join("boards/h1.png"), "png").unwrap();
    assert!(
        !pictures
            .take_board("h1", "mac.a.b", &dir.path().join("out"))
            .unwrap()
    );
}

#[test]
fn taking_a_picture_marks_it_used() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().to_path_buf());
    std::fs::create_dir_all(dir.path().join("boards")).unwrap();
    let png = dir.path().join("boards/h1.png");
    std::fs::write(&png, "png").unwrap();
    std::fs::write(dir.path().join("boards/h1.json"), "{}").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(&png)
        .unwrap()
        .set_modified(old)
        .unwrap();
    pictures
        .take_board("h1", "mac.a.b", &dir.path().join("out"))
        .unwrap();
    assert!(std::fs::metadata(&png).unwrap().modified().unwrap() > old);
}
