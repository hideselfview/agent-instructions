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
fn cache_run(places: &Places, key: &Key, exit: i64, findings: &str, records: bool) -> Cached {
    let staging = stage(places, key).unwrap();
    if records {
        std::fs::create_dir_all(staging.report()).unwrap();
        std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    }
    staging.finish(exit, findings).unwrap()
}

#[test]
fn a_run_is_cached_on_the_build_volume_under_its_repo_commit_lane_prefix_and_version() {
    let scratch = Scratch::new("shot-store-place");
    let places = scratch.places();
    let cached = cache_run(&places, &key("mac.welcome."), 1, "FAIL x\n", true);
    assert_eq!(
        cached.folder,
        places.root.join(format!(
            "shots/demo/runs/{}/mac/mac.welcome./0123456789abcdef",
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
        version: version.to_string(),
        ..key(prefix)
    };
    let find = |prefix: &str| {
        before_run(&places, "demo", &base, Lane::Mac, prefix, "v1")
            .unwrap()
            .map(|before| before.key.prefix)
    };
    assert_eq!(find("mac.welcome."), None);
    cache_run(&places, &key("mac.welcome.", "v2"), 1, "x\n", true);
    cache_run(&places, &key("mac.settings.", "v1"), 1, "x\n", true);
    cache_run(&places, &key("mac.welcome.a.", "v1"), 1, "x\n", true);
    assert_eq!(
        find("mac.welcome."),
        None,
        "another version's run, another screen's and a narrower prefix's do not stand in"
    );
    let all = cache_run(&places, &key("mac.", "v1"), 1, "x\n", true);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    let before = before_run(&places, "demo", &base, Lane::Mac, "mac.welcome.", "v1")
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
        before_run(&places, "demo", &base, Lane::Android, "android.", "v1").unwrap(),
        None
    );
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
        "mac.component.checkbox.",
        "0123456789abcdef",
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
fn a_prefix_is_cached_when_a_taken_mark_starts_it_and_took_the_window_shots_it_asks_for() {
    let scratch = Scratch::new("shot-store-taken");
    let places = scratch.places();
    let taken = pictures(&places, "demo").join("mac/tree1").join(TAKEN);
    std::fs::create_dir_all(&taken).unwrap();
    let kept = |prefix: &str, windows: bool| {
        shots_cached(&places, "demo", Lane::Mac, "tree1", prefix, windows).unwrap()
    };
    assert!(!kept("mac.a.", false));
    std::fs::write(taken.join("mac.a.@no-windows"), "").unwrap();
    assert!(kept("mac.a.b.", false));
    assert!(!kept("mac.a.", true), "that run took no window's shots");
    assert!(
        !kept("mac.", false),
        "a narrower mark does not cover a wider prefix"
    );
    std::fs::write(taken.join("mac.@windows"), "").unwrap();
    assert!(kept("mac.", true) && kept("mac.b.", false));
    assert!(!shots_cached(&places, "demo", Lane::Mac, "tree2", "mac.a.", false).unwrap());
    let other = pictures(&places, "demo").join("mac/tree3").join(TAKEN);
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("mac.c."), "").unwrap();
    let error = shots_cached(&places, "demo", Lane::Mac, "tree3", "mac.c.", false)
        .expect_err("a mark the shots program never writes fails")
        .text();
    assert!(error.contains("mac.c."), "{error}");
}
