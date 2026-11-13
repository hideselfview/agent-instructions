use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, draft, item, lists, question, stamp};

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn new_decision(title: &str) -> NewDecision {
    NewDecision { title: title.into(), by: "person".into(), body: "why".into(), question: None, replaces: None, plan: None }
}

#[test]
fn a_decision_records_its_items_and_dates_itself() {
    let mut lists = lists();
    let q = question(&mut lists);
    let mut new = new_decision("Chose the first way");
    new.question = Some(q);
    let work = [draft("One two three four five"), draft("Six seven eight nine ten")];
    let added = add_decision(&mut lists, &new, &work, &stamp(NOON)).unwrap();
    let decision = lists.decision(added.decision).unwrap();
    assert_eq!(decision.work, added.items);
    assert_eq!(decision.date, crate::text::local_day(NOON));
    assert_eq!(decision.question, Some(q));
    for id in &added.items {
        assert_eq!(lists.item(*id).unwrap().from, Some(added.decision));
    }
}

#[test]
fn a_decision_refused_up_front_changes_nothing() {
    let mut lists = lists();
    let before = lists.clone();
    for bad in [
        NewDecision { title: "two\nlines".into(), ..new_decision("x") },
        NewDecision { by: "".into(), ..new_decision("x") },
        NewDecision { body: " ".into(), ..new_decision("x") },
        NewDecision { question: Some(Id(9)), ..new_decision("x") },
        NewDecision { replaces: Some(Id(9)), ..new_decision("x") },
    ] {
        assert!(add_decision(&mut lists, &bad, &[], &stamp(NOON)).is_err());
        assert_eq!(lists, before);
    }
}

#[test]
fn a_decision_may_replace_an_earlier_one_and_answer_only_a_question() {
    let mut lists = lists();
    let first = add_decision(&mut lists, &new_decision("First"), &[], &stamp(NOON)).unwrap().decision;
    let mut second = new_decision("Second");
    second.replaces = Some(first);
    assert!(add_decision(&mut lists, &second, &[], &stamp(NOON)).is_ok());
    let i = item(&mut lists, vec![]);
    let mut wrong = new_decision("Third");
    wrong.question = Some(i);
    assert!(refusal(add_decision(&mut lists, &wrong, &[], &stamp(NOON))).contains("not a question"));
}
