use super::*;
use crate::review::judge::tests::{TOKEN, message, reviewed};
use crate::testing::{FakeInbox, running_session};

#[test]
fn a_listening_judge_with_no_acknowledgement_is_told_to_acknowledge() {
    let r = reviewed("judge-ack-told", None);
    let inbox = r.inbox("j.sock");
    r.listen_as_judge(&inbox);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(inbox.socket()));
    let sessions = r.demo.tool.places.claude_sessions();
    let line = tell_outdated(&r.demo.store, &sessions)
        .unwrap()
        .expect("the judge is told");
    let lines = received.received();
    assert!(lines[0].contains(TOKEN));
    let told = message(&lines);
    assert!(
        told.contains("builds ack --agent s-judge --project demo"),
        "{told}"
    );
    assert!(line.contains("to acknowledge the text"), "{line}");
}

#[test]
fn nobody_is_told_when_no_judge_listens() {
    let r = reviewed("judge-ack-none", None);
    let sessions = r.demo.tool.places.claude_sessions();
    assert_eq!(tell_outdated(&r.demo.store, &sessions).unwrap(), None);
}
