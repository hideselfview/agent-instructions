use super::*;
use crate::claim::cmd_note;
use crate::lists::fixtures::{NOON, db, design_drawn, design_tipped, item, question, read, stamp};
use crate::lists::item_fixtures::{Demo, demo, refusal};
use crate::lists::{How, Running};

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn running_in_db(db: &rusqlite::Connection, id: crate::lists::Id, worktree: &str) {
    crate::lists::running::start(
        db,
        &[id],
        &[Running {
            worktree: worktree.into(),
            branch: "work".into(),
            brief: "b.md".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
}

/// Starts an item running in <top> that waits on <waits>, in the demo store.
fn start(
    demo: &Demo,
    top: &std::path::Path,
    waits: impl FnOnce(&rusqlite::Connection) -> Vec<Wait>,
) {
    demo.store
        .write(&stamp(NOON), "start", |db| {
            let waits = waits(db);
            let id = item(db, waits);
            running_in_db(db, id, &top.display().to_string());
            Ok(())
        })
        .unwrap();
}

fn held(demo: &Demo, top: &std::path::Path) -> Result<()> {
    cmd_note(
        &demo.tool.places,
        &args(&[
            &top.display().to_string(),
            "--state",
            "held",
            "waits on the lock",
        ]),
    )
}

fn claim_of(top: &std::path::Path) -> Claim {
    Claim::read(&Checkout::at(top).unwrap().unwrap().claim_file())
        .unwrap()
        .unwrap()
}

#[test]
fn a_hold_is_refused_while_nothing_runs_in_the_worktree() {
    let demo = demo("held-nothing");
    let top = demo.add_side(1, "agent", "work");
    let fault = refusal(held(&demo, &top));
    assert!(
        fault.contains("runs no item") && fault.contains("builds item wait N --on REF"),
        "{fault}"
    );
    assert!(claim_of(&top).note.is_none());
}

#[test]
fn a_hold_is_refused_for_an_item_that_waits_on_nothing() {
    let demo = demo("held-no-wait");
    let top = demo.add_side(1, "agent", "work");
    start(&demo, &top, |_| Vec::new());
    let fault = refusal(held(&demo, &top));
    assert!(fault.contains("waits on nothing open"), "{fault}");
}

#[test]
fn a_hold_is_refused_for_an_item_whose_question_closed() {
    let demo = demo("held-closed");
    let top = demo.add_side(1, "agent", "work");
    start(&demo, &top, |db| {
        let q = question(db);
        crate::lists::record::close(db, q, How::Done, &stamp(NOON)).unwrap();
        vec![Wait::Question(q)]
    });
    assert!(refusal(held(&demo, &top)).contains("waits on nothing open"));
}

#[test]
fn a_hold_on_an_open_question_or_condition_is_taken() {
    let first = demo("held-open");
    let top = first.add_side(1, "agent", "work");
    start(&first, &top, |db| vec![Wait::Question(question(db))]);
    held(&first, &top).unwrap();
    assert_eq!(claim_of(&top).note_state.as_deref(), Some("held"));

    let other = demo("held-condition");
    let top = other.add_side(1, "agent", "work");
    start(&other, &top, |_| {
        vec![Wait::Condition("the screen is unlocked".into())]
    });
    held(&other, &top).unwrap();
}

#[test]
fn other_states_need_no_wait() {
    let demo = demo("held-other-states");
    let top = demo.add_side(1, "agent", "work");
    cmd_note(
        &demo.tool.places,
        &args(&[
            &top.display().to_string(),
            "--state",
            "winding-down",
            "last item",
        ]),
    )
    .unwrap();
}

fn worktree(top: &std::path::Path, who: &str, state: Option<&str>, ahead: Vec<String>) -> Worktree {
    Worktree {
        label: "demo/side-1".into(),
        top: top.to_path_buf(),
        repo: "demo".into(),
        claim: Claim {
            who: who.into(),
            since: 1_790_000_000,
            note: Some("kept".into()),
            note_state: state.map(str::to_string),
        },
        project: None,
        app_checkout: false,
        ahead,
        uncommitted: Some(0),
        committing: Vec::new(),
    }
}

fn loaded(lists: Lists) -> std::result::Result<Vec<Loaded>, String> {
    Ok(vec![Loaded {
        project: "demo".into(),
        lists: Ok(lists),
    }])
}

/// The fixture lists name the repos bae and coven.
fn bae_worktree(state: Option<&str>) -> Worktree {
    let top = std::path::Path::new("/w/bae-worktrees/side-1");
    let mut tree = worktree(top, "agent", state, Vec::new());
    tree.repo = "bae".into();
    tree
}

#[test]
fn health_names_a_hold_whose_wait_closed() {
    let db = db();
    let q = question(&db);
    let id = item(&db, vec![Wait::Question(q)]);
    running_in_db(&db, id, "/w/bae-worktrees/side-1");
    let mut tree = bae_worktree(Some("held"));
    assert!(findings(&loaded(read(&db)), std::slice::from_ref(&tree)).is_empty());
    crate::lists::record::close(&db, q, How::Done, &stamp(NOON)).unwrap();
    let found = findings(&loaded(read(&db)), std::slice::from_ref(&tree));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains("is held") && found[0].text.contains("waits on nothing open"),
        "{found:?}"
    );
    tree.claim.note_state = None;
    assert!(findings(&loaded(read(&db)), &[tree]).is_empty());
}

#[test]
fn health_names_a_hold_with_no_item() {
    let db = db();
    let found = findings(&loaded(read(&db)), &[bae_worktree(Some("held"))]);
    assert!(found[0].text.contains("runs no item"), "{found:?}");
    assert_eq!(
        found[0].command.as_deref(),
        Some("builds note /w/bae-worktrees/side-1 TEXT")
    );
}

/// A board change committed in a worktree, and the design change that
/// records it when <recorded>.
fn design_branch(name: &str, recorded: bool) -> (Demo, Worktree, Lists) {
    let demo = demo(name);
    let top = demo.add_side(1, "agent", "work");
    std::fs::create_dir_all(top.join("design/mocks")).unwrap();
    let sha = demo.commit(&top, "design/mocks/a.dc.html");
    let db = db();
    if recorded {
        design_drawn(
            &db,
            &top.display().to_string(),
            &[("design/mocks/a.dc.html", "https://canvas/a")],
            Vec::new(),
        );
    }
    let mut lists = read(&db);
    lists.repos = vec!["demo".into()];
    for design in &mut lists.designs {
        design.repo = "demo".into();
        design.tip = Some(sha.clone());
    }
    lists
        .design_places
        .insert("demo".into(), vec!["design/mocks/*.dc.html".into()]);
    (demo, worktree(&top, "agent", None, vec![sha]), lists)
}

/// Each commit counts as covered by any open design change whose last commit
/// is that commit or a later one and whose places hold the commit's places,
/// approved or waiting; a commit made after a change's last commit is not.
#[test]
fn health_counts_a_commit_covered_by_any_change_whose_range_holds_it() {
    let demo = demo("held-design-ranges");
    let top = demo.add_side(1, "agent", "work");
    std::fs::create_dir_all(top.join("design/mocks")).unwrap();
    let a = demo.commit(&top, "design/mocks/a.dc.html");
    let b = demo.commit(&top, "design/mocks/b.dc.html");
    let c = demo.commit(&top, "design/mocks/c.dc.html");
    let db = db();
    let path = top.display().to_string();
    let first = design_drawn(
        &db,
        &path,
        &[("design/mocks/a.dc.html", "https://canvas/a")],
        Vec::new(),
    );
    design_tipped(&db, first, &a);
    let second = design_drawn(
        &db,
        &path,
        &[
            ("design/mocks/b.dc.html", "https://canvas/b"),
            ("design/mocks/c.dc.html", "https://canvas/c"),
        ],
        Vec::new(),
    );
    design_tipped(&db, second, &b);
    let mut lists = read(&db);
    lists.repos = vec!["demo".into()];
    for design in &mut lists.designs {
        design.repo = "demo".into();
        if design.id == second {
            design.approved = Some(stamp(NOON));
        }
    }
    lists
        .design_places
        .insert("demo".into(), vec!["design/mocks/*.dc.html".into()]);
    let covered = worktree(&top, "agent", None, vec![a.clone(), b.clone()]);
    assert!(findings(&loaded(lists.clone()), &[covered]).is_empty());
    let after = worktree(&top, "agent", None, vec![a.clone(), b, c.clone()]);
    let found = findings(&loaded(lists), &[after]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains(&c[..10])
            && !found[0].text.contains(&a[..10])
            && found[0].text.contains("(design/mocks/c.dc.html)"),
        "{found:?}"
    );
}

#[test]
fn health_names_design_commits_no_design_change_covers() {
    let (_demo, tree, lists) = design_branch("held-design-no", false);
    let found = findings(&loaded(lists), &[tree]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains("design/mocks/a.dc.html")
            && found[0].command.as_deref() == Some("builds design record"),
        "{found:?}"
    );
}

#[test]
fn health_accepts_design_commits_an_open_design_change_covers() {
    let (_demo, tree, lists) = design_branch("held-design-yes", true);
    assert!(findings(&loaded(lists), &[tree]).is_empty());
}

#[test]
fn health_skips_branches_with_no_design_files() {
    let (demo, _tree, lists) = design_branch("held-design-skip", false);
    let top = demo.add_side(2, "agent", "other");
    let sha = demo.commit(&top, "src.rs");
    let plain = worktree(&top, "agent", None, vec![sha]);
    assert!(findings(&loaded(lists), &[plain]).is_empty());
}

#[test]
fn an_app_checkout_is_held_with_no_item_and_health_leaves_it() {
    let demo = demo("held-app");
    let top = demo.add_side(1, "agent", "work");
    std::fs::create_dir_all(demo.tool.places.root.join("demo")).unwrap();
    std::fs::write(
        demo.tool
            .places
            .app_checkouts_file("demo", crate::lane::Lane::ALL[0]),
        "side-1\n",
    )
    .unwrap();
    held(&demo, &top).unwrap();
    let mut tree = bae_worktree(Some("held"));
    tree.app_checkout = true;
    assert!(findings(&loaded(read(&db())), &[tree]).is_empty());
}

fn take_held(demo: &Demo, id: &str, top: &std::path::Path, extra: &[&str]) -> Result<()> {
    let brief = demo.brief("brief.md");
    let top = top.display().to_string();
    let mut args = vec![
        "take",
        id,
        "--worktree",
        &top,
        "--branch",
        "work",
        "--brief",
        &brief,
    ];
    args.extend_from_slice(extra);
    crate::lists::cmd_item(&demo.tool, &crate::lists::item_fixtures::words(&args))
}

#[test]
fn an_item_waiting_on_the_persons_question_is_taken_held_and_health_leaves_it() {
    let demo = demo("held-take");
    let top = demo.add_side(1, "agent", "work");
    demo.store
        .write(&stamp(NOON), "wait", |db| {
            let q = question(db);
            item(db, vec![Wait::Question(q)]);
            Ok(())
        })
        .unwrap();
    // Without --held an item that waits is refused.
    assert!(refusal(take_held(&demo, "2", &top, &[])).contains("waits on"));
    take_held(
        &demo,
        "2",
        &top,
        &["--held", "waits on the answer to question 1"],
    )
    .unwrap();
    let claim = claim_of(&top);
    assert_eq!(claim.note_state.as_deref(), Some("held"));
    assert_eq!(
        claim.note.as_deref(),
        Some("waits on the answer to question 1")
    );
    let lists = demo.store.load().unwrap();
    assert_eq!(
        crate::lists::running::running_in(&lists, &top.display().to_string()).len(),
        1
    );
    let mut tree = worktree(&top, "agent", Some("held"), Vec::new());
    tree.repo = "demo".into();
    assert!(findings(&loaded(lists), &[tree]).is_empty());
}

#[test]
fn held_takes_only_an_item_that_waits_on_something_open() {
    let demo = demo("held-take-nothing");
    let top = demo.add_side(1, "agent", "work");
    demo.store
        .write(&stamp(NOON), "item", |db| {
            item(db, Vec::new());
            Ok(())
        })
        .unwrap();
    let fault = refusal(take_held(&demo, "1", &top, &["--held", "why"]));
    assert!(fault.contains("waits on nothing open"), "{fault}");
    assert!(claim_of(&top).note.is_none());
}

#[test]
fn a_worktree_whose_design_change_waits_on_review_awaits_review_until_approved() {
    let (_demo, tree, mut lists) = design_branch("held-review", true);
    let trees = [tree];
    assert_eq!(
        awaiting_review(&loaded(lists.clone()), &trees),
        vec!["demo/side-1".to_string()]
    );
    for design in &mut lists.designs {
        design.approved = Some(stamp(NOON));
    }
    assert!(awaiting_review(&loaded(lists), &trees).is_empty());
}

#[test]
fn a_worktree_with_no_design_change_does_not_await_review() {
    let (_demo, tree, lists) = design_branch("held-review-none", false);
    assert!(awaiting_review(&loaded(lists), &[tree]).is_empty());
}

#[test]
fn health_names_a_claimed_worktree_whose_items_closed_and_has_nothing_to_land() {
    let db = db();
    let id = item(&db, vec![]);
    running_in_db(&db, id, "/w/bae-worktrees/side-1");
    let tree = bae_worktree(None);
    assert!(findings(&loaded(read(&db)), std::slice::from_ref(&tree)).is_empty());
    crate::lists::running::end(&db, id).unwrap();
    let found = findings(&loaded(read(&db)), std::slice::from_ref(&tree));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains("no item runs there") && !found[0].text.contains('`'),
        "{found:?}"
    );
    assert_eq!(
        found[0].command.as_deref(),
        Some("builds release /w/bae-worktrees/side-1")
    );
    let mut waiting = bae_worktree(None);
    waiting.ahead = vec!["abc".into()];
    let mut editing = bae_worktree(None);
    editing.uncommitted = Some(1);
    let mut committing = bae_worktree(None);
    committing.committing = vec![4242];
    for busy in [waiting, editing, committing] {
        assert!(
            findings(&loaded(read(&db)), std::slice::from_ref(&busy))
                .iter()
                .all(|line| !line.text.contains("no item runs there")),
            "{busy:?}"
        );
    }
}

#[test]
fn health_sees_an_item_of_either_project_in_a_repo_two_projects_hold() {
    // Both projects hold the fixture's repo bae; the item runs in the second's lists.
    let named = |project: &str, db: &rusqlite::Connection| {
        let mut lists = read(db);
        lists.project = project.into();
        Loaded {
            project: project.into(),
            lists: Ok(lists),
        }
    };
    let (first, second) = (db(), db());
    let id = item(&second, vec![]);
    running_in_db(&second, id, "/w/bae-worktrees/side-1");
    let loaded = Ok(vec![named("first", &first), named("second", &second)]);
    let mut tree = bae_worktree(None);
    tree.project = Some("second".into());
    assert!(findings(&loaded, std::slice::from_ref(&tree)).is_empty());
    // The item of the second project holds the worktree whatever project
    // the claim names.
    tree.project = Some("first".into());
    assert!(findings(&loaded, std::slice::from_ref(&tree)).is_empty());
    // A claim that names no project, or one that does not hold the repo, is a
    // finding of its own.
    for (named, fault) in [
        (None, "two projects (first, second)"),
        (Some("third"), "does not hold repo bae"),
    ] {
        tree.project = named.map(str::to_string);
        let found = findings(&loaded, std::slice::from_ref(&tree));
        assert!(
            found.len() == 1
                && found[0]
                    .text
                    .starts_with("lists: health can't choose the lists of demo/side-1: ")
                && found[0].text.contains(fault),
            "{found:?}"
        );
    }
}
