use super::*;
use crate::agent_record::{Agent, AgentRole, Process, Work};
use crate::lists::item_fixtures::{Demo, demo, words};
use crate::lists::record::{How, close};
use crate::model_tier::Tier;

const START: i64 = 1_794_571_200;
const PAST: u64 = agent_record::HANDOFF_TOKENS;

fn store(demo: &Demo) -> db::Held {
    db::open(&demo.store.database(), db::Make::Never).unwrap()
}

fn process(at: i64) -> Process {
    Process {
        pid: 1,
        pid_started: Some(at),
        started_at: at,
        message: "h".into(),
    }
}

fn work(role: AgentRole) -> Work {
    match role {
        AgentRole::Implementing => Work::Implementing { places: Vec::new() },
        AgentRole::Planner => Work::Planner,
    }
}

/// Records <session>'s agent, given <items> at <at> with one process that has
/// not ended, whose last turn had <tokens> of context, and makes its folder.
fn record(demo: &Demo, session: &str, role: AgentRole, items: &[u32], at: i64, tokens: u64) {
    let conn = store(demo);
    let agent = Agent {
        session: session.into(),
        work: work(role),
        model: "claude-opus-5-5".into(),
        tier: Tier::Strongest,
        folder: "/w".into(),
        brief: "b.md".into(),
        started_at: at,
        started_by: "coordinator".into(),
        items: items.to_vec(),
        context: None,
    };
    agent_record::start(&conn, &agent, &process(at)).unwrap();
    agent_record::set_tokens(&conn, session, tokens).unwrap();
    std::fs::create_dir_all(demo.tool.places.agents().join(session).join("scratch")).unwrap();
}

fn end(demo: &Demo, session: &str, at: i64, status: i32) {
    agent_record::end_process(&store(demo), session, 1, at, status).unwrap();
}

fn close_item(demo: &Demo, item: u32) {
    close(
        &store(demo),
        crate::lists::Id(item),
        How::Finished("made nothing".into()),
        &crate::lists::fixtures::stamp(crate::lists::fixtures::NOON),
    )
    .unwrap();
}

fn demo_with_items(name: &str) -> Demo {
    let demo = demo(name);
    let details = demo.brief("details.txt");
    for title in ["Do the first thing now", "Do the second thing now"] {
        crate::lists::item_words(
            &demo.tool,
            &crate::testing::no_ssh(),
            &words(&[
                "add",
                "--title",
                title,
                "--area",
                "Alpha",
                "--details-file",
                &details,
            ]),
        )
        .unwrap();
    }
    demo
}

/// Whether the folder of <session> is there after a real prune.
fn kept(demo: &Demo, session: &str) -> bool {
    prune(&demo.tool, Run::Real).unwrap();
    demo.tool.places.agents().join(session).is_dir()
}

#[test]
fn an_agent_under_the_handoff_size_keeps_its_folder_with_every_item_closed() {
    let demo = demo_with_items("prune-under");
    record(
        &demo,
        "under",
        AgentRole::Implementing,
        &[1],
        START,
        PAST - 1,
    );
    end(&demo, "under", START + 1, 0);
    close_item(&demo, 1);
    assert!(kept(&demo, "under"));
}

#[test]
fn an_agent_past_the_handoff_size_loses_its_folder_once_every_item_is_closed() {
    let demo = demo_with_items("prune-closed");
    record(&demo, "past", AgentRole::Implementing, &[1, 2], START, PAST);
    close_item(&demo, 1);
    assert!(kept(&demo, "past"), "an open item no agent took over");
    close_item(&demo, 2);
    crate::testing::running_session(&demo.tool.places, "past");
    assert!(kept(&demo, "past"), "a running session");
    std::fs::remove_dir_all(demo.tool.places.claude_sessions()).unwrap();
    let stray = demo.tool.places.agents().join("no-record");
    std::fs::create_dir_all(&stray).unwrap();
    prune(&demo.tool, Run::Dry).unwrap();
    assert!(demo.tool.places.agents().join("past").is_dir(), "a dry run");
    assert!(!kept(&demo, "past"));
    assert!(stray.is_dir(), "a folder no store records stays");
}

#[test]
fn a_later_agent_takes_an_item_over_once_it_ends_a_process_with_status_0() {
    let demo = demo_with_items("prune-over");
    record(&demo, "past", AgentRole::Implementing, &[1, 2], START, PAST);
    record(&demo, "next", AgentRole::Implementing, &[1], START + 10, 5);
    assert!(kept(&demo, "past"), "item 2 has no later agent");
    record(&demo, "third", AgentRole::Implementing, &[2], START + 20, 5);
    end(&demo, "next", START + 30, 0);
    assert!(kept(&demo, "past"), "item 2's agent has not ended");
    end(&demo, "third", START + 30, 0);
    assert!(!kept(&demo, "past"));
}

#[test]
fn a_later_agent_that_failed_has_taken_nothing_over() {
    let demo = demo_with_items("prune-failed");
    record(&demo, "past", AgentRole::Implementing, &[1], START, PAST);
    record(&demo, "next", AgentRole::Implementing, &[1], START + 10, 5);
    end(&demo, "next", START + 30, 1);
    assert!(kept(&demo, "past"));
}

#[test]
fn a_planner_takes_over_nothing_from_an_implementing_agent() {
    let demo = demo_with_items("prune-role");
    record(&demo, "past", AgentRole::Implementing, &[1], START, PAST);
    record(&demo, "planner", AgentRole::Planner, &[1], START + 10, 5);
    end(&demo, "planner", START + 30, 0);
    assert!(kept(&demo, "past"));
}

/// What counts is when each agent was given the item, not when it started.
#[test]
fn the_agent_given_an_item_later_takes_it_over_whichever_started_first() {
    let demo = demo_with_items("prune-order");
    record(&demo, "older", AgentRole::Implementing, &[2], START, 5);
    record(&demo, "newer", AgentRole::Implementing, &[1], START + 10, 5);
    end(&demo, "newer", START + 15, 0);
    agent_record::join(
        &store(&demo),
        "older",
        &work(AgentRole::Implementing),
        &[1],
        START + 20,
    )
    .unwrap();
    agent_record::set_tokens(&store(&demo), "older", PAST).unwrap();
    end(&demo, "older", START + 30, 0);
    assert!(kept(&demo, "older"), "newer had item 1 before older did");
    agent_record::set_tokens(&store(&demo), "newer", PAST).unwrap();
    assert!(!kept(&demo, "newer"), "older took item 1 over");
}

/// A process that ended before its agent was given the item read no handoff
/// of that item.
#[test]
fn an_end_before_the_later_agent_was_given_the_item_does_not_count() {
    let demo = demo_with_items("prune-early-end");
    record(
        &demo,
        "past",
        AgentRole::Implementing,
        &[1],
        START + 10,
        PAST,
    );
    record(&demo, "other", AgentRole::Implementing, &[2], START, 5);
    end(&demo, "other", START + 15, 0);
    agent_record::join(
        &store(&demo),
        "other",
        &work(AgentRole::Implementing),
        &[1],
        START + 20,
    )
    .unwrap();
    assert!(kept(&demo, "past"));
}
