use super::*;
use crate::testing::{Scratch, age, fill};

#[test]
fn a_hard_link_and_a_clone_count_once() {
    let scratch = Scratch::new("usage");
    let links = scratch.dir.join("links");
    fill(&links.join("a"), 2048);
    std::fs::hard_link(links.join("a"), links.join("b")).unwrap();
    let measured = measure(&links).unwrap();
    assert!(
        (2048..2200).contains(&measured.kib),
        "hard links took {} KiB",
        measured.kib
    );

    let clones = scratch.dir.join("clones");
    fill(&clones.join("a"), 4096);
    let status = std::process::Command::new("cp")
        .arg("-c")
        .arg(clones.join("a"))
        .arg(clones.join("b"))
        .status()
        .unwrap();
    assert!(status.success());
    let measured = measure(&clones).unwrap();
    assert!(
        (4096..4300).contains(&measured.kib),
        "a clone took {} KiB",
        measured.kib
    );

    let both = measure_many(&[links.clone(), scratch.dir.join("missing"), clones.clone()]).unwrap();
    assert_eq!(both[1], Measure::default());
    assert!((2048..2200).contains(&both[0].kib) && (4096..4300).contains(&both[2].kib));
}

#[test]
fn the_newest_write_is_found_at_any_depth() {
    let scratch = Scratch::new("newest");
    let deep = scratch.dir.join("folder/a/b/c/file");
    fill(&deep, 1);
    for path in ["folder/a/b/c", "folder/a/b", "folder/a", "folder"] {
        age(&scratch.dir.join(path), 48);
    }
    age(&deep, 1);
    let newest = measure(&scratch.dir.join("folder"))
        .unwrap()
        .newest
        .unwrap();
    let file_time = std::fs::metadata(&deep).unwrap().mtime();
    assert_eq!(newest, file_time);
}
