use super::*;
use crate::names::Apps;

#[test]
fn the_run_draws_only_the_boards_the_picture_folder_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        Name::parse("mac.a.b", &Apps::default()).unwrap(),
        Name::parse("mac.a.c", &Apps::default()).unwrap(),
    ];
    let hashes: BTreeMap<Name, String> = [
        (names[0].clone(), "kept".to_string()),
        (names[1].clone(), "new".to_string()),
    ]
    .into_iter()
    .collect();
    let boards = dir.path().join("boards");
    assert_eq!(
        take_pictures(None, &hashes, &names, &boards).unwrap(),
        names.to_vec()
    );
    let drawn = dir.path().join("drawn");
    std::fs::create_dir_all(&drawn).unwrap();
    std::fs::write(drawn.join("mac.a.b.png"), "png").unwrap();
    std::fs::write(drawn.join("mac.a.b.json"), "{}").unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
    pictures.keep_board("kept", "mac.a.b", &drawn).unwrap();
    assert_eq!(
        take_pictures(Some(&pictures), &hashes, &names, &boards).unwrap(),
        [names[1].clone()]
    );
    assert!(boards.join("mac.a.b.png").is_file() && boards.join("mac.a.b.json").is_file());
}
