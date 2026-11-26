use super::*;

#[test]
fn the_cache_ignores_legacy_verdicts() {
    let scratch = Scratch::new("legacy-shot-verdict");
    let places = scratch.places();
    let key = key("mac.a.");
    let cached = cache_run(&places, &key, 0, "passed\n", true);
    std::fs::remove_file(cached.folder.join(RUN_FILE)).unwrap();
    std::fs::write(
        cached.folder.join("run.json"),
        r#"{"recorded_at": 1, "exit_code": 0}"#,
    )
    .unwrap();
    assert!(lookup(&places, &key).unwrap().is_none());
    assert!(
        covering_folders(
            &places,
            &key.repo,
            &key.commit,
            key.lane,
            &key.prefix,
            &key.versions
        )
        .unwrap()
        .is_empty(),
        "waiting branches cannot pin a run without a current verdict"
    );
    crate::testing::age_all(&cached.folder, 48);
    prune_runs(&scratch.tool(0), Run::Real, 0).unwrap();
    assert!(!cached.folder.exists());
}

#[test]
fn a_run_with_missing_metadata_fields_fails_to_read() {
    let scratch = Scratch::new("incomplete-shot-verdict");
    let places = scratch.places();
    let key = key("mac.a.");
    let cached = cache_run(&places, &key, 0, "passed\n", true);
    let metadata = cached.folder.join(RUN_FILE);
    std::fs::write(&metadata, r#"{"recorded_at": 1, "exit_code": 0}"#).unwrap();

    let error = lookup(&places, &key).unwrap_err().text();
    assert!(error.contains("missing field `kind`"), "{error}");
}
use crate::testing::{Scratch, age, fill};

const ENVIRONMENT: &str = "26B131-17C529";

fn key(prefix: &str) -> Key {
    Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        environment: ENVIRONMENT.to_string(),
        prefix: prefix.to_string(),
        versions: Versions {
            capture: "c0c0c0c0c0c0c0c0".to_string(),

            comparison: "0123456789abcdef".to_string(),
        },
    }
}

/// Stores a run of <key> that exited <exit> and printed <findings>, with a
/// report folder holding a findings.json when <records> is set.
fn cache_run(places: &Places, key: &Key, exit: i64, findings: &str, records: bool) -> Cached {
    let staging = stage(places, key).unwrap();
    if records {
        std::fs::create_dir_all(staging.report()).unwrap();
        std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    }
    staging.finish(exit, findings).unwrap()
}

#[test]
fn a_run_is_cached_on_the_build_volume_under_its_repo_commit_lane_environment_prefix_and_version() {
    let scratch = Scratch::new("shot-store-place");
    let places = scratch.places();
    let cached = cache_run(&places, &key("mac.welcome."), 1, "FAIL x\n", true);
    assert_eq!(
        cached.folder,
        places.root.join(format!(
            "shots/demo/runs/{}/mac/26B131-17C529/mac.welcome./c0c0c0c0c0c0c0c0/0123456789abcdef",
            "a".repeat(40)
        ))
    );
    assert!(cached.findings_json().is_file());
    assert!(cached.findings_json().starts_with(cached.report()));
}

#[test]
fn a_cached_run_is_found_by_its_whole_key_and_a_later_run_of_the_key_replaces_it() {
    let scratch = Scratch::new("shot-store-key");
    let places = scratch.places();
    let key = key("mac.a.");
    assert!(lookup(&places, &key).unwrap().is_none());
    cache_run(&places, &key, 1, "first\n", true);
    cache_run(&places, &key, 0, "second\n", false);
    let found = lookup(&places, &key).unwrap().expect("cached");
    assert_eq!(
        (found.exit_code(), found.findings.as_str()),
        (0, "second\n")
    );
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
            environment: "25G241-27B52".to_string(),
            ..key.clone()
        },
        Key {
            prefix: "mac.b.".to_string(),
            ..key.clone()
        },
        Key {
            versions: Versions {
                comparison: "fedcba9876543210".to_string(),
                ..key.versions.clone()
            },
            ..key.clone()
        },
        Key {
            versions: Versions {
                capture: "fedcba9876543210".to_string(),
                ..key.versions.clone()
            },
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
    let discarded = stage(&places, &key).unwrap();
    std::fs::create_dir_all(discarded.report()).unwrap();
    discarded.discard().unwrap();
    // A run that stops or is not kept leaves no folder of its key behind,
    // which a later lookup would have to tell from a run.
    let runs = root(&places).join("demo/runs");
    let left: Vec<_> = match std::fs::read_dir(&runs) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    };
    assert!(left.is_empty(), "{left:?}");
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
        versions: Versions {
            comparison: version.to_string(),
            ..key(prefix).versions
        },
        ..key(prefix)
    };
    let versions = Versions {
        capture: "c0c0c0c0c0c0c0c0".to_string(),
        comparison: "v1".to_string(),
    };
    let find = |prefix: &str| {
        covering_run_used(
            &places,
            "demo",
            &base,
            Lane::Mac,
            Made::In(ENVIRONMENT),
            prefix,
            &versions,
        )
        .unwrap()
        .map(|before| before.key.prefix)
    };
    assert_eq!(find("mac.welcome."), None);
    cache_run(&places, &key("mac.welcome.", "v2"), 1, "x\n", true);
    cache_run(
        &places,
        &Key {
            versions: Versions {
                capture: "another".to_string(),
                ..key("mac.welcome.", "v1").versions
            },
            ..key("mac.welcome.", "v1")
        },
        1,
        "x\n",
        true,
    );
    cache_run(&places, &key("mac.settings.", "v1"), 1, "x\n", true);
    cache_run(&places, &key("mac.welcome.a.", "v1"), 1, "x\n", true);
    assert_eq!(
        find("mac.welcome."),
        None,
        "another comparison version's run, another capture version's, another screen's and a narrower prefix's do not stand in"
    );
    let all = cache_run(&places, &key("mac.", "v1"), 1, "x\n", true);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    let before = covering_run_used(
        &places,
        "demo",
        &base,
        Lane::Mac,
        Made::In(ENVIRONMENT),
        "mac.welcome.",
        &versions,
    )
    .unwrap()
    .unwrap();
    assert_eq!(before.findings_json(), all.findings_json());
    // A run kept later wins; one whose report keeps no findings.json does
    // not count. The run of mac. ended ten seconds before the runs below.
    ended_earlier(&all.folder, 10);
    cache_run(&places, &key("mac.welcome.", "v1"), 1, "x\n", false);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    cache_run(&places, &key("mac.welcome.", "v1"), 1, "x\n", true);
    assert_eq!(find("mac.welcome."), Some("mac.welcome.".to_string()));
    assert_eq!(
        covering_run_used(
            &places,
            "demo",
            &base,
            Lane::Android,
            Made::In(ENVIRONMENT),
            "android.",
            &versions
        )
        .unwrap(),
        None
    );
}

#[test]
fn the_before_run_is_a_run_made_in_the_same_environment_and_no_other() {
    let scratch = Scratch::new("shot-store-environment");
    let places = scratch.places();
    let commit = "a".repeat(40);
    let versions = key("mac.a.").versions;
    let other = Key {
        environment: "25G241-27B52".to_string(),
        ..key("mac.a.")
    };
    cache_run(&places, &other, 1, "x\n", true);
    let find = |environment: &str| {
        covering_run_used(
            &places,
            "demo",
            &commit,
            Lane::Mac,
            Made::In(environment),
            "mac.a.",
            &versions,
        )
        .unwrap()
        .map(|before| before.key.environment)
    };
    assert_eq!(
        find(ENVIRONMENT),
        None,
        "another environment's run is no before run"
    );
    assert_eq!(find("25G241-27B52"), Some("25G241-27B52".to_string()));
    cache_run(&places, &key("mac.a."), 1, "x\n", true);
    assert_eq!(find(ENVIRONMENT), Some(ENVIRONMENT.to_string()));
    assert_eq!(find("25G241-27B52"), Some("25G241-27B52".to_string()));
}

#[test]
fn a_read_in_any_environment_finds_the_newest_run_and_names_its_environment() {
    let scratch = Scratch::new("shot-store-anywhere");
    let places = scratch.places();
    let commit = "a".repeat(40);
    let versions = key("mac.a.").versions;
    let covering = |made: Made| {
        covering_run(
            &places,
            "demo",
            &commit,
            Lane::Mac,
            made,
            "mac.a.",
            &versions,
        )
        .unwrap()
    };
    assert!(covering(Made::Anywhere).is_none());
    let old = cache_run(&places, &key("mac.a."), 1, "x\n", true);
    ended_earlier(&old.folder, 10);
    let other = Key {
        environment: "25G241-27B52".to_string(),
        ..key("mac.a.")
    };
    cache_run(&places, &other, 1, "x\n", true);
    assert_eq!(
        covering(Made::Anywhere).unwrap().key.environment,
        "25G241-27B52"
    );
    assert_eq!(
        covering(Made::In(ENVIRONMENT)).unwrap().key.environment,
        ENVIRONMENT
    );
    assert_eq!(
        environments(&places, "demo", &commit, Lane::Mac).unwrap(),
        ["25G241-27B52", ENVIRONMENT]
    );
    assert_eq!(
        covering_folders(&places, "demo", &commit, Lane::Mac, "mac.a.", &versions)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn the_shots_cap_reports_a_malformed_run_marker() {
    let scratch = Scratch::new("shot-store-invalid-marker");
    let tool = scratch.tool(0);
    let cached = cache_run(&tool.places, &key("mac.a."), 0, "ok\n", true);
    let marker = cached.folder.join(RUN_FILE);
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(RUN_FILE, &marker).unwrap();

    let error = prune_runs(&tool, Run::Real, 0).unwrap_err().text();
    assert!(error.contains(marker.to_str().unwrap()), "{error}");
    assert!(cached.folder.is_dir());

    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    let error = prune_runs(&tool, Run::Real, 0).unwrap_err().text();
    assert!(error.contains("must be a file"), "{error}");
}

#[test]
fn the_cap_leaves_a_folder_of_the_layout_without_an_environment_alone() {
    let scratch = Scratch::new("shot-store-old-layout");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let old = root(&places)
        .join("demo/runs")
        .join("b".repeat(40))
        .join("mac/mac.a./c0c0c0c0c0c0c0c0/0123456789abcdef");
    std::fs::create_dir_all(old.join("report/img")).unwrap();
    std::fs::write(old.join(RUN_FILE), r#"{"recorded_at": 1, "exit_code": 0}"#).unwrap();
    fill(&old.join("report/img/a.png"), 400);
    crate::testing::age_all(&root(&places), 48);
    let current = cache_run(&places, &key("mac.b."), 0, "ok\n", true);
    fill(&current.report().join("img/a.png"), 400);
    age(&current.folder.join(RUN_FILE), 48);
    prune_runs(&tool, Run::Real, 500).unwrap();
    assert!(
        !current.folder.exists(),
        "the run of the current layout is the one the cap deletes"
    );
    assert!(old.exists(), "no code reads the layout the key replaced");
}

#[test]
fn the_run_that_covers_a_prefix_is_found_without_marking_it_used() {
    let scratch = Scratch::new("shot-store-covering");
    let places = scratch.places();
    let cached = cache_run(&places, &key("mac.component."), 1, "x\n", true);
    let file = cached.folder.join(RUN_FILE);
    age(&file, 48);
    let aged = std::fs::metadata(&file).unwrap().modified().unwrap();
    let found = covering_run(
        &places,
        "demo",
        &"a".repeat(40),
        Lane::Mac,
        Made::Anywhere,
        "mac.component.checkbox.",
        &Versions {
            capture: "c0c0c0c0c0c0c0c0".to_string(),
            comparison: "0123456789abcdef".to_string(),
        },
    )
    .unwrap()
    .expect("the run of mac.component. covers the component");
    assert_eq!(found.folder, cached.folder);
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        aged,
        "a page that reads the run again each time the dashboard changes writes nothing"
    );
    mark_used(&found).unwrap();
    assert!(std::fs::metadata(&file).unwrap().modified().unwrap() > aged);
}

#[test]
fn a_read_marks_the_run_used() {
    let scratch = Scratch::new("shot-store-used");
    let places = scratch.places();
    let key = key("mac.a.");
    let cached = cache_run(&places, &key, 0, "ok\n", true);
    let file = cached.folder.join(RUN_FILE);
    age(&file, 48);
    let aged = std::fs::metadata(&file).unwrap().modified().unwrap();
    lookup(&places, &key).unwrap().expect("cached");
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
        let cached = cache_run(&places, &key, 0, "ok\n", true);
        fill(&cached.report().join("img/a.png"), 400);
        age(&cached.folder.join(RUN_FILE), hours);
        folders.push(cached.folder);
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
    let cached = cache_run(&places, &key("mac.a."), 0, "ok\n", true);
    fill(&cached.report().join("img/a.png"), 400);
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(cached.folder.exists());
}

#[test]
fn a_staging_folder_goes_once_no_run_stores_its_output_and_stays_while_one_does() {
    let scratch = Scratch::new("shot-store-leftover");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let filling = stage(&places, &key("mac.a.")).unwrap();
    std::fs::create_dir_all(filling.report()).unwrap();
    let left = root(&places).join(".staging-v-1-2");
    std::fs::create_dir_all(left.join("report")).unwrap();
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(
        left.exists() && filling.inner.dir().exists(),
        "a run is storing its output"
    );
    let dir = filling.inner.dir().to_path_buf();
    drop(filling);
    assert!(!dir.exists());
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(!left.exists(), "the run that filled it ended");
}

#[test]
fn the_prune_deletes_nothing_while_a_run_uses_the_folder_and_then_the_folders_deleted_runs_leave_empty()
 {
    let scratch = Scratch::new("shot-store-empty");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let cached = cache_run(&places, &key("mac.a."), 0, "ok\n", true);
    fill(&cached.report().join("img/a.png"), 400);
    crate::testing::age_all(&root(&places), 2);
    let other = stage(
        &places,
        &Key {
            commit: "c2".to_string(),
            ..key("mac.b.")
        },
    )
    .unwrap();
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(cached.folder.exists(), "a run may be reading it");
    other.discard().unwrap();
    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(!cached.folder.exists());
    assert!(!root(&places).join("demo").exists());
    assert!(root(&places).exists());
}

/// Writes a board picture of <kib> KiB with its element list into <repo>'s
/// picture folder under <hash>, as the shots program keeps one, last used
/// <hours> ago.
fn keep_picture(places: &Places, repo: &str, hash: &str, kib: usize, hours: u64) -> PathBuf {
    let boards = pictures(places, repo).join(BOARD_PICTURES);
    let png = boards.join(format!("{hash}.png"));
    fill(&png, kib);
    std::fs::write(boards.join(format!("{hash}.json")), "{}").unwrap();
    age(&png, hours);
    png
}

#[test]
fn the_cap_deletes_board_pictures_and_runs_together_least_recently_used_first() {
    let scratch = Scratch::new("shot-store-pictures-cap");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let old_picture = keep_picture(&places, "demo", "old", 400, 30);
    let new_picture = keep_picture(&places, "demo", "new", 400, 2);
    let cached = cache_run(&places, &key("mac.a."), 0, "ok\n", true);
    fill(&cached.report().join("img/a.png"), 400);
    age(&cached.folder.join(RUN_FILE), 20);
    prune_runs(&tool, Run::Real, 500).unwrap();
    assert!(!old_picture.exists() && !old_picture.with_extension("json").exists());
    assert!(
        !cached.folder.exists(),
        "used 20h ago, before the newer picture"
    );
    assert!(new_picture.exists() && new_picture.with_extension("json").exists());
}

/// Writes the component instances of a page into <repo>'s picture folder
/// under <hash>, as the shots program keeps them, of <kib> KiB, last used
/// <hours> ago.
fn keep_instances(places: &Places, repo: &str, hash: &str, kib: usize, hours: u64) -> PathBuf {
    let boards = pictures(places, repo).join(BOARD_PICTURES);
    let file = boards.join(format!("{hash}.instances.json"));
    fill(&file, kib);
    age(&file, hours);
    file
}

#[test]
fn a_pictures_instances_go_with_the_picture_and_count_in_its_size() {
    let scratch = Scratch::new("shot-store-instances-with-picture");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let old_picture = keep_picture(&places, "demo", "old", 200, 30);
    let old_instances = keep_instances(&places, "demo", "old", 300, 30);
    let new_picture = keep_picture(&places, "demo", "new", 200, 2);
    let new_instances = keep_instances(&places, "demo", "new", 300, 2);
    prune_runs(&tool, Run::Real, 600).unwrap();
    assert!(
        !old_picture.exists() && !old_instances.exists(),
        "the old picture and its instances go together"
    );
    assert!(new_picture.exists() && new_instances.exists());
}

#[test]
fn instances_with_no_picture_are_kept_by_their_own_last_use() {
    let scratch = Scratch::new("shot-store-instances-alone");
    let places = scratch.places();
    let tool = scratch.tool(0);
    std::fs::create_dir_all(pictures(&places, "demo").join(BOARD_PICTURES)).unwrap();
    let old = keep_instances(&places, "demo", "old", 400, 30);
    let new = keep_instances(&places, "demo", "new", 400, 2);
    prune_runs(&tool, Run::Real, 500).unwrap();
    assert!(!old.exists(), "the least recently used goes first");
    assert!(new.exists(), "a recent page's instances stay under the cap");
}

#[test]
fn a_loose_picture_file_goes_once_no_run_writes_to_the_folder() {
    let scratch = Scratch::new("shot-store-loose");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let boards = pictures(&places, "demo").join(BOARD_PICTURES);
    std::fs::create_dir_all(&boards).unwrap();
    let temporary = boards.join(".h.png.123");
    let orphan = boards.join("h2.json");
    for file in [&temporary, &orphan] {
        std::fs::write(file, "x").unwrap();
    }
    let run = writing(&places).unwrap();
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(
        temporary.exists() && orphan.exists(),
        "a run may be writing them"
    );
    drop(run);
    prune_runs(&tool, Run::Real, CAP_KIB).unwrap();
    assert!(!temporary.exists() && !orphan.exists());
}

#[test]
fn each_repo_caches_its_own_runs_and_pictures_under_the_cap() {
    let scratch = Scratch::new("shot-store-per-repo");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let small = keep_picture(&places, "small", "h", 300, 30);
    let big_old = keep_picture(&places, "big", "old", 400, 20);
    let big_new = keep_picture(&places, "big", "new", 400, 10);
    prune_runs(&tool, Run::Real, 500).unwrap();
    assert!(
        small.exists(),
        "its repo holds less than the cap, though it is the oldest"
    );
    assert!(!big_old.exists() && big_new.exists());
}

#[test]
fn the_cap_deletes_an_app_source_trees_shots_whole_when_they_are_used_least_recently() {
    let scratch = Scratch::new("shot-store-app-shots");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let old_tree = pictures(&places, "demo").join("mac/aaaa");
    let new_tree = pictures(&places, "demo").join("mac/bbbb");
    for (tree, hours) in [(&old_tree, 20), (&new_tree, 2)] {
        fill(&tree.join("mac.a.b@macos.png"), 400);
        std::fs::write(tree.join("mac.a.b@macos.json"), "{}").unwrap();
        crate::testing::age_all(tree, hours);
    }
    prune_runs(&tool, Run::Real, 500).unwrap();
    assert!(!old_tree.exists() && new_tree.exists());
}

#[test]
fn a_lane_can_read_its_cached_app_pictures_after_a_prune_between_steps() {
    let scratch = Scratch::new("shot-store-lane-pictures");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let picture_root = pictures(&places, "demo");
    let lane_tree = picture_root.join("mac/active");
    let other_tree = picture_root.join("mac/other");
    let shot = "mac.a.b@macos.png";
    for tree in [&lane_tree, &other_tree] {
        fill(&tree.join(shot), 400);
        std::fs::write(tree.join("mac.a.b@macos.json"), "{}").unwrap();
        CaptureMarks::of(tree)
            .write(&[shots::Prefix::parse("mac.a.").unwrap()], true)
            .unwrap();
        crate::testing::age_all(tree, 2);
    }
    let pictures = AppPictures::of(&places, "demo", Lane::Mac, "active");
    let query = CaptureQuery {
        prefixes: vec![shots::Prefix::parse("mac.a.").unwrap()],
        windows: true,
    };
    let marks = CaptureMarks::of(&lane_tree);
    pictures
        .while_held(Some(&query), |cached| {
            assert!(cached);

            prune_runs(&tool, Run::Real, 100).unwrap();

            assert!(lane_tree.join(shot).is_file());
            assert!(marks.cover(&query.prefixes, query.windows).unwrap());
            assert!(!other_tree.exists());
        })
        .unwrap();

    prune_runs(&tool, Run::Real, 100).unwrap();
    assert!(!lane_tree.exists());
}

#[test]
fn pictures_a_host_made_move_into_place_and_cached_ones_stay() {
    let scratch = Scratch::new("shot-store-merge");
    let places = scratch.places();
    let folder = pictures(&places, "demo");
    std::fs::create_dir_all(folder.join(BOARD_PICTURES)).unwrap();
    std::fs::write(folder.join("boards/kept.png"), "here").unwrap();
    let incoming = incoming(&folder);
    for (path, text) in [
        ("boards/kept.png", "there"),
        ("boards/new.png", "png"),
        ("boards/new.json", "{}"),
        ("mac/tree1/mac.a.b@macos.png", "shot"),
        ("mac/tree1/.taken/mac.a.@no-windows", ""),
        ("boards/.half.png.123", "half"),
    ] {
        let file = incoming.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    merge_pictures(&incoming, &folder).unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.join("boards/kept.png")).unwrap(),
        "here"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("boards/new.png")).unwrap(),
        "png"
    );
    assert!(folder.join("boards/new.json").is_file());
    assert!(folder.join("mac/tree1/mac.a.b@macos.png").is_file());
    assert!(
        folder.join("mac/tree1/.taken/mac.a.@no-windows").is_file(),
        "the host run's marks come too"
    );
    assert!(!folder.join("boards/.half.png.123").exists());
    assert!(!incoming.exists());
}

#[test]
fn a_run_that_a_waiting_branch_names_survives_a_prune_that_drops_an_older_unnamed_run() {
    let scratch = Scratch::new("shot-store-named");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let mut folders = Vec::new();
    // The named run is the oldest, so the cap would delete it first.
    for (n, hours) in [(0, 30), (1, 20), (2, 10)] {
        let cached = cache_run(&places, &key(&format!("mac.p{n}.")), 0, "ok\n", true);
        fill(&cached.report().join("img/a.png"), 400);
        age(&cached.folder.join(RUN_FILE), hours);
        folders.push(cached.folder);
    }
    let named: Named = [(
        folders[0].clone(),
        BTreeSet::from(["android-components-4".to_string()]),
    )]
    .into();
    prune_naming(&tool, Run::Real, 1000, &|_| Ok(named.clone())).unwrap();
    let left: Vec<bool> = folders.iter().map(|folder| folder.exists()).collect();
    assert_eq!(
        left,
        [true, false, true],
        "the unnamed run used 20h ago went"
    );
}

#[test]
fn runs_in_use_that_pass_the_cap_all_stay() {
    let scratch = Scratch::new("shot-store-named-over");
    let places = scratch.places();
    let tool = scratch.tool(0);
    let mut named = Named::new();
    let mut folders = Vec::new();
    for (n, hours) in [(0, 30), (1, 20)] {
        let cached = cache_run(&places, &key(&format!("mac.p{n}.")), 0, "ok\n", true);
        fill(&cached.report().join("img/a.png"), 400);
        age(&cached.folder.join(RUN_FILE), hours);
        named.insert(cached.folder.clone(), BTreeSet::from(["work".to_string()]));
        folders.push(cached.folder);
    }
    prune_naming(&tool, Run::Real, 500, &|_| Ok(named.clone())).unwrap();
    assert!(folders.iter().all(|folder| folder.exists()));
}

#[test]
fn the_runs_of_an_environment_are_those_of_every_commit_and_prefix_with_records() {
    let scratch = Scratch::new("shot-store-all-runs");
    let places = scratch.places();
    let first = key("mac.welcome.");
    let second = Key {
        commit: "b".repeat(40),
        ..key("mac.")
    };
    let other_environment = Key {
        environment: "other".to_string(),
        ..key("mac.")
    };
    cache_run(&places, &first, 0, "", true);
    cache_run(&places, &second, 0, "", true);
    cache_run(&places, &key("mac.settings."), 0, "", false);
    cache_run(&places, &other_environment, 0, "", true);
    let found = runs_in_environment(
        &places,
        &first.repo,
        first.lane,
        &first.environment,
        &first.versions,
    )
    .unwrap();
    let mut found: Vec<(String, String)> = found
        .into_iter()
        .map(|cached| (cached.key.commit[..1].to_string(), cached.key.prefix))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            ("a".to_string(), "mac.welcome.".to_string()),
            ("b".to_string(), "mac.".to_string())
        ]
    );
}
