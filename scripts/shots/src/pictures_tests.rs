use super::*;

#[test]
fn a_kept_board_picture_is_taken_by_its_hash_under_any_shots_name() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
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
    let pictures = Pictures::new(dir.path().to_path_buf(), std::time::SystemTime::now());
    std::fs::create_dir_all(dir.path().join("boards")).unwrap();
    std::fs::write(dir.path().join("boards/h1.png"), "png").unwrap();
    assert!(
        !pictures
            .take_board("h1", "mac.a.b", &dir.path().join("out"))
            .unwrap()
    );
}

#[test]
fn taking_a_picture_marks_it_used_at_the_runs_time() {
    let dir = tempfile::tempdir().unwrap();
    let run_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
    let pictures = Pictures::new(dir.path().to_path_buf(), run_time);
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
    assert_eq!(
        std::fs::metadata(&png).unwrap().modified().unwrap(),
        run_time
    );
}

fn shot(text: &str) -> Name {
    Name::parse(text).unwrap()
}

#[test]
fn an_app_trees_shots_are_taken_only_when_the_folder_keeps_every_selected_one() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
    let names = [shot("mac.a.b"), shot("mac.a.c")];
    let taken = dir.path().join("taken");
    std::fs::create_dir_all(&taken).unwrap();
    for name in &names[..1] {
        std::fs::write(taken.join(name.file_name("png")), "png").unwrap();
        std::fs::write(taken.join(name.file_name("json")), "{}").unwrap();
    }
    pictures
        .keep_shots(Platform::Mac, "tree1", &names[..1], &taken)
        .unwrap();
    let out = dir.path().join("out");
    assert!(
        !pictures
            .take_shots(Platform::Mac, "tree1", &names, &out)
            .unwrap(),
        "the folder lacks mac.a.c"
    );
    assert!(!out.exists(), "nothing is copied when one shot is missing");
    assert!(
        pictures
            .take_shots(Platform::Mac, "tree1", &names[..1], &out)
            .unwrap()
    );
    assert!(out.join(names[0].file_name("png")).is_file());
    assert!(
        !pictures
            .take_shots(Platform::Mac, "tree2", &names[..1], &out)
            .unwrap(),
        "another app tree's shots never stand in"
    );
}

#[test]
fn keeping_a_shot_again_replaces_its_kept_picture_and_element_list_whole() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
    let names = [shot("mac.a.b")];
    let taken = dir.path().join("taken");
    std::fs::create_dir_all(&taken).unwrap();
    let file = |extension| taken.join(names[0].file_name(extension));
    std::fs::write(file("png"), "early").unwrap();
    std::fs::write(file("json"), "[1]").unwrap();
    pictures
        .keep_shots(Platform::Mac, "tree1", &names, &taken)
        .unwrap();
    std::fs::write(file("png"), "loaded").unwrap();
    std::fs::write(file("json"), "[2]").unwrap();
    pictures
        .keep_shots(Platform::Mac, "tree1", &names, &taken)
        .unwrap();
    let kept = dir.path().join("pictures/mac/tree1");
    assert_eq!(
        std::fs::read_to_string(kept.join(names[0].file_name("png"))).unwrap(),
        "loaded"
    );
    assert_eq!(
        std::fs::read_to_string(kept.join(names[0].file_name("json"))).unwrap(),
        "[2]"
    );
    assert_eq!(
        std::fs::read_dir(&kept).unwrap().count(),
        2,
        "only the picture and its element list: no temporary file stays"
    );
}

#[test]
fn a_taken_prefix_is_marked_with_whether_the_run_took_the_window_shots() {
    let dir = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(dir.path().to_path_buf(), std::time::SystemTime::now());
    pictures
        .mark_taken(Platform::Mac, "tree1", &["mac.a.".to_string()], true)
        .unwrap();
    pictures
        .mark_taken(Platform::Mac, "tree1", &["mac.b.".to_string()], false)
        .unwrap();
    let taken = dir.path().join("mac/tree1/.taken");
    assert!(taken.join("mac.a.@windows").is_file());
    assert!(taken.join("mac.b.@no-windows").is_file());
}
