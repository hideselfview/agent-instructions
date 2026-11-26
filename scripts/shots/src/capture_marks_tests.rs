use super::*;

#[test]
fn a_mark_covers_its_prefix_and_its_children_and_names_whether_windows_were_captured() {
    let scratch = tempfile::tempdir().unwrap();
    let marks = CaptureMarks::of(scratch.path());
    marks
        .write(&[Prefix::parse("mac.a.").unwrap()], false)
        .unwrap();
    assert!(
        marks
            .cover(&[Prefix::parse("mac.a.b.").unwrap()], false)
            .unwrap()
    );
    assert!(
        !marks
            .cover(&[Prefix::parse("mac.a.").unwrap()], true)
            .unwrap()
    );
    assert!(
        !marks
            .cover(&[Prefix::parse("mac.").unwrap()], false)
            .unwrap()
    );
    marks
        .write(
            &[Prefix::all_of(&crate::names::App::named_for(
                crate::names::Platform::Mac,
            ))],
            true,
        )
        .unwrap();
    assert!(
        marks
            .cover(
                &[
                    Prefix::parse("mac.").unwrap(),
                    Prefix::parse("mac.b.").unwrap()
                ],
                true
            )
            .unwrap()
    );
}

#[test]
fn a_directory_cannot_mark_a_prefix_captured() {
    let scratch = tempfile::tempdir().unwrap();
    let marks = CaptureMarks::of(scratch.path());
    std::fs::create_dir_all(marks.directory().join("mac.@windows")).unwrap();

    let error = marks
        .cover(&[Prefix::parse("mac.a.").unwrap()], true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected a capture mark file"), "{error}");
    assert!(error.contains("mac.@windows"), "{error}");
}

#[test]
fn an_invalid_mark_cannot_cover_a_prefix() {
    let scratch = tempfile::tempdir().unwrap();
    for name in ["mac.c.", "@windows", "m@windows"] {
        let tree = tempfile::tempdir_in(scratch.path()).unwrap();
        let marks = CaptureMarks::of(tree.path());
        std::fs::create_dir(marks.directory()).unwrap();
        std::fs::write(marks.directory().join(name), "").unwrap();
        let error = marks
            .cover(&[Prefix::parse("mac.a.").unwrap()], true)
            .unwrap_err()
            .to_string();
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn writing_a_mark_rejects_a_symlink_and_keeps_its_target() {
    let scratch = tempfile::tempdir().unwrap();
    let marks = CaptureMarks::of(scratch.path());
    std::fs::create_dir(marks.directory()).unwrap();
    let target = scratch.path().join("other-file");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, marks.directory().join("mac.@windows")).unwrap();
    let error = marks
        .write(
            &[Prefix::all_of(&crate::names::App::named_for(
                crate::names::Platform::Mac,
            ))],
            true,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("expected a capture mark file"), "{error}");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
}
