use super::*;
use crate::testing::{Scratch, age, fill};

/// Stores a unit of <kib> KiB at <root>/<place>, last used <hours> ago.
fn store(root: &Path, place: &str, kib: usize, hours: u64) -> PathBuf {
    let into = root.join(place);
    let staging = stage(root, into.clone(), "store").unwrap();
    fill(&staging.dir().join("data"), kib);
    staging.place().unwrap();
    age(&into.join("data"), hours);
    into
}

/// The units under <root> two folders down, as one group: the time of each
/// unit's file `data` is its last use.
fn group(root: &Path) -> Group {
    let mut kept = Vec::new();
    for outer in std::fs::read_dir(root).unwrap() {
        let outer = outer.unwrap().path();
        if !outer.is_dir()
            || outer
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with('.')
        {
            continue;
        }
        for unit in std::fs::read_dir(&outer).unwrap() {
            let folder = unit.unwrap().path();
            use std::os::unix::fs::MetadataExt;
            let last_use = std::fs::metadata(folder.join("data")).unwrap().mtime();
            kept.push(Kept::Folder {
                folder: Folder {
                    path: folder,
                    reader_lock: None,
                },
                last_use: Use::At(last_use),
                what: "unit",
            });
        }
    }
    Group {
        folder: root.to_path_buf(),
        kept,
    }
}

fn prune_now(tool: &Tool, run: Run, root: &Path, cap_kib: u64) {
    prune(tool, run, root, cap_kib, || Ok(vec![group(root)])).unwrap();
}

#[test]
fn a_unit_placed_again_replaces_the_whole_earlier_unit() {
    let scratch = Scratch::new("capped-replace");
    let root = scratch.dir.join("root");
    let into = store(&root, "g/a", 1, 0);
    fill(&into.join("old"), 1);
    store(&root, "g/a", 1, 0);
    assert!(into.join("data").is_file());
    assert!(!into.join("old").exists());
}

#[test]
fn a_staging_that_is_never_placed_leaves_nothing() {
    let scratch = Scratch::new("capped-unfinished");
    let root = scratch.dir.join("root");
    {
        let staging = stage(&root, root.join("g/a"), "t").unwrap();
        fill(&staging.dir().join("data"), 1);
    }
    let discarded = stage(&root, root.join("g/a"), "t").unwrap();
    fill(&discarded.dir().join("data"), 1);
    discarded.discard().unwrap();
    // The writer made no folder above the unit's place.
    assert!(!root.join("g").exists());
}

#[test]
fn over_the_cap_the_least_recently_used_units_go_first_until_the_root_fits() {
    let scratch = Scratch::new("capped-cap");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let units: Vec<PathBuf> = [("g/0", 30), ("g/1", 10), ("g/2", 20), ("g/3", 1)]
        .into_iter()
        .map(|(place, hours)| store(&root, place, 400, hours))
        .collect();
    prune_now(&tool, Run::Dry, &root, 1000);
    assert!(
        units.iter().all(|unit| unit.exists()),
        "a dry run deletes nothing"
    );
    prune_now(&tool, Run::Real, &root, 1000);
    let left: Vec<bool> = units.iter().map(|unit| unit.exists()).collect();
    assert_eq!(
        left,
        [false, true, false, true],
        "used 30h and 20h ago went first"
    );
}

#[test]
fn a_unit_used_in_the_last_half_hour_stays_over_the_cap() {
    let scratch = Scratch::new("capped-recent");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 0);
    prune_now(&tool, Run::Real, &root, 100);
    assert!(unit.exists());
}

#[test]
fn a_staging_folder_goes_once_nobody_holds_the_root_and_nothing_goes_while_someone_does() {
    let scratch = Scratch::new("capped-leftover");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let filling = stage(&root, root.join("g/a"), "t").unwrap();
    fill(&filling.dir().join("data"), 1);
    let left = root.join(".staging-old-1-2");
    fill(&left.join("data"), 1);
    let old = store(&root, "g/b", 400, 30);
    prune_now(&tool, Run::Real, &root, 100);
    assert!(
        left.exists() && filling.dir().exists() && old.exists(),
        "a writer holds the root, so nothing goes"
    );
    let dir = filling.dir().to_path_buf();
    drop(filling);
    assert!(!dir.exists());
    let reading = writing(&root).unwrap();
    prune_now(&tool, Run::Real, &root, 100);
    assert!(left.exists() && old.exists(), "a reader holds the root");
    drop(reading);
    prune_now(&tool, Run::Real, &root, 100);
    assert!(!left.exists() && !old.exists());
}

#[test]
fn the_folders_deleted_units_leave_empty_go_and_the_root_stays() {
    let scratch = Scratch::new("capped-empty");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/h/a", 400, 30);
    // The unit sits three folders down, so a listing here names it by hand.
    let group = Group {
        folder: root.clone(),
        kept: vec![Kept::Folder {
            folder: Folder {
                path: unit.clone(),
                reader_lock: None,
            },
            last_use: Use::At(0),
            what: "unit",
        }],
    };
    prune(&tool, Run::Real, &root, 100, || Ok(vec![group])).unwrap();
    assert!(!unit.exists());
    assert!(!root.join("g").exists());
    assert!(root.exists());
}

#[test]
fn files_and_loose_files_go_in_order_and_each_group_has_its_own_cap() {
    let scratch = Scratch::new("capped-files");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let mut groups = Vec::new();
    let mut files = Vec::new();
    for name in ["x", "y"] {
        let (old, loose) = (
            root.join(name).join("old.png"),
            root.join(name).join(".half"),
        );
        fill(&old, 400);
        fill(&loose, 1);
        fill(&root.join(name).join("old.json"), 1);
        age(&old, 30);
        files.push((old.clone(), loose.clone()));
        groups.push(Group {
            folder: root.join(name),
            kept: vec![
                Kept::Files {
                    files: vec![old.clone(), root.join(name).join("old.json")],
                    last_use: 0,
                    what: "a picture",
                },
                Kept::Loose { file: loose },
            ],
        });
    }
    // Only the first group, with 501 KiB, is over the cap of 450 KiB.
    fill(&root.join("x").join("more"), 100);
    prune(&tool, Run::Real, &root, 450, || Ok(groups)).unwrap();
    let (x, y) = (&files[0], &files[1]);
    assert!(!x.0.exists() && !x.0.with_extension("json").exists() && !x.1.exists());
    assert!(
        y.0.exists() && !y.1.exists(),
        "the loose file went, the unit stayed under its cap"
    );
}

/// A daemon that takes every connection and never answers, as one does while
/// it is busy: each question to it costs its asker the whole wait. The
/// receiver says when a first question arrives.
fn silent_daemon(
    tool: &Tool,
) -> (
    std::os::unix::net::UnixListener,
    std::sync::mpsc::Receiver<()>,
) {
    let socket = tool.places.daemon_socket();
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let held = listener.try_clone().unwrap();
    let (asked, first) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut open = Vec::new();
        while let Ok((stream, _)) = held.accept() {
            open.push(stream);
            let _ = asked.send(());
        }
    });
    (listener, first)
}

/// A prune holds the root exclusively only while it moves units into the
/// trash: the questions it asks the daemon, and the measuring, come before or
/// after, so a writer that arrives mid-prune waits for the renames only.
#[test]
fn a_writer_waits_for_a_prunes_renames_not_for_its_questions_to_the_daemon() {
    let scratch = Scratch::new("capped-bounded-wait");
    let mut tool = scratch.tool(0);
    // A socket's path must fit in SUN_LEN, which the scratch folder's does not.
    let volume = PathBuf::from(format!("/tmp/bw-{}", std::process::id()));
    std::fs::create_dir_all(&volume).unwrap();
    // /tmp is a link, and the prune resolves each folder it deletes.
    let volume = std::fs::canonicalize(&volume).unwrap();
    tool.places.root = volume.clone();
    let (_daemon, asked) = silent_daemon(&tool);
    let root = tool.places.root.join("shots");
    for n in 0..6 {
        store(&root, &format!("bae/{n}"), 400, 30 + n);
    }
    let waited = std::thread::scope(|scope| {
        let pruning = scope.spawn(|| prune_now(&tool, Run::Real, &root, 100));
        // The prune has asked the daemon, so it has reached the work that
        // used to run under the lock.
        asked.recv().unwrap();
        let started = std::time::Instant::now();
        let writer = writing(&root).unwrap();
        let waited = started.elapsed();
        drop(writer);
        pruning.join().unwrap();
        waited
    });
    assert!(
        waited < std::time::Duration::from_millis(1500),
        "a writer waited {waited:?} for a prune"
    );
    std::fs::remove_dir_all(&volume).unwrap();
}

#[test]
fn deleting_a_unit_leaves_empty_directories_inside_retained_units() {
    let scratch = Scratch::new("capped-retained-empty");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let old = store(&root, "g/old", 400, 30);
    let retained = store(&root, "g/retained", 400, 0);
    let empty = retained.join("report/empty");
    std::fs::create_dir_all(&empty).unwrap();

    prune_now(&tool, Run::Real, &root, 500);

    assert!(!old.exists());
    assert!(empty.is_dir());
}

#[test]
fn an_access_during_planning_keeps_the_updated_unit() {
    let scratch = Scratch::new("capped-plan-invalidated");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 30);
    prune(&tool, Run::Real, &root, 100, || {
        let stale = group(&root);
        // The plan is already open. A reader can acquire the shared lock
        // and update the unit before the planner returns its old snapshot.
        let _reading = writing(&root)?;
        mark_used(&unit.join("data"))?;
        Ok(vec![stale])
    })
    .unwrap();
    assert!(unit.join("data").is_file());
}

#[test]
fn one_failed_size_invalidation_does_not_stop_the_next_root() {
    let scratch = Scratch::new("capped-size-records");
    let tool = scratch.tool(0);
    let root = tool.places.root.join("shots");
    let a = store(&root, "a/run", 400, 30);
    let b = store(&root, "b/run", 400, 30);
    let records = tool.places.daemon_dir().join("sizes/shots");
    let first = records.join("a.json");
    let second = records.join("b.json");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::write(&second, "{}").unwrap();

    let error = prune(&tool, Run::Real, &root, 100, || Ok(vec![group(&root)]))
        .unwrap_err()
        .text();

    assert!(error.contains(&first.display().to_string()), "{error}");
    assert!(!a.exists() && !b.exists());
    assert!(first.is_dir());
    assert!(!second.exists());
}

#[test]
fn an_access_does_not_hide_an_unrelated_planning_error() {
    let scratch = Scratch::new("capped-planning-error");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 30);
    let error = prune(&tool, Run::Real, &root, 100, || {
        let _reading = writing(&root)?;
        fail!("the branch records could not be read")
    })
    .unwrap_err()
    .text();
    assert!(error.contains("the branch records could not be read"));
    assert!(unit.exists());
}

#[test]
fn the_open_file_snapshot_follows_discovery_and_prune_selects_an_alternative() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let scratch = Scratch::new("capped-held-in-discovery");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let oldest = store(&root, "g/oldest", 400, 30);
    let alternative = store(&root, "g/alternative", 400, 20);
    let leftover = root.join(".staging-leftover");
    fill(&leftover.join("data"), 1);
    let hold = scratch.script("hold", "exec 4<\"$1\"; echo open; read line");
    let mut reader = None;
    let pruned = prune(&tool, Run::Real, &root, 500, || {
        let mut child = Command::new(&hold)
            .arg(oldest.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut observed = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut observed)
            .unwrap();
        assert_eq!(observed.trim(), "open");
        reader = Some(child);
        Ok(vec![group(&root)])
    });
    let mut child = reader.unwrap();
    drop(child.stdin.take());
    child.wait().unwrap();
    pruned.unwrap();
    assert!(oldest.is_dir());
    assert!(!alternative.exists() && !leftover.exists());
}
