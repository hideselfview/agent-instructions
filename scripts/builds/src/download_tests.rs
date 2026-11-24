use super::*;
use crate::testing::Scratch;

#[test]
fn unpack_member_puts_only_the_named_file_in_the_folder() {
    let scratch = Scratch::new("unpack-member");
    let release = scratch.dir.join("release/node-v1-linux-x64");
    std::fs::create_dir_all(release.join("bin")).unwrap();
    std::fs::create_dir_all(release.join("lib")).unwrap();
    std::fs::write(release.join("bin/node"), "the program").unwrap();
    std::fs::write(release.join("lib/other"), "not wanted").unwrap();
    let archive = scratch.dir.join("node.tar.gz");
    let made = Command::new("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(scratch.dir.join("release"))
        .arg("node-v1-linux-x64")
        .status()
        .unwrap();
    assert!(made.success());
    let into = scratch.dir.join("into");
    std::fs::create_dir_all(&into).unwrap();
    let url = format!("file://{}", archive.display());
    unpack_member(&url, "node-v1-linux-x64/bin/node", &into, "node 1").unwrap();
    assert_eq!(
        std::fs::read_to_string(into.join("node")).unwrap(),
        "the program"
    );
    assert_eq!(std::fs::read_dir(&into).unwrap().count(), 1);

    let error = unpack_member(&url, "node-v1-linux-x64/bin/missing", &into, "node 1")
        .unwrap_err()
        .text();
    assert!(error.contains("downloading node 1 failed"), "{error}");
}
