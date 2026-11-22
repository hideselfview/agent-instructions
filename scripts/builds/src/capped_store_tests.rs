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
    prune(tool, run, root, cap_kib, || Ok(Some(vec![group(root)]))).unwrap();
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
    prune(&tool, Run::Real, &root, 100, || Ok(Some(vec![group]))).unwrap();
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
    prune(&tool, Run::Real, &root, 450, || Ok(Some(groups))).unwrap();
    let (x, y) = (&files[0], &files[1]);
    assert!(!x.0.exists() && !x.0.with_extension("json").exists() && !x.1.exists());
    assert!(
        y.0.exists() && !y.1.exists(),
        "the loose file went, the unit stayed under its cap"
    );
}

#[test]
fn a_writer_can_take_its_lease_while_prune_waits_for_the_process_scan() {
    use std::sync::mpsc;
    use std::time::Duration;
    let scratch = Scratch::new("capped-bounded-wait");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 30);
    let lsof = scratch.script("lsof", "exit 0");
    let (scanning, started) = mpsc::channel();
    let (release, finish) = mpsc::channel();
    let (wrote, acquired) = mpsc::channel();
    std::thread::scope(|scope| {
        let tool = &tool;
        let cache = &root;
        let prune = scope.spawn(move || {
            prune_with(
                tool,
                Run::Real,
                cache,
                100,
                || Ok(Some(vec![group(cache)])),
                || {
                    scanning.send(()).unwrap();
                    finish.recv().unwrap();
                    HeldPaths::read_with(&lsof)
                },
            )
        });
        if let Err(error) = started.recv_timeout(Duration::from_secs(5)) {
            drop(release);
            panic!("prune did not start its process scan: {error}");
        }
        let writer = scope.spawn(move || {
            let _hold = writing(cache).unwrap();
            wrote.send(()).unwrap();
        });
        let acquired = acquired.recv_timeout(Duration::from_millis(1500));
        release.send(()).unwrap();
        writer.join().unwrap();
        prune.join().unwrap().unwrap();
        assert!(acquired.is_ok(), "the writer waited for the process scan");
    });
    assert!(unit.is_dir(), "the writer invalidated the prune plan");
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
        Ok(Some(vec![stale]))
    })
    .unwrap();
    assert!(unit.join("data").is_file());
}

#[test]
fn an_invalidated_plan_does_not_prepare_a_trash_slot() {
    let scratch = Scratch::new("capped-invalidated-trash");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 30);
    std::fs::write(tool.places.trash(), "this path cannot hold a slot").unwrap();

    prune(&tool, Run::Real, &root, 100, || {
        let planned = group(&root);
        let _reading = writing(&root)?;
        Ok(Some(vec![planned]))
    })
    .unwrap();
    assert!(unit.is_dir());
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
        Ok(Some(vec![group(&root)]))
    });
    let mut child = reader.unwrap();
    drop(child.stdin.take());
    child.wait().unwrap();
    pruned.unwrap();
    assert!(oldest.is_dir());
    assert!(!alternative.exists() && !leftover.exists());
}

#[test]
fn missing_files_are_not_counted_as_a_deletion() {
    let scratch = Scratch::new("capped-file-vanished");
    let missing = scratch.dir.join("gone");
    let second = scratch.dir.join("second");
    let files = [missing, second.clone()];
    let mut changes = Vec::new();

    Target::Files(&files).delete(0, &mut changes).unwrap();
    assert!(changes.iter().all(|entry| !entry.removed));
    changes.clear();
    std::fs::write(&second, "entry").unwrap();
    Target::Files(&files).delete(0, &mut changes).unwrap();
    assert_eq!(changes[0].path, files[0]);
    assert!(!changes[0].removed);
    assert_eq!(changes[1].path, second);
    assert!(changes[1].removed);
    assert!(!second.exists());
}

#[test]
fn an_unlink_failure_after_a_refused_move_still_fails_the_cache_prune() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("capped-two-failures");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let refused = store(&root, "locked/run", 400, 30);
    let bad_file = root.join("directory-in-place-of-file");
    std::fs::create_dir(&bad_file).unwrap();
    std::fs::write(root.join(PLAN), "").unwrap();
    let parent = refused.parent().unwrap();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();

    let steps = vec![
        Deletion {
            unit: Unit::Folder(refused.clone()),
            reason: "old run".into(),
        },
        Deletion {
            unit: Unit::Files(vec![bad_file.clone()]),
            reason: "old file".into(),
        },
    ];
    let error = apply_plan(&tool, Run::Real, &root, steps).unwrap_err();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(matches!(error, fail::Fail::Message(_)), "{error:?}");
    assert!(error.text().contains(bad_file.to_str().unwrap()));
    assert!(refused.is_dir());
}

#[test]
fn a_failed_open_file_scan_keeps_folder_candidates() {
    let scratch = Scratch::new("capped-scan-error");
    let tool = scratch.tool(0);
    let root = scratch.dir.join("root");
    let unit = store(&root, "g/a", 400, 30);
    let probe = scratch.script(
        "lsof",
        "printf 'p123\\nn/tmp/partial\\n'; echo 'process listing failed' >&2; exit 1",
    );
    let error = prune_with(
        &tool,
        Run::Real,
        &root,
        100,
        || Ok(Some(vec![group(&root)])),
        || HeldPaths::read_with(&probe),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("process listing failed"));
    assert!(unit.is_dir());
}
