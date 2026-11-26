use super::*;
use crate::checkout::Checkout;
use crate::claim::cmd_release;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo, demo_over, refusal, words};
use crate::lists::{Closed, Record};

fn item(demo: &Demo, args: &[&str]) -> Result<()> {
    cmd_item(&demo.tool, &words(args))
}

fn add(demo: &Demo, title: &str, extra: &[&str]) -> Result<()> {
    let details = demo.brief("details.txt");
    let mut args = vec![
        "add",
        "--title",
        title,
        "--area",
        "Alpha",
        "--details-file",
        &details,
    ];
    args.extend_from_slice(extra);
    item(demo, &args)
}

const FIVE: &str = "Do the first thing now";

fn open(demo: &Demo) -> Lists {
    demo.store.load().unwrap()
}

#[test]
fn health_lists_an_old_wait_through_to_the_item_at_its_end() {
    let mut demo = demo("health-item-wait");
    add(&demo, FIVE, &[]).unwrap();
    demo.tool.now += 10 * 60;
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    add(&demo, "Do the third thing now", &["--wait", "item:2"]).unwrap();
    demo.tool.now += 30 * 60;
    let mut health = crate::health::health(&demo.tool).unwrap();
    let loaded = crate::lists::store::load_all(&demo.tool.places);
    crate::health_stillness::append(
        &mut health,
        &loaded,
        &crate::health_stillness::Progress::read(&demo.tool.places, &loaded),
        None,
        &demo.tool.places,
        demo.tool.now,
        demo.tool.rules.still_minutes,
    );
    let lines = crate::health::lines(&health);
    let rows: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("item #"))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "only the item at the end of the waits: {lines:?}"
    );
    assert!(
        rows[0].contains("item #1")
            && rows[0].contains("40m")
            && rows[0].contains("It blocks 2 items."),
        "{lines:?}"
    );
    assert!(health.schedule_needed);
}

#[test]
fn health_uses_the_configured_threshold_and_the_last_item_note() {
    let mut demo = demo("health-item-note");
    demo.tool.rules.still_minutes = 10;
    add(&demo, FIVE, &[]).unwrap();
    demo.tool.now += 20 * 60;
    item(&demo, &["note", "1", "The build finished"]).unwrap();
    let still = |demo: &Demo| {
        let mut health = crate::health::health(&demo.tool).unwrap();
        let loaded = crate::lists::store::load_all(&demo.tool.places);
        crate::health_stillness::append(
            &mut health,
            &loaded,
            &crate::health_stillness::Progress::read(&demo.tool.places, &loaded),
            None,
            &demo.tool.places,
            demo.tool.now,
            demo.tool.rules.still_minutes,
        );
        health.findings.iter().any(|finding| {
            finding.kind == crate::health::FindingKind::Still
                && finding.advice.text.contains("item #1")
        })
    };
    demo.tool.now += 10 * 60 - 1;
    assert!(!still(&demo));
    demo.tool.now += 1;
    assert!(still(&demo));
}

#[test]
fn add_makes_a_ready_item_and_refuses_what_it_lacks() {
    let demo = demo("item-add");
    add(&demo, FIVE, &[]).unwrap();
    let lists = open(&demo);
    assert_eq!(lists.items[0].title, FIVE);
    assert_eq!(lists.items[0].opened.by, "tester");
    let before = demo.state();
    assert!(refusal(add(&demo, "Too short", &[])).contains("2 words"));
    let details = demo.brief("d");
    let no_area = refusal(item(
        &demo,
        &[
            "add",
            "--title",
            FIVE,
            "--area",
            "Nowhere",
            "--details-file",
            &details,
        ],
    ));
    assert!(
        no_area.contains("does not exist") && no_area.contains("Alpha"),
        "{no_area}"
    );
    assert!(
        refusal(add(&demo, FIVE, &["--brief", "/nonexistent/brief.md"])).contains("does not exist")
    );
    assert!(refusal(add(&demo, FIVE, &["--wait", "item:99"])).contains("no record has"));
    assert!(refusal(add(&demo, FIVE, &["--priority", "first"])).contains("needs --why"));
    assert!(refusal(add(&demo, FIVE, &["--why", "x"])).contains("belongs to --priority"));
    assert!(refusal(add(&demo, FIVE, &["--wait", "bogus"])).contains("is not a wait"));
    assert_eq!(demo.state(), before, "a refused add changed the file");
}

#[test]
fn add_takes_waits_a_brief_a_priority_and_the_design_sessions_audience() {
    let demo = demo("item-add-all");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("brief.md");
    add(
        &demo,
        "Do the second thing now",
        &[
            "--wait",
            "item:1",
            "--wait",
            "condition:a quiet machine",
            "--until",
            "coordinator",
            "--brief",
            &brief,
            "--priority",
            "first",
            "--why",
            "person said so",
            "--for",
            "design-session",
        ],
    )
    .unwrap();
    let second = &open(&demo).items[1];
    assert_eq!(
        second.waits,
        [
            Wait::Item(Id(1)),
            Wait::condition("a quiet machine", crate::lists::Ender::Coordinator)
        ]
    );
    assert_eq!(
        (second.brief.as_deref(), second.r#for),
        (Some(brief.as_str()), Audience::DesignSession)
    );
    assert_eq!(second.priority.as_ref().unwrap().level, Level::First);
}

/// A take in a worktree of repo `other` of a project over `demo` and `other`,
/// naming the relative brief <brief>.
fn take_in_other_repo(demo: &Demo, brief: &str) -> Result<()> {
    let side = demo.add_side_of("other", 1, "agent A", "work");
    add(demo, FIVE, &[]).unwrap();
    item(
        demo,
        &take_args(&side.display().to_string(), brief, &["1"], "work"),
    )
}

#[test]
fn take_finds_a_relative_brief_in_the_main_checkout_of_another_repo() {
    let demo = demo_over("item-brief-other-repo", &["demo", "other"]);
    std::fs::create_dir_all(demo.main.join("plans")).unwrap();
    std::fs::write(demo.main.join("plans/b.md"), "x").unwrap();
    take_in_other_repo(&demo, "plans/b.md").unwrap();
    let running = &open(&demo).items[0].running[0];
    assert_eq!(running.brief, "plans/b.md");
}

#[test]
fn take_refuses_a_relative_brief_that_two_main_checkouts_hold() {
    let demo = demo_over("item-brief-ambiguous", &["demo", "other"]);
    let other = demo.scratch.repo_path("other");
    for main in [&demo.main, &other] {
        std::fs::create_dir_all(main.join("plans")).unwrap();
        std::fs::write(main.join("plans/b.md"), "x").unwrap();
    }
    let said = refusal(take_in_other_repo(&demo, "plans/b.md"));
    assert!(said.contains("is ambiguous"), "{said}");
    assert!(open(&demo).items[0].running.is_empty());
}

#[test]
fn take_refuses_a_relative_brief_that_no_main_checkout_holds() {
    let demo = demo_over("item-brief-missing", &["demo", "other"]);
    let said = refusal(take_in_other_repo(&demo, "plans/b.md"));
    assert!(said.contains("does not exist"), "{said}");
    assert!(open(&demo).items[0].running.is_empty());
}

#[test]
fn edit_records_a_new_details_text_and_a_decision_origin_as_changes() {
    let demo = demo("item-edit-changes");
    add(&demo, FIVE, &[]).unwrap();
    let old = open(&demo).items[0].details.clone();
    let details = demo.scratch.dir.join("new-details.txt");
    std::fs::write(&details, "Cite the new decision.\n").unwrap();
    let details = details.display().to_string();
    item(&demo, &["edit", "1", "--details-file", &details]).unwrap();
    item(&demo, &["edit", "1", "--area", "Beta"]).unwrap();
    let new = open(&demo).items[0].details.clone();
    let changes = open(&demo).items[0].changes.clone();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        crate::lists::item_change::What::Details { old, new }
    );
}

#[test]
fn edit_records_a_decision_as_the_origin_but_a_question_as_no_change() {
    let demo = demo("item-edit-origin-changes");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    item(&demo, &["edit", "1", "--origin", "2"]).unwrap();
    assert!(open(&demo).items[0].changes.is_empty());
    let decision = demo
        .store
        .write(&crate::lists::fixtures::stamp(NOON), "decision", |db| {
            let asked = crate::lists::fixtures::question(db);
            Ok(crate::lists::fixtures::decision_answering(db, asked))
        })
        .unwrap();
    item(&demo, &["edit", "1", "--origin", &decision.0.to_string()]).unwrap();
    let changes = open(&demo).items[0].changes.clone();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0].what,
        crate::lists::item_change::What::Decision(decision)
    );
}

#[test]
fn edit_changes_fields_and_refuses_a_closed_or_empty_edit() {
    let demo = demo("item-edit");
    add(&demo, FIVE, &[]).unwrap();
    item(
        &demo,
        &[
            "edit",
            "1",
            "--area",
            "Beta",
            "--title",
            "Do the renamed thing now",
        ],
    )
    .unwrap();
    assert_eq!(open(&demo).items[0].area, "Beta");
    assert!(refusal(item(&demo, &["edit", "1"])).contains("changes nothing"));
    assert!(refusal(item(&demo, &["edit", "1", "--area", "Gamma"])).contains("does not exist"));
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
    assert!(
        refusal(item(&demo, &["edit", "1", "--area", "Alpha"])).contains("is closed (dropped)")
    );
}

#[test]
fn note_adds_a_line_and_wants_text() {
    let demo = demo("item-note");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &["note", "1", "R1", "landed", "today"]).unwrap();
    let note = &open(&demo).items[0].notes[0];
    assert_eq!(
        (note.text.as_str(), note.by.as_str()),
        ("R1 landed today", "tester")
    );
    assert!(refusal(item(&demo, &["note", "1"])).contains("needs its text"));
    assert!(refusal(item(&demo, &["note", "7", "x"])).contains("not a record"));
}

#[test]
fn wait_adds_and_removes_and_refuses_a_cycle_or_a_closed_record() {
    let demo = demo("item-wait");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    item(&demo, &["wait", "2", "--on", "item:1"]).unwrap();
    assert_eq!(open(&demo).items[1].waits, [Wait::Item(Id(1))]);
    let before = demo.state();
    let cycle = refusal(item(&demo, &["wait", "1", "--on", "item:2"]));
    assert!(cycle.contains("in a circle"), "{cycle}");
    assert!(
        refusal(item(&demo, &["wait", "1", "--on", "item:1"])).contains("cannot wait on itself")
    );
    assert!(refusal(item(&demo, &["wait", "2", "--on", "item:1"])).contains("already waits"));
    assert!(
        refusal(item(&demo, &["wait", "1", "--off", "item:2"])).contains("does not wait on item:2")
    );
    assert!(refusal(item(&demo, &["wait", "1"])).contains("exactly one of --on REF and --off REF"));
    assert_eq!(demo.state(), before);
    item(&demo, &["wait", "2", "--off", "item:1"]).unwrap();
    item(
        &demo,
        &[
            "wait",
            "2",
            "--on",
            "condition:a quiet machine",
            "--until",
            "ci",
        ],
    )
    .unwrap();
    assert_eq!(
        open(&demo).items[1].waits,
        [Wait::condition("a quiet machine", crate::lists::Ender::Ci)]
    );
    item(&demo, &["wait", "2", "--off", "condition:a quiet machine"]).unwrap();
    assert!(open(&demo).items[1].waits.is_empty());
}

/// A condition names who ends it, and the person is never that one: a wait
/// on the person is a your-step question.
#[test]
fn a_condition_needs_an_ender_that_is_not_the_person() {
    let demo = demo("wait-ender");
    add(&demo, "Do the first thing now", &[]).unwrap();
    let without = refusal(item(&demo, &["wait", "1", "--on", "condition:later"]));
    assert!(without.contains("says who ends it"), "{without}");
    let person = refusal(item(
        &demo,
        &["wait", "1", "--on", "condition:later", "--until", "person"],
    ));
    assert!(
        person.contains("builds question ask --kind your-step")
            && person.contains("--on question:N"),
        "{person}"
    );
    let unknown = refusal(item(
        &demo,
        &["wait", "1", "--on", "condition:later", "--until", "someone"],
    ));
    assert!(unknown.contains("agent, coordinator or ci"), "{unknown}");
    let stray = refusal(item(
        &demo,
        &["wait", "1", "--on", "item:1", "--until", "ci"],
    ));
    assert!(
        stray.contains("--until goes with a `condition:TEXT` wait"),
        "{stray}"
    );
    let off = refusal(item(
        &demo,
        &["wait", "1", "--off", "condition:later", "--until", "ci"],
    ));
    assert!(off.contains("--until goes with --on"), "{off}");
    assert!(open(&demo).items[0].waits.is_empty());
}

#[test]
fn wait_off_the_last_item_a_question_blocks_takes_why_the_question_is_asked_anyway() {
    let demo = demo("item-wait-question");
    demo.store
        .write(&crate::lists::fixtures::stamp(1), "seed", |db| {
            let q = crate::lists::fixtures::question(db);
            crate::lists::fixtures::item(db, vec![]);
            crate::lists::fixtures::item(db, vec![]);
            crate::lists::item::add_wait(db, Id(2), Wait::Question(q))
        })
        .unwrap();
    item(&demo, &["wait", "3", "--on", "question:1"]).unwrap();
    let before = demo.state();
    assert!(
        refusal(item(
            &demo,
            &["wait", "3", "--on", "item:2", "--blocks-nothing", "x"]
        ))
        .contains("--blocks-nothing goes with --off question:N")
    );
    let still = refusal(item(
        &demo,
        &["wait", "3", "--off", "question:1", "--blocks-nothing", "x"],
    ));
    assert!(
        still.contains(
            "question #1 gives a reason it blocks nothing (x) while open item #2 waits on it"
        ),
        "{still}"
    );
    assert_eq!(demo.state(), before);
    item(&demo, &["wait", "3", "--off", "question:1"]).unwrap();
    let last = refusal(item(&demo, &["wait", "2", "--off", "question:1"]));
    assert!(
        last.contains("question #1 blocks no open item and gives no reason it stays on the stack")
            && last.contains("`builds item wait ITEM --off question:1 --blocks-nothing WHY`"),
        "{last}"
    );
    item(
        &demo,
        &[
            "wait",
            "2",
            "--off",
            "question:1",
            "--blocks-nothing",
            "asked to know",
        ],
    )
    .unwrap();
    let lists = open(&demo);
    assert_eq!(
        lists.question(Id(1)).unwrap().blocks_nothing.as_deref(),
        Some("asked to know")
    );
    assert!(lists.items.iter().all(|i| i.waits.is_empty()));
    item(&demo, &["wait", "2", "--on", "question:1"]).unwrap();
    assert_eq!(open(&demo).question(Id(1)).unwrap().blocks_nothing, None);
}

fn take_args<'a>(side: &'a str, brief: &'a str, ids: &[&'a str], branch: &'a str) -> Vec<&'a str> {
    let mut args = vec!["take"];
    args.extend_from_slice(ids);
    args.extend(["--worktree", side, "--branch", branch, "--brief", brief]);
    args
}

#[test]
fn take_refuses_a_brief_that_pastes_workflow_text() {
    let demo = demo("item-take-pasted");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    let workflow: Vec<&str> = crate::help::WORKFLOW_HELP
        .lines()
        .filter(|line| !line.trim().is_empty())
        .skip(12)
        .take(4)
        .collect();
    let brief = demo.scratch.dir.join("pasted.md");
    std::fs::write(
        &brief,
        format!("Make it work.\n\n{}\n", workflow.join("\n")),
    )
    .unwrap();
    let side_arg = side.display().to_string();
    let brief_arg = brief.display().to_string();
    let refused = refusal(item(
        &demo,
        &take_args(&side_arg, &brief_arg, &["1"], "work"),
    ));
    assert!(
        refused.contains("pastes text of builds help workflow"),
        "{refused}"
    );
    assert!(open(&demo).items[0].running.is_empty());
}

#[test]
fn take_marks_items_running_in_the_worktree_and_stores_no_agent() {
    let demo = demo("item-take");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    let brief = demo.brief("b.md");
    let side_arg = side.display().to_string();
    item(&demo, &take_args(&side_arg, &brief, &["1", "2"], "work")).unwrap();
    let lists = open(&demo);
    for item in &lists.items {
        let running = item.running.first().unwrap();
        assert_eq!(
            (
                running.worktree.as_str(),
                running.branch.as_str(),
                running.brief.as_str()
            ),
            (side_arg.as_str(), "work", brief.as_str())
        );
    }
    assert!(!demo.state().contains("agent A"));
    assert!(
        crate::lists::running::agent_of(lists.items[0].running.first().unwrap()).unwrap()
            == crate::lists::running::Agent::Claimed("agent A".into())
    );
}

#[test]
fn take_records_the_tier_of_the_model_the_items_take() {
    use crate::model_tier::Tier;
    let demo = demo("item-take-tier");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    let brief = demo.brief("b.md");
    let side_arg = side.display().to_string();
    assert!(
        open(&demo).items[0].running.is_empty(),
        "an item that waits has no tier"
    );
    let mut args = take_args(&side_arg, &brief, &["1"], "work");
    args.extend(["--tier", "strongest"]);
    item(&demo, &args).unwrap();
    assert_eq!(open(&demo).items[0].running[0].tier, Tier::Strongest);
    // With no --tier, the items take the implementation tier.
    let side_2 = demo.add_side(2, "agent B", "other");
    let side_2_arg = side_2.display().to_string();
    item(&demo, &take_args(&side_2_arg, &brief, &["2"], "other")).unwrap();
    assert_eq!(open(&demo).items[1].running[0].tier, Tier::Implementation);
}

#[test]
fn take_refuses_a_tier_it_does_not_know() {
    let demo = demo("item-take-bad-tier");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    let side_arg = side.display().to_string();
    let mut args = take_args(&side_arg, &brief, &["1"], "work");
    args.extend(["--tier", "best"]);
    let error = item(&demo, &args).unwrap_err().text();
    assert!(
        error.contains("--tier takes implementation or strongest"),
        "{error}"
    );
    assert!(open(&demo).items[0].running.is_empty());
}

#[test]
fn take_refuses_what_is_not_ready_or_not_right() {
    let demo = demo("item-take-refuse");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    add(
        &demo,
        "Do the design sessions thing",
        &["--for", "design-session"],
    )
    .unwrap();
    let before = demo.state();
    let waits = refusal(item(&demo, &take_args(&side_arg, &brief, &["2"], "work")));
    assert!(waits.contains("2 waits on item #1"), "{waits}");
    assert!(
        refusal(item(&demo, &take_args(&side_arg, &brief, &["3"], "work")))
            .contains("--as design-session")
    );
    let mut as_design = take_args(&side_arg, &brief, &["1"], "work");
    as_design.extend(["--as", "design-session"]);
    assert!(refusal(item(&demo, &as_design)).contains("takes only the design session's"));
    assert!(
        refusal(item(
            &demo,
            &take_args(&side_arg, &brief, &["1"], "nobranch")
        ))
        .contains("has no branch nobranch")
    );
    assert!(
        refusal(item(
            &demo,
            &take_args(&side_arg, "/no/brief.md", &["1"], "work")
        ))
        .contains("does not exist")
    );
    assert!(
        refusal(item(
            &demo,
            &take_args(&side_arg, &brief, &["1", "1"], "work")
        ))
        .contains("names #1 twice")
    );
    let main = demo.main.display().to_string();
    assert!(
        refusal(item(&demo, &take_args(&main, &brief, &["1"], "main")))
            .contains("not one of the repo's side worktrees")
    );
    assert_eq!(demo.state(), before);

    let unclaimed = demo.add_side(2, "x", "other");
    cmd_release(&demo.tool.places, &[unclaimed.display().to_string()]).unwrap();
    let no_claim = refusal(item(
        &demo,
        &take_args(&unclaimed.display().to_string(), &brief, &["1"], "other"),
    ));
    assert!(no_claim.contains("has no claim"), "{no_claim}");

    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    assert!(
        refusal(item(&demo, &take_args(&side_arg, &brief, &["1"], "work")))
            .contains("already runs in")
    );
    demo.add_side_branch(&side, "second");
    add(&demo, "Do the fourth thing now", &[]).unwrap();
    assert!(
        refusal(item(&demo, &take_args(&side_arg, &brief, &["4"], "second")))
            .contains("holds one branch of work")
    );
}

#[test]
fn a_design_session_item_is_taken_as_the_design_session() {
    let demo = demo("item-take-design");
    let side = demo.add_side(1, "design", "canvas");
    add(
        &demo,
        "Do the design sessions thing",
        &["--for", "design-session"],
    )
    .unwrap();
    let brief = demo.brief("b.md");
    let mut args = take_args(side.to_str().unwrap(), &brief, &["1"], "canvas");
    args.extend(["--as", "design-session"]);
    item(&demo, &args).unwrap();
    assert!(!open(&demo).items[0].running.is_empty());
}

#[test]
fn stop_returns_the_item_to_the_queue_with_the_why_and_handoff_changes_the_brief() {
    let demo = demo("item-stop");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    let second = demo.brief("b2.md");
    assert!(refusal(item(&demo, &["stop", "1", "--why", "x"])).contains("is not running"));
    assert!(refusal(item(&demo, &["handoff", "1", "--brief", &second])).contains("is not running"));
    item(
        &demo,
        &take_args(side.to_str().unwrap(), &brief, &["1"], "work"),
    )
    .unwrap();
    item(&demo, &["handoff", "1", "--brief", &second]).unwrap();
    assert_eq!(open(&demo).items[0].running.first().unwrap().brief, second);
    item(&demo, &["stop", "1", "--why", "agent ran out of room"]).unwrap();
    let item = &open(&demo).items[0];
    assert!(item.running.is_empty());
    assert_eq!(item.notes[0].text, "stopped: agent ran out of room");
}

#[test]
fn release_is_refused_while_an_item_runs_in_the_worktree() {
    let demo = demo("item-release");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(
        &demo,
        &take_args(side.to_str().unwrap(), &brief, &["1"], "work"),
    )
    .unwrap();
    let side_arg = side.display().to_string();
    let refused = refusal(cmd_release(
        &demo.tool.places,
        std::slice::from_ref(&side_arg),
    ));
    assert!(
        refused.contains("still runs #1") && refused.contains("item stop 1"),
        "{refused}"
    );
    assert!(
        crate::claim::Claim::read(&Checkout::find(&side_arg).unwrap().claim_file())
            .unwrap()
            .is_some()
    );
    item(&demo, &["stop", "1", "--why", "done here"]).unwrap();
    cmd_release(&demo.tool.places, &[side_arg]).unwrap();
}

/// A claim made for an item that is not yet taken carries the starting state,
/// and `builds item take` clears it, so health releases no claim between the
/// claim and the take.
#[test]
fn take_clears_the_starting_state_of_the_claim() {
    let demo = demo("item-take-starting");
    let side = demo.add_side(1, "agent A", "work");
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    crate::claim::write_claim(
        &checkout,
        "agent A",
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .unwrap()
            .since,
        Some(crate::claim::Note::Starting),
    )
    .unwrap();
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(
        &demo,
        &take_args(side.to_str().unwrap(), &brief, &["1"], "work"),
    )
    .unwrap();
    let claim = crate::claim::Claim::read(&checkout.claim_file())
        .unwrap()
        .unwrap();
    assert_eq!(claim.note, None);
}

#[test]
fn drop_refuses_a_waited_on_or_last_blocking_item() {
    let demo = demo("item-drop");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    let before = demo.state();
    let waited = refusal(item(&demo, &["drop", "1", "--why", "no"]));
    assert!(
        waited.contains("2 \"Do the second thing now\" waits on #1")
            && waited.contains("item wait 2 --off item:1"),
        "{waited}"
    );
    assert_eq!(demo.state(), before);
    item(&demo, &["drop", "2", "--why", "not needed"]).unwrap();
    let lists = open(&demo);
    assert!(
        matches!(&lists.closed[0], Closed { record: Record::Item(i), how: How::Dropped(w), .. } if i.id == Id(2) && w == "not needed")
    );
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
}

#[test]
fn drop_refuses_the_last_item_a_question_blocks() {
    let demo = demo("item-drop-last");
    demo.store
        .write(&crate::lists::fixtures::stamp(1), "seed", |db| {
            let blocked = crate::lists::fixtures::item(db, vec![]);
            let q = crate::lists::fixtures::question(db);
            crate::lists::item::add_wait(db, blocked, Wait::Question(q))?;
            Ok(())
        })
        .unwrap();
    let refused = refusal(item(&demo, &["drop", "1", "--why", "no"]));
    assert!(
        refused.contains("last item question #2") && refused.contains("--blocks-nothing"),
        "{refused}"
    );
}

#[test]
fn defer_wants_an_open_issue() {
    let demo = demo("item-defer");
    add(&demo, FIVE, &[]).unwrap();
    let cwd = current_dir().unwrap();
    let args = |url: &str| words(&["1", "--issue", url]);
    let closed = |_: &[&str]| Ok(r#"{"state":"CLOSED"}"#.to_string());
    let open_issue = |_: &[&str]| Ok(r#"{"state":"OPEN"}"#.to_string());
    let before = demo.state();
    assert!(refusal(defer(&demo.tool, &args("https://x/1"), &cwd, &closed)).contains("is CLOSED"));
    assert_eq!(demo.state(), before);
    defer(&demo.tool, &args("https://x/1"), &cwd, &open_issue).unwrap();
    assert!(matches!(&open(&demo).closed[0].how, How::Deferred(url) if url == "https://x/1"));
}

#[test]
fn place_moves_an_open_item_first_or_last_or_clears_it_and_logs_the_write() {
    let demo = demo("item-place");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &["place", "1", "first", "--why", "top priority"]).unwrap();
    let placed = open(&demo).items[0].priority.clone().unwrap();
    assert_eq!(
        (placed.level, placed.why.as_str()),
        (Level::First, "top priority")
    );
    let db = crate::db::open_reader(&demo.store.database())
        .unwrap()
        .unwrap();
    assert!(
        crate::lists::store::read_log(&db)
            .unwrap()
            .last()
            .unwrap()
            .3
            .starts_with("item place #1 first")
    );
    item(&demo, &["place", "1", "last", "--why", "later"]).unwrap();
    assert_eq!(
        open(&demo).items[0].priority.as_ref().unwrap().level,
        Level::Last
    );
    item(&demo, &["place", "1", "none", "--why", "back to the order"]).unwrap();
    assert_eq!(open(&demo).items[0].priority, None);
}

#[test]
fn place_refuses_a_closed_item_a_missing_why_and_an_unknown_place() {
    let demo = demo("item-place-refuse");
    add(&demo, FIVE, &[]).unwrap();
    let before = demo.state();
    assert!(refusal(item(&demo, &["place", "1", "first"])).contains("--why is required"));
    assert!(
        refusal(item(&demo, &["place", "1", "first", "--why", " "])).contains("--why is empty")
    );
    assert!(
        refusal(item(&demo, &["place", "1", "middle", "--why", "w"]))
            .contains("first, last or none, not 'middle'")
    );
    assert!(
        refusal(item(&demo, &["place", "99", "first", "--why", "w"])).contains("is not a record")
    );
    assert_eq!(demo.state(), before, "a refused place changed the file");
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
    assert!(
        refusal(item(&demo, &["place", "1", "first", "--why", "w"]))
            .contains("is closed (dropped)")
    );
}

#[test]
fn tags_go_on_with_add_and_edit_open_or_closed_and_list_chooses_by_them() {
    let demo = demo("item-tags");
    add(
        &demo,
        FIVE,
        &["--tag", "screen-reads audit", "--tag", "mac"],
    )
    .unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    let lists = open(&demo);
    assert_eq!(lists.items[0].tags, ["mac", "screen-reads audit"]);
    assert!(lists.items[1].tags.is_empty());
    assert!(
        refusal(add(&demo, "Do the third thing now", &["--tag", " x"]))
            .contains("spaces at its ends")
    );

    item(&demo, &["edit", "2", "--tag", "screen-reads audit"]).unwrap();
    item(&demo, &["edit", "1", "--untag", "mac"]).unwrap();
    let lists = open(&demo);
    assert_eq!(lists.items[0].tags, ["screen-reads audit"]);
    assert_eq!(lists.items[1].tags, ["screen-reads audit"]);
    assert!(refusal(item(&demo, &["edit", "1", "--untag", "mac"])).contains("has no tag 'mac'"));

    // A closed item keeps its tags and still takes and loses them.
    item(&demo, &["drop", "2", "--why", "not needed"]).unwrap();
    item(&demo, &["edit", "2", "--tag", "coven-trust audit"]).unwrap();
    let shown = crate::lists::item_show::render_item(&open(&demo), Id(2), 0).unwrap();
    assert!(
        shown.contains("trail: coven-trust audit; screen-reads audit → #2"),
        "{shown}"
    );
    assert!(
        refusal(item(
            &demo,
            &["edit", "2", "--title", "Do another thing right now"]
        ))
        .contains("closed")
    );

    let line = crate::lists::show::item_line(&open(&demo), &open(&demo).items[0]).unwrap();
    assert!(
        line.contains("\n      trail: screen-reads audit → #1"),
        "{line}"
    );
    let text = crate::lists::show::render(&open(&demo), Some("coven-trust audit")).unwrap();
    assert!(
        text.contains("tagged coven-trust audit") && !text.contains(FIVE),
        "{text}"
    );
    assert!(
        crate::lists::show::render(&open(&demo), Some("screen-reads audit"))
            .unwrap()
            .contains(FIVE)
    );
}

#[test]
fn origins_go_on_with_add_and_edit_open_or_closed_and_the_trail_keeps_the_tags() {
    let demo = demo("item-origins");
    add(&demo, FIVE, &["--tag", "screen-reads audit"]).unwrap();
    add(&demo, "Do the second thing now", &["--origin", "1"]).unwrap();
    add(
        &demo,
        "Do the third thing now",
        &["--tag", "screen-reads audit", "--tag", "mac"],
    )
    .unwrap();
    assert_eq!(open(&demo).items[1].origin, Some(Id(1)));
    assert!(
        refusal(add(&demo, "Do the fourth thing now", &["--origin", "99"]))
            .contains("the origin #99 is not a record")
    );
    assert!(
        refusal(add(
            &demo,
            "Do the fourth thing now",
            &["--origin", "1", "--tag", "screen-reads audit"]
        ))
        .contains("#1, earlier on the trail of #4, already carries the tag 'screen-reads audit'")
    );

    // #1, on #3's new trail, carries the audit, so #3 no longer carries it.
    item(&demo, &["edit", "3", "--origin", "2"]).unwrap();
    let lists = open(&demo);
    assert_eq!(lists.items[2].origin, Some(Id(2)));
    assert_eq!(lists.items[2].tags, ["mac"]);
    let shown = crate::lists::item_show::render_item(&lists, Id(3), 0).unwrap();
    assert!(
        shown.contains("trail: screen-reads audit; mac → #1 → #2 Do the second thing now → #3"),
        "{shown}"
    );
    let line = crate::lists::show::item_line(&lists, &lists.items[1]).unwrap();
    assert!(
        line.contains("\n      trail: screen-reads audit → #1 Do the first thing now → #2"),
        "{line}"
    );
    assert!(
        crate::lists::show::render(&lists, Some("screen-reads audit"))
            .unwrap()
            .contains("Do the third thing now")
    );
    assert!(
        refusal(item(&demo, &["edit", "1", "--origin", "3"]))
            .contains("#1 cannot come from #3: #3 already comes from #1")
    );
    assert!(refusal(item(&demo, &["edit", "1", "--origin", "99"])).contains("is not a record"));

    // A closed item still takes an origin.
    item(&demo, &["drop", "3", "--why", "not needed"]).unwrap();
    item(&demo, &["edit", "3", "--origin", "1"]).unwrap();
    let shown = crate::lists::item_show::render_item(&open(&demo), Id(3), 0).unwrap();
    assert!(
        shown.contains("trail: screen-reads audit; mac → #1 Do the first thing now → #3"),
        "{shown}"
    );
}

/// An item whose work made no commits closes as finished while it runs,
/// saying what it made: its running ends, and an item waiting on it is
/// ready.
#[test]
fn finished_closes_a_running_item_with_what_it_made() {
    let demo = demo("item-finished");
    let side = demo.add_side(1, "agent A", "audit");
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    item(
        &demo,
        &take_args(side.to_str().unwrap(), &brief, &["1"], "audit"),
    )
    .unwrap();
    assert!(refusal(item(&demo, &["finished", "1"])).contains("--made"));
    assert!(
        refusal(item(&demo, &["finished", "1", "--made", " "]))
            .contains("closing_reason_says_something")
    );
    item(&demo, &["finished", "1", "--made", "items 3 and 4"]).unwrap();
    let lists = open(&demo);
    assert!(
        matches!(&lists.closed[0], Closed { record: Record::Item(i), how: How::Finished(made), .. } if i.id == Id(1) && made == "items 3 and 4")
    );
    assert!(lists.items.iter().all(|i| i.running.is_empty()));
    assert!(
        lists.items[0]
            .waits
            .iter()
            .all(|wait| crate::lists::derive::wait_met(&lists, wait))
    );
    assert!(refusal(item(&demo, &["finished", "1", "--made", "again"])).contains("already closed"));
    assert!(refusal(item(&demo, &["finished", "9", "--made", "nothing"])).contains("not a record"));
}

/// Stopping an item that ran where work is left keeps the worktree for it:
/// the item stops running, its worktree loses its claim, `next` lists the
/// worktree, and health says nothing of the unclaimed worktree's commits.
/// Taking the item without --worktree runs it there again.
#[test]
fn stop_keeps_the_worktree_of_unfinished_work_and_take_runs_the_item_there_again() {
    let demo = demo("item-stop-keeps");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    demo.commit(&side, "unlanded");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();

    item(&demo, &["stop", "1", "--why", "the agent ran out of room"]).unwrap();
    let lists = open(&demo);
    let stopped = &lists.items[0];
    assert!(stopped.running.is_empty(), "a stopped item still runs");
    assert_eq!(
        stopped
            .kept
            .iter()
            .map(|p| (p.worktree.as_str(), p.branch.as_str()))
            .collect::<Vec<_>>(),
        [(side_arg.as_str(), "work")]
    );
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none(),
        "the kept worktree kept its agent's claim"
    );
    let next = crate::lists::item_show::next_text(&lists, Audience::Coordinator);
    assert!(
        next.contains(FIVE) && next.contains(&format!("keeps {side_arg} on work")),
        "{next}"
    );
    let health = crate::health::health(&demo.tool).unwrap();
    assert!(
        health
            .findings
            .iter()
            .all(|finding| !finding.advice.text.contains("has no claim")),
        "{:?}",
        health.findings
    );

    crate::claim::cmd_claim(
        &demo.tool.places,
        &[side_arg.clone(), "agent B".into()],
        NOON,
    )
    .unwrap();
    item(&demo, &["take", "1", "--brief", &brief]).unwrap();
    let taken = &open(&demo).items[0];
    assert!(taken.kept.is_empty(), "a taken item kept its worktree");
    assert_eq!(taken.running[0].worktree, side_arg);
}

/// When stop cannot remove a kept worktree's claim, the item already keeps
/// the worktree, the failure names the worktree and the command to run
/// again, and health says the worktree still has a claim. The same stop
/// again removes the claim; a third does nothing.
#[test]
fn a_stop_whose_claim_stayed_finishes_when_run_again_and_then_does_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let demo = demo("item-stop-again");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    demo.commit(&side, "unlanded");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    // The claim file stays while its folder refuses to lose a file.
    let folder = checkout.claim_file().parent().unwrap().to_path_buf();
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = item(&demo, &["stop", "1", "--why", "out of room"]);
    let health = crate::health::health(&demo.tool).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(mode)).unwrap();
    let said = refusal(failed);
    assert!(
        said.contains(&side_arg) && said.contains("builds item stop 1 --why WHY"),
        "{said}"
    );
    assert_eq!(open(&demo).items[0].kept.len(), 1, "the keep was written");
    assert!(open(&demo).items[0].running.is_empty());
    assert!(
        health.findings.iter().any(|finding| {
            finding.advice.text.contains("still has a claim")
                && finding.advice.command.as_deref() == Some("builds item stop 1 --why WHY")
        }),
        "{:?}",
        health.findings
    );

    item(&demo, &["stop", "1", "--why", "out of room"]).unwrap();
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none()
    );
    let before = demo.state();
    item(&demo, &["stop", "1", "--why", "out of room"]).unwrap();
    assert_eq!(demo.state(), before, "a third stop wrote something");
    assert_eq!(open(&demo).items[0].kept.len(), 1);
}

/// A claim made after the item kept the worktree, to take the item, is a new
/// agent's: health does not report it and the same stop leaves it.
#[test]
fn a_claim_made_after_the_keep_is_neither_reported_nor_removed() {
    let demo = demo("item-stop-fresh-claim");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    demo.commit(&side, "unlanded");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    item(&demo, &["stop", "1", "--why", "out of room"]).unwrap();
    crate::claim::write_claim(&checkout, "agent B", demo.tool.now + 600, None).unwrap();
    let health = crate::health::health(&demo.tool).unwrap();
    assert!(
        health
            .findings
            .iter()
            .all(|finding| !finding.advice.text.contains("still has a claim")),
        "{:?}",
        health.findings
    );
    let before = demo.state();
    item(&demo, &["stop", "1", "--why", "out of room"]).unwrap();
    assert_eq!(demo.state(), before);
    let claim = crate::claim::Claim::read(&checkout.claim_file())
        .unwrap()
        .expect("the new agent's claim stayed");
    assert_eq!(claim.who, "agent B");
}

/// Stopping an item whose worktree holds nothing origin/main lacks keeps
/// nothing and leaves the claim.
#[test]
fn stop_keeps_no_worktree_that_holds_no_work() {
    let demo = demo("item-stop-clean");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    item(&demo, &["stop", "1", "--why", "wrong start"]).unwrap();
    assert!(open(&demo).items[0].kept.is_empty());
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_some()
    );
    let said = refusal(item(&demo, &["take", "1", "--brief", &brief]));
    assert!(said.contains("keeps no worktree"), "{said}");
}

/// take refuses another worktree for an item that keeps one and another item
/// for a kept worktree; finished refuses an item that keeps a worktree.
#[test]
fn take_and_finished_refuse_what_would_split_an_item_from_its_kept_work() {
    let demo = demo("item-stop-refuse");
    let one = demo.add_side(1, "agent A", "work");
    let two = demo.add_side(2, "agent B", "other");
    let (one_arg, two_arg) = (one.display().to_string(), two.display().to_string());
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    add(&demo, "Do the third thing now", &[]).unwrap();
    item(&demo, &take_args(&two_arg, &brief, &["2"], "other")).unwrap();
    demo.commit(&two, "unlanded");
    item(&demo, &["stop", "2", "--why", "the agent ran out of room"]).unwrap();
    crate::claim::cmd_claim(
        &demo.tool.places,
        &[two_arg.clone(), "agent C".into()],
        NOON,
    )
    .unwrap();
    let wrong_item = refusal(item(&demo, &take_args(&two_arg, &brief, &["3"], "other")));
    assert!(wrong_item.contains("kept work of #2"), "{wrong_item}");
    let wrong_place = refusal(item(&demo, &take_args(&one_arg, &brief, &["2"], "work")));
    assert!(
        wrong_place.contains(&format!("keeps its work in {two_arg} on other")),
        "{wrong_place}"
    );
    let said = refusal(item(&demo, &["finished", "2", "--made", "nothing"]));
    assert!(said.contains("keeps"), "{said}");
}

/// In a repo two projects hold, an item of the project the claim does not
/// name still holds the worktree: release refuses, and health does not say
/// to release it.
#[test]
fn release_and_health_see_an_item_of_another_project_in_the_worktree() {
    let demo = demo("release-other-project");
    let other = crate::lists::fixtures::make_store_named(&demo.tool.places, "other", &["demo"]);
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    std::fs::write(checkout.project_file(), "demo\n").unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    other
        .write(&crate::lists::fixtures::stamp(NOON), "start", |db| {
            let id = crate::lists::fixtures::item(db, vec![]);
            crate::lists::running::start(
                db,
                &[id],
                &[crate::lists::Running {
                    worktree: side_arg.clone(),
                    branch: "work".into(),
                    brief: "b.md".into(),
                    since: crate::lists::fixtures::stamp(NOON),
                    tier: crate::model_tier::Tier::Implementation,
                }],
            )
        })
        .unwrap();
    let refused = refusal(cmd_release(
        &demo.tool.places,
        std::slice::from_ref(&side_arg),
    ));
    assert!(refused.contains("still runs #1"), "{refused}");
    let health = crate::health::health(&demo.tool).unwrap();
    assert!(
        health
            .findings
            .iter()
            .all(|finding| !finding.advice.text.contains("no item runs there")),
        "{:?}",
        health.findings
    );
}

/// Dropping a running item throws its work away in one step: the closed item
/// records the branch, the commits origin/main lacks and the files not
/// committed, the branch is gone, the worktree is detached at origin/main
/// with nothing left uncommitted, and its claim is gone.
#[test]
fn drop_records_and_throws_away_a_running_items_work() {
    let demo = demo("item-drop-work");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    let sha = demo.commit(&side, "unlanded");
    std::fs::write(side.join("loose"), "x").unwrap();

    item(
        &demo,
        &["drop", "1", "--why", "the person does not want it"],
    )
    .unwrap();
    let lists = open(&demo);
    let Closed {
        record: Record::Item(dropped),
        how: How::Dropped(why),
        ..
    } = &lists.closed[0]
    else {
        panic!("{:?}", lists.closed)
    };
    assert_eq!(why, "the person does not want it");
    assert_eq!(dropped.dropped.len(), 1, "{:?}", dropped.dropped);
    let work = &dropped.dropped[0];
    assert_eq!(
        (work.worktree.as_str(), work.branch.as_str()),
        (side_arg.as_str(), "work")
    );
    assert_eq!(
        work.commits
            .iter()
            .map(|c| c.sha.as_str())
            .collect::<Vec<_>>(),
        [sha.as_str()]
    );
    assert_eq!(work.files, ["loose"]);
    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(
        checkout.branch().unwrap(),
        None,
        "the worktree is still on a branch"
    );
    assert!(
        !side.join("loose").exists(),
        "the files not committed stayed"
    );
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none(),
        "the worktree kept its claim"
    );
    let branches = boards::git_text(
        boards::git(&demo.main).args(["branch", "--list", "work"]),
        "list branches",
    )
    .unwrap();
    assert!(branches.trim().is_empty(), "the branch stayed: {branches}");
}

/// Dropping an item that keeps a worktree throws away the work in it the
/// same way.
#[test]
fn drop_throws_away_a_kept_worktrees_work() {
    let demo = demo("item-drop-kept");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    demo.commit(&side, "unlanded");
    item(&demo, &["stop", "1", "--why", "the agent ran out of room"]).unwrap();
    item(&demo, &["drop", "1", "--why", "no longer wanted"]).unwrap();
    let lists = open(&demo);
    assert!(lists.items.is_empty());
    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(checkout.branch().unwrap(), None);
    assert!(
        crate::side_git::waiting_to_land(&side, "HEAD")
            .unwrap()
            .is_empty()
    );
}

/// An answer whose `blocked` drops an item that keeps a worktree drops it as
/// `item drop` does: the closed item records the work, the branch is gone and
/// the worktree is free.
#[test]
fn an_answer_that_drops_an_item_keeping_a_worktree_throws_its_work_away() {
    let demo = demo("item-drop-by-answer");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    let sha = demo.commit(&side, "unlanded");
    item(&demo, &["stop", "1", "--why", "the agent ran out of room"]).unwrap();
    let ask = words(&[
        "ask",
        "--title",
        "Which_way",
        "--kind",
        "question",
        "--effect",
        "cleanup",
        "--details-file",
        &demo.brief("q.md"),
        "--blocks",
        "1",
    ]);
    let no_gh = |args: &[&str]| -> Result<String> { panic!("gh must not run: {args:?}") };
    crate::lists::question_command::run(&demo.tool, &ask, &demo.scratch.dir, &no_gh).unwrap();
    let request = demo.scratch.dir.join("answer.json");
    std::fs::write(
        &request,
        r#"{"decision": {"title": "Use the plain way", "body": "Because it is plain.", "by": "person"}, "blocked": {"1": {"drop": "not wanted"}}, "no_work": "x"}"#,
    )
    .unwrap();
    let answer = words(&["answer", "2", "--request", &request.display().to_string()]);
    let out = crate::lists::question_command::run(&demo.tool, &answer, &demo.scratch.dir, &no_gh)
        .unwrap();
    assert!(out.contains("threw away the work in"), "{out}");
    let lists = open(&demo);
    let Closed {
        record: Record::Item(dropped),
        how: How::Dropped(_),
        ..
    } = lists.closed_record(Id(1)).unwrap()
    else {
        panic!("{:?}", lists.closed)
    };
    assert_eq!(dropped.dropped[0].commits[0].sha, sha);
    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(checkout.branch().unwrap(), None);
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none(),
        "the worktree kept its claim"
    );
    let branches = boards::git_text(
        boards::git(&demo.main).args(["branch", "--list", "work"]),
        "list branches",
    )
    .unwrap();
    assert!(branches.trim().is_empty(), "the branch stayed: {branches}");
}

/// When an answer drops two items and git fails to throw away the work of
/// both, the refusal names each item and the drop that finishes it.
#[test]
fn an_answer_whose_drops_both_fail_in_git_names_each_item() {
    let demo = demo("item-drop-by-answer-fails");
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    for (n, branch, id) in [(1, "work", "1"), (2, "other", "2")] {
        let side = demo.add_side(n, "agent A", branch);
        let side_arg = side.display().to_string();
        item(&demo, &take_args(&side_arg, &brief, &[id], branch)).unwrap();
        demo.commit(&side, "unlanded");
        item(&demo, &["stop", id, "--why", "the agent ran out of room"]).unwrap();
        std::fs::write(
            Checkout::find(&side_arg)
                .unwrap()
                .git_dir
                .join("index.lock"),
            "",
        )
        .unwrap();
    }
    let ask = words(&[
        "ask",
        "--title",
        "Which_way",
        "--kind",
        "question",
        "--effect",
        "cleanup",
        "--details-file",
        &demo.brief("q.md"),
        "--blocks",
        "1,2",
    ]);
    let no_gh = |args: &[&str]| -> Result<String> { panic!("gh must not run: {args:?}") };
    crate::lists::question_command::run(&demo.tool, &ask, &demo.scratch.dir, &no_gh).unwrap();
    let request = demo.scratch.dir.join("answer.json");
    std::fs::write(
        &request,
        r#"{"decision": {"title": "Use the plain way", "body": "Because it is plain.", "by": "person"}, "blocked": {"1": {"drop": "not wanted"}, "2": {"drop": "not wanted"}}, "no_work": "x"}"#,
    )
    .unwrap();
    let answer = words(&["answer", "3", "--request", &request.display().to_string()]);
    let said = refusal(crate::lists::question_command::run(
        &demo.tool,
        &answer,
        &demo.scratch.dir,
        &no_gh,
    ));
    assert!(
        said.contains("builds item drop 1 --why WHY")
            && said.contains("builds item drop 2 --why WHY"),
        "{said}"
    );
}

/// When git fails to throw the work away, the drop has already closed the
/// item and recorded the work, keeps the worktree's claim, and says to run
/// the drop again; the second drop throws away the rest.
#[test]
fn a_drop_that_git_fails_keeps_its_record_and_finishes_when_run_again() {
    let demo = demo("item-drop-again");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    let sha = demo.commit(&side, "unlanded");
    let checkout = Checkout::find(&side_arg).unwrap();
    // git refuses to check anything out while another command holds the
    // worktree's index.
    let lock = checkout.git_dir.join("index.lock");
    std::fs::write(&lock, "").unwrap();
    let said = refusal(item(&demo, &["drop", "1", "--why", "not wanted"]));
    assert!(
        said.contains("#1 is dropped and its work recorded")
            && said.contains("builds item drop 1 --why WHY"),
        "{said}"
    );
    let lists = open(&demo);
    assert!(lists.items.is_empty(), "the drop left the item open");
    let Closed {
        record: Record::Item(dropped),
        ..
    } = &lists.closed[0]
    else {
        panic!("{:?}", lists.closed)
    };
    assert_eq!(dropped.dropped[0].commits[0].sha, sha);
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_some(),
        "the worktree lost its claim before its work was thrown away"
    );

    std::fs::remove_file(&lock).unwrap();
    item(&demo, &["drop", "1", "--why", "not wanted"]).unwrap();
    assert_eq!(checkout.branch().unwrap(), None);
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none()
    );
    assert_eq!(open(&demo).closed.len(), 1);
}

/// A drop refused for another reason changes nothing in git: the branch and
/// the files stay.
#[test]
fn a_refused_drop_leaves_the_work_where_it_was() {
    let demo = demo("item-drop-refused-work");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    demo.commit(&side, "unlanded");
    std::fs::write(side.join("loose"), "x").unwrap();
    let before = demo.state();
    assert!(refusal(item(&demo, &["drop", "1", "--why", "no"])).contains("waits on #1"));
    assert_eq!(demo.state(), before);
    assert!(side.join("loose").exists());
    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("work"));
}

/// Drop refuses an item that shares its worktree with another running item,
/// since the work there is that item's too: the branch, its commits and the
/// claim stay. Once `item stop` ends the item's run, drop closes it and
/// leaves the worktree to the other item.
#[test]
fn drop_refuses_an_item_whose_worktree_runs_another_item() {
    let demo = demo("item-drop-shared");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1", "2"], "work")).unwrap();
    let sha = demo.commit(&side, "unlanded");
    let before = demo.state();

    let said = refusal(item(&demo, &["drop", "1", "--why", "not wanted"]));
    assert!(
        said.contains("#2 \"Do the second thing now\"")
            && said.contains("builds item stop 1 --why WHY"),
        "{said}"
    );
    assert_eq!(demo.state(), before, "a refused drop changed the store");
    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("work"));
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_some(),
        "a refused drop removed the claim"
    );

    item(&demo, &["stop", "1", "--why", "not wanted"]).unwrap();
    item(&demo, &["drop", "1", "--why", "not wanted"]).unwrap();
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("work"));
    assert_eq!(
        crate::side_git::waiting_to_land(&side, "HEAD").unwrap()[0].full,
        sha,
        "the drop threw away the other item's commit"
    );
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_some(),
        "the drop removed the claim of the other item's agent"
    );
    let lists = open(&demo);
    assert_eq!(lists.items.len(), 1);
    assert_eq!(lists.items[0].running[0].worktree, side_arg);
}

/// Runs release on <side>, which the test expects release to refuse, and
/// returns what release said.
fn release_refused(demo: &Demo, side: &std::path::Path) -> String {
    refusal(cmd_release(
        &demo.tool.places,
        &[side.display().to_string()],
    ))
}

/// Release refuses a worktree whose branch holds commits origin/main lacks,
/// names them and the commands that keep the work, and keeps the claim.
#[test]
fn release_refuses_a_worktree_whose_branch_holds_unlanded_commits() {
    let demo = demo("release-branch");
    let side = demo.add_side(1, "agent A", "work");
    demo.commit(&side, "unlanded");
    let said = release_refused(&demo, &side);
    assert!(
        said.contains("branch work: ")
            && said.contains(" unlanded")
            && said.contains("builds land")
            && said.contains("builds item take")
            && said.contains("--held"),
        "{said}"
    );
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_some()
    );
}

/// Release refuses a worktree with files not committed, and names them.
#[test]
fn release_refuses_a_worktree_with_uncommitted_files() {
    let demo = demo("release-files");
    let side = demo.add_side(1, "agent A", "work");
    std::fs::write(side.join("loose"), "x").unwrap();
    let said = release_refused(&demo, &side);
    assert!(said.contains("1 file not committed: loose"), "{said}");
}

/// Switching the worktree off its branch does not hide the branch's work:
/// release names the branch it left. Release also refuses a detached HEAD
/// past origin/main with no branch.
#[test]
fn release_refuses_a_branch_the_worktree_left_and_detached_commits() {
    let demo = demo("release-left");
    let side = demo.add_side(1, "agent A", "work");
    demo.commit(&side, "unlanded");
    crate::testing::git(&side, &["switch", "-q", "--detach", "main"]);
    let said = release_refused(&demo, &side);
    assert!(said.contains("branch work, which it left: "), "{said}");

    let other = demo.add_side(2, "agent B", "other");
    crate::testing::git(&other, &["switch", "-q", "--detach"]);
    demo.commit(&other, "detached");
    let said = release_refused(&demo, &other);
    assert!(said.contains("detached HEAD: "), "{said}");
}

/// While a `git commit` runs in a worktree (its hook can wait for minutes),
/// release refuses it and health does not say to release it; once the
/// commit ends with nothing committed, both let it go.
#[test]
fn release_and_health_wait_for_a_git_commit_that_runs_in_the_worktree() {
    let demo = demo("release-committing");
    let side = demo.add_side(1, "agent A", "work");
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    let hooks = demo.scratch.dir.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let gate = demo.scratch.dir.join("gate");
    let started = demo.scratch.dir.join("started");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&gate)
            .arg(&started)
            .status()
            .unwrap()
            .success()
    );
    // The hook writes to `started`, which blocks until the test reads it,
    // waits for the test to write to `gate`, and then fails, so the commit
    // commits nothing.
    let hook = hooks.join("pre-commit");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho started > '{}'\nread line < '{}'\nexit 1\n",
            started.display(),
            gate.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut commit = std::process::Command::new("git")
        .arg("-C")
        .arg(&side)
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(["commit", "-q", "--allow-empty", "-m", "waits"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&started).unwrap(), "started\n");
    let releases = |health: &crate::health::Health| {
        health
            .findings
            .iter()
            .any(|finding| finding.advice.text.contains("no item runs there"))
    };
    let said = release_refused(&demo, &side);
    assert!(said.contains("a git commit runs in it (pid "), "{said}");
    assert!(!releases(&crate::health::health(&demo.tool).unwrap()));

    std::fs::write(&gate, "go\n").unwrap();
    assert!(
        !commit.wait().unwrap().success(),
        "the hook let the commit through"
    );
    assert!(releases(&crate::health::health(&demo.tool).unwrap()));
    cmd_release(&demo.tool.places, &[side.display().to_string()]).unwrap();
}

/// Release removes the claim of a worktree whose work an item keeps, or
/// whose work landed.
#[test]
fn release_lets_go_of_kept_or_landed_work() {
    let demo = demo("release-ok");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let sha = demo.commit(&side, "landed");
    crate::testing::git(&demo.main, &["cherry-pick", &sha]);
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&side, &["fetch", "-q", "origin"]);
    cmd_release(&demo.tool.places, std::slice::from_ref(&side_arg)).unwrap();

    let two = demo.add_side(2, "agent B", "other");
    let two_arg = two.display().to_string();
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&two_arg, &brief, &["1"], "other")).unwrap();
    demo.commit(&two, "unlanded");
    item(&demo, &["stop", "1", "--why", "the agent ran out of room"]).unwrap();
    crate::claim::cmd_claim(
        &demo.tool.places,
        &[two_arg.clone(), "agent C".into()],
        NOON,
    )
    .unwrap();
    cmd_release(&demo.tool.places, &[two_arg]).unwrap();
}

/// Commits <sha> of <from> onto a new worktree <n>, on branch <branch>, as
/// a cherry-pick, so the copy has the same patch and another hash.
fn copy_to_side(demo: &Demo, n: usize, branch: &str, sha: &str) -> std::path::PathBuf {
    let side = demo.add_side(n, "agent B", branch);
    crate::testing::git(&side, &["cherry-pick", sha]);
    side
}

/// Release frees a worktree whose commits origin/main lacks all have the same
/// patch as commits on a branch that another worktree checks out, and says
/// which branch keeps them. Health says to release it too, and a worktree
/// with one commit that no other branch holds is refused as before.
#[test]
fn release_frees_a_worktree_whose_commits_are_copies_on_another_branch() {
    let demo = demo("release-copies");
    let first = demo.add_side(1, "agent A", "staging");
    let sha = demo.commit(&first, "copied");
    let copy = copy_to_side(&demo, 2, "work", &sha);
    let checkout = Checkout::find(&copy.display().to_string()).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
    let advises = |label: &str| {
        crate::health::health(&demo.tool)
            .unwrap()
            .findings
            .iter()
            .any(|finding| {
                finding.advice.text.contains("no item runs there")
                    && finding.advice.text.contains(label)
            })
    };
    assert!(advises("demo/side-2"));

    let extra = demo.add_side(3, "agent C", "extra");
    demo.commit(&extra, "held by no one");
    crate::testing::git(&extra, &["cherry-pick", &sha]);
    let said = release_refused(&demo, &extra);
    assert!(
        said.contains("branch extra: ") && said.contains("held by no one"),
        "{said}"
    );
    assert!(!said.contains(" copied"), "{said}");

    let kept = crate::kept::read(
        &crate::lists::store::every_lists(&demo.tool.places).unwrap(),
        &Checkout::find(&copy.display().to_string()).unwrap(),
        &crate::procs::git_commits(&crate::procs::Processes::read().unwrap()),
    )
    .unwrap();
    assert!(kept.unheld.is_empty());
    assert_eq!(
        kept.unheld.copied_on.iter().collect::<Vec<_>>(),
        ["staging"]
    );
    cmd_release(&demo.tool.places, &[copy.display().to_string()]).unwrap();
    let checkout = Checkout::find(&copy.display().to_string()).unwrap();
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none()
    );
}

/// The findings about loose branches that health reports for <demo>.
fn branch_findings(demo: &Demo) -> Vec<crate::health::Finding> {
    crate::health::health(&demo.tool)
        .unwrap()
        .findings
        .into_iter()
        .filter(|f| f.kind == crate::health::FindingKind::Branch)
        .collect()
}

/// A branch `loose` in <demo>'s repo with one commit origin/main lacks,
/// which no worktree checks out; returns the commit. Sets up the main
/// checkout's build root, which is how health finds the repo.
fn add_loose_branch(demo: &Demo) -> String {
    let main = Checkout::find(&demo.main.display().to_string()).unwrap();
    crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &main).unwrap();
    crate::testing::git(&demo.main, &["switch", "-q", "-c", "loose", "origin/main"]);
    let sha = demo.commit(&demo.main, "unlanded");
    crate::testing::git(&demo.main, &["switch", "-q", "main"]);
    sha
}

/// Health lists a branch with commits origin/main lacks that no worktree
/// checks out and no item names, and
/// stops listing it once a worktree checks it out again or the branch lands.
#[test]
fn health_lists_a_loose_branch_until_a_worktree_takes_it_or_it_lands() {
    let demo = demo("health-loose");
    let sha = add_loose_branch(&demo);
    let found = branch_findings(&demo);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .advice
            .text
            .starts_with("branch: demo loose holds 1 commit"),
        "{found:?}"
    );
    assert_eq!(found[0].advice.command, None, "the row has no control");

    crate::worktree::cmd_new(
        &demo.tool,
        &crate::testing::no_ssh(),
        &[
            demo.main.display().to_string(),
            "--resume".into(),
            "loose".into(),
            "--project".into(),
            "demo".into(),
        ],
    )
    .unwrap();
    assert!(
        branch_findings(&demo).is_empty(),
        "a worktree checks it out"
    );

    let side = crate::checkout::worktree_list(&demo.main)
        .unwrap()
        .into_iter()
        .find(|listed| listed.branch.as_deref() == Some("loose"))
        .unwrap()
        .top;
    crate::testing::git(&side, &["switch", "-q", "--detach", "origin/main"]);
    assert_eq!(branch_findings(&demo).len(), 1, "the branch is loose again");
    crate::testing::git(&demo.main, &["cherry-pick", &sha]);
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&demo.main, &["fetch", "-q", "origin"]);
    assert!(branch_findings(&demo).is_empty(), "the branch landed");
}

/// An open item of any project that names a loose branch in one of its
/// running places holds it, so health does not list the branch.
#[test]
fn health_leaves_a_branch_an_item_of_another_project_names() {
    let demo = demo("health-loose-other");
    add_loose_branch(&demo);
    let other = crate::lists::fixtures::make_store_named(&demo.tool.places, "other", &["demo"]);
    other
        .write(&crate::lists::fixtures::stamp(NOON), "start", |db| {
            let id = crate::lists::fixtures::item(db, vec![]);
            crate::lists::running::start(
                db,
                &[id],
                &[crate::lists::Running {
                    worktree: demo
                        .scratch
                        .dir
                        .join("demo-worktrees/side-1")
                        .display()
                        .to_string(),
                    branch: "loose".into(),
                    brief: "b.md".into(),
                    since: crate::lists::fixtures::stamp(NOON),
                    tier: crate::model_tier::Tier::Implementation,
                }],
            )
        })
        .unwrap();
    let found = branch_findings(&demo);
    assert!(found.is_empty(), "{found:?}");
}
