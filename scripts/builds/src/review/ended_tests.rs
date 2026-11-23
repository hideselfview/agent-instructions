use std::collections::BTreeSet;

use super::*;
use crate::review::judge::tests::{Reviewed, reviewed, stamp};
use crate::testing::git;

/// What `close` found for review `r`'s landing when nothing names a branch.
fn closed(r: &Reviewed, named: &[(&str, &str)]) -> Vec<String> {
    let named: BTreeSet<(String, String)> = named
        .iter()
        .map(|(repo, branch)| (repo.to_string(), branch.to_string()))
        .collect();
    close(
        &r.demo.store,
        &stamp("health"),
        std::slice::from_ref(&r.demo.main),
        &named,
    )
    .unwrap()
}

fn landing(r: &Reviewed) -> record::LandingRow {
    record::landing(&r.demo.store.open_existing().unwrap(), r.landing).unwrap()
}

/// Deletes the branch `work` and its worktree, as a branch removal does.
fn delete_branch(r: &Reviewed) {
    git(
        &r.demo.main,
        &[
            "worktree",
            "remove",
            "--force",
            &r.side.display().to_string(),
        ],
    );
    git(&r.demo.main, &["branch", "-D", "work"]);
}

/// Puts the commit of `work` on origin/main as a cherry-pick, which keeps its
/// patch and gives it another hash.
fn land_on_main(r: &Reviewed) {
    git(&r.demo.main, &["cherry-pick", "work"]);
    git(&r.demo.main, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_review_whose_branch_holds_commits_to_land_stays_open() {
    let r = reviewed("ended-holds", None);
    assert_eq!(closed(&r, &[]), Vec::<String>::new());
    assert!(landing(&r).is_open());
}

#[test]
fn a_review_whose_branch_and_commits_are_gone_ends_abandoned_and_keeps_its_findings() {
    let r = reviewed("ended-gone", None);
    delete_branch(&r);
    git(&r.demo.main, &["reflog", "expire", "--expire=now", "--all"]);
    git(&r.demo.main, &["gc", "-q", "--prune=now"]);
    let said = closed(&r, &[]);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains(&format!("review landing {}", r.landing)),
        "{said:?}"
    );
    let row = landing(&r);
    assert!(!row.is_open());
    assert_eq!(row.landed_run(), None, "it never landed");
    let conn = r.demo.store.open_existing().unwrap();
    assert!(
        record::abandoned_why(&conn, r.landing)
            .unwrap()
            .unwrap()
            .contains("work")
    );
    assert_eq!(
        record::findings_of_landing(&conn, r.landing).unwrap().len(),
        1,
        "the finding stays on the record"
    );
    assert!(record::open_progress(&conn).unwrap().is_empty());
    // The same branch name can be reviewed again.
    r.demo
        .store
        .write(&stamp("tester"), "open again", |db| {
            record::add_landing(
                db,
                record::Change {
                    repo: "demo",
                    key: "branch:work",
                },
                1,
            )
        })
        .unwrap();
    // The new landing has no run yet, so builds says it cannot judge it, and
    // the ended landing is not read again.
    let said = closed(&r, &[]);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("no last review run"), "{said:?}");
}

#[test]
fn a_review_whose_branch_an_item_names_stays_open_when_the_branch_is_gone() {
    let r = reviewed("ended-named", None);
    delete_branch(&r);
    assert_eq!(closed(&r, &[("demo", "work")]), Vec::<String>::new());
    assert!(landing(&r).is_open());
}

#[test]
fn a_review_whose_commits_are_on_main_by_patch_ends_landed_with_its_last_run() {
    let r = reviewed("ended-on-main", None);
    land_on_main(&r);
    let said = closed(&r, &[]);
    assert_eq!(said.len(), 1, "{said:?}");
    let row = landing(&r);
    assert!(!row.is_open());
    assert_eq!(row.landed_run(), row.reviewed_run);
    assert!(row.landed_run().is_some());
    assert_ne!(row.end, record::End::Abandoned);
}

#[test]
fn a_deleted_branch_whose_commits_are_on_main_ends_landed() {
    let r = reviewed("ended-on-main-gone", None);
    land_on_main(&r);
    delete_branch(&r);
    closed(&r, &[]);
    let row = landing(&r);
    assert!(row.landed_run().is_some(), "{row:?}");
}

#[test]
fn a_review_whose_repo_has_no_main_checkout_says_it_could_not_be_judged() {
    let r = reviewed("ended-no-main", None);
    let said = close(&r.demo.store, &stamp("health"), &[], &BTreeSet::new()).unwrap();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("no main checkout of repo demo"),
        "{said:?}"
    );
    assert!(landing(&r).is_open());
}
