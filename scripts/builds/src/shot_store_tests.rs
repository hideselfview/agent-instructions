use super::*;
use crate::testing::{Scratch, age, fill};

fn key(prefix: &str) -> Key {
    Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        prefix: prefix.to_string(),
        version: "0123456789abcdef".to_string(),
    }
}

/// Stores a run of <key> that exited <exit> and printed <findings>, with a
/// report folder holding a findings.json when <records> is set.
fn store_run(places: &Places, key: &Key, exit: i64, findings: &str, records: bool) -> Stored {
    let staging = stage(places, key).unwrap();
    if records {
        std::fs::create_dir_all(staging.report()).unwrap();
        std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    }
    staging.finish(exit, findings).unwrap()
}

#[test]
fn a_run_is_kept_on_the_build_volume_under_its_repo_commit_lane_prefix_and_version() {
    let scratch = Scratch::new("shot-store-place");
    let places = scratch.places();
    let stored = store_run(&places, &key("mac.welcome."), 1, "FAIL x\n", true);
    assert_eq!(
        stored.folder,
        places.root.join(format!(
            "shots/demo/runs/{}/mac/mac.welcome./0123456789abcdef",
            "a".repeat(40)
        ))
    );
    assert!(stored.findings_json().is_file());
    assert!(stored.findings_json().starts_with(stored.report()));
}

#[test]
fn a_stored_run_is_found_by_its_whole_key_and_a_later_run_of_the_key_replaces_it() {
    let scratch = Scratch::new("shot-store-key");
    let places = scratch.places();
    let key = key("mac.a.");
    assert!(lookup(&places, &key).unwrap().is_none());
    store_run(&places, &key, 1, "first\n", true);
    store_run(&places, &key, 0, "second\n", false);
    let found = lookup(&places, &key).unwrap().expect("stored");
    assert_eq!((found.exit_code, found.findings.as_str()), (0, "second\n"));
    assert!(
        !found.findings_json().exists(),
        "the later run replaced the whole folder"
    );
    for other in [
        Key {
            commit: "c".repeat(40),
            ..key.clone()
        },
        Key {
            lane: Lane::Android,
            ..key.clone()
        },
        Key {
            prefix: "mac.b.".to_string(),
            ..key.clone()
        },
        Key {
            version: "fedcba9876543210".to_string(),
            ..key.clone()
        },
        Key {
            repo: "other".to_string(),
            ..key.clone()
        },
    ] {
        assert!(lookup(&places, &other).unwrap().is_none(), "{other:?}");
    }
}

#[test]
fn a_staging_that_is_never_finished_leaves_nothing() {
    let scratch = Scratch::new("shot-store-unfinished");
    let places = scratch.places();
    let key = key("mac.a.");
    {
        let staging = stage(&places, &key).unwrap();
        std::fs::create_dir_all(staging.report()).unwrap();
    }
    assert!(lookup(&places, &key).unwrap().is_none());
    let prefix = run_folder(&places, &key).parent().unwrap().to_path_buf();
    assert_eq!(std::fs::read_dir(prefix).unwrap().count(), 0);
}

#[test]
fn a_prefix_that_names_no_single_folder_is_refused() {
    let scratch = Scratch::new("shot-store-prefix");
    let places = scratch.places();
    for prefix in ["", "mac/a.", ".mac", ".."] {
        assert!(stage(&places, &key(prefix)).is_err(), "{prefix:?}");
        assert!(lookup(&places, &key(prefix)).is_err(), "{prefix:?}");
    }
}

/// Sets the end of the run in <folder> <seconds> earlier.
fn ended_earlier(folder: &Path, seconds: i64) {
    let file = folder.join(RUN_FILE);
    let mut ending: Ending =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    ending.recorded_at -= seconds;
    std::fs::write(&file, serde_json::to_string(&ending).unwrap()).unwrap();
}

#[test]
fn the_before_run_is_the_newest_run_of_the_version_whose_prefix_starts_the_runs_and_keeps_its_findings()
 {
    let scratch = Scratch::new("shot-store-before");
    let places = scratch.places();
    let base = "a".repeat(40);
    let key = |prefix: &str, version: &str| Key {
        version: version.to_string(),
        ..key(prefix)
    };
    let find = |prefix: &str| {
        before_run(&places, "demo", &base, Lane::Mac, prefix, "v1")
            .unwrap()
            .map(|before| before.key.prefix)
    };
    assert_eq!(find("mac.welcome."), None);
    store_run(&places, &key("mac.welcome.", "v2"), 1, "x\n", true);
    store_run(&places, &key("mac.settings.", "v1"), 1, "x\n", true);
    store_run(&places, &key("mac.welcome.a.", "v1"), 1, "x\n", true);
    assert_eq!(
        find("mac.welcome."),
        None,
        "another version's run, another screen's and a narrower prefix's do not stand in"
    );
    let all = store_run(&places, &key("mac.", "v1"), 1, "x\n", true);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    let before = before_run(&places, "demo", &base, Lane::Mac, "mac.welcome.", "v1")
        .unwrap()
        .unwrap();
    assert_eq!(before.findings_json(), all.findings_json());
    // A run kept later wins; one whose report keeps no findings.json does
    // not count. The run of mac. ended ten seconds before the runs below.
    ended_earlier(&all.folder, 10);
    store_run(&places, &key("mac.welcome.", "v1"), 1, "x\n", false);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    store_run(&places, &key("mac.welcome.", "v1"), 1, "x\n", true);
    assert_eq!(find("mac.welcome."), Some("mac.welcome.".to_string()));
    assert_eq!(
        before_run(&places, "demo", &base, Lane::Android, "android.", "v1").unwrap(),
        None
    );
}

#[test]
fn a_read_marks_the_run_used() {
    let scratch = Scratch::new("shot-store-used");
    let places = scratch.places();
    let key = key("mac.a.");
    let stored = store_run(&places, &key, 0, "ok\n", true);
    let file = stored.folder.join(RUN_FILE);
    age(&file, 48);
    let aged = std::fs::metadata(&file).unwrap().modified().unwrap();
    lookup(&places, &key).unwrap().expect("stored");
    assert!(std::fs::metadata(&file).unwrap().modified().unwrap() > aged);
}

#[test]
fn over_the_cap_the_least_recently_used_runs_go_first_until_the_store_fits() {
    let scratch = Scratch::new("shot-store-cap");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let mut folders = Vec::new();
    for (n, hours) in [(0, 30), (1, 10), (2, 20), (3, 1)] {
        let key = key(&format!("mac.p{n}."));
        let stored = store_run(&places, &key, 0, "ok\n", true);
        fill(&stored.report().join("img/a.png"), 400);
        age(&stored.folder.join(RUN_FILE), hours);
        folders.push(stored.folder);
    }
    prune_runs(&tool, Run::Dry, 1000).unwrap();
    assert!(
        folders.iter().all(|folder| folder.exists()),
        "a dry run deletes nothing"
    );
    prune_runs(&tool, Run::Real, 1000).unwrap();
    let left: Vec<bool> = folders.iter().map(|folder| folder.exists()).collect();
    assert_eq!(
        left,
        [false, true, false, true],
        "used 30h and 20h ago went first"
    );
}

#[test]
fn a_run_used_in_the_last_half_hour_stays_over_the_cap() {
    let scratch = Scratch::new("shot-store-recent");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let stored = store_run(&places, &key("mac.a."), 0, "ok\n", true);
    fill(&stored.report().join("img/a.png"), 400);
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(stored.folder.exists());
}

#[test]
fn a_staging_folder_left_by_a_run_that_died_goes_once_nothing_wrote_to_it_for_half_an_hour() {
    let scratch = Scratch::new("shot-store-leftover");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let staging = stage(&places, &key("mac.a.")).unwrap();
    std::fs::create_dir_all(staging.report()).unwrap();
    let left = staging.dir.clone();
    std::mem::forget(staging);
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(left.exists(), "a run may still be filling it");
    crate::testing::age_all(&left, 1);
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(!left.exists());
}

#[test]
fn the_folders_deleted_runs_leave_empty_go_once_nothing_changed_in_them_for_half_an_hour() {
    let scratch = Scratch::new("shot-store-empty");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let stored = store_run(&places, &key("mac.a."), 0, "ok\n", true);
    fill(&stored.report().join("img/a.png"), 400);
    crate::testing::age_all(&root(&places), 2);
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(!stored.folder.exists());
    let prefix = stored.folder.parent().unwrap().to_path_buf();
    assert!(prefix.exists(), "the deletion just changed it");
    crate::testing::age_all(&root(&places), 2);
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(!root(&places).join("demo").exists());
    assert!(root(&places).exists());
}
