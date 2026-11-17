use super::*;
use crate::checkout::Checkout;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
use crate::lists::{Running, Wait};
use crate::testing::Scratch;
use std::path::PathBuf;

/// A worktree of a git repo, `<scratch>/<repo>-worktrees/side-1`. The repo has
/// no build root, no `.build-dirs`, nothing on a build volume.
fn live_worktree(scratch: &Scratch, claimed: bool) -> PathBuf {
    let dir = scratch.dir.canonicalize().unwrap();
    let main = dir.join("bae");
    crate::testing::repo(&main, "target\n");
    let top = dir.join("bae-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &top.display().to_string(),
        ],
    );
    if claimed {
        let checkout = Checkout::at(&top).unwrap().unwrap();
        Claim {
            who: "agent".into(),
            since: 1_790_000_000,
            note: None,
            note_state: None,
        }
        .write(&checkout.claim_file())
        .unwrap();
    }
    top
}

fn loaded_running_in(worktree: &Path) -> Vec<Loaded> {
    let db = db();
    let id = item(&db, vec![]);
    crate::lists::running::start(
        &db,
        &[id],
        &[Running {
            worktree: worktree.display().to_string(),
            branch: "work".into(),
            brief: "b.md".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
    vec![Loaded {
        project: "demo".into(),
        lists: Ok(read(&db)),
    }]
}

#[test]
fn a_running_item_in_a_claimed_worktree_is_fine() {
    let scratch = Scratch::new("running-ok");
    let top = live_worktree(&scratch, true);
    let found = findings(&scratch.places(), &Ok(loaded_running_in(&top))).unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_running_item_in_an_unclaimed_or_gone_worktree_is_named() {
    let scratch = Scratch::new("running-bad");
    let top = live_worktree(&scratch, false);
    let unclaimed = findings(&scratch.places(), &Ok(loaded_running_in(&top))).unwrap();
    assert_eq!(unclaimed.len(), 1);
    assert!(
        unclaimed[0].text.contains("has no claim") && !unclaimed[0].text.contains('`'),
        "{unclaimed:?}"
    );
    assert_eq!(unclaimed[0].command.as_deref(), Some("builds item stop 1"));
    let gone = findings(
        &scratch.places(),
        &Ok(loaded_running_in(&scratch.dir.join("bae-worktrees/side-9"))),
    )
    .unwrap();
    assert!(gone[0].text.contains("is not a live worktree"), "{gone:?}");
}

#[test]
fn a_folder_where_a_worktree_was_is_not_live() {
    let scratch = Scratch::new("running-inside");
    live_worktree(&scratch, true);
    let leftover = scratch
        .dir
        .canonicalize()
        .unwrap()
        .join("bae-worktrees/side-2");
    std::fs::create_dir_all(&leftover).unwrap();
    let found = findings(&scratch.places(), &Ok(loaded_running_in(&leftover))).unwrap();
    assert!(
        found[0].text.contains("is not a live worktree"),
        "{found:?}"
    );
}

#[test]
fn a_store_failing_its_check_is_a_finding() {
    let scratch = Scratch::new("running-check");
    let bad = vec![Loaded {
        project: "demo".into(),
        lists: Err("ids repeat".into()),
    }];
    assert_eq!(
        findings(&scratch.places(), &Ok(bad)).unwrap(),
        vec![Advice::said(
            "lists: project demo fails its check: ids repeat".to_string()
        )]
    );
    assert_eq!(
        findings(&scratch.places(), &Err("listing failed".into())).unwrap(),
        vec![Advice::said("lists: listing failed".to_string())]
    );
}

#[test]
fn every_named_constraint_of_the_running_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    let db = db();
    let id = item(&db, vec![]);
    let other = item(&db, vec![]);
    start(
        &db,
        &[id],
        &[Running {
            worktree: "/d/bae-worktrees/side-2".into(),
            branch: "b".into(),
            brief: "p".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
    let mut refused = Refusals::new(include_str!("running.sql"));
    let running = [
        ("item_id", v(other.0)),
        ("brief", v("p")),
        ("since_at", v(1)),
        ("since_by", v("a")),
    ];
    refused.check(
        "running_brief_says_something",
        plant(&db, "running", &row(&running, &[("brief", v(" "))])),
    );
    refused.check(
        "running_since_by_says_something",
        plant(&db, "running", &row(&running, &[("since_by", v(" "))])),
    );
    plant(&db, "running", &running).unwrap();
    let place = [
        ("item_id", v(other.0)),
        ("repo", v("bae")),
        ("parent", v("/d")),
        ("checkout", v("side-3")),
        ("branch", v("b")),
    ];
    refused.check(
        "running_place_parent_is_absolute",
        plant(&db, "running_place", &row(&place, &[("parent", v("d"))])),
    );
    for checkout in ["main", "side-", "side-1x", "side-x1", "side-1-2"] {
        refused.check(
            "running_place_checkout_is_a_side_worktree",
            plant(
                &db,
                "running_place",
                &row(&place, &[("checkout", v(checkout))]),
            ),
        );
    }
    refused.check(
        "running_place_branch_says_something",
        plant(&db, "running_place", &row(&place, &[("branch", v(" "))])),
    );
    plant(&db, "running_place", &place).unwrap();
    let run = [
        ("id", v(1)),
        ("item_id", v(id.0)),
        ("since_at", v(1)),
        ("since_by", v("a")),
        ("how", v("stopped")),
        ("ended_at", v(2)),
        ("ended_by", v("a")),
    ];
    for (name, change) in [
        ("item_run_since_by_says_something", ("since_by", v(" "))),
        ("item_run_how_known", ("how", v("landed"))),
        ("item_run_ended_by_says_something", ("ended_by", v(" "))),
        ("item_run_ends_after_it_starts", ("ended_at", v(0))),
    ] {
        refused.check(name, plant(&db, "item_run", &row(&run, &[change])));
    }
    plant(&db, "item_run", &run).unwrap();
    let run_place = [
        ("run_id", v(1)),
        ("repo", v("bae")),
        ("worktree", v("/d/bae-worktrees/side-2")),
        ("branch", v("b")),
    ];
    refused.check(
        "item_run_place_worktree_is_absolute",
        plant(
            &db,
            "item_run_place",
            &row(&run_place, &[("worktree", v("d"))]),
        ),
    );
    refused.check(
        "item_run_place_branch_says_something",
        plant(
            &db,
            "item_run_place",
            &row(&run_place, &[("branch", v(" "))]),
        ),
    );
    plant(&db, "item_run_place", &run_place).unwrap();
    let open = [
        ("item_id", v(id.0)),
        ("kind", v("held")),
        ("since_at", v(1)),
    ];
    refused.check(
        "running_span_kind_known",
        plant(&db, "running_span", &row(&open, &[("kind", v("idle"))])),
    );
    plant(&db, "running_span", &open).unwrap();
    let ended = [
        ("item_id", v(id.0)),
        ("kind", v("waiting")),
        ("since_at", v(2)),
        ("ended_at", v(3)),
    ];
    refused.check(
        "item_span_kind_known",
        plant(&db, "item_span", &row(&ended, &[("kind", v("idle"))])),
    );
    refused.check(
        "item_span_ends_after_it_starts",
        plant(&db, "item_span", &row(&ended, &[("ended_at", v(1))])),
    );
    plant(&db, "item_span", &ended).unwrap();
    refused.check(
        "spans_recorded_is_single",
        plant(
            &db,
            "spans_recorded",
            &[("singleton", v(2)), ("from_at", v(1))],
        ),
    );
    refused.done();
}

fn place(worktree: &str, branch: &str) -> Running {
    Running {
        worktree: worktree.into(),
        branch: branch.into(),
        brief: "b.md".into(),
        since: stamp(NOON),
    }
}

#[test]
fn ending_a_run_keeps_it_with_how_it_ended_where_it_ran_and_its_spans() {
    let db = db();
    let id = item(&db, vec![]);
    start(&db, &[id], &[place("/d/bae-worktrees/side-1", "work")]).unwrap();
    open_span(&db, id, SpanKind::Held, NOON + 60).unwrap();
    open_span(&db, id, SpanKind::Held, NOON + 90).unwrap();
    end(&db, id, Ended::Stopped, &stamp(NOON + 600)).unwrap();
    let lists = read(&db);
    let item = &lists.items[0];
    assert!(item.running.is_empty());
    assert_eq!(
        item.runs,
        [Run {
            since: stamp(NOON),
            ended: stamp(NOON + 600),
            how: Ended::Stopped,
            places: vec![RunPlace {
                repo: "bae".into(),
                worktree: "/d/bae-worktrees/side-1".into(),
                branch: "work".into(),
            }],
        }]
    );
    assert_eq!(
        item.spans,
        [Span {
            kind: SpanKind::Held,
            since: NOON + 60,
            ended: Some(NOON + 600),
        }]
    );
    assert_eq!(end(&db, id, Ended::Stopped, &stamp(NOON + 700)).unwrap(), 0);
    assert!(open_span(&db, id, SpanKind::Held, NOON + 700).is_err());
}

#[test]
fn a_waiting_span_follows_the_items_waits_on_the_person() {
    use crate::lists::fixtures::question;
    use crate::lists::item::{add_wait, remove_wait};
    let db = db();
    let asked = question(&db);
    let id = item(&db, vec![]);
    start(&db, &[id], &[place("/d/bae-worktrees/side-1", "work")]).unwrap();
    follow_waits(&db, &read(&db), NOON + 10).unwrap();
    assert!(read(&db).items[0].spans.is_empty());
    add_wait(&db, id, Wait::Question(asked)).unwrap();
    follow_waits(&db, &read(&db), NOON + 20).unwrap();
    follow_waits(&db, &read(&db), NOON + 30).unwrap();
    remove_wait(&db, id, &Wait::Question(asked)).unwrap();
    follow_waits(&db, &read(&db), NOON + 40).unwrap();
    add_wait(&db, id, Wait::Question(asked)).unwrap();
    follow_waits(&db, &read(&db), NOON + 50).unwrap();
    let waiting = |since, ended| Span {
        kind: SpanKind::Waiting,
        since,
        ended,
    };
    assert_eq!(
        read(&db).items[0].spans,
        [
            waiting(NOON + 20, Some(NOON + 40)),
            waiting(NOON + 50, None)
        ]
    );
}

#[test]
fn a_write_through_the_store_opens_and_ends_waiting_spans() {
    use crate::lists::fixtures::question;
    use crate::lists::item::add_wait;
    let scratch = Scratch::new("running-follow");
    let store = crate::lists::fixtures::make_store(&scratch.places(), &["bae"]);
    let (asked, id) = store
        .write(&stamp(NOON), "seed", |db| {
            let asked = question(db);
            let id = item(db, vec![]);
            start(db, &[id], &[place("/d/bae-worktrees/side-1", "work")])?;
            Ok((asked, id))
        })
        .unwrap();
    store
        .write(&stamp(NOON + 100), "wait", |db| {
            add_wait(db, id, Wait::Question(asked))
        })
        .unwrap();
    store
        .write(&stamp(NOON + 400), "question done", |db| {
            crate::lists::record::close(db, asked, crate::lists::How::Done, &stamp(NOON + 400))
        })
        .unwrap();
    assert_eq!(
        store.load().unwrap().items[0].spans,
        [Span {
            kind: SpanKind::Waiting,
            since: NOON + 100,
            ended: Some(NOON + 400),
        }]
    );
}

#[test]
fn a_places_worktree_is_generated_from_its_parts_and_a_repo_has_one_worktree_per_item() {
    use crate::lists::fixtures::{db, v};
    let db = db();
    let id = item(&db, vec![]);
    start(
        &db,
        &[id],
        &[
            Running {
                worktree: "/d/bae-worktrees/side-2".into(),
                branch: "b".into(),
                brief: "p".into(),
                since: stamp(NOON),
            },
            Running {
                worktree: "/e/coven-worktrees/side-10".into(),
                branch: "c".into(),
                brief: "p".into(),
                since: stamp(NOON),
            },
        ],
    )
    .unwrap();
    let worktrees: Vec<String> = db
        .prepare("SELECT worktree FROM running_place ORDER BY repo")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|w| w.unwrap())
        .collect();
    assert_eq!(
        worktrees,
        ["/d/bae-worktrees/side-2", "/e/coven-worktrees/side-10"]
    );
    let repeated = plant(
        &db,
        "running_place",
        &[
            ("item_id", v(id.0)),
            ("repo", v("bae")),
            ("parent", v("/x")),
            ("checkout", v("side-1")),
            ("branch", v("b")),
        ],
    );
    assert!(
        repeated
            .unwrap_err()
            .to_string()
            .contains("UNIQUE constraint failed")
    );
    let elsewhere = plant(
        &db,
        "running_place",
        &[
            ("item_id", v(id.0)),
            ("repo", v("nope")),
            ("parent", v("/x")),
            ("checkout", v("side-1")),
            ("branch", v("b")),
        ],
    );
    assert!(elsewhere.unwrap_err().to_string().contains("FOREIGN KEY"));
}
