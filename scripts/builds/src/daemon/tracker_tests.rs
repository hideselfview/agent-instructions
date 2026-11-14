use super::*;
use crate::testing::{Scratch, fill};
use crate::usage::measure as walk_and_measure;

/// A tracker whose questions wait for the stream as long as it takes, up to a
/// hang guard no loaded machine reaches.
fn start(root: &Path) -> Arc<Tracker> {
    Tracker::start_waiting(
        root,
        &root.join(".builds-daemon"),
        std::time::Duration::from_secs(300),
    )
    .unwrap()
}

/// The folders each batch the tracker handled named, from when it was made.
struct Handled(std::sync::mpsc::Receiver<Vec<PathBuf>>);

impl Handled {
    fn listen(tracker: &Tracker) -> Handled {
        let (told, heard) = std::sync::mpsc::channel();
        tracker.listen(Box::new(move |paths| {
            let _ = told.send(paths.to_vec());
        }));
        Handled(heard)
    }

    /// Makes <write>, then blocks until handled batches have named every one
    /// of <folders>, the folders whose listing the write changed. FSEvents
    /// orders a mark's event against no write reliably (another process's, or
    /// a folder elsewhere), so each write waits for its own events. The limit
    /// is only a guard against a stuck stream.
    fn after(&self, folders: &[PathBuf], write: impl FnOnce()) {
        while self.0.try_recv().is_ok() {}
        write();
        let mut waiting: Vec<&PathBuf> = folders.iter().collect();
        let guard = std::time::Instant::now() + std::time::Duration::from_secs(300);
        while !waiting.is_empty() {
            let left = guard.saturating_duration_since(std::time::Instant::now());
            let paths = self
                .0
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no event named {waiting:?}"));
            waiting.retain(|folder| !paths.contains(folder));
        }
    }
}

/// The tracker's answer for each of <paths>, after FSEvents has delivered
/// every write made so far, equals a fresh walk's.
fn agrees(tracker: &Tracker, paths: &[PathBuf], when: &str) {
    agrees_with(tracker, paths, when, true);
}

/// Like `agrees`, for a state the test made without FSEvents (a synchronous
/// rescan, or a root whole before the tracker saw it): the answer needs no
/// wait for the stream, which a loaded machine can hold back for minutes.
fn agrees_now(tracker: &Tracker, paths: &[PathBuf], when: &str) {
    agrees_with(tracker, paths, when, false);
}

fn agrees_with(tracker: &Tracker, paths: &[PathBuf], when: &str, fresh: bool) {
    let answers = tracker.measure(paths, fresh);
    for (path, answer) in paths.iter().zip(answers) {
        let walked = walk_and_measure(path).unwrap();
        assert_eq!(
            answer,
            Some(walked),
            "{when}: {} differs from a walk; tracker: {}",
            path.display(),
            tracker.describe()
        );
    }
}

fn run(command: &str, args: &[&std::ffi::OsStr]) {
    let status = std::process::Command::new(command)
        .args(args)
        .status()
        .unwrap();
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
    let tracker = start(&root);
    tracker.wait_ready();
    let handled = Handled::listen(&tracker);
    assert!(
        !root.join(".builds-daemon/mark-7").exists(),
        "a killed daemon's mark was left"
    );
    // Not the root itself: each answer's mark writes in the daemon's own
    // folder there, after the answer.
    let folders = [
        root.join("demo"),
        target.clone(),
        target.join("debug/incremental"),
    ];
    agrees(&tracker, &folders, "after the walk");

    let deps = target.join("debug/deps");
    let linked = target.join("debug/incremental/one-1/s-1");
    handled.after(
        &[deps.clone(), linked.clone(), target.join("debug")],
        || {
            fill(&deps.join("libtwo.rlib"), 2048);
            std::fs::hard_link(deps.join("libtwo.rlib"), linked.join("two.o")).unwrap();
            run(
                "cp",
                &[
                    "-c".as_ref(),
                    deps.join("libtwo.rlib").as_os_str(),
                    target.join("debug/libtwo.rlib").as_os_str(),
                ],
            );
        },
    );
    agrees(&tracker, &folders, "after a write, a hard link and a clone");

    // A write through one name of a linked file shows through the other.
    handled.after(std::slice::from_ref(&deps), || {
        fill(&deps.join("libtwo.rlib"), 3000)
    });
    agrees(&tracker, &folders, "after rewriting a linked file");

    let side_2 = root.join("demo/side-2/target");
    handled.after(&[root.join("demo")], || {
        std::fs::create_dir_all(side_2.join("a/b/c")).unwrap();
        fill(&side_2.join("a/b/c/file"), 64);
    });
    agrees(
        &tracker,
        &[side_2.clone(), root.join("demo")],
        "after a new tree",
    );

    // A folder renamed in place, then moved into another folder.
    handled.after(std::slice::from_ref(&side_2), || {
        std::fs::rename(side_2.join("a"), side_2.join("z")).unwrap()
    });
    agrees(
        &tracker,
        std::slice::from_ref(&side_2),
        "after a rename in place",
    );
    handled.after(&[side_2.join("z"), target.join("debug")], || {
        std::fs::rename(side_2.join("z/b"), target.join("debug/b")).unwrap();
    });
    agrees(
        &tracker,
        &[side_2.clone(), target.clone(), root.join("demo")],
        "after a move between folders",
    );

    // An eviction: renamed into the trash, then removed. The trash isn't
    // followed, so a question about it gets no answer.
    let slot = root.join(".trash/evict.1");
    std::fs::create_dir_all(&slot).unwrap();
    handled.after(&[root.join("demo/side-1")], || {
        std::fs::rename(&target, slot.join("target")).unwrap();
        std::fs::create_dir(&target).unwrap();
    });
    agrees(
        &tracker,
        &[target.clone(), root.join("demo")],
        "after a rename into the trash",
    );
    assert_eq!(
        tracker.measure(&[slot.clone(), root.join(".trash")], true),
        vec![None, None],
        "the trash was answered"
    );
    std::fs::remove_dir_all(&slot).unwrap();
    agrees(&tracker, &[root.join("demo")], "after the trash emptied");

    assert_eq!(
        tracker.measure(&[scratch.dir.join("elsewhere")], true),
        vec![None],
        "a path outside the root was answered"
    );
}

#[test]
fn a_replaced_root_is_walked_again() {
    let scratch = Scratch::new("tracker-root");
    let base = std::fs::canonicalize(&scratch.dir).unwrap();
    let root = base.join("volume");
    fill(&root.join("demo/main/target/a"), 100);
    let tracker = start(&root);
    tracker.wait_ready();
    // The volume goes and another comes back in its place.
    // The new one is whole before it takes the old one's place, so no walk
    // of it sees half of it.
    let new = base.join("new");
    fill(&new.join("demo/main/target/b"), 300);
    std::fs::create_dir_all(new.join(".builds-daemon")).unwrap();
    std::fs::rename(&root, base.join("old")).unwrap();
    std::fs::rename(&new, &root).unwrap();
    tracker.wait_following_current_root();
    agrees_now(
        &tracker,
        &[root.join("demo/main/target"), root.join("demo")],
        "after the root came back",
    );
}

#[test]
fn dropped_events_and_a_rescan_read_the_disk_again() {
    let scratch = Scratch::new("tracker-rescan");
    let root = std::fs::canonicalize(&scratch.dir).unwrap().join("volume");
    fill(&root.join("demo/side-1/target/a"), 100);
    let tracker = start(&root);
    tracker.wait_ready();
    fill(&root.join("demo/side-1/target/deep/b"), 200);
    std::fs::remove_file(root.join("demo/side-1/target/a")).unwrap();
    tracker.on_events(vec![Event {
        path: root.join("demo/side-1"),
        flags: flags::MUST_SCAN_SUBDIRS,
    }]);
    agrees_now(
        &tracker,
        &[root.join("demo/side-1/target")],
        "after a rescan of a subtree",
    );
    // A lost batch naming a folder walks that folder alone again.
    fill(&root.join("demo/side-2/target/c"), 50);
    let walks = tracker.flags.lock().unwrap().walks;
    tracker.on_events(vec![Event {
        path: root.join("demo/side-2"),
        flags: flags::MUST_SCAN_SUBDIRS | flags::USER_DROPPED,
    }]);
    agrees_now(
        &tracker,
        &[root.join("demo/side-2/target"), root.join("demo")],
        "after events lost under a folder",
    );
    assert_eq!(
        tracker.flags.lock().unwrap().walks,
        walks,
        "a lost batch naming a folder walked the whole root"
    );
    // One naming no folder walks the whole root.
    fill(&root.join("demo/side-2/target/d"), 70);
    tracker.on_events(vec![Event {
        path: root.clone(),
        flags: flags::USER_DROPPED,
    }]);
    agrees_now(
        &tracker,
        &[root.join("demo/side-2/target"), root.join("demo")],
        "after dropped events",
    );
    assert_eq!(
        tracker.flags.lock().unwrap().walks,
        walks + 1,
        "a lost batch naming no folder didn't walk the root"
    );
}
