use super::*;
use crate::testing::{Scratch, fill};
use crate::usage::measure as walk_and_measure;

/// The tracker's answer for each of <paths> equals a fresh walk's.
fn agrees(tracker: &Tracker, paths: &[PathBuf], when: &str) {
    let answers = tracker.measure(paths, true);
    for (path, answer) in paths.iter().zip(answers) {
        let walked = walk_and_measure(path).unwrap();
        assert_eq!(answer, Some(walked), "{when}: {} differs from a walk", path.display());
    }
}

fn run(command: &str, args: &[&std::ffi::OsStr]) {
    let status = std::process::Command::new(command).args(args).status().unwrap();
    assert!(status.success(), "{command} {args:?}");
}

#[test]
fn follows_writes_links_clones_renames_and_deletions() {
    let scratch = Scratch::new("tracker");
    let root = std::fs::canonicalize(&scratch.dir).unwrap().join("volume");
    let target = root.join("demo/side-1/target");
    fill(&target.join("debug/deps/libone.rlib"), 1500);
    fill(&target.join("debug/incremental/one-1/s-1/part.o"), 300);
    fill(&root.join(".builds-daemon/mark-7"), 0);
    let tracker = Tracker::start(&root, &root.join(".builds-daemon")).unwrap();
    tracker.wait_ready();
    assert!(!root.join(".builds-daemon/mark-7").exists(), "a killed daemon's mark was left");
    // Not the root itself: each answer's mark writes in the daemon's own
    // folder there, after the answer.
    let folders = [root.join("demo"), target.clone(), target.join("debug/incremental")];
    agrees(&tracker, &folders, "after the walk");

    fill(&target.join("debug/deps/libtwo.rlib"), 2048);
    std::fs::hard_link(target.join("debug/deps/libtwo.rlib"), target.join("debug/incremental/one-1/s-1/two.o")).unwrap();
    run("cp", &["-c".as_ref(), target.join("debug/deps/libtwo.rlib").as_os_str(), target.join("debug/libtwo.rlib").as_os_str()]);
    agrees(&tracker, &folders, "after a write, a hard link and a clone");

    // A write through one name of a linked file shows through the other.
    fill(&target.join("debug/deps/libtwo.rlib"), 3000);
    agrees(&tracker, &folders, "after rewriting a linked file");

    std::fs::create_dir_all(root.join("demo/side-2/target/a/b/c")).unwrap();
    fill(&root.join("demo/side-2/target/a/b/c/file"), 64);
    agrees(&tracker, &[root.join("demo/side-2/target"), root.join("demo")], "after a new tree");

    // An eviction: renamed into the trash, then removed. The trash isn't
    // followed, so a question about it gets no answer.
    let slot = root.join(".trash/evict.1");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::rename(&target, slot.join("target")).unwrap();
    std::fs::create_dir(&target).unwrap();
    agrees(&tracker, &[target.clone(), root.join("demo")], "after a rename into the trash");
    assert_eq!(tracker.measure(&[slot.clone(), root.join(".trash")], true), vec![None, None], "the trash was answered");
    std::fs::remove_dir_all(&slot).unwrap();
    agrees(&tracker, &[root.join("demo")], "after the trash emptied");

    assert_eq!(tracker.measure(&[scratch.dir.join("elsewhere")], true), vec![None], "a path outside the root was answered");
}

#[test]
fn a_replaced_root_is_walked_again() {
    let scratch = Scratch::new("tracker-root");
    let base = std::fs::canonicalize(&scratch.dir).unwrap();
    let root = base.join("volume");
    fill(&root.join("demo/main/target/a"), 100);
    let tracker = Tracker::start(&root, &root.join(".builds-daemon")).unwrap();
    tracker.wait_ready();
    // The volume goes and another comes back in its place.
    std::fs::rename(&root, base.join("old")).unwrap();
    fill(&root.join("demo/main/target/b"), 300);
    std::fs::create_dir_all(root.join(".builds-daemon")).unwrap();
    tracker.wait_walks(2);
    agrees(&tracker, &[root.join("demo/main/target"), root.join("demo")], "after the root came back");
}

#[test]
fn dropped_events_and_a_rescan_read_the_disk_again() {
    let scratch = Scratch::new("tracker-rescan");
    let root = std::fs::canonicalize(&scratch.dir).unwrap().join("volume");
    fill(&root.join("demo/side-1/target/a"), 100);
    let tracker = Tracker::start(&root, &root.join(".builds-daemon")).unwrap();
    tracker.wait_ready();
    fill(&root.join("demo/side-1/target/deep/b"), 200);
    std::fs::remove_file(root.join("demo/side-1/target/a")).unwrap();
    tracker.on_events(vec![Event { path: root.join("demo/side-1"), flags: flags::MUST_SCAN_SUBDIRS }]);
    agrees(&tracker, &[root.join("demo/side-1/target")], "after a rescan of a subtree");
    fill(&root.join("demo/side-2/target/c"), 50);
    tracker.on_events(vec![Event { path: root.clone(), flags: flags::USER_DROPPED }]);
    agrees(&tracker, &[root.join("demo/side-2/target"), root.join("demo")], "after dropped events");
}
