use super::*;
use crate::agent_record::{Agent, AgentRole, Process};
use crate::lists::item_fixtures::{Demo, demo, words};
use crate::lists::record::{How, close};
use crate::model_tier::Tier;

fn record(demo: &Demo, session: &str) {
    let conn = db::open(&demo.store.database(), db::Make::Never).unwrap();
    let agent = Agent {
        session: session.into(),
        role: AgentRole::Planner,
        model: "claude-opus-5-5".into(),
        tier: Tier::Strongest,
        folder: "/w".into(),
        brief: None,
        places: Vec::new(),
        started_at: 1_794_571_200,
        started_by: "coordinator".into(),
        items: vec![1],
        context: None,
    };
    let process = Process {
        pid: 1,
        pid_started: 1_794_571_200,
        started_at: 1_794_571_200,
        message: "h".into(),
    };
    agent_record::start(&conn, &agent, &process).unwrap();
    std::fs::create_dir_all(demo.tool.places.agents().join(session).join("scratch")).unwrap();
}

fn close_item(demo: &Demo) {
    let conn = db::open(&demo.store.database(), db::Make::Never).unwrap();
    close(
        &conn,
        crate::lists::Id(1),
        How::Finished("made nothing".into()),
        &crate::lists::fixtures::stamp(crate::lists::fixtures::NOON),
    )
    .unwrap();
}

fn demo_with_item(name: &str) -> Demo {
    let demo = demo(name);
    let details = demo.brief("details.txt");
    crate::lists::cmd_item(
        &demo.tool,
        &words(&[
            "add",
            "--title",
            "Do the first thing now",
            "--area",
            "Alpha",
            "--details-file",
            &details,
        ]),
    )
    .unwrap();
    demo
}

#[test]
fn a_session_runs_or_unfinished_keeps_its_folder_and_the_rest_go() {
    let scratch = crate::testing::Scratch::new("agent-prune-pure");
    for name in ["a", "b", "c"] {
        std::fs::create_dir_all(scratch.dir.join(name)).unwrap();
    }
    let running: HashSet<std::ffi::OsString> = ["b".into()].into();
    let found = deletable(
        &scratch.dir,
        &["a".to_string(), "b".to_string(), "gone".to_string()],
        &running,
    );
    assert_eq!(found, [scratch.dir.join("a")]);
}

#[test]
fn prune_deletes_the_folder_of_an_agent_whose_items_are_closed_and_that_does_not_run() {
    let demo = demo_with_item("agent-prune");
    record(&demo, "s1");
    let folder = demo.tool.places.agents().join("s1");
    prune(&demo.tool, Run::Real).unwrap();
    assert!(folder.is_dir(), "an open item keeps the folder");
    close_item(&demo);
    crate::testing::running_session(&demo.tool.places, "s1");
    prune(&demo.tool, Run::Real).unwrap();
    assert!(folder.is_dir(), "a running session keeps the folder");
    std::fs::remove_dir_all(demo.tool.places.claude_sessions()).unwrap();
    let stray = demo.tool.places.agents().join("no-record");
    std::fs::create_dir_all(&stray).unwrap();
    prune(&demo.tool, Run::Dry).unwrap();
    assert!(folder.is_dir(), "a dry run deletes nothing");
    prune(&demo.tool, Run::Real).unwrap();
    assert!(!folder.exists());
    assert!(stray.is_dir(), "a folder no store records stays");
}
