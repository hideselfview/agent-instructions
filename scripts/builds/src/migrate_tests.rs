use super::*;
use crate::testing::Scratch;
use std::os::unix::fs::MetadataExt;

/// A volume with an old-layout android lane for repo `demo`, its lock file and
/// an empty queue.
fn old_volume(scratch: &Scratch) -> Places {
    let places = scratch.places();
    let dir = places.root.join("demo");
    std::fs::create_dir_all(dir.join("app-android.queue")).unwrap();
    std::fs::write(dir.join("app-android-checkout"), "side-3\n").unwrap();
    std::fs::write(dir.join("app-android.lock"), "").unwrap();
    places
}

/// Waits until nothing holds the lock on <path>. A process another test forks
/// meanwhile has a copy of every open descriptor until it execs, and with it
/// the lock, for a moment after the test closed its own.
fn until_free(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while is_held(path).unwrap() {
        assert!(std::time::Instant::now() < deadline, "{} stayed held", path.display());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_old_files_become_the_new_ones_with_the_lock_keeping_its_inode() {
    let scratch = Scratch::new("migrate");
    let places = old_volume(&scratch);
    let dir = places.root.join("demo");
    let inode = std::fs::metadata(dir.join("app-android.lock")).unwrap().ino();
    migrate_app_files(&places).unwrap();
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    let lock = places.app_lock_file("demo", Lane::Android, "side-3");
    assert_eq!(std::fs::metadata(lock).unwrap().ino(), inode, "the lock a holder has open is the new lock");
    // Again: nothing left to move.
    migrate_app_files(&places).unwrap();
}

#[test]
fn builds_from_before_keep_their_lock_and_queue_and_the_old_names_go_when_they_end() {
    let scratch = Scratch::new("migrate-busy");
    let places = old_volume(&scratch);
    let dir = places.root.join("demo");
    // An old build holds the old lock.
    let held = crate::queue::open_with(&dir.join("app-android.lock"), libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    let new_lock = places.app_lock_file("demo", Lane::Android, "side-3");
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    assert!(dir.join("app-android-checkout").is_file() && dir.join("app-android.lock").is_file(), "the old names stay");
    assert!(crate::queue::try_lock(&new_lock).unwrap().is_none(), "the new path is the lock the old build holds");
    unsafe { libc::close(held) };
    until_free(&dir.join("app-android.lock"));

    // A ticket of an old waiter keeps them too.
    let ticket = crate::queue::open_with(&dir.join("app-android.queue/123-1"), libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    assert!(dir.join("app-android.lock").is_file());
    unsafe { libc::close(ticket) };
    until_free(&dir.join("app-android.queue/123-1"));

    migrate_app_files(&places).unwrap();
    assert!(!dir.join("app-android-checkout").exists() && !dir.join("app-android.lock").exists());
    assert_eq!(places.app_checkouts_of("demo", Lane::Android).unwrap(), vec!["side-3"]);
    let fd = crate::queue::try_lock(&new_lock).unwrap().expect("free once the old build ended");
    unsafe { libc::close(fd) };
}

#[test]
fn a_lane_never_locked_gets_its_old_lock_made_before_it_is_linked() {
    let scratch = Scratch::new("migrate-fresh");
    let places = scratch.places();
    let dir = places.root.join("demo");
    std::fs::create_dir_all(dir.join("app.queue")).unwrap();
    std::fs::write(dir.join("app-checkout"), "side-1\n").unwrap();
    // A waiter of an older `builds` holds a ticket, so the old name stays and
    // must be the file the new path is.
    let ticket = crate::queue::open_with(&dir.join("app.queue/9-1"), libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    migrate_app_files(&places).unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("app.lock")).unwrap().ino(),
        std::fs::metadata(places.app_lock_file("demo", Lane::Mac, "side-1")).unwrap().ino()
    );
    unsafe { libc::close(ticket) };
}

/// A store of project `demo` with design change 1 open and design change 2
/// landed, then rewritten in the shape before review pages: boards with a
/// `render`, no worktree, no review.
fn old_design_store(places: &Places) -> crate::lists::store::Store {
    use crate::lists::draft::close;
    use crate::lists::fixtures::{NOON, design, lists, stamp};
    use crate::lists::{Board, How};
    let mut state = lists();
    for _ in 0..2 {
        let id = design(&mut state);
        let change = state.design_mut(id).unwrap();
        change.title = format!("Cover grid {}", id.0);
        change.boards.insert("design/mocks/mac.library.grid.dc.html".into(), Board { link: "https://canvas.example/#grid".into() });
        change.boards.insert("design/mocks/mac.library.empty.dc.html".into(), Board { link: "https://canvas.example/#empty".into() });
        change.places.insert("design/mocks/mac.library.grid.dc.html".into(), crate::lists::Resolution::Matches("x".into()));
        change.places.insert("design/mocks/mac.library.empty.dc.html".into(), crate::lists::Resolution::Matches("x".into()));
    }
    let landed = state.designs[1].id;
    close(&mut state, landed, How::Landed(vec!["abc".into()]), &stamp(NOON)).unwrap();
    let store = crate::lists::store::Store::named(places, "demo").unwrap();
    store.create(state, &stamp(NOON), "lists init demo").unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(store.state_path()).unwrap()).unwrap();
    let old = |design: &mut serde_json::Value| {
        let design = design.as_object_mut().unwrap();
        design.remove("worktree");
        design.remove("review");
        for board in design["boards"].as_object_mut().unwrap().values_mut() {
            board["render"] = serde_json::json!("/tmp/render.png");
        }
    };
    old(&mut raw["designs"][0]);
    old(&mut raw["closed"][0]["record"]["design"]);
    std::fs::write(store.state_path(), serde_json::to_string_pretty(&raw).unwrap()).unwrap();
    assert!(store.load().is_err(), "the old shape is not the current one");
    store
}

#[test]
fn open_and_landed_design_changes_gain_a_worktree_and_a_review_page_and_lose_their_images() {
    let scratch = Scratch::new("migrate-designs");
    let places = scratch.places();
    let store = old_design_store(&places);
    migrate_lists(&places, 1_000).unwrap();

    let state = store.load().unwrap();
    let main_checkout = places.home.join("dev/bae").display().to_string();
    let open = &state.designs[0];
    assert_eq!(open.worktree, main_checkout);
    assert_eq!(open.review.len(), 1);
    assert_eq!(open.review[0].heading, open.title);
    let page: Vec<(&str, &str, &str)> =
        open.review[0].boards.iter().map(|b| (b.board.as_str(), b.caption.as_str(), b.theme.word())).collect();
    assert_eq!(
        page,
        [
            ("design/mocks/mac.library.empty.dc.html", "mac.library.empty", "dark"),
            ("design/mocks/mac.library.grid.dc.html", "mac.library.grid", "dark"),
        ]
    );
    assert_eq!(open.boards["design/mocks/mac.library.grid.dc.html"].link, "https://canvas.example/#grid");
    let crate::lists::Record::Design(landed) = &state.closed[0].record else { panic!("a design change") };
    assert_eq!((landed.worktree.as_str(), landed.review.len()), (main_checkout.as_str(), 1));
    let last = state.log.last().unwrap();
    assert_eq!((last.version, last.at, last.by.as_str()), (state.version, 1_000, "builds install"));
}

#[test]
fn a_store_already_in_the_current_shape_is_left_as_it_is() {
    let scratch = Scratch::new("migrate-designs-again");
    let places = scratch.places();
    let store = old_design_store(&places);
    migrate_lists(&places, 1_000).unwrap();
    let once = std::fs::read(store.state_path()).unwrap();
    migrate_lists(&places, 2_000).unwrap();
    assert_eq!(std::fs::read(store.state_path()).unwrap(), once, "no write, no new version");
}

#[test]
fn a_state_that_cannot_be_read_as_the_current_shape_after_the_move_is_not_written() {
    let scratch = Scratch::new("migrate-designs-bad");
    let places = scratch.places();
    let store = old_design_store(&places);
    let mut raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(store.state_path()).unwrap()).unwrap();
    raw["designs"][0]["surprise"] = serde_json::json!(1);
    std::fs::write(store.state_path(), serde_json::to_string_pretty(&raw).unwrap()).unwrap();
    let before = std::fs::read(store.state_path()).unwrap();
    let refused = migrate_lists(&places, 1_000).unwrap_err().text();
    assert!(refused.contains("not valid lists") && refused.contains("surprise"), "{refused}");
    assert_eq!(std::fs::read(store.state_path()).unwrap(), before);
}

/// A store of project `demo` with item 1 running and item 2 landed, rewritten
/// in the shape before `running` was a list: an object on the open item, null
/// on the closed one.
fn old_running_store(places: &Places) -> crate::lists::store::Store {
    use crate::lists::draft::close;
    use crate::lists::fixtures::{NOON, item, lists, stamp};
    use crate::lists::{How, Running};
    let mut state = lists();
    let running = item(&mut state, vec![]);
    let landed = item(&mut state, vec![]);
    state.item_mut(running).unwrap().running = vec![Running {
        worktree: "/d/bae-worktrees/side-2".into(),
        branch: "import-tags".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
    }];
    close(&mut state, landed, How::Landed(vec!["abc".into()]), &stamp(NOON)).unwrap();
    let store = crate::lists::store::Store::named(places, "demo").unwrap();
    store.create(state, &stamp(NOON), "lists init demo").unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(store.state_path()).unwrap()).unwrap();
    let entry = raw["items"][0]["running"][0].clone();
    raw["items"][0]["running"] = entry;
    raw["closed"][0]["record"]["item"]["running"] = serde_json::Value::Null;
    std::fs::write(store.state_path(), serde_json::to_string_pretty(&raw).unwrap()).unwrap();
    assert!(store.load().is_err(), "the old shape is not the current one");
    store
}

#[test]
fn a_running_object_becomes_a_one_entry_list_and_null_an_empty_one() {
    let scratch = Scratch::new("migrate-running");
    let places = scratch.places();
    let store = old_running_store(&places);
    migrate_lists(&places, 1_000).unwrap();
    let state = store.load().unwrap();
    let running = &state.items[0].running;
    assert_eq!(
        running.iter().map(|r| (r.worktree.as_str(), r.branch.as_str())).collect::<Vec<_>>(),
        [("/d/bae-worktrees/side-2", "import-tags")]
    );
    let crate::lists::Record::Item(closed) = &state.closed[0].record else { panic!("an item") };
    assert!(closed.running.is_empty());
    let last = state.log.last().unwrap();
    assert_eq!((last.version, last.at, last.by.as_str()), (state.version, 1_000, "builds install"));
}

#[test]
fn a_store_whose_running_is_already_a_list_is_left_as_it_is() {
    let scratch = Scratch::new("migrate-running-again");
    let places = scratch.places();
    let store = old_running_store(&places);
    migrate_lists(&places, 1_000).unwrap();
    let once = std::fs::read(store.state_path()).unwrap();
    migrate_lists(&places, 2_000).unwrap();
    assert_eq!(std::fs::read(store.state_path()).unwrap(), once, "no write, no new version");
}
