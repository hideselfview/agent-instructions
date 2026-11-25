use super::*;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, db, design, item, make_store, read, stamp};
use crate::lists::store::load_all;
use crate::testing::Scratch;
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

/// The limit on a wait for the feed: a guard against a stuck stream, not the
/// pass condition.
const HANG_GUARD: Duration = Duration::from_secs(300);

fn loaded(project: &str, lists: Result<crate::lists::Lists, String>) -> Loaded {
    Loaded {
        project: project.to_string(),
        lists,
    }
}

fn no_claims(_: &str) -> ClaimFacts {
    panic!("no item runs")
}

fn no_reviews(_: &str, _: crate::lists::Id) -> RulesReview {
    panic!("no design change waits on approval")
}

fn error_of(snapshot: Snapshot) -> String {
    let composed = compose(&snapshot, NOON, &no_claims, &no_reviews);
    assert_eq!(composed.len(), 1);
    let view = serde_json::to_value(&composed[0]).unwrap();
    assert_eq!(view["items"], json!([]));
    view["error"].as_str().expect("an error").to_string()
}

#[test]
fn a_store_that_fails_its_check_is_served_as_the_error() {
    let fault = error_of(Ok(vec![loaded("demo", Err("ids repeat".to_string()))]));
    assert_eq!(fault, "ids repeat");
}

#[test]
fn a_trail_that_fails_is_served_as_the_error() {
    let db = db();
    // A decision record with no decision row: the lists read no such decision.
    let missing =
        crate::lists::record::open(&db, crate::lists::Kind::Decision, &stamp(NOON)).unwrap();
    let work = item(&db, vec![]);
    crate::lists::trail::set_origin(&db, crate::lists::trail::Entry::Item, work, missing).unwrap();
    let fault = error_of(Ok(vec![loaded("demo", Ok(read(&db)))]));
    assert_eq!(
        fault,
        format!("the trail of {work} names {missing}, which is not a record")
    );
}

#[test]
fn no_project_is_an_error_and_a_failing_store_is_only_its_own_projects() {
    assert!(error_of(Ok(vec![])).contains("no lists"));
    let several = compose(
        &Ok(vec![
            loaded("a", Ok(read(&db()))),
            loaded("b", Err("ids repeat".into())),
        ]),
        NOON,
        &no_claims,
        &no_reviews,
    );
    let views: Vec<serde_json::Value> = several
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect();
    assert_eq!(views[0]["error"], json!(null));
    assert_eq!(
        (&views[1]["project"], &views[1]["error"]),
        (&json!("b"), &json!("ids repeat"))
    );
    assert_eq!(error_of(Err("listing failed".into())), "listing failed");
}

/// An open design change that waits on the person's approval carries where
/// its rules review stands, and is under Waiting on you only once that
/// review takes its branch (decision #362); an approved one carries nothing.
#[test]
fn a_design_change_to_approve_carries_its_rules_review() {
    let db = db();
    let waiting = design(&db);
    let approved = design(&db);
    crate::lists::design::approve(&db, approved, &stamp(NOON)).unwrap();
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(read(&db)))]);
    let view_with = |review: RulesReview| {
        let composed = compose(&snapshot, NOON, &no_claims, &|project, id| {
            assert_eq!((project, id), ("demo", waiting));
            review.clone()
        });
        serde_json::to_value(&composed[0]).unwrap()
    };
    let view = view_with(RulesReview::InReview);
    let rules_review = |view: &serde_json::Value, id: crate::lists::Id| {
        view["designs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == json!(id.0))
            .unwrap()["rules_review"]
            .clone()
    };
    assert_eq!(rules_review(&view, waiting), json!("in-review"));
    assert_eq!(rules_review(&view, approved), json!(null));
    assert_eq!(view["board"]["waiting"], json!(0));
    let view = view_with(RulesReview::Read);
    assert_eq!(rules_review(&view, waiting), json!("read"));
    assert_eq!(view["board"]["waiting"], json!(1));
}

#[test]
fn a_running_item_carries_its_claim_facts_and_nothing_else_does() {
    let db = db();
    let running = item(&db, vec![]);
    let idle = item(&db, vec![]);
    let worktree = "/d/bae-worktrees/side-2";
    crate::lists::running::start(
        &db,
        &[running],
        &[Running {
            worktree: worktree.into(),
            branch: "import-tags".into(),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(read(&db)))]);
    assert_eq!(running_worktrees(&snapshot), vec![worktree.to_string()]);
    let facts = ClaimFacts {
        agent: Some("import-tags".into()),
        note: Some("tests next".into()),
        note_state: Some("held".into()),
        waiting_to_land: 2,
    };
    let composed = compose(
        &snapshot,
        NOON,
        &|asked| {
            assert_eq!(asked, worktree);
            facts.clone()
        },
        &no_reviews,
    );
    assert_eq!(composed.len(), 1);
    let view = serde_json::to_value(&composed[0]).unwrap();
    let find = |id: crate::lists::Id| {
        view["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == json!(id.0))
            .unwrap()
            .clone()
    };
    let r = &find(running)["running"][0];
    assert_eq!(
        (
            &r["agent"],
            &r["note"],
            &r["note_state"],
            &r["waiting_to_land"]
        ),
        (
            &json!("import-tags"),
            &json!("tests next"),
            &json!("held"),
            &json!(2)
        )
    );
    assert_eq!(find(idle)["running"], json!([]));
    assert_eq!(view["works"].as_array().unwrap().len(), 1);
}

#[test]
fn a_write_to_the_store_moves_the_version_the_feed_reports() {
    let scratch = Scratch::new("daemon-lists");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let feed = Feed::start(places);
    let (before, snapshot) = feed.current();
    assert_eq!(before, 1);
    assert_eq!(
        snapshot.as_ref().as_ref().unwrap()[0]
            .lists
            .as_ref()
            .unwrap()
            .version,
        3
    );
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move |_| {
        let _ = sent.send(());
    }));

    // A write lands in builds.sqlite-wal, not builds.sqlite, and still wakes the feed.
    store.write(&stamp(NOON), "touch", |_| Ok(())).unwrap();
    moved
        .recv_timeout(HANG_GUARD)
        .expect("the write reached the feed");
    let (after, snapshot) = feed.current();
    assert_eq!(after, before + 1);
    assert_eq!(
        snapshot.as_ref().as_ref().unwrap()[0]
            .lists
            .as_ref()
            .unwrap()
            .version,
        4
    );

    // A store that fails is read and served, not skipped. The feed's held
    // connection keeps the write-ahead log, whose pages a reader would take
    // over the damaged file's, so the damage removes it too, unless the feed
    // closed its connections on the damaged file first and SQLite removed it.
    std::fs::write(
        store.database(),
        "not a database, and long enough to have a header of sorts",
    )
    .unwrap();
    for log in ["-wal", "-shm"] {
        let mut path = store.database().into_os_string();
        path.push(log);
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => panic!("{error}"),
            _ => {}
        }
    }
    // The feed tells after its integrity checks too, which may read the file
    // while the damage is written, so a tell may come before the read that
    // finds the damage.
    let fault = loop {
        moved
            .recv_timeout(HANG_GUARD)
            .expect("the damage reached the feed");
        let (_, snapshot) = feed.current();
        if let Err(fault) = &snapshot.as_ref().as_ref().unwrap()[0].lists {
            break fault.clone();
        }
    };
    assert!(fault.contains("not a database"), "{fault}");
}

#[test]
fn a_store_edited_outside_builds_reaches_the_feeds_integrity_findings() {
    let scratch = Scratch::new("daemon-integrity");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let feed = Feed::start(places);
    assert_eq!(*feed.integrity(), Vec::<String>::new());
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move |_| {
        let _ = sent.send(());
    }));

    // Written with the foreign keys off, as the sqlite3 shell does.
    let db = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
    db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    crate::lists::item::plant(
        &db,
        "item_wait_item",
        &[("item_id", 41_i64.into()), ("on_item", 42_i64.into())],
    )
    .unwrap();
    drop(db);
    let found = loop {
        moved
            .recv_timeout(HANG_GUARD)
            .expect("the edit reached the feed");
        let found = feed.integrity();
        if !found.is_empty() {
            break found;
        }
    };
    assert!(
        found
            .iter()
            .any(|f| f.contains("item_wait_item") && f.contains("points at nothing")),
        "{found:?}"
    );
}

/// The feed's own reads of a store (the loads, the integrity checks, the
/// hook runs' figures) commit nothing and leave its database as it was, so
/// the feed's next look finds no database changed; a write is found.
#[test]
fn the_feeds_own_reads_change_no_database_and_a_write_does() {
    let scratch = Scratch::new("daemon-quiet");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let mut watches = HashMap::new();
    assert_eq!(
        scan_databases(&places, &mut watches).unwrap().moved,
        vec![store.database()]
    );
    let loaded = load_all(&places);
    let paths = database_paths(&places).unwrap();
    assert!(!reload(&places, &paths, &loaded).1);
    assert_eq!(store.integrity(), Vec::<String>::new());
    crate::hook_stats::dashboard_json_at(&places, NOON);
    assert_eq!(
        scan_databases(&places, &mut watches).unwrap().moved,
        Vec::<PathBuf>::new()
    );
    store.write(&stamp(NOON), "touch", |_| Ok(())).unwrap();
    assert_eq!(
        scan_databases(&places, &mut watches).unwrap().moved,
        vec![store.database()]
    );
}

#[test]
fn an_open_design_change_is_found_by_id_with_its_view_and_a_landed_or_missing_one_is_not() {
    let db = db();
    let id = design(&db);
    let snapshot: Snapshot = Ok(vec![loaded("demo", Ok(read(&db)))]);
    let (design, view) = open_design(&snapshot, "demo", id.0).expect("open");
    let view = view.unwrap();
    assert_eq!(
        (design.id, design.worktree.as_str(), view.id),
        (id, "/d/bae-worktrees/side-2", id)
    );
    assert!(open_design(&snapshot, "demo", id.0 + 1).is_none());
    assert!(
        open_design(&Ok(vec![]), "demo", id.0).is_none()
            && open_design(&snapshot, "other", id.0).is_none()
    );
}

#[test]
fn a_recorded_hook_run_reaches_the_feeds_hooks_json_and_moves_no_version() {
    use crate::hook_stats::{CheckRecord, DATABASE, Outcome, Run, record};
    let scratch = Scratch::new("daemon-hooks");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    // The window ends an hour after the run below, whatever the date is.
    let feed = Feed::start_with_clock(places, Box::new(|| NOON + 3600));
    let (version, _) = feed.current();
    let before = feed.hooks();
    assert!(before.contains("\"repos\":[]"), "{before}");
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move |_| {
        let _ = sent.send(());
    }));

    let run = Run {
        at: NOON,
        repo: "demo".into(),
        worktree: "/d/demo".into(),
        branch: Some("main".into()),
        base: None,
        passed: true,
        plan_ms: 1,
        total_ms: 5,
        files: vec![],
        checks: vec![CheckRecord {
            name: "fmt".into(),
            kind: crate::checks::Kind::Check,
            outcome: Outcome::Ran {
                result: crate::step_failure::RunResult::Passed,
                handed: (0, 0),
                ms: Some(3),
                started: None,
                cargo: None,
            },
        }],
    };
    record(&store.dir.join(DATABASE), &run).unwrap();
    moved
        .recv_timeout(HANG_GUARD)
        .expect("the run reached the feed");
    let after = feed.hooks();
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    assert!(
        after.contains("\"runs\":1") && after.contains("\"fmt\""),
        "{after}"
    );
    assert_eq!(feed.current().0, version);
}

#[test]
fn a_recorded_app_build_run_reaches_the_feeds_hooks_and_health() {
    use crate::app_timing::{DATABASE, RunRecord, StepKind, StepRecord, StepResult, record};
    let scratch = Scratch::new("daemon-app-timing");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let feed = Feed::start_with_clock(places, Box::new(|| NOON));
    let before_progress = feed.health_data().1;
    let before = feed.hooks();
    assert!(!before.contains("\"median_wait_ms\""), "{before}");
    let (sent, moved) = mpsc::channel();
    feed.listen(Box::new(move |_| {
        let _ = sent.send(());
    }));
    let at = NOON - 60;
    let run = RunRecord {
        started_at: at,
        repo: "bae".into(),
        lane: crate::lane::Lane::Mac,
        checkout: "side-4".into(),
        built_ref: "branch".into(),
        requested_commit: None,
        built_commit: None,
        who: None,
        wait_ms: 1234,
        outcome: crate::run_end::RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            at_merge_base: None,
            name: "macos".into(),
            kind: StepKind::Step,
            started_at: at,
            wall_ms: 5000,
            cpu_ms: 4000,
            result: StepResult::Passed,
            shots: None,
        }],
    };
    record(&store.dir.join(DATABASE), &run).unwrap();
    moved
        .recv_timeout(HANG_GUARD)
        .expect("the run reached the feed");
    assert!(feed.health_data().1.as_ref() != before_progress.as_ref());
    let after = feed.hooks();
    assert!(
        after.contains("\"app_builds\"")
            && after.contains("\"median_wait_ms\":1234")
            && after.contains("\"macos\""),
        "{after}"
    );
}

#[test]
fn a_hook_runs_write_reads_no_store_again_and_the_served_snapshot_is_the_same() {
    use crate::hook_stats::{DATABASE, Run, record};
    let scratch = Scratch::new("daemon-reload");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let first = load_all(&places);
    // Nothing moved: the same stores come back, and nothing differs.
    let (same, changed) = reload(&places, &database_paths(&places).unwrap(), &first);
    assert!(!changed && same == first);
    record(
        &store.dir.join(DATABASE),
        &Run {
            at: NOON,
            repo: "demo".into(),
            worktree: "/d/demo".into(),
            branch: None,
            base: None,
            passed: true,
            plan_ms: 1,
            total_ms: 2,
            files: vec![],
            checks: vec![],
        },
    )
    .unwrap();
    let (after_hook, changed) = reload(&places, &database_paths(&places).unwrap(), &first);
    assert!(
        !changed && after_hook == first,
        "a hook run moves no lists version"
    );
    // A lists write does.
    store.write(&stamp(NOON), "touch", |_| Ok(())).unwrap();
    let (after_write, changed) = reload(&places, &database_paths(&places).unwrap(), &first);
    assert!(changed);
    assert_eq!(after_write.unwrap()[0].lists.as_ref().unwrap().version, 4);
}

#[test]
fn a_worktrees_running_items_come_in_list_order_with_their_numbers_and_titles() {
    let db = db();
    let first = item(&db, vec![]);
    let second = item(&db, vec![]);
    let elsewhere = item(&db, vec![]);
    let running = |worktree: &str| Running {
        worktree: worktree.into(),
        branch: "b".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
        tier: crate::model_tier::Tier::Implementation,
    };
    let here = "/d/bae-worktrees/side-2";
    crate::lists::running::start(&db, &[first], &[running(here)]).unwrap();
    crate::lists::running::start(&db, &[second], &[running(here)]).unwrap();
    crate::lists::running::start(&db, &[elsewhere], &[running("/d/bae-worktrees/side-3")]).unwrap();
    let lists = read(&db);
    let expected: Vec<crate::daemon::cards::RunningItem> = [first, second]
        .iter()
        .map(|id| crate::daemon::cards::RunningItem {
            id: id.0,
            title: lists.item(*id).unwrap().title.clone(),
        })
        .collect();
    assert_eq!(running_items(&lists, here), expected);
    assert!(running_items(&lists, "/d/bae-worktrees/side-9").is_empty());
}

#[test]
fn a_running_items_waits_come_in_the_order_it_lists_them_and_a_lost_record_fails() {
    use crate::lists::Wait;
    let db = db();
    let question = crate::lists::fixtures::question(&db);
    let other = item(&db, vec![]);
    let waiting = item(&db, vec![Wait::Item(other), Wait::Question(question)]);
    let here = "/d/bae-worktrees/side-2";
    crate::lists::running::start(
        &db,
        &[waiting],
        &[Running {
            worktree: here.into(),
            branch: "b".into(),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    let lists = read(&db);
    assert_eq!(
        running_waits(&lists, here).unwrap(),
        [
            crate::daemon::cards::ItemWait::Answer {
                id: question.0,
                title: lists.question(question).unwrap().title.clone(),
            },
            crate::daemon::cards::ItemWait::Item {
                id: other.0,
                title: lists.item(other).unwrap().title.clone(),
                state: "ready",
            },
        ]
    );
    assert!(
        running_waits(&lists, "/d/bae-worktrees/side-9")
            .unwrap()
            .is_empty()
    );
    let mut lost = lists.clone();
    lost.questions.clear();
    assert_eq!(
        running_waits(&lost, here),
        Err(format!(
            "{waiting} waits on {question}, which the lists do not hold"
        ))
    );
}

#[test]
fn a_running_items_condition_is_one_of_its_waits() {
    use crate::lists::Wait;
    let db = db();
    let waiting = item(
        &db,
        vec![Wait::condition(
            "builds #122, #121 and #125 land",
            crate::lists::Ender::Coordinator,
        )],
    );
    let here = "/d/bae-worktrees/side-2";
    crate::lists::running::start(
        &db,
        &[waiting],
        &[Running {
            worktree: here.into(),
            branch: "b".into(),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    assert_eq!(
        running_waits(&read(&db), here).unwrap(),
        [crate::daemon::cards::ItemWait::Condition {
            text: "builds #122, #121 and #125 land".into(),
            until: crate::lists::Ender::Coordinator
        }]
    );
}

fn running_on_design(db: &rusqlite::Connection, change: crate::lists::Id) -> &'static str {
    let waiting = item(db, vec![crate::lists::Wait::Design(change)]);
    let here = "/d/bae-worktrees/side-2";
    crate::lists::running::start(
        db,
        &[waiting],
        &[Running {
            worktree: here.into(),
            branch: "b".into(),
            brief: "plans/briefs/x.md".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    here
}

#[test]
fn a_running_item_waits_on_the_persons_review_of_a_design_change_until_they_approve_it() {
    let db = db();
    let change = design(&db);
    let here = running_on_design(&db, change);
    let lists = read(&db);
    let title = lists.design(change).unwrap().title.clone();
    assert_eq!(
        running_waits(&lists, here).unwrap(),
        [crate::daemon::cards::ItemWait::Review {
            id: change.0,
            title
        }]
    );
}

#[test]
fn a_running_item_waits_on_an_approved_design_change_to_land() {
    let db = db();
    let change = design(&db);
    let here = running_on_design(&db, change);
    crate::lists::design::approve(&db, change, &stamp(NOON)).unwrap();
    let lists = read(&db);
    let title = lists.design(change).unwrap().title.clone();
    assert_eq!(
        running_waits(&lists, here).unwrap(),
        [crate::daemon::cards::ItemWait::Landing {
            id: change.0,
            title
        }]
    );
}

#[test]
fn a_design_change_id_two_projects_both_hold_opens_by_its_project() {
    let with_design = |name: &str| {
        let db = db();
        design(&db);
        let mut lists = read(&db);
        lists.project = name.to_string();
        loaded(name, Ok(lists))
    };
    let both: Snapshot = Ok(vec![with_design("bae"), with_design("builds")]);
    let id = lists_id_of_first_design(&both);
    assert!(open_design(&both, "bae", id).is_some());
    assert!(open_design(&both, "builds", id).is_some());
    assert!(open_design(&both, "coven", id).is_none());
    let one: Snapshot = Ok(vec![with_design("bae"), loaded("builds", Ok(read(&db())))]);
    assert!(open_design(&one, "builds", id).is_none());
}

fn lists_id_of_first_design(snapshot: &Snapshot) -> u32 {
    snapshot.as_deref().unwrap()[0]
        .lists
        .as_ref()
        .unwrap()
        .designs[0]
        .id
        .0
}

#[test]
fn the_project_design_repo_reports_a_failed_snapshot() {
    let snapshot = Err("could not list the project stores".to_string());
    let Some(Err(error)) = project_design_repo(&snapshot, "demo") else {
        panic!("the project lookup must return the snapshot error");
    };
    assert_eq!(error.text(), "could not list the project stores");
}

#[test]
fn the_design_repo_is_the_one_repo_with_design_places_and_any_design_places_give_a_designs_page() {
    let mut lists = read(&db());
    lists.design_places.clear();
    assert_eq!(design_repo(&lists).unwrap(), None);
    lists.design_places.insert("coven".into(), Vec::new());
    lists
        .design_places
        .insert("bae".into(), vec!["design/mocks/*.html".into()]);
    assert_eq!(design_repo(&lists).unwrap(), Some("bae"));
    let one = compose(
        &Ok(vec![loaded("a", Ok(lists.clone()))]),
        NOON,
        &no_claims,
        &no_reviews,
    );
    assert_eq!(
        serde_json::to_value(&one[0]).unwrap()["has_designs"],
        json!(true)
    );
    lists
        .design_places
        .insert("coven".into(), vec!["design/*".into()]);
    let error = design_repo(&lists).unwrap_err().text();
    assert!(error.contains("in 2 repos (bae, coven)"), "{error}");
    // The page says why it draws no boards.
    let several = compose(
        &Ok(vec![loaded("a", Ok(lists.clone()))]),
        NOON,
        &no_claims,
        &no_reviews,
    );
    assert_eq!(
        serde_json::to_value(&several[0]).unwrap()["has_designs"],
        json!(true)
    );
    lists.design_places.clear();
    let none = compose(
        &Ok(vec![loaded("a", Ok(lists))]),
        NOON,
        &no_claims,
        &no_reviews,
    );
    assert_eq!(
        serde_json::to_value(&none[0]).unwrap()["has_designs"],
        json!(false)
    );
}

#[test]
fn a_projects_lists_json_keeps_each_field_once_with_its_own_shape() {
    let db = db();
    let question = crate::lists::fixtures::question(&db);
    let change = design(&db);
    let lists = read(&db);
    let composed = compose(
        &Ok(vec![loaded("demo", Ok(lists))]),
        NOON,
        &no_claims,
        &|_, _| RulesReview::Read,
    );
    // The page reads the text a client gets: a key that appears twice in it
    // keeps only its last value there.
    let text = serde_json::to_string(&Served {
        projects: &composed,
    })
    .unwrap();
    for key in ["stack", "designs", "board"] {
        let count = text.matches(&format!("\"{key}\":")).count();
        assert_eq!(count, 1, "{key} appears {count} times in {text}");
    }
    let project = &serde_json::from_str::<serde_json::Value>(&text).unwrap()["projects"][0];
    assert_eq!(project["stack"][0]["id"], json!(question.0));
    assert_eq!(project["designs"][0]["id"], json!(change.0));
    assert_eq!(project["board"]["waiting"], json!(2));
}
