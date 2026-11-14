use super::*;
use crate::claim::cmd_note;
use crate::lists::fixtures::{NOON, db, design_drawn, item, question, read, stamp};
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
            since: "2026-11-14 08:00".into(),
            note: Some("kept".into()),
            note_state: state.map(str::to_string),
        },
        app_checkout: false,
        ahead,
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
        found[0].contains("is held") && found[0].contains("waits on nothing open"),
        "{found:?}"
    );
    tree.claim.note_state = None;
    assert!(findings(&loaded(read(&db)), &[tree]).is_empty());
}

#[test]
fn health_names_a_hold_with_no_item() {
    let db = db();
    let found = findings(&loaded(read(&db)), &[bae_worktree(Some("held"))]);
    assert!(found[0].contains("runs no item"), "{found:?}");
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
    }
    lists
        .design_places
        .insert("demo".into(), vec!["design/mocks/*.dc.html".into()]);
    (demo, worktree(&top, "agent", None, vec![sha]), lists)
}

#[test]
fn health_names_design_commits_no_design_change_covers() {
    let (_demo, tree, lists) = design_branch("held-design-no", false);
    let found = findings(&loaded(lists), &[tree]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("design/mocks/a.dc.html") && found[0].contains("builds design record"),
        "{found:?}"
    );
}

#[test]
fn health_accepts_design_commits_an_open_design_change_covers() {
    let (_demo, tree, lists) = design_branch("held-design-yes", true);
    assert!(findings(&loaded(lists), &[tree]).is_empty());
}

#[test]
fn health_skips_the_landing_worktree_and_branches_with_no_design_files() {
    let (demo, mut tree, lists) = design_branch("held-design-skip", false);
    tree.claim.who = "coordinator (landing)".into();
    assert!(findings(&loaded(lists.clone()), &[tree]).is_empty());
    let top = demo.add_side(2, "agent", "other");
    let sha = demo.commit(&top, "src.rs");
    let plain = worktree(&top, "agent", None, vec![sha]);
    assert!(findings(&loaded(lists), &[plain]).is_empty());
}

#[test]
fn a_landing_worktree_is_held_with_no_item_and_health_leaves_it() {
    let demo = demo("held-landing");
    let top = demo.add_side(1, "coordinator (landing)", "work");
    held(&demo, &top).unwrap();
    let mut tree = bae_worktree(Some("held"));
    tree.claim.who = "coordinator (landing)".into();
    assert!(findings(&loaded(read(&db())), &[tree]).is_empty());
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
