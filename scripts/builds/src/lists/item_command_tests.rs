use super::*;
use crate::checkout::Checkout;
use crate::claim::cmd_release;
use crate::lists::draft::brief_base;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::{Closed, Record};

fn item(demo: &Demo, args: &[&str]) -> Result<()> {
    cmd_item(&demo.tool, &words(args))
}

fn add(demo: &Demo, title: &str, extra: &[&str]) -> Result<()> {
    let details = demo.brief("details.txt");
    let mut args = vec!["add", "--title", title, "--area", "Alpha", "--details-file", &details];
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
    let before = demo.bytes();
    assert!(refusal(add(&demo, "Too short", &[])).contains("2 words"));
    let details = demo.brief("d");
    let no_area = refusal(item(&demo, &["add", "--title", FIVE, "--area", "Nowhere", "--details-file", &details]));
    assert!(no_area.contains("does not exist") && no_area.contains("Alpha"), "{no_area}");
    assert!(refusal(add(&demo, FIVE, &["--brief", "/nonexistent/brief.md"])).contains("does not exist"));
    assert!(refusal(add(&demo, FIVE, &["--wait", "item:99"])).contains("no record has"));
    assert!(refusal(add(&demo, FIVE, &["--priority", "first"])).contains("needs --why"));
    assert!(refusal(add(&demo, FIVE, &["--why", "x"])).contains("belongs to --priority"));
    assert!(refusal(add(&demo, FIVE, &["--wait", "bogus"])).contains("is not a wait"));
    assert_eq!(demo.bytes(), before, "a refused add changed the file");
}

#[test]
fn add_takes_waits_a_brief_a_priority_and_the_design_sessions_audience() {
    let demo = demo("item-add-all");
    add(&demo, FIVE, &[]).unwrap();
    let brief = demo.brief("brief.md");
    add(&demo, "Do the second thing now", &["--wait", "item:1", "--wait", "condition:a quiet machine", "--brief", &brief, "--priority", "first", "--why", "person said so", "--for", "design-session"]).unwrap();
    let second = &open(&demo).items[1];
    assert_eq!(second.waits, [Wait::Item(Id(1)), Wait::Condition("a quiet machine".into())]);
    assert_eq!((second.brief.as_deref(), second.r#for), (Some(brief.as_str()), Audience::DesignSession));
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
    item(&demo, &["edit", "1", "--area", "Beta", "--title", "Do the renamed thing now"]).unwrap();
    assert_eq!(open(&demo).items[0].area, "Beta");
    assert!(refusal(item(&demo, &["edit", "1"])).contains("changes nothing"));
    assert!(refusal(item(&demo, &["edit", "1", "--area", "Gamma"])).contains("does not exist"));
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
    assert!(refusal(item(&demo, &["edit", "1", "--area", "Alpha"])).contains("is closed (dropped)"));
}

#[test]
fn note_adds_a_line_and_wants_text() {
    let demo = demo("item-note");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &["note", "1", "R1", "landed", "today"]).unwrap();
    let note = &open(&demo).items[0].notes[0];
    assert_eq!((note.text.as_str(), note.by.as_str()), ("R1 landed today", "tester"));
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
    let before = demo.bytes();
    let cycle = refusal(item(&demo, &["wait", "1", "--on", "item:2"]));
    assert!(cycle.contains("in a circle"), "{cycle}");
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:1"])).contains("cannot wait on itself"));
    assert!(refusal(item(&demo, &["wait", "2", "--on", "item:1"])).contains("already waits"));
    assert!(refusal(item(&demo, &["wait", "1", "--off", "item:2"])).contains("does not wait on item:2"));
    assert!(refusal(item(&demo, &["wait", "1"])).contains("exactly one of --on REF and --off REF"));
    assert_eq!(demo.bytes(), before);
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
        assert_eq!((running.worktree.as_str(), running.branch.as_str(), running.brief.as_str()), (side_arg.as_str(), "work", brief.as_str()));
    }
    assert!(!std::fs::read_to_string(demo.store.state_path()).unwrap().contains("agent A"));
    assert!(crate::lists::running::agent_of(lists.items[0].running.first().unwrap()).unwrap() == crate::lists::running::Agent::Claimed("agent A".into()));
}

#[test]
fn take_refuses_what_is_not_ready_or_not_right() {
    let demo = demo("item-take-refuse");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    add(&demo, "Do the design sessions thing", &["--for", "design-session"]).unwrap();
    let before = demo.bytes();
    let waits = refusal(item(&demo, &take_args(&side_arg, &brief, &["2"], "work")));
    assert!(waits.contains("2 waits on item #1"), "{waits}");
    assert!(refusal(item(&demo, &take_args(&side_arg, &brief, &["3"], "work"))).contains("--as design-session"));
    let mut as_design = take_args(&side_arg, &brief, &["1"], "work");
    as_design.extend(["--as", "design-session"]);
    assert!(refusal(item(&demo, &as_design)).contains("takes only the design session's"));
    assert!(refusal(item(&demo, &take_args(&side_arg, &brief, &["1"], "nobranch"))).contains("has no branch nobranch"));
    assert!(refusal(item(&demo, &take_args(&side_arg, "/no/brief.md", &["1"], "work"))).contains("does not exist"));
    assert!(refusal(item(&demo, &take_args(&side_arg, &brief, &["1", "1"], "work"))).contains("named twice"));
    let main = demo.main.display().to_string();
    assert!(refusal(item(&demo, &take_args(&main, &brief, &["1"], "main"))).contains("not one of the repo's side worktrees"));
    assert_eq!(demo.bytes(), before);

    let unclaimed = demo.add_side(2, "x", "other");
    cmd_release(&demo.tool.places, &[unclaimed.display().to_string()]).unwrap();
    let no_claim = refusal(item(&demo, &take_args(&unclaimed.display().to_string(), &brief, &["1"], "other")));
    assert!(no_claim.contains("has no claim"), "{no_claim}");

    item(&demo, &take_args(&side_arg, &brief, &["1"], "work")).unwrap();
    assert!(refusal(item(&demo, &take_args(&side_arg, &brief, &["1"], "work"))).contains("already runs in"));
    demo.add_side_branch(&side, "second");
    add(&demo, "Do the fourth thing now", &[]).unwrap();
    assert!(refusal(item(&demo, &take_args(&side_arg, &brief, &["4"], "second"))).contains("holds one branch of work"));
}

#[test]
fn a_design_session_item_is_taken_as_the_design_session() {
    let demo = demo("item-take-design");
    let side = demo.add_side(1, "design", "canvas");
    add(&demo, "Do the design sessions thing", &["--for", "design-session"]).unwrap();
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
    item(&demo, &take_args(side.to_str().unwrap(), &brief, &["1"], "work")).unwrap();
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
    item(&demo, &take_args(side.to_str().unwrap(), &brief, &["1"], "work")).unwrap();
    let side_arg = side.display().to_string();
    let refused = refusal(cmd_release(&demo.tool.places, std::slice::from_ref(&side_arg)));
    assert!(refused.contains("still runs #1") && refused.contains("item stop 1"), "{refused}");
    assert!(crate::claim::Claim::read(&Checkout::find(&side_arg).unwrap().claim_file()).unwrap().is_some());
    item(&demo, &["stop", "1", "--why", "done here"]).unwrap();
    cmd_release(&demo.tool.places, &[side_arg]).unwrap();
}

#[test]
fn drop_refuses_a_waited_on_running_or_last_blocking_item() {
    let demo = demo("item-drop");
    add(&demo, FIVE, &[]).unwrap();
    add(&demo, "Do the second thing now", &["--wait", "item:1"]).unwrap();
    let before = demo.bytes();
    let waited = refusal(item(&demo, &["drop", "1", "--why", "no"]));
    assert!(waited.contains("2 \"Do the second thing now\" waits on #1") && waited.contains("item wait 2 --off item:1"), "{waited}");
    assert_eq!(demo.bytes(), before);
    item(&demo, &["drop", "2", "--why", "not needed"]).unwrap();
    let lists = open(&demo);
    assert!(matches!(&lists.closed[0], Closed { record: Record::Item(i), how: How::Dropped(w), .. } if i.id == Id(2) && w == "not needed"));
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
}

#[test]
fn drop_refuses_the_last_item_a_question_blocks_and_a_running_item() {
    let demo = demo("item-drop-last");
    let mut lists = demo.store.load().unwrap();
    let blocked = crate::lists::fixtures::item(&mut lists, vec![]);
    let q = crate::lists::fixtures::question(&mut lists);
    let first = lists.questions[0].id;
    crate::lists::fixtures::blocks_something(&mut lists, first);
    crate::lists::item::add_wait(&mut lists, blocked, Wait::Question(q)).unwrap();
    // Written through the store so the file stays valid.
    demo.store.with_write(&crate::lists::fixtures::stamp(1), "seed", |on_disk| { *on_disk = Lists { version: on_disk.version, log: on_disk.log.clone(), ..lists.clone() }; Ok(()) }).unwrap();
    let refused = refusal(item(&demo, &["drop", "1", "--why", "no"]));
    assert!(refused.contains("last item question #2") && refused.contains("--blocks-nothing"), "{refused}");

}

#[test]
fn drop_refuses_a_running_item() {
    let demo = demo("item-drop-running");
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &take_args(side.to_str().unwrap(), &brief, &["1"], "work")).unwrap();
    assert!(refusal(item(&demo, &["drop", "1", "--why", "no"])).contains("item stop 1"));
}

#[test]
fn defer_wants_an_open_issue() {
    let demo = demo("item-defer");
    add(&demo, FIVE, &[]).unwrap();
    let cwd = current_dir().unwrap();
    let args = |url: &str| words(&["1", "--issue", url]);
    let closed = |_: &[&str]| Ok(r#"{"state":"CLOSED"}"#.to_string());
    let open_issue = |_: &[&str]| Ok(r#"{"state":"OPEN"}"#.to_string());
    let before = demo.bytes();
    assert!(refusal(defer(&demo.tool, &args("https://x/1"), &cwd, &closed)).contains("is CLOSED"));
    assert_eq!(demo.bytes(), before);
    defer(&demo.tool, &args("https://x/1"), &cwd, &open_issue).unwrap();
    assert!(matches!(&open(&demo).closed[0].how, How::Deferred(url) if url == "https://x/1"));
}

#[test]
fn place_moves_an_open_item_first_or_last_or_clears_it_and_logs_the_write() {
    let demo = demo("item-place");
    add(&demo, FIVE, &[]).unwrap();
    item(&demo, &["place", "1", "first", "--why", "top priority"]).unwrap();
    let placed = open(&demo).items[0].priority.clone().unwrap();
    assert_eq!((placed.level, placed.why.as_str()), (Level::First, "top priority"));
    assert!(open(&demo).log.last().unwrap().words.starts_with("item place #1 first"));
    item(&demo, &["place", "1", "last", "--why", "later"]).unwrap();
    assert_eq!(open(&demo).items[0].priority.as_ref().unwrap().level, Level::Last);
    item(&demo, &["place", "1", "none", "--why", "back to the order"]).unwrap();
    assert_eq!(open(&demo).items[0].priority, None);
}

#[test]
fn place_refuses_a_closed_item_a_missing_why_and_an_unknown_place() {
    let demo = demo("item-place-refuse");
    add(&demo, FIVE, &[]).unwrap();
    let before = demo.bytes();
    assert!(refusal(item(&demo, &["place", "1", "first"])).contains("--why is required"));
    assert!(refusal(item(&demo, &["place", "1", "first", "--why", " "])).contains("--why is empty"));
    assert!(refusal(item(&demo, &["place", "1", "middle", "--why", "w"])).contains("first, last or none, not 'middle'"));
    assert!(refusal(item(&demo, &["place", "99", "first", "--why", "w"])).contains("is not a record"));
    assert_eq!(demo.bytes(), before, "a refused place changed the file");
    item(&demo, &["drop", "1", "--why", "no"]).unwrap();
    assert!(refusal(item(&demo, &["place", "1", "first", "--why", "w"])).contains("is closed (dropped)"));
}
