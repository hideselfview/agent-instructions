use super::*;
use crate::checkout::Checkout;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
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
            since: "2026-11-13 10:00".into(),
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
        unclaimed[0].contains("has no claim") && unclaimed[0].contains("`builds item stop 1`"),
        "{unclaimed:?}"
    );
    let gone = findings(
        &scratch.places(),
        &Ok(loaded_running_in(&scratch.dir.join("bae-worktrees/side-9"))),
    )
    .unwrap();
    assert!(gone[0].contains("is not a live worktree"), "{gone:?}");
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
    assert!(found[0].contains("is not a live worktree"), "{found:?}");
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
        vec!["lists: project demo fails its check: ids repeat".to_string()]
    );
    assert_eq!(
        findings(&scratch.places(), &Err("listing failed".into())).unwrap(),
        vec!["lists: listing failed".to_string()]
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
    refused.done();
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
