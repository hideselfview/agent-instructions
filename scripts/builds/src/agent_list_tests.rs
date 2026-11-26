use super::*;
use crate::agent_record::{Agent, Place, Process, Work};
use crate::lists::item_fixtures::{Demo, demo, words};
use crate::model_tier::Tier;

fn agent(session: &str) -> Agent {
    Agent {
        session: session.into(),
        work: Work::Implementing {
            places: vec![Place {
                worktree: "/w/side-1".into(),
                branch: "work".into(),
            }],
        },
        model: "claude-sonnet-5-5".into(),
        tier: Tier::Implementation,
        folder: "/w/side-1".into(),
        brief: "b.md".into(),
        started_at: 1_794_571_200,
        started_by: "coordinator".into(),
        items: vec![1],
        context: None,
    }
}

fn record(demo: &Demo, session: &str, tokens: u64, window: Option<u64>, end: Option<i32>) {
    let conn = db::open(&demo.store.database(), db::Make::Never).unwrap();
    let process = Process {
        pid: std::process::id() as i32,
        pid_started: Some(1_794_571_200),
        started_at: 1_794_571_200,
        message: "h".into(),
    };
    agent_record::start(&conn, &agent(session), &process).unwrap();
    agent_record::set_tokens(&conn, session, tokens).unwrap();
    if let Some(window) = window {
        agent_record::set_window(&conn, session, window).unwrap();
    }
    if let Some(status) = end {
        agent_record::end_process(&conn, session, 1, 1_794_571_400, status).unwrap();
    }
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

fn listed(demo: &Demo, args: &[&str]) -> String {
    let mut out = Vec::new();
    list(&demo.tool, &words(args), &demo.scratch.dir, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn the_context_reads_as_thousands_against_the_window_and_marks_the_handoff_size() {
    let at = |tokens, window| Some(Context { tokens, window });
    assert_eq!(context_words(at(292_345, Some(1_000_000))), "292k of 1000k");
    assert_eq!(context_words(at(10_000, None)), "10k");
    assert_eq!(context_words(None), "no turn read");
    assert_eq!(
        context_words(at(550_000, Some(1_000_000))),
        "550k of 1000k, past the handoff size of 550k"
    );
}

#[test]
fn the_handoff_size_is_the_one_the_workflow_page_names() {
    assert!(crate::help::WORKFLOW_HELP.contains("reaches about 550k tokens"));
}

#[test]
fn list_gives_a_line_per_agent_and_running_comes_from_claude_codes_record_only() {
    let demo = demo_with_item("agent-list");
    record(&demo, "ended-one", 600_000, Some(1_000_000), Some(0));
    record(&demo, "live-one", 100_000, None, None);
    crate::testing::running_session(&demo.tool.places, "live-one");
    let all = listed(&demo, &[]);
    let lines: Vec<&str> = all.lines().collect();
    assert_eq!(lines.len(), 2, "{all}");
    let ended = lines
        .iter()
        .find(|line| line.starts_with("ended-one"))
        .unwrap();
    assert!(
        ended.contains("implementing  claude-sonnet-5-5  #1  /w/side-1  ended "),
        "{ended}"
    );
    assert!(
        ended.ends_with("600k of 1000k, past the handoff size of 550k"),
        "{ended}"
    );
    let live = lines
        .iter()
        .find(|line| line.starts_with("live-one"))
        .unwrap();
    assert!(live.contains("  running  100k"), "{live}");
    let only = listed(&demo, &["--running"]);
    assert_eq!(only.lines().count(), 1, "{only}");
    assert!(only.starts_with("live-one"));
}

#[test]
fn show_prints_the_record_each_process_and_the_last_result_of_the_log() {
    let demo = demo_with_item("agent-show");
    record(&demo, "s1", 292_000, Some(1_000_000), Some(3));
    let dir = demo.tool.places.agents().join("s1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("log.jsonl"),
        "{\"type\":\"result\",\"result\":\"first\"}\n{\"type\":\"result\",\"result\":\"Landed abc.\\nDone.\"}\n",
    )
    .unwrap();
    let mut out = Vec::new();
    show(&demo.tool, &words(&["s1"]), &demo.scratch.dir, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    for part in [
        "agent s1: implementing of project demo, ended ",
        "model: claude-sonnet-5-5 (implementation tier)",
        "items: #1",
        "worktree: /w/side-1 on branch work",
        "context: 292k of 1000k",
        "with exit status 3",
        "last result:\nLanded abc.\nDone.",
    ] {
        assert!(out.contains(part), "{part}\n{out}");
    }
    assert!(!out.contains("first"), "{out}");
}
