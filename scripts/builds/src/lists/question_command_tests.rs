use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, item, make_store, question, question_kind, stamp, state_of};
use crate::lists::{Closed, Record};
use crate::testing::Scratch;
use rusqlite::Connection;
use std::cell::RefCell;

const AS: &str = "--project demo --by tester";

/// A store holding what <build> puts in the fixture lists.
fn setup<T>(name: &str, build: impl FnOnce(&Connection) -> T) -> (Scratch, Tool, Store, T) {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let store = make_store(&tool.places, &["bae", "coven"]);
    // Written around the code check, which the store's own writes run at their end.
    let db = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
    let made = build(&db);
    drop(db);
    (scratch, tool, store, made)
}

fn no_gh(args: &[&str]) -> Result<String> {
    panic!("gh must not run: {args:?}")
}

fn go(scratch: &Scratch, tool: &Tool, command: &str) -> Result<String> {
    go_gh(scratch, tool, command, &no_gh)
}

fn go_gh(scratch: &Scratch, tool: &Tool, command: &str, gh: GhRunner<'_>) -> Result<String> {
    // Ids print as #N; commands take bare numbers.
    let words: Vec<String> = format!("{command} {AS}")
        .replace('#', "")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    run(tool, &words, &scratch.dir, gh)
}

fn file(scratch: &Scratch, name: &str, text: &str) -> String {
    let path = scratch.dir.join(name);
    std::fs::write(&path, text.replace('#', "")).unwrap();
    path.display().to_string()
}

fn refused(result: Result<String>) -> String {
    match result.unwrap_err() {
        Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
    }
}

/// <command> is refused with <expected> in its message and the file is
/// byte for byte as it was.
fn refuses(scratch: &Scratch, tool: &Tool, store: &Store, command: &str, expected: &str) {
    let before = state_of(store);
    let message = refused(go(scratch, tool, command));
    assert!(message.contains(expected), "{command}: {message}");
    assert_eq!(state_of(store), before, "{command} changed the store");
}

fn closed_as(lists: &Lists, id: Id) -> &How {
    &lists
        .closed_record(id)
        .unwrap_or_else(|| panic!("{id} is not closed"))
        .how
}

fn details(scratch: &Scratch) -> String {
    file(scratch, "details.md", "What is asked, and the facts.\n")
}

// ask

#[test]
fn ask_with_blocks_makes_the_question_and_the_items_wait_on_it() {
    let (scratch, tool, store, (a, b)) =
        setup("q-ask-blocks", |l| (item(l, vec![]), item(l, vec![])));
    let d = details(&scratch);
    let out = go(
        &scratch,
        &tool,
        &format!("ask --title Which_way --kind question --effect data-loss --details-file {d} --blocks {a},{b}"),
    )
    .unwrap();
    let lists = store.load().unwrap();
    let q = lists.questions[0].id;
    assert_eq!(
        (
            lists.questions[0].kind,
            lists.questions[0].effect,
            lists.questions[0].blocks_nothing.clone()
        ),
        (QuestionKind::Question, Effect::DataLoss, None)
    );
    assert_eq!(
        lists.questions[0].details,
        "What is asked, and the facts.\n"
    );
    assert_eq!(lists.questions[0].opened.by, "tester");
    for id in [a, b] {
        assert_eq!(lists.item(id).unwrap().waits, [Wait::Question(q)]);
    }
    assert!(
        out.contains(&format!("asked question {q}"))
            && out.contains("[question, data loss]: blocks 2 (2 through chains)"),
        "{out}"
    );
    let db = crate::db::open_reader(&store.database()).unwrap().unwrap();
    assert_eq!(crate::lists::store::read_log(&db).unwrap().pop().unwrap().3, format!("question ask --title Which_way --kind question --effect data-loss --details-file {d} --blocks {a},{b} {AS}").replace('#', ""));
}

#[test]
fn ask_needs_what_it_blocks_or_why_it_blocks_nothing() {
    let (scratch, tool, store, a) = setup("q-ask-needs", |l| item(l, vec![]));
    let d = details(&scratch);
    let base = format!("ask --title Which_way --kind question --effect cleanup --details-file {d}");
    refuses(
        &scratch,
        &tool,
        &store,
        &base,
        "say what the question blocks",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --blocks 99"),
        "no record has",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --blocks {a} --blocks-nothing why"),
        "contradict",
    );
}

#[test]
fn ask_refuses_blocks_that_are_not_open_items() {
    let (scratch, tool, store, (gone, q)) = setup("q-ask-closed", |l| {
        let gone = item(l, vec![]);
        close(l, gone, How::Dropped("x".into()), &stamp(NOON)).unwrap();
        (gone, question(l))
    });
    let d = details(&scratch);
    let base = format!("ask --title Which_way --kind question --effect cleanup --details-file {d}");
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --blocks {gone}"),
        "an item that is closed (dropped)",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --blocks {q}"),
        "a question, not an item",
    );
}

#[test]
fn ask_refuses_a_bad_title_kind_effect_or_details() {
    let (scratch, tool, store, _) = setup("q-ask-bad", |_| ());
    let d = details(&scratch);
    let empty = file(&scratch, "empty.md", "  \n");
    let ok = format!(
        "--title T --kind question --effect cleanup --details-file {d} --blocks-nothing why"
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!(
            "ask --title T --kind other --effect cleanup --details-file {d} --blocks-nothing why"
        ),
        "--kind is question, your-step or held, not 'other'",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("ask --title T --kind held --details-file {d} --blocks-nothing why"),
        "--effect is required",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("ask --title T --kind held --effect big --details-file {d} --blocks-nothing why"),
        "--effect is data-loss, bug, slows-agents, product-design or cleanup, not 'big'",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!(
            "ask --title T --kind held --effect cleanup --details-file {empty} --blocks-nothing why"
        ),
        "details are empty",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        "ask --kind held --effect cleanup --details-file x --blocks-nothing why",
        "--title is required",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("ask {ok} --plan p.md"),
        "--plan and --plan-number go together",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("ask {ok} --nope 1"),
        "unknown flag --nope",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        "ask --title T --kind held --effect cleanup --details-file /nonexistent/x --blocks-nothing why",
        "reading /nonexistent/x",
    );
}

#[test]
fn ask_first_places_the_question_on_top() {
    let (scratch, tool, store, (a, other)) = setup("q-ask-first", |l| {
        let a = item(l, vec![]);
        let other = question(l);
        crate::lists::item::add_wait(l, a, Wait::Question(other)).unwrap();
        (a, other)
    });
    let d = details(&scratch);
    let out = go(&scratch, &tool, &format!("ask --title Look_first --kind your-step --effect cleanup --details-file {d} --blocks {a} --first no_idea_if_needed")).unwrap();
    let lists = store.load().unwrap();
    let placed = lists.questions.last().unwrap().placed.clone().unwrap();
    assert_eq!(
        (placed.at.word(), placed.why.as_str(), placed.by_person),
        ("first", "no_idea_if_needed", false)
    );
    assert!(
        out.contains("the stack's top is") && out.contains("Look_first"),
        "{out}"
    );
    assert_ne!(derive::stack_order(&lists)[0].question.id, other);
}

#[test]
fn ask_with_a_plan_keeps_its_link() {
    let (scratch, tool, store, _) = setup("q-ask-plan", |_| ());
    let d = details(&scratch);
    go(&scratch, &tool, &format!("ask --title T --kind question --effect cleanup --details-file {d} --blocks-nothing why --plan plans/x.md --plan-number Q3")).unwrap();
    let plan = store.load().unwrap().questions[0].plan.clone().unwrap();
    assert_eq!(
        (plan.path.as_str(), plan.number.as_str()),
        ("plans/x.md", "Q3")
    );
}

#[test]
fn ask_from_an_item_moves_it_to_the_stack_and_its_waiters_follow() {
    let (scratch, tool, store, (mine, waiter, other)) = setup("q-ask-from", |l| {
        let mine = item(l, vec![]);
        let waiter = item(
            l,
            vec![
                Wait::Item(mine),
                Wait::condition("quiet", crate::lists::Ender::Coordinator),
            ],
        );
        let other = item(l, vec![]);
        (mine, waiter, other)
    });
    let d = details(&scratch);
    let out = go(&scratch, &tool, &format!("ask --title Do_it_yourself --kind your-step --effect cleanup --details-file {d} --from-item {mine} --blocks {other}")).unwrap();
    let lists = store.load().unwrap();
    let q = lists.questions[0].id;
    assert_eq!(closed_as(&lists, mine), &How::MovedToStack(q));
    assert_eq!(
        lists.item(waiter).unwrap().waits,
        [
            Wait::Question(q),
            Wait::condition("quiet", crate::lists::Ender::Coordinator)
        ]
    );
    assert_eq!(lists.item(other).unwrap().waits, [Wait::Question(q)]);
    assert!(
        out.contains(&format!("item {mine} moved to the stack")),
        "{out}"
    );
    assert!(lists.item(mine).is_none());
    // The question comes from the item whose place it takes.
    assert_eq!(lists.questions[0].origin, Some(mine));
}

#[test]
fn ask_from_an_item_needs_waiters_or_a_reason_and_refuses_a_running_item() {
    let (scratch, tool, store, (lonely, running)) = setup("q-ask-from-refused", |l| {
        let lonely = item(l, vec![]);
        let running = item(l, vec![]);
        crate::lists::running::start(
            l,
            &[running],
            &[crate::lists::Running {
                worktree: "/x/bae-worktrees/side-1".into(),
                branch: "b".into(),
                brief: "p".into(),
                since: stamp(NOON),
                tier: crate::model_tier::Tier::Implementation,
            }],
        )
        .unwrap();
        (lonely, running)
    });
    let d = details(&scratch);
    let base = format!("ask --title T --kind your-step --effect cleanup --details-file {d}");
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --from-item {lonely}"),
        "say what the question blocks",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --from-item {running} --blocks-nothing why"),
        "is running",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --from-item {lonely} --blocks {lonely}"),
        "the item --from-item closes",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --from-item 99 --blocks-nothing why"),
        "not an open item",
    );
    // With a reason it moves, though nothing waited.
    go(
        &scratch,
        &tool,
        &format!("{base} --from-item {lonely} --blocks-nothing because"),
    )
    .unwrap();
}

#[test]
fn ask_from_an_item_that_others_wait_on_refuses_a_why_for_blocking_nothing() {
    let (scratch, tool, store, (moved, behind)) = setup("q-ask-from-waited", |l| {
        let moved = item(l, vec![]);
        (moved, item(l, vec![Wait::Item(moved)]))
    });
    let d = details(&scratch);
    refuses(
        &scratch,
        &tool,
        &store,
        &format!(
            "ask --title T --kind your-step --effect cleanup --details-file {d} --from-item {moved} --blocks-nothing why"
        ),
        &format!(
            "--blocks-nothing contradicts the items that waited on item {moved}: {behind} waits on the new question instead, so the new question blocks that item"
        ),
    );
}

// answer, settle, done

fn answer_request(extra: &str) -> String {
    format!(
        r#"{{"decision": {{"title": "Use the plain way", "body": "Because it is plain.", "by": "person"}}, {extra}}}"#
    )
}

#[test]
fn answer_writes_the_decision_the_work_the_fates_and_the_close_in_one_step() {
    let (scratch, tool, store, (q, proceeds, edited, dropped, ready_later)) =
        setup("q-answer", |l| {
            let q = blocking_with_plan(
                l,
                PlanRef {
                    path: "plans/x.md".into(),
                    number: "Q1".into(),
                },
            );
            let proceeds = item(l, vec![Wait::Question(q)]);
            let edited = item(l, vec![Wait::Question(q)]);
            let dropped = item(l, vec![Wait::Question(q)]);
            let ready_later = item(
                l,
                vec![
                    Wait::Question(q),
                    Wait::condition("quiet", crate::lists::Ender::Coordinator),
                ],
            );
            (q, proceeds, edited, dropped, ready_later)
        });
    let request = answer_request(&format!(
        r#""blocked": {{"{proceeds}": "proceeds", "{edited}": {{"edit": {{"title": "Do the edited work now"}}}}, "{dropped}": {{"drop": "no longer needed"}}, "{ready_later}": "proceeds"}},
           "work": [{{"title": "Build the plain way now", "area": "Beta", "details": "d", "for": "coordinator", "waits": [], "brief": null, "priority": null}}]"#
    ));
    let r = file(&scratch, "answer.json", &request);
    let out = go(&scratch, &tool, &format!("answer {q} --request {r}")).unwrap();
    let lists = store.load().unwrap();
    let decision = &lists.decisions[0];
    assert_eq!(
        (
            decision.question,
            decision.by.as_str(),
            decision.title.as_str()
        ),
        (Some(q), "person", "Use the plain way")
    );
    assert_eq!(
        decision.plan,
        Some(PlanRef {
            path: "plans/x.md".into(),
            number: "Q1".into()
        })
    );
    assert_eq!(decision.work.len(), 1);
    let made = lists.item(decision.work[0]).unwrap();
    assert_eq!(
        (made.origin, made.area.as_str(), made.title.as_str()),
        (Some(decision.id), "Beta", "Build the plain way now")
    );
    assert_eq!(closed_as(&lists, q), &How::Answered(decision.id));
    assert_eq!(
        closed_as(&lists, dropped),
        &How::Dropped("no longer needed".into())
    );
    assert_eq!(lists.item(edited).unwrap().title, "Do the edited work now");
    assert_eq!(
        derive::item_state(&lists, lists.item(proceeds).unwrap()),
        ItemState::Ready
    );
    assert_eq!(
        derive::item_state(&lists, lists.item(ready_later).unwrap()),
        ItemState::Waiting
    );
    assert!(lists.question(q).is_none());
    assert_eq!(lists.version, 4);
    for expected in [
        format!("answered question {q} with decision {}", decision.id),
        format!("made items {}", made.id),
        format!("dropped item {dropped}: no longer needed"),
        format!("ready now: {proceeds}"),
        format!("ready now: {edited} Do the edited work now"),
        "the stack is empty".to_string(),
    ] {
        assert!(out.contains(&expected), "missing {expected:?} in\n{out}");
    }
    assert!(!out.contains(&format!("ready now: {ready_later}")), "{out}");
}

#[test]
fn answer_prints_the_new_top_question_and_what_it_blocks() {
    let (scratch, tool, store, (q, next, nexts_item, qs_item)) = setup("q-answer-top", |l| {
        let q = question(l);
        let qs_item = item(l, vec![Wait::Question(q)]);
        let next = question(l);
        let nexts_item = item(l, vec![Wait::Question(next)]);
        (q, next, nexts_item, qs_item)
    });
    let r = file(
        &scratch,
        "a.json",
        &answer_request(&format!(
            r#""blocked": {{"{qs_item}": "proceeds"}}, "no_work": "nothing follows""#
        )),
    );
    let out = go(&scratch, &tool, &format!("answer {q} --request {r}")).unwrap();
    assert!(
        out.contains(&format!("the stack's top is {next}"))
            && out.contains(&format!("{nexts_item} ")),
        "{out}"
    );
    assert_eq!(store.load().unwrap().decisions[0].work, []);
}

#[test]
fn answer_leaving_out_a_waiting_item_changes_nothing() {
    let (scratch, tool, store, (q, a, b)) = setup("q-answer-leaves-out", |l| {
        let q = question(l);
        (
            q,
            item(l, vec![Wait::Question(q)]),
            item(l, vec![Wait::Question(q)]),
        )
    });
    let r = file(
        &scratch,
        "a.json",
        &answer_request(&format!(
            r#""blocked": {{"{a}": "proceeds"}}, "no_work": "none""#
        )),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("answer {q} --request {r}"),
        &format!("{b} waits on {q}"),
    );
}

#[test]
fn answer_refuses_what_it_is_not_given_correctly() {
    let (scratch, tool, store, (q, a, stranger, yours)) = setup("q-answer-refusals", |l| {
        let q = question(l);
        let a = item(l, vec![Wait::Question(q)]);
        let stranger = item(l, vec![]);
        let yours = question_kind(l, QuestionKind::YourStep);
        (q, a, stranger, yours)
    });
    let ok_blocked = format!(r#""blocked": {{"{a}": "proceeds"}}"#);
    let cases: Vec<(String, String, String)> = vec![
        (
            answer_request(&format!(r#"{ok_blocked}, "no_work": "x", "work": []"#)),
            q.to_string(),
            ("`work` or `no_work`, not both").to_string(),
        ),
        (
            answer_request(&ok_blocked),
            q.to_string(),
            ("give `work`").to_string(),
        ),
        (
            answer_request(&format!(r#"{ok_blocked}, "no_work": " ""#)),
            q.to_string(),
            ("`no_work` needs its reason").to_string(),
        ),
        (
            answer_request(&format!(
                r#""blocked": {{"{a}": "proceeds", "{stranger}": "proceeds"}}, "no_work": "x""#
            )),
            q.to_string(),
            format!("{stranger}, which do not wait"),
        ),
        (
            answer_request(&format!(
                r#""blocked": {{"{a}": {{"drop": " "}}}}, "no_work": "x""#
            )),
            q.to_string(),
            ("with no reason").to_string(),
        ),
        (
            answer_request(&format!(
                r#""blocked": {{"{a}": {{"edit": {{}}}}}}, "no_work": "x""#
            )),
            q.to_string(),
            ("names no field").to_string(),
        ),
        (
            format!(
                r#"{{"decision": {{"title": "T", "body": "  ", "by": "person"}}, {ok_blocked}, "no_work": "x"}}"#
            ),
            q.to_string(),
            ("body is empty").to_string(),
        ),
        (
            format!(
                r#"{{"decision": {{"title": "T", "body": "b", "by": " "}}, {ok_blocked}, "no_work": "x"}}"#
            ),
            q.to_string(),
            ("names who decided").to_string(),
        ),
        (
            format!(r#"{{"blocked": {{"{a}": "proceeds"}}, "no_work": "x"}}"#),
            q.to_string(),
            ("needs `decision`").to_string(),
        ),
        (
            answer_request(&format!(r#"{ok_blocked}, "no_work": "x", "by_rule": "r""#)),
            q.to_string(),
            ("`by_rule` is not part").to_string(),
        ),
        (
            answer_request(r#""blocked": {}, "no_work": "x""#),
            yours.to_string(),
            ("`builds question done").to_string(),
        ),
        (
            answer_request(r#""blocked": {}, "no_work": "x""#),
            "99".to_string(),
            ("is not a record").to_string(),
        ),
        (
            answer_request(&format!(
                r#"{ok_blocked}, "work": [{{"title": "Too short", "area": "Alpha", "details": "d", "for": "coordinator", "waits": []}}]"#
            )),
            q.to_string(),
            ("item titles are 5 to 7").to_string(),
        ),
        (
            answer_request(&format!(
                r#"{ok_blocked}, "work": [{{"title": "Five words in this title", "area": "Alpha", "details": "d", "for": "coordinator", "waits": [{{"question": {q}}}]}}]"#
            )),
            q.to_string(),
            ("waits on").to_string(),
        ),
        (
            answer_request(&format!(
                r#"{ok_blocked}, "work": [{{"title": "Five words in this title", "area": "Alpha", "details": "d", "for": "coordinator", "waits": [], "brief": "{}/no/such/brief.md"}}]"#,
                scratch.dir.display()
            )),
            q.to_string(),
            ("does not exist").to_string(),
        ),
        (
            answer_request(&format!(
                r#"{ok_blocked}, "work": [{{"title": "Five words in this title", "area": "Alpha", "details": "d", "for": "coordinator", "waits": [], "brief": "no/such/brief.md"}}]"#
            )),
            q.to_string(),
            ("does not exist: no file at").to_string(),
        ),
        (
            answer_request(&format!(r#"{ok_blocked}, "no_work": "x", "extra": 1"#)),
            q.to_string(),
            ("not valid").to_string(),
        ),
    ];
    for (n, (request, target, expected)) in cases.into_iter().enumerate() {
        let r = file(&scratch, &format!("r{n}.json"), &request);
        refuses(
            &scratch,
            &tool,
            &store,
            &format!("answer {target} --request {r}"),
            &expected,
        );
    }
}

#[test]
fn answer_that_drops_an_items_last_open_question_blocker_is_refused_whole() {
    let (scratch, tool, store, (q, a)) = setup("q-answer-orphans", |l| {
        let q = question(l);
        let a = item(l, vec![Wait::Question(q)]);
        let other = question(l);
        crate::lists::item::add_wait(l, a, Wait::Question(other)).unwrap();
        (q, a)
    });
    // Dropping `a` leaves `other` waited on by nothing: the check refuses the whole write.
    let r = file(
        &scratch,
        "a.json",
        &answer_request(&format!(
            r#""blocked": {{"{a}": {{"drop": "gone"}}}}, "no_work": "x""#
        )),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("answer {q} --request {r}"),
        "blocks no open item",
    );
}

#[test]
fn settle_records_no_decision_and_needs_a_rule() {
    let (scratch, tool, store, (q, a)) = setup("q-settle", |l| {
        let q = question(l);
        (q, item(l, vec![Wait::Question(q)]))
    });
    let blocked = format!(r#""blocked": {{"{a}": "proceeds"}}"#);
    let no_rule = file(
        &scratch,
        "n.json",
        &format!(r#"{{{blocked}, "no_work": "x"}}"#),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("settle {q} --request {no_rule}"),
        "needs `by_rule`",
    );
    let with_decision = file(
        &scratch,
        "d.json",
        &answer_request(&format!(r#"{blocked}, "no_work": "x", "by_rule": "r""#)),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("settle {q} --request {with_decision}"),
        "`decision` is not part",
    );
    let ok = file(
        &scratch,
        "ok.json",
        &format!(
            r#"{{"by_rule": "rules/x.md says so", {blocked}, "work": [{{"title": "Do the settled work now", "area": "Alpha", "details": "d", "for": "coordinator", "waits": []}}]}}"#
        ),
    );
    let out = go(&scratch, &tool, &format!("settle {q} --request {ok}")).unwrap();
    let lists = store.load().unwrap();
    assert_eq!(
        closed_as(&lists, q),
        &How::Settled("rules/x.md says so".into())
    );
    assert!(lists.decisions.is_empty());
    let shown = go(&scratch, &tool, &format!("show {q}")).unwrap();
    assert!(
        shown.contains("closed settled") && shown.contains("settled by rules/x.md says so"),
        "{shown}"
    );
    assert_eq!(lists.items.last().unwrap().origin, None);
    assert!(
        out.contains("settled question") && out.contains("no decision recorded"),
        "{out}"
    );
    assert_eq!(
        derive::item_state(&lists, lists.item(a).unwrap()),
        ItemState::Ready
    );
}

#[test]
fn done_closes_your_step_and_held_but_not_a_question() {
    let (scratch, tool, store, (step, held, plain, waiting)) = setup("q-done", |l| {
        let step = question_kind(l, QuestionKind::YourStep);
        let waiting = item(l, vec![Wait::Question(step)]);
        let held = question_kind(l, QuestionKind::Held);
        (step, held, question(l), waiting)
    });
    let r = file(
        &scratch,
        "d.json",
        r#"{"shown_by": "the person's words", "blocked": {}, "no_work": "nothing follows"}"#,
    );
    let with_waiter = file(
        &scratch,
        "w.json",
        &format!(
            r#"{{"shown_by": "the person's words", "blocked": {{"{waiting}": "proceeds"}}, "no_work": "nothing follows"}}"#
        ),
    );
    let without_reason = file(
        &scratch,
        "n.json",
        r#"{"blocked": {}, "no_work": "nothing follows"}"#,
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("done {step} --request {without_reason}"),
        "needs `shown_by`",
    );
    assert!(store.load().unwrap().question(step).is_some());
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("done {plain} --request {r}"),
        "`builds question answer",
    );
    let out = go(
        &scratch,
        &tool,
        &format!("done {step} --request {with_waiter}"),
    )
    .unwrap();
    go(&scratch, &tool, &format!("done {held} --request {r}")).unwrap();
    let lists = store.load().unwrap();
    assert_eq!(
        (closed_as(&lists, step), closed_as(&lists, held)),
        (
            &How::Done("the person's words".into()),
            &How::Done("the person's words".into())
        )
    );
    assert!(out.contains(&format!("question {step} done")), "{out}");
    let shown = go(&scratch, &tool, &format!("show {step}")).unwrap();
    assert!(shown.contains("done: the person's words"), "{shown}");
    let with_decision = file(
        &scratch,
        "x.json",
        &answer_request(r#""blocked": {}, "no_work": "x""#),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("done {plain} --request {with_decision}"),
        "done takes",
    );
}

// drop, defer

#[test]
fn drop_refuses_while_items_wait_and_names_the_command_that_frees_them() {
    let (scratch, tool, store, (q, a)) = setup("q-drop", |l| {
        let q = question(l);
        (q, item(l, vec![Wait::Question(q)]))
    });
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("drop {q} --why no"),
        &format!("`builds item wait {} --off question:{}`", a.0, q.0),
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("drop {q}"),
        "--why is required",
    );
    refuses(&scratch, &tool, &store, &format!("drop {q} --why  "), "");
    store
        .write(&stamp(NOON), "free", |l| {
            crate::lists::item::remove_wait(l, a, &Wait::Question(q))?;
            crate::lists::fixtures::blocks_nothing_because(l, q, "asked anyway");
            Ok(())
        })
        .unwrap();
    let out = go(
        &scratch,
        &tool,
        &format!("drop {q} --why not_asked_any_more"),
    )
    .unwrap();
    assert_eq!(
        closed_as(&store.load().unwrap(), q),
        &How::Dropped("not_asked_any_more".into())
    );
    assert!(
        out.contains(&format!("dropped question {q}")) && out.contains("the stack is empty"),
        "{out}"
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("drop {q} --why again"),
        "is closed (dropped)",
    );
}

#[test]
fn defer_asks_github_before_the_lock_and_needs_an_open_issue() {
    let (scratch, tool, store, q) = setup("q-defer", question);
    let calls = RefCell::new(Vec::<Vec<String>>::new());
    let state = RefCell::new("CLOSED");
    let gh = |args: &[&str]| -> Result<String> {
        calls
            .borrow_mut()
            .push(args.iter().map(|s| s.to_string()).collect());
        Ok(format!(r#"{{"state":"{}"}}"#, state.borrow()))
    };
    let url = "https://github.com/o/r/issues/7";
    let before = state_of(&store);
    let message = refused(go_gh(
        &scratch,
        &tool,
        &format!("defer {q} --issue {url}"),
        &gh,
    ));
    assert!(message.contains("is CLOSED, not open"), "{message}");
    assert_eq!(state_of(&store), before);
    *state.borrow_mut() = "OPEN";
    let out = go_gh(&scratch, &tool, &format!("defer {q} --issue {url}"), &gh).unwrap();
    assert_eq!(
        closed_as(&store.load().unwrap(), q),
        &How::Deferred(url.into())
    );
    assert!(out.contains(&format!("deferred question {q}")), "{out}");
    assert_eq!(calls.borrow()[1], ["issue", "view", url, "--json", "state"]);
}

#[test]
fn defer_refuses_while_items_wait_and_when_github_fails() {
    let (scratch, tool, store, (q, a)) = setup("q-defer-refused", |l| {
        let q = question(l);
        (q, item(l, vec![Wait::Question(q)]))
    });
    let open = |_: &[&str]| -> Result<String> { Ok(r#"{"state":"OPEN"}"#.into()) };
    let before = state_of(&store);
    let message = refused(go_gh(
        &scratch,
        &tool,
        &format!("defer {q} --issue u"),
        &open,
    ));
    assert!(
        message.contains(&format!("builds item wait {} --off question:{}", a.0, q.0)),
        "{message}"
    );
    let failing = |_: &[&str]| -> Result<String> { fail!("gh issue view failed: not found") };
    let message = refused(go_gh(
        &scratch,
        &tool,
        &format!("defer {q} --issue u"),
        &failing,
    ));
    assert!(message.contains("not found"), "{message}");
    let garbage = |_: &[&str]| -> Result<String> { Ok("hello".into()) };
    assert!(
        refused(go_gh(
            &scratch,
            &tool,
            &format!("defer {q} --issue u"),
            &garbage
        ))
        .contains("no issue state")
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("defer {q}"),
        "--issue is required",
    );
    assert_eq!(state_of(&store), before);
}

// place, edit

#[test]
fn place_sets_and_clears_where_a_question_sits() {
    let (scratch, tool, store, (low, high)) = setup("q-place", |l| {
        let low = question(l);
        item(l, vec![Wait::Question(low)]);
        let high = question(l);
        item(l, vec![Wait::Question(high)]);
        item(l, vec![Wait::Question(high)]);
        (low, high)
    });
    assert_eq!(
        derive::stack_order(&store.load().unwrap())[0].question.id,
        high
    );
    let out = go(
        &scratch,
        &tool,
        &format!("place {low} first --why the_person_said_so --by-person"),
    )
    .unwrap();
    let lists = store.load().unwrap();
    let placed = lists.question(low).unwrap().placed.clone().unwrap();
    assert_eq!(
        (placed.at.word(), placed.why.as_str(), placed.by_person),
        ("first", "the_person_said_so", true)
    );
    assert!(out.contains(&format!("the stack's top is {low}")), "{out}");
    go(&scratch, &tool, &format!("place {low} last --why later")).unwrap();
    assert!(
        !store
            .load()
            .unwrap()
            .question(low)
            .unwrap()
            .placed
            .clone()
            .unwrap()
            .by_person
    );
    go(
        &scratch,
        &tool,
        &format!("place {low} auto --why back_to_order"),
    )
    .unwrap();
    assert_eq!(store.load().unwrap().question(low).unwrap().placed, None);
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("place {low} auto --why w --by-person"),
        "--by-person goes with first, last, --above or --below",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("place {low} middle --why w"),
        "first, last or auto, not 'middle'",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("place {low} first"),
        "--why is required",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        "place 99 first --why w",
        "is not a record",
    );
}

/// Five questions, each asked later blocking more items, so neither what
/// each unblocks nor when it was asked gives the order the person chooses.
fn five_questions(name: &str) -> (Scratch, Tool, Store, [Id; 5]) {
    setup(name, |l| {
        let mut ids = [Id(0); 5];
        for (n, id) in ids.iter_mut().enumerate() {
            *id = question(l);
            for _ in 0..=n {
                item(l, vec![Wait::Question(*id)]);
            }
        }
        ids
    })
}

/// The ids of the stack as `question list` prints them, top first.
fn listed(scratch: &Scratch, tool: &Tool) -> Vec<Id> {
    go(scratch, tool, "list")
        .unwrap()
        .lines()
        .filter_map(|line| {
            let (number, rest) = line.trim_start().split_once(". #")?;
            number.parse::<usize>().ok()?;
            Id::parse(rest.split_whitespace().next()?).ok()
        })
        .collect()
}

#[test]
fn place_ranks_the_stack_in_the_order_the_person_chooses() {
    let (scratch, tool, store, [a, b, c, d, e]) = five_questions("q-rank");
    let place = |words: String| go(&scratch, &tool, &words).unwrap();
    place(format!("place {d} first --why w --by-person"));
    place(format!("place {c} first --why w --by-person"));
    place(format!("place {a} --below {c} --why w --by-person"));
    place(format!("place {b} --above {d} --why w --by-person"));
    let out = place(format!("place {e} --above {b} --why w --by-person"));
    assert!(
        out.contains(&format!("placed question {e} above {b}")),
        "{out}"
    );
    assert!(out.contains(&format!("the stack's top is {c}")), "{out}");
    let chosen = vec![c, a, e, b, d];
    assert_eq!(listed(&scratch, &tool), chosen);
    // `question show`, `lists show` and the dashboard number and list the
    // same order.
    let shown = go(&scratch, &tool, &format!("show {e}")).unwrap();
    assert!(shown.starts_with(&format!("  3. {e} ")), "{shown}");
    let lists = store.load().unwrap();
    let text = crate::lists::show::render(&lists, None).unwrap();
    for (n, id) in chosen.iter().enumerate() {
        assert!(
            text.contains(&format!("{:>3}. {id} ", n + 1)),
            "{id} is not number {} in:\n{text}",
            n + 1
        );
    }
    let view = crate::lists::view::View::build(&lists, NOON).unwrap();
    assert_eq!(view.stack.iter().map(|q| q.id).collect::<Vec<_>>(), chosen);
    // A question placed again moves: first puts it on top, and --below puts
    // it back.
    place(format!("place {d} first --why w"));
    assert_eq!(listed(&scratch, &tool), vec![d, c, a, e, b]);
    place(format!("place {d} --below {b} --why w"));
    assert_eq!(listed(&scratch, &tool), chosen);
    // auto takes a question out of the ranked order, and the rest keep
    // theirs.
    place(format!("place {a} auto --why w"));
    assert_eq!(store.load().unwrap().question(a).unwrap().placed, None);
    assert_eq!(listed(&scratch, &tool), vec![c, e, b, d, a]);
    let placed = store.load().unwrap().question(e).unwrap().placed.clone();
    assert_eq!(
        placed.map(|p| (p.why, p.by_person)),
        Some(("w".to_string(), true))
    );
}

#[test]
fn place_next_to_a_question_needs_one_in_the_ranked_order() {
    let (scratch, tool, store, [a, b, c, d, _]) = five_questions("q-rank-refused");
    go(&scratch, &tool, &format!("place {a} first --why w")).unwrap();
    go(&scratch, &tool, &format!("place {b} last --why w")).unwrap();
    for (command, expected) in [
        (
            format!("place {c} --above {a} --below {a} --why w"),
            "--above and --below contradict each other".to_string(),
        ),
        (
            format!("place {c} first --above {a} --why w"),
            "--above and --below take the place of first, last or auto".to_string(),
        ),
        (
            format!("place {c} --above {a}"),
            "--why is required".to_string(),
        ),
        (
            format!("place {a} --above {a} --why w"),
            format!("question {a} can't sit above or below itself"),
        ),
        (
            format!("place {c} --below {d} --why w"),
            format!("question {d} is not in the ranked order"),
        ),
        (
            format!("place {c} --above {b} --why w"),
            format!("question {b} is not in the ranked order"),
        ),
        (
            format!("place {c} --above 99 --why w"),
            "#99 is not an open question".to_string(),
        ),
    ] {
        refuses(&scratch, &tool, &store, &command, &expected);
    }
}

#[test]
fn edit_changes_the_fields_and_gives_a_why_only_while_no_item_waits() {
    let (scratch, tool, store, (waited, a, free)) = setup("q-edit", |l| {
        let waited = question(l);
        let a = item(l, vec![Wait::Question(waited)]);
        (waited, a, question(l))
    });
    let d = file(&scratch, "new.md", "New details.\n");
    go(
        &scratch,
        &tool,
        &format!("edit {waited} --title New_title --details-file {d}"),
    )
    .unwrap();
    let lists = store.load().unwrap();
    assert_eq!(
        (
            lists.question(waited).unwrap().title.as_str(),
            lists.question(waited).unwrap().details.as_str()
        ),
        ("New_title", "New details.\n")
    );
    go(&scratch, &tool, &format!("edit {waited} --effect bug")).unwrap();
    assert_eq!(
        store.load().unwrap().question(waited).unwrap().effect,
        Effect::Bug
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("edit {waited} --effect worse"),
        "--effect is data-loss, bug,",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("edit {free}"),
        "changes nothing",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("edit {waited} --blocks-nothing x"),
        &format!(
            "question {waited} gives a reason it blocks nothing (x) while open item {a} waits on it: a question gives that reason only while no open item waits on it, and `builds item wait ITEM --off question:{} --blocks-nothing WHY` gives the reason in the write that removes the last open item's wait on it",
            waited.0
        ),
    );
    go(
        &scratch,
        &tool,
        &format!("edit {free} --blocks-nothing a_better_reason"),
    )
    .unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .question(free)
            .unwrap()
            .blocks_nothing
            .as_deref(),
        Some("a_better_reason")
    );
}

// show, list

#[test]
fn show_and_list_print_the_stack_and_a_closed_question() {
    let (scratch, tool, _store, (q, a, other)) = setup("q-show", |l| {
        let q = question(l);
        let a = item(l, vec![Wait::Question(q)]);
        (q, a, question(l))
    });
    let shown = go(&scratch, &tool, &format!("show {q}")).unwrap();
    assert!(
        shown.contains(&format!("1. {q} "))
            && shown.contains("effect: cleanup")
            && shown.contains("blocks 1 (1 through chains):")
            && shown.contains(&format!("  {a} Do the work numbered {}", a.0))
            && shown.contains("details"),
        "{shown}"
    );
    let listed = go(&scratch, &tool, "list").unwrap();
    assert!(
        listed.contains(&format!("1. {q} ")) && listed.contains(&format!("2. {other} ")),
        "{listed}"
    );
    assert_eq!(
        listed.lines().count(),
        2,
        "one line per question:\n{listed}"
    );
    let r = file(
        &scratch,
        "d.json",
        &answer_request(&format!(
            r#""blocked": {{"{a}": "proceeds"}}, "no_work": "x""#
        )),
    );
    go(&scratch, &tool, &format!("answer {q} --request {r}")).unwrap();
    let shown = go(&scratch, &tool, &format!("show {q}")).unwrap();
    assert!(shown.contains("closed answered"), "{shown}");
    assert!(refused(go(&scratch, &tool, &format!("show {a}"))).contains("not a question"));
    assert!(refused(go(&scratch, &tool, "show 99")).contains("not a record"));
}

#[test]
fn an_unknown_command_and_no_command_are_refused() {
    let (scratch, tool, _store, _) = setup("q-unknown", |_| ());
    assert!(
        refused(go(&scratch, &tool, "frobnicate")).contains("unknown question command frobnicate")
    );
    let words: Vec<String> = Vec::new();
    assert!(
        matches!(run(&tool, &words, &scratch.dir, &no_gh), Err(Fail::Message(m)) if m.contains("usage"))
    );
    assert!(refused(go(&scratch, &tool, "show 1.5")).contains("not an id"));
    let hash: Vec<String> = ["show", "#3", "--project", "demo", "--by", "t"]
        .map(String::from)
        .to_vec();
    assert!(
        matches!(run(&tool, &hash, &scratch.dir, &no_gh), Err(Fail::Message(m)) if m.contains("bare numbers"))
    );
}

#[test]
fn decisions_made_by_answers_pass_the_whole_state_check_with_closed_records_kept() {
    let (scratch, tool, store, (q, a)) = setup("q-closed-kept", |l| {
        let q = question(l);
        (q, item(l, vec![Wait::Question(q)]))
    });
    let r = file(
        &scratch,
        "a.json",
        &answer_request(&format!(
            r#""blocked": {{"{a}": {{"drop": "x"}}}}, "no_work": "n""#
        )),
    );
    go(&scratch, &tool, &format!("answer {q} --request {r}")).unwrap();
    let lists = store.load().unwrap();
    assert_eq!(lists.closed.len(), 2);
    assert!(lists.closed.iter().any(
        |Closed { record, .. }| matches!(record, Record::Question(question) if question.id == q)
    ));
}

/// A question that blocks something (an item waits on it once one is made),
/// with <plan>.
fn blocking_with_plan(db: &Connection, plan: PlanRef) -> Id {
    let id = crate::lists::fixtures::next_id(db);
    crate::lists::question::ask(
        db,
        crate::lists::question::NewQuestion {
            title: format!("Which way for number {}", id.0),
            details: "details".into(),
            kind: QuestionKind::Question,
            effect: Effect::Cleanup,
            blocks_nothing: None,
            first: None,
            plan: Some(plan),
            origin: None,
        },
        &stamp(NOON + i64::from(id.0)),
    )
    .unwrap()
}

#[test]
fn ask_and_edit_tag_a_question_open_or_closed_and_list_chooses_by_tag() {
    let (scratch, tool, store, one) = setup("question-tags", |db| item(db, Vec::new()));
    let details = details(&scratch);
    let words = |line: &[&str]| -> Vec<String> {
        line.iter()
            .map(|word| word.to_string())
            .chain(AS.split_whitespace().map(str::to_string))
            .collect()
    };
    let ask = |title: &str, blocks: &[&str], tag: &str| {
        let mut line = vec![
            "ask",
            "--title",
            title,
            "--kind",
            "question",
            "--effect",
            "cleanup",
            "--details-file",
            &details,
            "--tag",
            tag,
        ];
        line.extend_from_slice(blocks);
        run(&tool, &words(&line), &scratch.dir, &no_gh)
    };
    let one = one.0.to_string();
    ask(
        "Which way does the first go?",
        &["--blocks", &one],
        "screen-reads audit",
    )
    .unwrap();
    ask(
        "Which way does the second go?",
        &["--blocks-nothing", "asked to know"],
        "coven-trust audit",
    )
    .unwrap();
    let lists = store.load().unwrap();
    let first = lists.questions[0].id;
    let second = lists.questions[1].id;
    assert_eq!(lists.questions[0].tags, ["screen-reads audit"]);

    let listed = run(
        &tool,
        &words(&["list", "--tag", "coven-trust audit"]),
        &scratch.dir,
        &no_gh,
    )
    .unwrap();
    assert!(
        listed.contains("Which way does the second go?")
            && !listed.contains("Which way does the first go?")
            && listed.contains(&format!("trail: coven-trust audit → {second}")),
        "{listed}"
    );
    assert!(
        run(
            &tool,
            &words(&["list", "--tag", "nothing"]),
            &scratch.dir,
            &no_gh
        )
        .unwrap()
        .contains("no question on the stack has the tag nothing on its trail")
    );

    run(
        &tool,
        &words(&[
            "edit",
            &first.0.to_string(),
            "--tag",
            "mac",
            "--untag",
            "screen-reads audit",
        ]),
        &scratch.dir,
        &no_gh,
    )
    .unwrap();
    assert_eq!(store.load().unwrap().questions[0].tags, ["mac"]);

    // A closed question still takes tags, and shows them.
    go(&scratch, &tool, &format!("drop {second} --why not-asked")).unwrap();
    run(
        &tool,
        &words(&["edit", &second.0.to_string(), "--tag", "later"]),
        &scratch.dir,
        &no_gh,
    )
    .unwrap();
    let shown = go(&scratch, &tool, &format!("show {second}")).unwrap();
    assert!(
        shown.contains(&format!("trail: coven-trust audit; later → {second}")),
        "{shown}"
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("edit {second} --title Another"),
        "closed",
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("edit {first}"),
        "changes nothing",
    );
}

#[test]
fn ask_and_edit_give_a_question_its_origin_and_the_trail_shows_it() {
    let (scratch, tool, store, (work, other)) = setup("question-origins", |db| {
        let work = item(db, Vec::new());
        crate::lists::tag::add_tags(db, work, &["audit".into()]).unwrap();
        (work, item(db, Vec::new()))
    });
    let d = details(&scratch);
    let base = format!(
        "ask --title Sort_records --kind question --effect cleanup --details-file {d} --blocks-nothing asked"
    );
    go(&scratch, &tool, &format!("{base} --origin {work}")).unwrap();
    let lists = store.load().unwrap();
    let asked = lists.questions[0].id;
    assert_eq!(lists.questions[0].origin, Some(work));
    // The question carries no tag itself, and its trail has the item's tag.
    assert!(lists.questions[0].tags.is_empty());
    assert!(
        go(&scratch, &tool, "list --tag audit")
            .unwrap()
            .contains("Sort_records")
    );
    let shown = go(&scratch, &tool, &format!("show {asked}")).unwrap();
    assert!(
        shown.contains(&format!(
            "trail: audit → {work} Do the work numbered 1 → {asked}"
        )),
        "{shown}"
    );
    refuses(
        &scratch,
        &tool,
        &store,
        &format!("{base} --from-item {other} --origin {work}"),
        "--from-item and --origin contradict each other",
    );
    go(&scratch, &tool, &format!("edit {asked} --origin {other}")).unwrap();
    assert_eq!(store.load().unwrap().questions[0].origin, Some(other));
    go(&scratch, &tool, &format!("drop {asked} --why not-asked")).unwrap();
    go(&scratch, &tool, &format!("edit {asked} --origin {work}")).unwrap();
    let shown = go(&scratch, &tool, &format!("show {asked}")).unwrap();
    assert!(
        shown.contains(&format!(
            "trail: audit → {work} Do the work numbered 1 → {asked}"
        )),
        "{shown}"
    );
}
