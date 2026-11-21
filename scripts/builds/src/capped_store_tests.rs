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
            if is_staging(&folder.file_name().unwrap().to_string_lossy()) {
                kept.push(Kept::Staging { folder });
                continue;
            }
            use std::os::unix::fs::MetadataExt;
            let last_use = std::fs::metadata(folder.join("data")).unwrap().mtime();
            kept.push(Kept::Folder {
                folder,
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
    prune(tool, run, root, cap_kib, vec![group(root)]).unwrap();
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
    assert!(!root.join("g/a").exists());
    assert_eq!(std::fs::read_dir(root.join("g")).unwrap().count(), 0);
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
    let left = root.join("g/.staging-old-1-2");
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
            folder: unit.clone(),
            last_use: Use::At(0),
            what: "unit",
        }],
    };
    prune(&tool, Run::Real, &root, 100, vec![group]).unwrap();
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
    prune(&tool, Run::Real, &root, 450, groups).unwrap();
    let (x, y) = (&files[0], &files[1]);
    assert!(!x.0.exists() && !x.0.with_extension("json").exists() && !x.1.exists());
    assert!(
        y.0.exists() && !y.1.exists(),
        "the loose file went, the unit stayed under its cap"
    );
}
