use super::*;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn live_record(sha256: &str) -> String {
    format!(
        "canvas = \"https://example.test/live\"\ncanvas_version = \"v2\"\ntype_release = \"t\"\nruntime = \"artifact-type/dc-runtime.js\"\nruntime_sha256 = \"{sha256}\"\npublished_at = \"2026-11-12T00:00:00Z\"\npublished_by = \"design\"\n"
    )
}

#[test]
fn sha256_is_the_hex_digest() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_canvas_record_picks_the_copy_of_its_sha256() {
    let root = tempfile::tempdir().unwrap();
    let copy = &copies::COPIES[0];
    write(root.path(), LIVE_RECORD, &live_record(copy.sha256));
    let checked = check(root.path()).unwrap();
    assert_eq!(checked.copy, copy);
    assert!(
        checked
            .said
            .contains("canvas v2 published 2026-11-12T00:00:00Z"),
        "{}",
        checked.said
    );
}

#[test]
fn a_record_of_a_runtime_builds_has_no_copy_of_says_how_to_add_it() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), LIVE_RECORD, &live_record(&"f".repeat(64)));
    let error = check(root.path()).unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, RuntimeError::NoCopy(_)));
    assert!(message.contains(&"f".repeat(64)), "{message}");
    assert!(
        message.contains("2026-11-12T00:00:00Z by design"),
        "{message}"
    );
    assert!(message.contains("To add the copy"), "{message}");
    assert!(message.contains("https://example.test/live"), "{message}");
}

#[test]
fn a_repo_with_no_record_uses_the_newest_copy() {
    let root = tempfile::tempdir().unwrap();
    let checked = check(root.path()).unwrap();
    assert_eq!(checked.copy, copies::newest());
    assert!(
        checked.said.contains("has no design/mocks/runtime.toml"),
        "{}",
        checked.said
    );
}

#[test]
fn a_record_with_an_unknown_key_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let record = format!("{}extra = 1\n", live_record(copies::COPIES[0].sha256));
    write(root.path(), LIVE_RECORD, &record);
    assert!(matches!(
        check(root.path()),
        Err(RuntimeError::BadRecord { .. })
    ));
}
