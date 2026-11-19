use super::*;
use crate::db::Sql;
use crate::lists::fixtures::{NOON, item_opened, make_store, stamp};
use crate::lists::record::close;
use crate::lists::{How, Running};
use crate::testing::Scratch;

const T0: i64 = NOON;
const WORKTREE: &str = "/d/bae-worktrees/side-2";

fn sha(c: char) -> String {
    c.to_string().repeat(40)
}

/// A card that was ready at T0, taken 60s later, committed at +300s, reviewed
/// from +330s to +390s, fixed until +450s, reviewed again, landed from +600s to
/// +640s, and closed at +640s, with one record of every kind the card's steps
/// come from.
fn landed_card(scratch: &Scratch) -> (crate::lists::store::Store, Id) {
    let places = scratch.places();
    let store = make_store(&places, &["bae"]);
    let id = store
        .write(&stamp(T0), "add item", |db| {
            Ok(item_opened(db, vec![], stamp(T0)))
        })
        .unwrap();
    store
        .write(&stamp(T0 + 60), "take", |db| {
            crate::lists::running::start(
                db,
                &[id],
                &[Running {
                    worktree: WORKTREE.into(),
                    branch: "work".into(),
                    brief: "b.md".into(),
                    since: stamp(T0 + 60),
                }],
            )
        })
        .unwrap();
    let path = store.database();
    // A cargo run the agent made, and one a hook check made.
    let cargo = |at: i64, ms: u64, hook: Option<(i64, u32)>| crate::cargo_run::CargoRun {
        worktree: WORKTREE.into(),
        started_at: at,
        subcommand: Some("nextest run".into()),
        host: "m1".into(),
        load: crate::host::Load {
            cores: 10,
            one_minute: 4.0,
        },
        lock_wait: None,
        took: std::time::Duration::from_millis(ms),
        hook_check: hook,
    };
    crate::cargo_run::record_run(
        &path,
        &cargo(T0 + 100, 40_000, None),
        &["builds".into()],
        &crate::cargo_run_outcome::Outcome::passed(),
    )
    .unwrap();
    // A hook run of 30s, 10s of it a cargo run of its own.
    crate::cargo_run::record_run(
        &path,
        &cargo(T0 + 200, 10_000, Some((T0 + 200, 0))),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
    )
    .unwrap();
    crate::hook_stats::record(
        &path,
        &crate::hook_stats::Run {
            at: T0 + 200,
            repo: "bae".into(),
            worktree: WORKTREE.into(),
            branch: Some("work".into()),
            base: None,
            passed: true,
            plan_ms: 2_000,
            total_ms: 30_000,
            files: Vec::new(),
            checks: vec![crate::hook_stats::CheckRecord {
                name: "clippy".into(),
                kind: crate::checks::Kind::Check,
                outcome: crate::hook_stats::Outcome::Ran {
                    passed: true,
                    handed: (1, 1),
                    ms: Some(25_000),
                    started: None,
                    cargo: None,
                },
            }],
        },
    )
    .unwrap();
    // Load on the host the cargo run used.
    crate::host_sample::record(
        &mut crate::db::open(&path, crate::db::Make::Never).unwrap(),
        T0 + 110,
        &[crate::host_sample::Sample {
            host: "m1".into(),
            cores: 10,
            load_1: 12.0,
            memory: crate::host_memory::Pressure::Warning,
        }],
    )
    .unwrap();
    // Two review runs of the branch: the first found two things.
    store
        .write(&stamp(T0 + 330), "review", |db| {
            use crate::review::record::{Kind, NewCall, NewExchange, NewFinding, NewRun};
            let landing = crate::review::record::add_landing(
                db,
                crate::review::record::Change {
                    repo: "bae",
                    key: "branch:work",
                },
                T0 + 330,
            )?;
            let call = |group: &str, started_at: i64, ms: i64, found: usize| NewCall {
                group: group.into(),
                model: "sonnet".into(),
                prompt: "p".into(),
                rules: vec!["r".into()],
                tokens_in: 0,
                tokens_out: 0,
                tokens_cached: 0,
                tokens_written: 0,
                cost_microdollars: 0,
                sites: None,
                failure: None,
                verdicts: vec![("r".into(), "none".into())],
                findings: (0..found)
                    .map(|n| NewFinding {
                        rule: "r".into(),
                        clause: "c".into(),
                        file: "f".into(),
                        line: n as i64,
                        code: format!("x{n}"),
                        problem: "p".into(),
                        blocking: true,
                        check_failed: None,
                    })
                    .collect(),
                closes: Vec::new(),
                exchanges: vec![NewExchange {
                    model: "sonnet".into(),
                    unit: "f".into(),
                    pass: 0,
                    system: "s".into(),
                    message: format!("m {group} {started_at}"),
                    output: "{}".into(),
                    tokens_in: 0,
                    tokens_out: 0,
                    tokens_cached: 0,
                    tokens_written: 0,
                    started_at,
                    ms,
                }],
            };
            let run = |at: i64| NewRun {
                landing: Some(landing),
                kind: Kind::Review { changed_lines: 1 },
                label: None,
                at,
                base: "b",
                head: "h",
                since: None,
                rules_version: "",
            };
            crate::review::record::add_run(
                db,
                &run(T0 + 330),
                &[call("Errors", T0 + 331, 59_000, 2)],
            )?;
            crate::review::record::add_run(
                db,
                &run(T0 + 450),
                &[call("Errors", T0 + 451, 20_000, 0)],
            )?;
            Ok(())
        })
        .unwrap();
    // The coordinator settled the first run's two findings at +400s.
    store
        .write(&stamp(T0 + 400), "settle", |db| {
            let ids: Vec<i64> = db
                .prepare("SELECT id FROM review_finding ORDER BY id")
                .sql()?
                .query_map([], |row| row.get(0))
                .sql()?
                .collect::<std::result::Result<_, _>>()
                .sql()?;
            for id in ids {
                crate::review::record::settle(
                    db,
                    id,
                    "confirmed",
                    "real",
                    T0 + 400,
                    "coordinator",
                )?;
            }
            Ok(())
        })
        .unwrap();
    // The card's dashboard states: Working from the take, then the review's
    // states from the commit, Ready to land from the second review's end, and
    // gone when the landing released the worktree.
    {
        let mut db = crate::db::open(&path, crate::db::Make::Never).unwrap();
        let none = std::collections::BTreeSet::new();
        for (at, state) in [
            (60, "working"),
            (300, "needs-review"),
            (330, "reviewing"),
            (390, "findings-to-settle"),
            (400, "open-findings"),
            (440, "unreviewed-changes"),
            (450, "reviewing"),
            (471, "ready-to-land"),
        ] {
            let seen = crate::card_state::Seen {
                worktree: WORKTREE.into(),
                state,
                items: [id.0].into(),
            };
            crate::card_state::follow(&mut db, T0 + at, &[seen], &none).unwrap();
        }
        crate::card_state::follow(&mut db, T0 + 640, &[], &none).unwrap();
    }
    // An app build that waited 20s in the lane and built for 50s.
    crate::app_timing::record(
        &path,
        &crate::app_timing::RunRecord {
            started_at: T0 + 480,
            repo: "bae".into(),
            lane: crate::lane::Lane::Mac,
            checkout: "side-1".into(),
            built_ref: "work".into(),
            who: None,
            wait_ms: 20_000,
            passed: true,
            load: None,
            steps: vec![crate::app_timing::StepRecord {
                name: "build".into(),
                kind: crate::app_timing::StepKind::Step,
                started_at: T0 + 500,
                wall_ms: 50_000,
                cpu_ms: 0,
                result: crate::app_timing::StepResult::Passed,
            }],
        },
    )
    .unwrap();
    // The landing: 40s, of which the push took 5s.
    crate::land_run::record(
        &path,
        &crate::land_run::LandRun {
            project: "demo".into(),
            started_at: T0 + 600,
            repo: "bae".into(),
            worktree: WORKTREE.into(),
            branch: Some("work".into()),
            by: "agent".into(),
            ms: 40_000,
            landed: true,
            items: vec![id],
            commits: vec![(sha('a'), T0 + 300)],
            steps: vec![
                crate::land_run::Step {
                    kind: crate::land_run::StepKind::Checks,
                    started_at: T0 + 605,
                    ms: 30_000,
                },
                crate::land_run::Step {
                    kind: crate::land_run::StepKind::Push,
                    started_at: T0 + 635,
                    ms: 5_000,
                },
            ],
        },
    )
    .unwrap();
    store
        .write(&stamp(T0 + 640), "land", |db| {
            crate::lists::running::end(db, id, crate::lists::Ended::Closed, &stamp(T0 + 640))?;
            close(db, id, How::Landed(vec![sha('a')]), &stamp(T0 + 640))
        })
        .unwrap();
    (store, id)
}

fn seconds_of(kinds: &BTreeMap<Kind, i64>, kind: Kind) -> i64 {
    kinds.get(&kind).copied().unwrap_or(0)
}

#[test]
fn a_landed_card_gives_every_second_to_one_kind() {
    let scratch = Scratch::new("card-profile-attribute");
    let (store, id) = landed_card(&scratch);
    let lists = store.load().unwrap();
    let db = store.open_existing().unwrap();
    let card = gather(&db, &lists, id, T0 + 9_999).unwrap();
    assert_eq!((card.from, card.to), (T0, T0 + 640));
    let kinds = attribute(&card);
    assert_eq!(kinds.values().sum::<i64>(), 640, "{kinds:?}");
    assert_eq!(seconds_of(&kinds, Kind::Queued), 60, "{kinds:?}");
    // The hook's 30s hold its own cargo run, which counts once.
    assert_eq!(seconds_of(&kinds, Kind::Hook), 30, "{kinds:?}");
    assert_eq!(seconds_of(&kinds, Kind::Cargo), 40, "{kinds:?}");
    assert_eq!(
        seconds_of(&kinds, Kind::Review),
        59 + 1 + 20 + 1,
        "{kinds:?}"
    );
    // The review's waits come from the dashboard's states.
    assert_eq!(
        seconds_of(&kinds, Kind::WaitingForReview),
        30 + 10,
        "{kinds:?}"
    );
    assert_eq!(seconds_of(&kinds, Kind::WaitingForSettle), 10, "{kinds:?}");
    assert_eq!(seconds_of(&kinds, Kind::Fixing), 40, "{kinds:?}");
    assert_eq!(seconds_of(&kinds, Kind::AppBuildQueue), 20, "{kinds:?}");
    assert_eq!(seconds_of(&kinds, Kind::AppBuildStep), 50, "{kinds:?}");
    // Ready to land from +471s to the landing at +600s, less the app build.
    assert_eq!(
        seconds_of(&kinds, Kind::WaitingToLand),
        129 - 70,
        "{kinds:?}"
    );
    assert_eq!(seconds_of(&kinds, Kind::Landing), 40, "{kinds:?}");
    assert_eq!(
        dashboard_seconds(&card),
        BTreeMap::from([
            ("working", 240),
            ("needs-review", 30),
            ("reviewing", 60 + 21),
            ("findings-to-settle", 10),
            ("open-findings", 40),
            ("unreviewed-changes", 10),
            ("ready-to-land", 640 - 471),
        ])
    );
}

#[test]
fn the_timeline_shows_each_step_with_its_hosts_load_and_the_time_by_kind() {
    let scratch = Scratch::new("card-profile-text");
    let (store, id) = landed_card(&scratch);
    let lists = store.load().unwrap();
    let db = store.open_existing().unwrap();
    let text = text(&db, &lists, &[id], T0 + 9_999).unwrap();
    assert!(text.contains("cargo nextest run took 40.0s"), "{text}");
    assert!(
        text.contains(
            "m1 load 12 on average and 12 at most of 10 cores, memory warning (1 sample)"
        ),
        "{text}"
    );
    assert!(text.contains("builds; load 4/10 when it started"), "{text}");
    assert!(text.contains("hook check clippy took 25.0s"), "{text}");
    assert!(
        text.contains("review Errors took 59.0s (2 findings)"),
        "{text}"
    );
    assert!(text.contains("land push took 5.0s"), "{text}");
    assert!(text.contains("commit aaaaaaaaaa"), "{text}");
    assert!(
        text.contains("closed, landed (aaaaaaaaa) by coordinator"),
        "{text}"
    );
    assert!(text.contains("time by kind, 10m"), "{text}");
    assert!(
        text.contains("finding 1 settled as confirmed by coordinator"),
        "{text}"
    );
    assert!(
        text.contains("dashboard open-findings took 40.0s"),
        "{text}"
    );
    assert!(text.contains("time in each dashboard state"), "{text}");
    assert!(text.contains(" 26%  ready-to-land"), "{text}");
    assert!(text.contains("ready, not taken"), "{text}");
}

#[test]
fn the_totals_count_each_kind_and_step_over_the_cards_that_landed_lately() {
    let scratch = Scratch::new("card-profile-totals");
    let (store, _) = landed_card(&scratch);
    let lists = store.load().unwrap();
    let db = store.open_existing().unwrap();
    let totals = text(&db, &lists, &[], T0 + 3_600).unwrap();
    assert!(
        totals.contains("1 card that landed in the last 14 days, 10m in all"),
        "{totals}"
    );
    assert!(
        totals.contains("1 cards  cargo runs") || totals.contains("1 cards  commit hooks"),
        "{totals}"
    );
    assert!(totals.contains("review Errors"), "{totals}");
    assert!(totals.contains("1 cards  open-findings"), "{totals}");
    assert!(!totals.contains("hook check clippy  "), "{totals}");
    let none = text(&db, &lists, &[], T0 + 40 * 86400).unwrap();
    assert!(
        none.contains("no card landed in the last 14 days"),
        "{none}"
    );
}

#[test]
fn an_item_that_never_ran_is_queued_from_its_first_event_and_a_closed_record_that_is_no_item_is_refused()
 {
    let scratch = Scratch::new("card-profile-queued");
    let store = make_store(&scratch.places(), &["bae"]);
    let id = store
        .write(&stamp(T0), "add item", |db| {
            Ok(item_opened(db, vec![], stamp(T0)))
        })
        .unwrap();
    let lists = store.load().unwrap();
    let db = store.open_existing().unwrap();
    let card = gather(&db, &lists, id, T0 + 120).unwrap();
    assert_eq!(attribute(&card), BTreeMap::from([(Kind::Queued, 120)]));
    assert!(gather(&db, &lists, Id(999), T0).is_err());
}

#[test]
fn a_failed_check_is_fixed_flaky_or_standing_by_the_next_pass() {
    let run = |name: &str, passed: bool, base: &str| CheckRun {
        name: name.into(),
        passed,
        base: Some(base.into()),
    };
    let checks = vec![
        run("clippy", false, "a"),
        run("clippy", true, "b"),
        run("fmt", false, "b"),
        run("fmt", true, "b"),
        run("spec", false, "b"),
    ];
    assert_eq!(
        failure_outcomes(&checks),
        vec![
            ("clippy".to_string(), Outcome::Fixed),
            ("fmt".to_string(), Outcome::Flaky),
            ("spec".to_string(), Outcome::Stood),
        ]
    );
}
