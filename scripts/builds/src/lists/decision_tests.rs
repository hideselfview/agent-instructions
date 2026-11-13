use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, item, lists, stamp};
use crate::lists::{Lists, PlanRef, Wait};
use crate::testing::Scratch;

fn refusal(result: Result<()>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn run(tool: &Tool, command: &str) -> Result<()> {
    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    cmd_decision(tool, &words)
}

/// A tool whose clock is `NOON` and whose store `demo` holds <lists>.
fn tool_with(scratch: &Scratch, lists: Lists) -> Tool {
    let mut tool = scratch.tool(0);
    tool.now = NOON;
    Store::named(&tool.places, "demo").unwrap().create(lists, &stamp(NOON), "lists init demo").unwrap();
    tool
}

fn state(tool: &Tool) -> Lists {
    Store::named(&tool.places, "demo").unwrap().load().unwrap()
}

fn bytes(tool: &Tool) -> Vec<u8> {
    std::fs::read(tool.places.lists.join("demo/lists.json")).unwrap()
}

fn request(scratch: &Scratch, name: &str, json: &str) -> String {
    let path = scratch.dir.join(name);
    std::fs::write(&path, json).unwrap();
    path.display().to_string()
}

const BASE: &str = r#""title": "Use one store", "by": "person", "body": "One store, because two drift.""#;

fn add(tool: &Tool, scratch: &Scratch, name: &str, json: &str) -> Result<()> {
    let file = request(scratch, name, json);
    run(tool, &format!("add --request {file} --project demo --by tester"))
}

#[test]
fn add_records_the_decision_dated_today_with_no_work() {
    let scratch = Scratch::new("decision-add");
    let tool = tool_with(&scratch, lists());
    add(&tool, &scratch, "r.json", &format!(r#"{{{BASE}, "no_work": "nothing to build"}}"#)).unwrap();
    let after = state(&tool);
    let decision = &after.decisions[0];
    assert_eq!((decision.title.as_str(), decision.by.as_str()), ("Use one store", "person"));
    assert_eq!(decision.date, crate::text::local_day(NOON));
    assert_eq!((decision.question, decision.replaces, decision.work.len()), (None, None, 0));
    assert_eq!((decision.recorded.by.as_str(), after.version), ("tester", 3));
    assert!(after.log.last().unwrap().words.starts_with("decision add"));
}

#[test]
fn add_makes_its_work_from_the_decision() {
    let scratch = Scratch::new("decision-work");
    let tool = tool_with(&scratch, lists());
    let work = r#"[{"title": "Build the one store now", "area": "Alpha", "details": "d", "for": "coordinator",
                    "waits": [{"condition": "a quiet machine"}], "brief": null, "priority": null}]"#;
    add(&tool, &scratch, "r.json", &format!(r#"{{{BASE}, "work": {work}}}"#)).unwrap();
    let after = state(&tool);
    let decision = &after.decisions[0];
    assert_eq!(decision.work, [after.items[0].id]);
    assert_eq!(after.items[0].from, Some(decision.id));
    assert_eq!(after.items[0].waits, [Wait::Condition("a quiet machine".into())]);
}

#[test]
fn add_refuses_work_and_no_work_together_or_neither() {
    let scratch = Scratch::new("decision-workwhy");
    let tool = tool_with(&scratch, lists());
    let before = bytes(&tool);
    let both = refusal(add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "work": [], "no_work": "none"}}"#)));
    assert!(both.contains("not both"), "{both}");
    let neither = refusal(add(&tool, &scratch, "b.json", &format!("{{{BASE}}}")));
    assert!(neither.contains("`work`") && neither.contains("`no_work`"), "{neither}");
    let empty = refusal(add(&tool, &scratch, "c.json", &format!(r#"{{{BASE}, "no_work": " "}}"#)));
    assert!(empty.contains("needs its reason"), "{empty}");
    assert_eq!(bytes(&tool), before);
}

#[test]
fn add_refuses_an_empty_body_title_or_decider() {
    let scratch = Scratch::new("decision-empty");
    let tool = tool_with(&scratch, lists());
    let before = bytes(&tool);
    let no_body = refusal(add(&tool, &scratch, "a.json", r#"{"title": "T", "by": "person", "body": " ", "no_work": "x"}"#));
    assert!(no_body.contains("body is empty"), "{no_body}");
    let no_title = refusal(add(&tool, &scratch, "b.json", r#"{"title": "", "by": "person", "body": "b", "no_work": "x"}"#));
    assert!(no_title.contains("title is empty"), "{no_title}");
    let no_by = refusal(add(&tool, &scratch, "c.json", r#"{"title": "T", "by": " ", "body": "b", "no_work": "x"}"#));
    assert!(no_by.contains("names who decided"), "{no_by}");
    assert_eq!(bytes(&tool), before);
}

#[test]
fn add_refuses_a_request_with_a_question_or_an_unknown_field() {
    let scratch = Scratch::new("decision-fields");
    let tool = tool_with(&scratch, lists());
    let before = bytes(&tool);
    let message = refusal(add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "no_work": "x", "question": 2}}"#)));
    assert!(message.contains("unknown field `question`"), "{message}");
    let message = refusal(add(&tool, &scratch, "b.json", &format!(r#"{{{BASE}, "no_work": "x", "replaced_by": 2}}"#)));
    assert!(message.contains("unknown field `replaced_by`"), "{message}");
    assert_eq!(bytes(&tool), before);
}

#[test]
fn add_refuses_a_replaces_that_is_no_decision() {
    let scratch = Scratch::new("decision-replaces");
    let mut seed = lists();
    let some_item = item(&mut seed, vec![]);
    let tool = tool_with(&scratch, seed);
    let before = bytes(&tool);
    for target in [some_item.0, 99] {
        let message = refusal(add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "no_work": "x", "replaces": {target}}}"#)));
        assert!(message.contains("is not a decision"), "{message}");
    }
    assert_eq!(bytes(&tool), before);
}

#[test]
fn replaced_by_is_derived_and_never_stored() {
    let scratch = Scratch::new("decision-replaced-by");
    let tool = tool_with(&scratch, lists());
    add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "no_work": "x"}}"#)).unwrap();
    let first = state(&tool).decisions[0].id;
    add(&tool, &scratch, "b.json", &format!(r#"{{"title": "Use two stores", "by": "person", "body": "Revised.", "no_work": "x", "replaces": {}}}"#, first.0)).unwrap();
    let after = state(&tool);
    let second = after.decisions[1].id;
    assert_eq!(after.decisions[1].replaces, Some(first));
    assert_eq!(derive::replaced_by(&after, first), Some(second));
    assert_eq!(derive::replaced_by(&after, second), None);
    assert!(!String::from_utf8(bytes(&tool)).unwrap().contains("replaced_by"));
    assert!(render(&after, &after.decisions[0]).contains(&format!("replaced by {second} \"Use two stores\"")));
    assert!(render(&after, &after.decisions[1]).contains(&format!("replaces {first} \"Use one store\"")));
}

#[test]
fn add_refuses_a_brief_that_is_no_file_and_a_draft_the_item_rules_refuse() {
    let scratch = Scratch::new("decision-brief");
    let tool = tool_with(&scratch, lists());
    let before = bytes(&tool);
    let draft = |title: &str, brief: &str| {
        format!(
            r#"{{{BASE}, "work": [{{"title": "{title}", "area": "Alpha", "details": "d", "for": "coordinator", "waits": [], "brief": {brief}, "priority": null}}]}}"#
        )
    };
    let message = refusal(add(&tool, &scratch, "a.json", &draft("Build the one store now", r#""no/such/brief.md""#)));
    assert!(message.contains("does not exist"), "{message}");
    let message = refusal(add(&tool, &scratch, "b.json", &draft("Too short", "null")));
    assert!(message.contains("2 words"), "{message}");
    assert_eq!(bytes(&tool), before);
}

#[test]
fn add_with_a_plan_keeps_the_link() {
    let scratch = Scratch::new("decision-plan");
    let tool = tool_with(&scratch, lists());
    add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "no_work": "x", "plan": {{"path": "plans/p.md", "number": "Q2"}}}}"#)).unwrap();
    assert_eq!(state(&tool).decisions[0].plan, Some(PlanRef { path: "plans/p.md".into(), number: "Q2".into() }));
}

#[test]
fn edit_changes_title_or_body_and_refuses_the_rest() {
    let scratch = Scratch::new("decision-edit");
    let tool = tool_with(&scratch, lists());
    add(&tool, &scratch, "a.json", &format!(r#"{{{BASE}, "no_work": "x"}}"#)).unwrap();
    let id = state(&tool).decisions[0].id.0;
    let body = request(&scratch, "body.md", "A better body.\n");
    run(&tool, &format!("edit {id} --title Renamed --body-file {body} --project demo --by tester")).unwrap();
    let after = state(&tool);
    assert_eq!((after.decisions[0].title.as_str(), after.decisions[0].body.as_str()), ("Renamed", "A better body.\n"));
    assert!(after.log.last().unwrap().words.starts_with("decision edit"));

    let before = bytes(&tool);
    let empty = request(&scratch, "empty.md", " \n");
    let cases = [
        (format!("edit {id} --project demo --by tester"), "changes nothing"),
        (format!("edit {id} --body-file {empty} --project demo --by tester"), "body is empty"),
        ("edit 99 --title X --project demo --by tester".to_string(), "is not a decision"),
        (format!("edit #{id} --title X --project demo --by tester"), "bare numbers"),
    ];
    for (command, expected) in cases {
        let message = refusal(run(&tool, &command));
        assert!(message.contains(expected), "{command}: {message}");
    }
    let empty_title: Vec<String> =
        ["edit", &id.to_string(), "--title", "", "--project", "demo", "--by", "tester"].map(str::to_string).to_vec();
    assert!(refusal(cmd_decision(&tool, &empty_title)).contains("title is empty"));
    assert_eq!(bytes(&tool), before);
}

fn decide(lists: &mut Lists, date: &str, title: &str, body: &str, plan: Option<&str>) -> Id {
    let id = lists.alloc_id();
    lists.decisions.push(Decision {
        id,
        date: date.into(),
        title: title.into(),
        by: "person".into(),
        body: body.into(),
        question: None,
        work: vec![],
        replaces: None,
        plan: plan.map(|path| PlanRef { path: path.into(), number: format!("Q{}", id.0) }),
        recorded: stamp(NOON),
    });
    id
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn find_matches_every_word_in_title_or_body_ignoring_case_newest_first() {
    let mut lists = lists();
    let old = decide(&mut lists, "2026-11-01", "Queue order", "Oldest first, because it is fair.", None);
    let newer = decide(&mut lists, "2026-11-10", "Fairness of the QUEUE", "Nothing here.", None);
    let newest_same_day = decide(&mut lists, "2026-11-10", "Other", "the queue is fair", None);
    let _unrelated = decide(&mut lists, "2026-11-12", "Colours", "queue-free", None);
    let ids = |found: Vec<&Decision>| found.iter().map(|d| d.id).collect::<Vec<_>>();
    assert_eq!(ids(matching(&lists, &words("queue fair"))), [newest_same_day, newer, old]);
    assert_eq!(ids(matching(&lists, &words("FAIR"))), [newest_same_day, newer, old]);
    assert_eq!(ids(matching(&lists, &words("oldest queue"))), [old]);
    assert!(matching(&lists, &words("queue purple")).is_empty());
}

#[test]
fn find_needs_a_word() {
    let scratch = Scratch::new("decision-find");
    let tool = tool_with(&scratch, lists());
    assert!(refusal(run(&tool, "find --project demo")).contains("say what to look for"));
    run(&tool, "find anything --project demo").unwrap();
}

#[test]
fn list_filters_by_plan_and_since_newest_first() {
    let mut lists = lists();
    let a = decide(&mut lists, "2026-11-01", "A", "b", Some("plans/p.md"));
    let b = decide(&mut lists, "2026-11-05", "B", "b", Some("plans/q.md"));
    let c = decide(&mut lists, "2026-11-09", "C", "b", Some("plans/p.md"));
    let d = decide(&mut lists, "2026-11-09", "D", "b", None);
    let ids = |found: Vec<&Decision>| found.iter().map(|d| d.id).collect::<Vec<_>>();
    assert_eq!(ids(listed(&lists, None, None)), [d, c, b, a]);
    assert_eq!(ids(listed(&lists, Some("plans/p.md"), None)), [c, a]);
    assert_eq!(ids(listed(&lists, None, Some("2026-11-05"))), [d, c, b]);
    assert_eq!(ids(listed(&lists, Some("plans/p.md"), Some("2026-11-05"))), [c]);
}

#[test]
fn list_refuses_a_date_that_is_not_one_and_show_reads_one_decision() {
    let scratch = Scratch::new("decision-list");
    let mut seed = lists();
    let made = item(&mut seed, vec![]);
    let id = decide(&mut seed, "2026-11-05", "Pick one", "The body.", None);
    seed.decisions[0].work = vec![made];
    let tool = tool_with(&scratch, seed);
    assert!(refusal(run(&tool, "list --since 2026-13-01 --project demo")).contains("not a date"));
    assert!(refusal(run(&tool, "list --since yesterday --project demo")).contains("YYYY-MM-DD"));
    run(&tool, "list --since 2026-11-01 --project demo").unwrap();
    run(&tool, &format!("show {} --project demo", id.0)).unwrap();
    assert!(refusal(run(&tool, &format!("show {} --project demo", made.0))).contains("is not a decision"));
    let after = state(&tool);
    let text = render(&after, &after.decisions[0]);
    assert!(text.contains("made item #1 \"Do the work numbered 1\" [open]"), "{text}");
    assert!(text.ends_with("The body.\n"), "{text}");
}

#[test]
fn an_unknown_command_or_flag_is_refused() {
    let scratch = Scratch::new("decision-unknown");
    let tool = tool_with(&scratch, lists());
    assert!(refusal(run(&tool, "frobnicate")).contains("unknown decision command frobnicate"));
    assert!(refusal(cmd_decision(&tool, &[])).contains("usage"));
    assert!(refusal(run(&tool, "list --nope 1 --project demo")).contains("--nope"));
    assert!(refusal(run(&tool, "add --project demo --by me")).contains("--request is required"));
}
