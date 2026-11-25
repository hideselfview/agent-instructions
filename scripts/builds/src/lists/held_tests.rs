use super::*;
use crate::claim::{Note, cmd_note};
use crate::lists::design_reads::DesignReads;
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
            tier: crate::model_tier::Tier::Implementation,
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
        NOON,
    )
}

fn claim_of(top: &std::path::Path) -> Claim {
    Claim::read(&Checkout::at(top).unwrap().unwrap().claim_file())
        .unwrap()
        .unwrap()
}

fn findings(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
    reads: Coverage<'_>,
) -> Vec<Advice> {
    inspect(loaded, worktrees, reads).findings
}

fn awaiting_approval(
    loaded: &std::result::Result<Vec<Loaded>, String>,
    worktrees: &[Worktree],
    reads: Coverage<'_>,
) -> Vec<String> {
    inspect(loaded, worktrees, reads)
        .design_waits
        .into_iter()
        .filter(|(_, wait)| matches!(wait, DesignWait::Approval(_)))
        .map(|(label, _)| label)
        .collect()
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
        crate::lists::record::close(db, q, How::Done("shown".into()), &stamp(NOON)).unwrap();
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
    assert_eq!(claim_of(&top).state(), Some("held"));

    let other = demo("held-condition");
    let top = other.add_side(1, "agent", "work");
    start(&other, &top, |_| {
        vec![Wait::condition(
            "the screen is unlocked",
            crate::lists::Ender::Coordinator,
        )]
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
        NOON,
    )
    .unwrap();
}

#[test]
fn noting_a_worktree_held_and_then_not_held_keeps_a_held_span_on_its_items() {
    use crate::lists::{Span, SpanKind};
    let demo = demo("held-span");
    let top = demo.add_side(1, "agent", "work");
    start(&demo, &top, |db| vec![Wait::Question(question(db))]);
    let top_arg = top.display().to_string();
    for (words, at) in [
        (vec!["--state", "held", "waits on the lock"], NOON + 100),
        (vec!["--state", "held", "still waits"], NOON + 200),
        (vec!["back at work"], NOON + 300),
    ] {
        let mut note = vec![top_arg.as_str()];
        note.extend(words);
        cmd_note(&demo.tool.places, &args(&note), at).unwrap();
    }
    assert_eq!(
        demo.store.load().unwrap().items[0].spans,
        [
            Span {
                kind: SpanKind::Held,
                since: NOON + 100,
                ended: Some(NOON + 300),
            },
            Span {
                kind: SpanKind::Waiting,
                since: NOON,
                ended: None,
            },
        ]
    );
    assert_eq!(claim_of(&top).note_text(), Some("back at work"));
}

fn worktree(top: &std::path::Path, who: &str, state: Option<&str>, ahead: Vec<String>) -> Worktree {
    Worktree {
        label: "demo/side-1".into(),
        checkout: Checkout {
            top: top.to_path_buf(),
            main: top.parent().unwrap().join("demo"),
            git_dir: top.join(".git"),
        },
        branch: None,
        claim: Claim {
            who: who.into(),
            since: 1_790_000_000,
            note: Some(Note::with_state(state, "kept".into()).unwrap()),
        },
        project: None,
        app_checkout: false,
        git: Ok(side_read(&ahead)),
        committing: Ok(Vec::new()),
    }
}

/// What git says of a clean worktree whose commits waiting to land are
/// <ahead>, full hashes oldest first.
fn side_read(ahead: &[String]) -> SideRead {
    SideRead {
        head: None,
        ahead: ahead
            .iter()
            .rev()
            .map(|full| crate::side_git::Commit {
                sha: full.chars().take(7).collect(),
                full: full.clone(),
                time: 1,
                authored: 1,
                subject: "a commit".into(),
            })
            .collect(),
        main: "0".repeat(40),
        uncommitted: 0,
        last_write: None,
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
    tree.checkout.main = top.parent().unwrap().join("bae");
    tree
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
            &["design/mocks/a.dc.html"],
            None,
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
    let first = design_drawn(&db, &path, &["design/mocks/a.dc.html"], None);
    design_tipped(&db, first, &a);
    let second = design_drawn(
        &db,
        &path,
        &["design/mocks/b.dc.html", "design/mocks/c.dc.html"],
        None,
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
    assert!(
        findings(
            &loaded(lists.clone()),
            &[covered],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
    let after = worktree(&top, "agent", None, vec![a.clone(), b, c.clone()]);
    let found = findings(
        &loaded(lists),
        &[after],
        Coverage::Read(&DesignReads::default()),
    );
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
    let found = findings(
        &loaded(lists),
        &[tree],
        Coverage::Read(&DesignReads::default()),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains("design/mocks/a.dc.html")
            && found[0].command.as_deref() == Some("builds design record"),
        "{found:?}"
    );
}

#[test]
fn health_names_a_worktree_whose_commits_git_cannot_read_and_never_calls_it_covered() {
    let (_demo, mut tree, lists) = design_branch("held-design-unread", false);
    tree.git = Err("git could not read it".into());
    let found = findings(
        &loaded(lists.clone()),
        std::slice::from_ref(&tree),
        Coverage::Read(&DesignReads::default()),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .text
            .contains("could not read the commits of demo/side-1")
            && found[0].text.contains("git could not read it"),
        "{found:?}"
    );
    assert!(
        tree.idle_claim(true, &[]).unwrap().is_none(),
        "commits git could not read may wait to land"
    );
    assert!(
        awaiting_approval(
            &loaded(lists),
            &[tree],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
}

#[test]
fn health_accepts_design_commits_an_open_design_change_covers() {
    let (_demo, tree, lists) = design_branch("held-design-yes", true);
    assert!(
        findings(
            &loaded(lists),
            &[tree],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
}

#[test]
fn health_skips_branches_with_no_design_files() {
    let (demo, _tree, lists) = design_branch("held-design-skip", false);
    let top = demo.add_side(2, "agent", "other");
    let sha = demo.commit(&top, "src.rs");
    let plain = worktree(&top, "agent", None, vec![sha]);
    assert!(
        findings(
            &loaded(lists),
            &[plain],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
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
    assert!(
        findings(
            &loaded(read(&db())),
            &[tree],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
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
    assert_eq!(claim.state(), Some("held"));
    assert_eq!(claim.note_text(), Some("waits on the answer to question 1"));
    let lists = demo.store.load().unwrap();
    assert_eq!(
        crate::lists::running::running_in(&lists, &top.display().to_string()).len(),
        1
    );
    let kinds: Vec<_> = lists.items[0]
        .spans
        .iter()
        .map(|span| (span.kind, span.ended))
        .collect();
    assert_eq!(
        kinds,
        [
            (crate::lists::SpanKind::Held, None),
            (crate::lists::SpanKind::Waiting, None)
        ]
    );
    let mut tree = worktree(&top, "agent", Some("held"), Vec::new());
    tree.checkout.main = top.parent().unwrap().join("demo");
    assert!(
        findings(
            &loaded(lists),
            &[tree],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
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
fn a_worktree_whose_design_change_waits_on_approval_awaits_it_until_approved() {
    let (_demo, tree, mut lists) = design_branch("held-review", true);
    let trees = [tree];
    assert_eq!(
        awaiting_approval(
            &loaded(lists.clone()),
            &trees,
            Coverage::Read(&DesignReads::default())
        ),
        vec!["demo/side-1".to_string()]
    );
    for design in &mut lists.designs {
        design.approved = Some(stamp(NOON));
    }
    assert!(
        awaiting_approval(
            &loaded(lists),
            &trees,
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
}

#[test]
fn a_worktree_with_no_design_change_does_not_await_approval() {
    let (_demo, tree, lists) = design_branch("held-review-none", false);
    assert!(
        awaiting_approval(
            &loaded(lists),
            &[tree],
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
}

#[test]
fn health_names_a_claimed_worktree_whose_items_closed_and_has_nothing_to_land() {
    let demo = demo("claim-release-advice");
    let top = demo.add_side(1, "agent", "work");
    start(&demo, &top, |_| Vec::new());
    let id = demo.store.load().unwrap().items[0].id;
    let mut tree = worktree(&top, "agent", None, Vec::new());
    tree.checkout = Checkout::at(&top).unwrap().unwrap();
    assert!(
        findings(
            &loaded(demo.store.load().unwrap()),
            std::slice::from_ref(&tree),
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
    demo.store
        .write(&stamp(NOON), "end", |db| {
            let stamp = stamp(NOON);
            crate::lists::record::close(db, id, How::Finished("finished".into()), &stamp)?;
            crate::lists::running::end(db, id, crate::lists::Ended::Closed, &stamp)
        })
        .unwrap();
    let found = findings(
        &loaded(demo.store.load().unwrap()),
        std::slice::from_ref(&tree),
        Coverage::Read(&DesignReads::default()),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.contains("no item runs there") && !found[0].text.contains('`'),
        "{found:?}"
    );
    assert_eq!(
        found[0].command.as_deref(),
        Some(format!("builds release {}", top.display()).as_str())
    );
    let sha = demo.commit(&top, "only-here");
    let mut waiting = worktree(&top, "agent", None, vec![sha]);
    waiting.checkout = Checkout::at(&top).unwrap().unwrap();

    let mut committing = bae_worktree(None);
    committing.committing = Ok(vec![4242]);
    let mut unknown = bae_worktree(None);
    unknown.committing = Err("builds cannot read process 4242".into());
    for busy in [waiting, committing, unknown] {
        assert!(
            findings(
                &loaded(demo.store.load().unwrap()),
                std::slice::from_ref(&busy),
                Coverage::Read(&DesignReads::default())
            )
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
    assert!(
        findings(
            &loaded,
            std::slice::from_ref(&tree),
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
    // The item of the second project holds the worktree whatever project
    // the claim names.
    tree.project = Some("first".into());
    assert!(
        findings(
            &loaded,
            std::slice::from_ref(&tree),
            Coverage::Read(&DesignReads::default())
        )
        .is_empty()
    );
    // A claim that names no project, or one that does not hold the repo, is a
    // finding of its own.
    for (named, fault) in [
        (None, "two projects (first, second)"),
        (Some("third"), "does not hold repo bae"),
    ] {
        tree.project = named.map(str::to_string);
        let found = findings(
            &loaded,
            std::slice::from_ref(&tree),
            Coverage::Read(&DesignReads::default()),
        );
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

#[test]
fn an_active_worktree_does_not_need_a_release_copy_check() {
    let demo = demo("held-active-copies");
    let top = demo.add_side(1, "agent", "work");
    let original = demo.commit(&top, "preserved");
    let copy = demo.add_side(2, "other", "copy");
    demo.commit(&copy, "other-file");
    crate::testing::git(&copy, &["cherry-pick", &original]);

    let checkout = Checkout::at(&top).unwrap().unwrap();
    let mut worktree = Worktree::read(
        &demo.tool.places,
        "demo/side-1".into(),
        &checkout,
        &claim_of(&top),
        checkout.branch().unwrap(),
        &crate::side_git::read(&top),
        &crate::procs::GitCommits::default(),
    )
    .unwrap();
    let lists = [demo.store.load().unwrap()];
    assert_eq!(
        worktree.idle_claim(true, &lists).unwrap(),
        Some(BTreeSet::from(["copy".into()]))
    );

    // The retained git read has commits, but a copy probe on the missing
    // checkout would fail. An active item keeps the claim without that probe.
    worktree.checkout.top = top.join("missing");
    assert!(worktree.idle_claim(true, &lists).is_err());
    assert_eq!(worktree.idle_claim(false, &lists).unwrap(), None);
}

#[test]
fn an_idle_worktree_keeps_a_branch_it_left_with_uncopied_work() {
    let demo = demo("held-release-left-branch");
    let top = demo.add_side(1, "agent", "work");
    demo.commit(&top, "uncopied");
    crate::testing::git(&top, &["switch", "--detach", "origin/main"]);
    let checkout = Checkout::at(&top).unwrap().unwrap();
    let git = crate::side_git::read(&top).unwrap();
    assert!(git.ahead.is_empty());
    let tree = Worktree::read(
        &demo.tool.places,
        "demo/side-1".into(),
        &checkout,
        &claim_of(&top),
        None,
        &Ok(git),
        &crate::procs::GitCommits::default(),
    )
    .unwrap();
    assert_eq!(
        tree.idle_claim(true, &[demo.store.load().unwrap()])
            .unwrap(),
        None
    );
}

/// A claim is idle, and `builds health` says to release it and releases it,
/// only while nothing holds the worktree: no item runs there, no commit waits
/// to land, no file is uncommitted, no `git commit` runs, and the claim is
/// neither held nor waiting for its item, nor an app checkout's.
#[test]
fn a_claim_is_idle_only_when_nothing_holds_the_worktree() {
    let demo = demo("claim-release-gates");
    let top = demo.add_side(1, "agent", "work");
    assert!(
        worktree(&top, "a", None, Vec::new())
            .idle_claim(true, &[])
            .unwrap()
            .is_some()
    );
    assert!(
        worktree(&top, "a", None, Vec::new())
            .idle_claim(false, &[])
            .unwrap()
            .is_none()
    );
    assert!(
        worktree(&top, "a", Some("held"), Vec::new())
            .idle_claim(true, &[])
            .unwrap()
            .is_none()
    );
    assert!(
        worktree(&top, "a", Some("starting"), Vec::new())
            .idle_claim(true, &[])
            .unwrap()
            .is_none()
    );
    let mut dirty = worktree(&top, "a", None, Vec::new());
    std::fs::write(top.join("dirty"), "").unwrap();
    dirty.git = crate::side_git::read(&top);
    assert!(dirty.idle_claim(true, &[]).unwrap().is_none());
    let mut committing = worktree(&top, "a", None, Vec::new());
    committing.committing = Ok(vec![7]);
    assert!(committing.idle_claim(true, &[]).unwrap().is_none());
    let mut unknown = worktree(&top, "a", None, Vec::new());
    unknown.committing = Err("ps failed".into());
    assert!(unknown.idle_claim(true, &[]).is_err());
    let mut app = worktree(&top, "a", None, Vec::new());
    app.app_checkout = true;
    assert!(app.idle_claim(true, &[]).unwrap().is_none());
}

/// Health's advice to release a claim and its own release use one test, so
/// a claim that waits for its item gets neither.
#[test]
fn health_advises_no_release_of_a_claim_waiting_for_its_item() {
    let db = db();
    let demo = demo("claim-release-starting");
    let top = demo.add_side(1, "agent", "work");
    let loaded = loaded(read(&db));
    let idle = findings(
        &loaded,
        &[worktree(&top, "a", None, Vec::new())],
        Coverage::Read(&DesignReads::default()),
    );
    assert!(
        idle.iter().any(|f| f.line().contains("release it with")),
        "{idle:?}"
    );
    let starting = findings(
        &loaded,
        &[worktree(&top, "a", Some("starting"), Vec::new())],
        Coverage::Read(&DesignReads::default()),
    );
    assert!(
        starting
            .iter()
            .all(|f| !f.line().contains("release it with")),
        "{starting:?}"
    );
}

#[test]
fn a_pending_design_comparison_blocks_quiet_advice_without_creating_a_finding() {
    let (_demo, tree, lists) = design_branch("held-design-pending", true);
    let reads = DesignReads::default();
    let reading = inspect(&loaded(lists), &[tree], Coverage::Completed(&reads));
    assert_eq!(
        reading.design_waits.get("demo/side-1"),
        Some(&DesignWait::Checking)
    );
    assert!(reading.findings.is_empty());
}
