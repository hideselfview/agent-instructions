use super::*;
use crate::checkout::Checkout;
use crate::claim::cmd_release;
use crate::lists::draft::brief_base;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
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
        [Wait::Item(Id(1)), Wait::Condition("a quiet machine".into())]
    );
    assert_eq!(
        (second.brief.as_deref(), second.r#for),
        (Some(brief.as_str()), Audience::DesignSession)
    );
    assert_eq!(second.priority.as_ref().unwrap().level, Level::First);
}

#[test]
fn a_relative_brief_is_found_under_the_main_checkout_of_the_repo_run_in() {
    let demo = demo("item-brief-base");
    std::fs::create_dir_all(demo.main.join("plans")).unwrap();
    std::fs::write(demo.main.join("plans/b.md"), "x").unwrap();
    assert_eq!(brief_base(&demo.main).unwrap(), demo.main);
    let side = demo.add_side(1, "agent", "br");
    assert_eq!(brief_base(&side).unwrap(), demo.main);
    assert!(refusal(brief_base(&demo.scratch.dir)).contains("in no git checkout"));
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
    item(&demo, &["wait", "2", "--on", "condition:a quiet machine"]).unwrap();
    item(&demo, &["wait", "2", "--off", "condition:a quiet machine"]).unwrap();
    assert!(open(&demo).items[1].waits.is_empty());
}

fn take_args<'a>(side: &'a str, brief: &'a str, ids: &[&'a str], branch: &'a str) -> Vec<&'a str> {
    let mut args = vec!["take"];
    args.extend_from_slice(ids);
    args.extend(["--worktree", side, "--branch", branch, "--brief", brief]);
    args
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
            crate::lists::fixtures::blocks_something(db, q);
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
    let shown = crate::lists::item_show::render_item(&open(&demo), Id(2)).unwrap();
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
    let shown = crate::lists::item_show::render_item(&lists, Id(3)).unwrap();
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
    let shown = crate::lists::item_show::render_item(&open(&demo), Id(3)).unwrap();
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

fn park_args<'a>(id: &'a str, side: &'a str, branch: &'a str) -> Vec<&'a str> {
    vec!["park", id, "--worktree", side, "--branch", branch]
}

/// When park cannot remove the worktree's claim after it parked the item,
/// the same park again removes it.
#[test]
fn a_park_whose_claim_stayed_finishes_when_run_again() {
    use std::os::unix::fs::PermissionsExt;
    let demo = demo("item-park-again");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    // The claim file stays while its folder refuses to lose a file.
    let folder = checkout.claim_file().parent().unwrap().to_path_buf();
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = item(&demo, &park_args("1", &side_arg, "work"));
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(mode)).unwrap();
    assert!(refusal(failed).contains("removing"));
    assert_eq!(
        open(&demo).items[0].parked.len(),
        1,
        "the park was not written"
    );

    item(&demo, &park_args("1", &side_arg, "work")).unwrap();
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none()
    );
    assert_eq!(open(&demo).items[0].parked.len(), 1);
}

/// A running item parks where it runs: it stops running, its worktree loses
/// its claim, `next` lists it with the worktree, and health says nothing of
/// the unclaimed worktree's commits. Taking it there runs it again.
#[test]
fn park_keeps_the_work_for_the_queue_and_take_runs_it_there_again() {
    let demo = demo("item-park");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let checkout = Checkout::find(&side_arg).unwrap();
    crate::setup::setup(&demo.tool, &checkout).unwrap();
    demo.commit(&side, "unlanded");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("b.md");
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();

    item(&demo, &park_args("1", &side_arg, "work")).unwrap();
    let lists = open(&demo);
    let parked = &lists.items[0];
    assert!(parked.running.is_empty(), "a parked item still runs");
    assert_eq!(
        parked
            .parked
            .iter()
            .map(|p| (p.worktree.as_str(), p.branch.as_str()))
            .collect::<Vec<_>>(),
        [(side_arg.as_str(), "work")]
    );
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none(),
        "the parked worktree kept its agent's claim"
    );
    let next = crate::lists::item_show::next_text(&lists);
    assert!(
        next.contains(FIVE) && next.contains(&format!("parked in {side_arg} on work")),
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
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    let taken = &open(&demo).items[0];
    assert!(taken.parked.is_empty(), "a taken item stayed parked");
    assert_eq!(taken.running[0].worktree, side_arg);
}

/// A queued item that never ran parks too; park refuses an item running
/// elsewhere, one parked already and a worktree another item runs in; take
/// refuses another worktree for a parked item and another item for a parked
/// worktree; finished refuses a parked item.
#[test]
fn park_and_take_refuse_what_would_split_an_item_from_its_work() {
    let demo = demo("item-park-refuse");
    let one = demo.add_side(1, "agent A", "work");
    let two = demo.add_side(2, "agent B", "other");
    let (one_arg, two_arg) = (one.display().to_string(), two.display().to_string());
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &[]).unwrap();
    add(&demo, "Do the third thing now", &[]).unwrap();
    item(&demo, &take_args(&one_arg, &brief, &["1"], "work")).unwrap();
    let before = demo.state();
    let elsewhere = refusal(item(&demo, &park_args("1", &two_arg, "other")));
    assert!(
        elsewhere.contains(&format!("runs in {one_arg} on work")),
        "{elsewhere}"
    );
    let busy = refusal(item(&demo, &park_args("2", &one_arg, "work")));
    assert!(busy.contains("still runs #1"), "{busy}");
    assert_eq!(demo.state(), before);

    item(&demo, &park_args("2", &two_arg, "other")).unwrap();
    // Parking it there again writes nothing: it only removes claims left.
    let parked = demo.state();
    item(&demo, &park_args("2", &two_arg, "other")).unwrap();
    assert_eq!(demo.state(), parked);
    let other_item = refusal(item(&demo, &park_args("3", &two_arg, "other")));
    assert!(other_item.contains("holds #2"), "{other_item}");
    crate::claim::cmd_claim(
        &demo.tool.places,
        &[two_arg.clone(), "agent C".into()],
        NOON,
    )
    .unwrap();
    let wrong_item = refusal(item(&demo, &take_args(&two_arg, &brief, &["3"], "other")));
    assert!(wrong_item.contains("holds #2"), "{wrong_item}");
    let wrong_place = refusal(item(&demo, &take_args(&one_arg, &brief, &["2"], "work")));
    assert!(
        wrong_place.contains(&format!("is parked in {two_arg} on other")),
        "{wrong_place}"
    );
    let said = refusal(item(&demo, &["finished", "2", "--made", "nothing"]));
    assert!(said.contains("is parked in"), "{said}");
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
    crate::setup::setup(&demo.tool, &checkout).unwrap();
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
    let branches = crate::checkout::git_text(
        crate::checkout::git(&demo.main).args(["branch", "--list", "work"]),
        "list branches",
    )
    .unwrap();
    assert!(branches.trim().is_empty(), "the branch stayed: {branches}");
}

/// Dropping a parked item throws away the work in its worktree the same way.
#[test]
fn drop_throws_away_a_parked_items_work() {
    let demo = demo("item-drop-parked");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    demo.commit(&side, "unlanded");
    item(&demo, &park_args("1", &side_arg, "work")).unwrap();
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
            && said.contains("builds item park")
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
    crate::setup::setup(&demo.tool, &checkout).unwrap();
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

/// Release removes the claim of a worktree whose work an item holds parked,
/// or whose work landed.
#[test]
fn release_lets_go_of_parked_or_landed_work() {
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
    demo.commit(&two, "unlanded");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &park_args("1", &two_arg, "other")).unwrap();
    crate::claim::cmd_claim(
        &demo.tool.places,
        &[two_arg.clone(), "agent C".into()],
        NOON,
    )
    .unwrap();
    cmd_release(&demo.tool.places, &[two_arg]).unwrap();
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
    crate::setup::setup(&demo.tool, &main).unwrap();
    crate::testing::git(&demo.main, &["switch", "-q", "-c", "loose", "origin/main"]);
    let sha = demo.commit(&demo.main, "unlanded");
    crate::testing::git(&demo.main, &["switch", "-q", "main"]);
    sha
}

/// Health lists a branch with commits origin/main lacks that no worktree
/// checks out and no item names, with the command that checks it out, and
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
    assert_eq!(
        found[0].advice.command.as_deref(),
        Some(format!("builds new {} --resume loose", demo.main.display()).as_str())
    );

    crate::worktree::cmd_new(
        &demo.tool,
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
                }],
            )
        })
        .unwrap();
    let found = branch_findings(&demo);
    assert!(found.is_empty(), "{found:?}");
}
