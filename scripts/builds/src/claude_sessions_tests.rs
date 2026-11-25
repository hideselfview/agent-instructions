use super::*;
use crate::testing::{Scratch, running_session, session_started};

fn listed(places: &crate::places::Places) -> Vec<Running> {
    running(&places.claude_sessions(), &Processes::read().unwrap()).unwrap()
}

#[test]
fn a_session_whose_process_started_before_its_record_runs() {
    let scratch = Scratch::new("sessions-live");
    let places = scratch.tool(0).places;
    running_session(&places, "s1");
    assert_eq!(listed(&places).len(), 1);
}

#[test]
fn a_pid_that_a_later_process_took_is_no_session() {
    let scratch = Scratch::new("sessions-taken");
    let places = scratch.tool(0).places;
    // The record is a few seconds older than this process.
    let started = Processes::read()
        .unwrap()
        .get(std::process::id() as i32)
        .unwrap()
        .started;
    session_started(&places, "s1", (started - 5) * 1000);
    assert!(listed(&places).is_empty());
}

#[test]
fn a_pid_the_process_list_lacks_is_no_session() {
    let scratch = Scratch::new("sessions-unlisted");
    let places = scratch.tool(0).places;
    running_session(&places, "s1");
    assert!(
        running(&places.claude_sessions(), &Processes::empty())
            .unwrap()
            .is_empty()
    );
}
