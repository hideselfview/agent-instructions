use super::*;
use crate::testing::Scratch;

/// The build root of a checkout whose manifest names `target` and
/// `node_modules`, with a file in `target`.
fn root_with_folders(scratch: &Scratch) -> (Places, PathBuf) {
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\nnode_modules 1G\n");
    let checkout = crate::checkout::Checkout::find(&main.display().to_string()).unwrap();
    crate::setup::setup(&tool, &crate::testing::no_ssh(), &checkout).unwrap();
    let root = checkout.build_root(&tool.places);
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/file"), vec![0u8; 20_000]).unwrap();
    (tool.places.clone(), root)
}

#[test]
fn a_root_outside_the_volume_has_no_record_file() {
    let scratch = Scratch::new("sizes-outside");
    let places = scratch.tool(0).places;
    assert_eq!(root_of(&places, Path::new("/elsewhere/a/b")), None);
    assert_eq!(
        root_of(&places, &places.root.join("demo/main/target/x")),
        Some(places.root.join("demo/main"))
    );
    assert_eq!(root_of(&places, &places.root.join("demo")), None);
}

#[test]
fn a_record_is_what_was_written_and_a_missing_one_is_empty() {
    let scratch = Scratch::new("sizes-roundtrip");
    let (places, root) = root_with_folders(&scratch);
    assert!(read(&places, &root).unwrap().is_empty());
    let sizes: Sizes = [(
        "target".to_string(),
        Measure {
            kib: 7,
            newest: Some(3),
        },
    )]
    .into();
    write(&places, &root, &sizes).unwrap();
    assert_eq!(read(&places, &root).unwrap(), sizes);
}

#[test]
fn a_command_that_wrote_a_root_records_the_size_of_each_folder_and_the_dashboard_reads_it() {
    let scratch = Scratch::new("sizes-record");
    let (places, root) = root_with_folders(&scratch);
    assert!(record(&places, &root).unwrap());
    let sizes = read(&places, &root).unwrap();
    assert!(sizes["target"].kib >= 20, "{sizes:?}");
    // A record is not measured again when read: a later write shows only
    // after the next command that measures the root.
    std::fs::write(root.join("target/more"), vec![0u8; 400_000]).unwrap();
    let live = Root::read(&places, &root).unwrap().unwrap();
    let keys = keys_of(&live).unwrap();
    let known = known(&places, &live, &keys).unwrap();
    assert_eq!(known[0], sizes["target"]);
    // A folder with no record is measured once, and kept.
    assert!(record_after_removing(&places, &root, "node_modules"));
}

/// Removes <key> from <root>'s record, as a record from before the manifest
/// named the folder lacks it, then reads the folder through `known`.
fn record_after_removing(places: &Places, root: &Path, key: &str) -> bool {
    let mut sizes = read(places, root).unwrap();
    sizes.remove(key);
    write(places, root, &sizes).unwrap();
    let live = Root::read(places, root).unwrap().unwrap();
    known(places, &live, &[key.to_string()]).unwrap();
    read(places, root).unwrap().contains_key(key)
}

#[test]
fn a_root_with_no_marker_has_no_record() {
    let scratch = Scratch::new("sizes-gone");
    let (places, root) = root_with_folders(&scratch);
    record(&places, &root).unwrap();
    std::fs::remove_file(root.join(".checkout")).unwrap();
    assert!(!record(&places, &root).unwrap());
    assert!(read(&places, &root).unwrap().is_empty());
}

#[test]
fn a_status_reads_recorded_folders_and_leaves_the_rest_to_be_measured() {
    let scratch = Scratch::new("sizes-recorded");
    let (places, root) = root_with_folders(&scratch);
    record(&places, &root).unwrap();
    let paths = vec![
        root.join("target"),
        root.join("elsewhere"),
        places.root.join("x"),
    ];
    let answers = recorded(&places, &paths).unwrap();
    assert!(answers[0].is_some());
    assert_eq!(answers[1], None);
    assert_eq!(answers[2], None);
}

#[test]
fn a_path_that_is_no_build_root_has_no_record_to_read_or_write() {
    let scratch = Scratch::new("sizes-no-root");
    let places = scratch.tool(0).places;
    let outside = Path::new("/elsewhere/a/b");
    assert!(read(&places, outside).is_err());
    assert!(write(&places, outside, &Sizes::new()).is_err());
    assert!(read(&places, &places.root.join("demo")).is_err());
}

#[test]
fn a_corrupt_record_is_an_error_not_an_empty_record() {
    let scratch = Scratch::new("sizes-corrupt");
    let (places, root) = root_with_folders(&scratch);
    record(&places, &root).unwrap();
    let file = record_file(&places, &root).unwrap();
    std::fs::write(&file, b"not json").unwrap();
    assert!(read(&places, &root).is_err());
    assert!(recorded(&places, &[root.join("target")]).is_err());
}

#[test]
fn a_record_of_a_root_that_is_gone_is_forgotten_by_prune() {
    let scratch = Scratch::new("sizes-forget");
    let (places, root) = root_with_folders(&scratch);
    record(&places, &root).unwrap();
    assert!(forget_unrooted(&places).is_ok());
    assert!(!read(&places, &root).unwrap().is_empty());
    std::fs::remove_file(root.join(".checkout")).unwrap();
    forget_unrooted(&places).unwrap();
    assert!(read(&places, &root).unwrap().is_empty());
}
