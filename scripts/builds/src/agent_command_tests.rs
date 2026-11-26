use super::*;
use crate::lists::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};

const TOOLS_JSON: &str =
    r#"["Bash","Edit","LSP","Monitor","Read","TaskStop","ToolSearch","Write"]"#;

fn add_item(demo: &Demo, title: &str) {
    let details = demo.brief("details.txt");
    cmd_item(
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

fn take(demo: &Demo, ids: &[&str], side: &std::path::Path, branch: &str, brief: &str) {
    let side = side.display().to_string();
    let mut args = vec!["take"];
    args.extend_from_slice(ids);
    args.extend(["--worktree", &side, "--branch", branch, "--brief", brief]);
    cmd_item(&demo.tool, &crate::testing::no_ssh(), &words(&args)).unwrap();
}

/// A demo with the project file the run reads, two items and a brief.
fn project_demo(name: &str) -> Demo {
    let demo = demo(name);
    let root = &demo.tool.reviewer.rules_root;
    std::fs::write(
        root.join("projects/demo.md"),
        "# demo\n\nDemo is a tool.\n\n## Hard limits\n\n- Never edit the person's checkout.\n",
    )
    .unwrap();
    add_item(&demo, "Do the first thing now");
    add_item(&demo, "Do the second thing now");
    demo
}

/// A `claude` stand-in that keeps its prompt, arguments and folder in
/// <demo>'s scratch folder, prints <stream> and exits with <status>.
fn fake_claude(demo: &mut Demo, stream: &[&str], status: i32) -> std::path::PathBuf {
    let dir = demo.scratch.dir.display().to_string();
    let lines: String = stream
        .iter()
        .map(|line| format!("printf '%s\\n' '{line}'\n"))
        .collect();
    demo.tool.places.claude_program = demo.scratch.script(
        "claude",
        &format!(
            "cat > '{dir}/prompt.txt'\nprintf '%s\\n' \"$@\" > '{dir}/args.txt'\npwd > '{dir}/cwd.txt'\n{lines}exit {status}"
        ),
    );
    demo.scratch.dir.clone()
}

fn init_line(tools: &str, mcp: &str, skills: &str, model: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"init","session_id":"s","model":"{model}","tools":{tools},"mcp_servers":{mcp},"skills":{skills}}}"#
    )
}

fn turn(input: u64, created: u64, read: u64) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":{input},"cache_creation_input_tokens":{created},"cache_read_input_tokens":{read}}}}}}}"#
    )
}

const RESULT: &str = r#"{"type":"result","is_error":false,"result":"Landed abc.\nChecks passed.","modelUsage":{"claude-sonnet-5-5":{"contextWindow":1000000}}}"#;

fn run_command(demo: &Demo, extra: &[&str]) -> (Result<()>, String) {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    let mut out = Vec::new();
    let result = run(&demo.tool, &words(&args[1..]), &demo.scratch.dir, &mut out);
    (result, String::from_utf8(out).unwrap())
}

fn rows(demo: &Demo, sql: &str) -> Vec<String> {
    let conn = db::open(&demo.store.database(), db::Make::Never).unwrap();
    let mut statement = conn.prepare(sql).unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |row| {
            (0..count)
                .map(|at| {
                    row.get::<_, rusqlite::types::Value>(at)
                        .map(|v| format!("{v:?}"))
                })
                .collect::<rusqlite::Result<Vec<_>>>()
                .map(|cells| cells.join("|"))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn an_implementing_agent_starts_in_its_worktree_with_the_prompt_the_flags_and_records() {
    let mut demo = project_demo("agent-run-implementing");
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    take(&demo, &["1", "2"], &side, "work", &brief);
    let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
    let dir = fake_claude(
        &mut demo,
        &[&init, &turn(2, 100, 1000), &turn(3, 200, 2000), RESULT],
        0,
    );
    let (result, out) = run_command(&demo, &["--role", "implementing", "1", "2"]);
    result.unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("agent "), "{out}");
    assert!(
        lines[0].contains("implementing of project demo, model claude-sonnet-5-5"),
        "{out}"
    );
    assert_eq!(lines[1..3], ["Landed abc.", "Checks passed."], "{out}");
    assert!(lines[3].ends_with("ended with exit status 0"), "{out}");
    let prompt = std::fs::read_to_string(dir.join("prompt.txt")).unwrap();
    for part in [
        "Demo is a tool.",
        "You are an implementing agent of project demo.",
        &format!(
            "builds land {} --done 1 --done 2 --project demo",
            side.display()
        ),
        "#1 Do the first thing now",
        "#2 Do the second thing now",
        "The brief, ",
        "- Never edit the person's checkout.",
        "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>",
        "/agents/",
    ] {
        assert!(prompt.contains(part), "{part}\n{prompt}");
    }
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(
        args[..4],
        [
            "-p",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--tools"
        ]
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "claude-sonnet-5-5"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--name", "demo 1 2 implementing"])
    );
    let session = args[args.len() - 1];
    assert_eq!(args[args.len() - 2], "--session-id");
    assert_eq!(
        std::fs::canonicalize(std::fs::read_to_string(dir.join("cwd.txt")).unwrap().trim())
            .unwrap(),
        std::fs::canonicalize(&side).unwrap()
    );
    assert_eq!(
        rows(
            &demo,
            "SELECT session, role, model, tier, brief, context_tokens, context_window FROM agent"
        )
        .len(),
        1
    );
    assert_eq!(
        rows(&demo, "SELECT context_tokens, context_window FROM agent"),
        ["Integer(2203)|Integer(1000000)"]
    );
    assert_eq!(
        rows(&demo, "SELECT item FROM agent_item ORDER BY item"),
        ["Integer(1)", "Integer(2)"]
    );
    assert_eq!(
        rows(&demo, "SELECT number, exit_status FROM agent_process"),
        ["Integer(1)|Integer(0)"]
    );
    let log =
        std::fs::read_to_string(demo.tool.places.agents().join(session).join("log.jsonl")).unwrap();
    assert!(log.contains("Landed abc."), "{log}");
}

#[test]
fn claudes_exit_status_is_buildss_and_the_report_still_prints() {
    let mut demo = project_demo("agent-run-exit");
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    take(&demo, &["1"], &side, "work", &brief);
    let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
    fake_claude(&mut demo, &[&init, RESULT], 3);
    let (result, out) = run_command(&demo, &["--role", "implementing", "1"]);
    assert!(matches!(result, Err(Fail::Exit(3))));
    assert!(out.contains("Landed abc."), "{out}");
    assert_eq!(
        rows(&demo, "SELECT exit_status FROM agent_process"),
        ["Integer(3)"]
    );
}

#[test]
fn an_agent_started_with_a_skill_a_server_or_a_ninth_tool_is_stopped_and_the_run_fails() {
    let mut demo = project_demo("agent-run-check");
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    take(&demo, &["1"], &side, "work", &brief);
    let init = init_line(
        TOOLS_JSON,
        "[]",
        r#"["publish-designs"]"#,
        "claude-sonnet-5-5",
    );
    let dir = demo.scratch.dir.display().to_string();
    demo.tool.places.claude_program = demo.scratch.script(
        "claude",
        &format!("cat > '{dir}/prompt.txt'\nprintf '%s\\n' '{init}'\nexec sleep 60"),
    );
    let (result, _) = run_command(&demo, &["--role", "implementing", "1"]);
    let error = result.unwrap_err().text();
    assert!(
        error.contains("publish-designs") && error.contains("builds stopped the agent"),
        "{error}"
    );
    let ended = rows(&demo, "SELECT ended_at IS NOT NULL FROM agent_process");
    assert_eq!(ended, ["Integer(1)"]);
}

#[test]
fn a_planner_runs_the_strongest_model_in_the_worktrees_folder_of_the_briefs_repo() {
    let mut demo = project_demo("agent-run-planner");
    let worktrees = demo.main.parent().unwrap().join("demo-worktrees");
    std::fs::create_dir_all(&worktrees).unwrap();
    std::fs::create_dir_all(demo.main.join("plans/briefs")).unwrap();
    std::fs::write(demo.main.join("plans/briefs/p.md"), "Plan the thing.\n").unwrap();
    let init = init_line(TOOLS_JSON, "[]", "[]", "claude-opus-5-5");
    let dir = fake_claude(&mut demo, &[&init, RESULT], 0);
    let (result, _) = run_command(
        &demo,
        &["--role", "planner", "1", "--brief", "plans/briefs/p.md"],
    );
    result.unwrap();
    let prompt = std::fs::read_to_string(dir.join("prompt.txt")).unwrap();
    assert!(
        prompt.contains("You are a planner of project demo."),
        "{prompt}"
    );
    assert!(prompt.contains("Plan the thing."), "{prompt}");
    assert!(
        prompt.contains("Co-Authored-By: Claude Opus 5.5"),
        "{prompt}"
    );
    assert!(!prompt.contains("Your worktree"), "{prompt}");
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(args.contains("claude-opus-5-5"), "{args}");
    assert_eq!(
        std::fs::canonicalize(std::fs::read_to_string(dir.join("cwd.txt")).unwrap().trim())
            .unwrap(),
        std::fs::canonicalize(&worktrees).unwrap()
    );
}

#[test]
fn a_project_whose_file_lacks_hard_limits_starts_no_agent() {
    let mut demo = project_demo("agent-run-no-limits");
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    take(&demo, &["1"], &side, "work", &brief);
    std::fs::write(
        demo.tool.reviewer.rules_root.join("projects/demo.md"),
        "# demo\n\nDemo is a tool.\n",
    )
    .unwrap();
    fake_claude(&mut demo, &[], 0);
    let (result, _) = run_command(&demo, &["--role", "implementing", "1"]);
    let error = result.unwrap_err().text();
    assert!(
        error.contains("projects/demo.md") && error.contains("## Hard limits"),
        "{error}"
    );
    assert!(rows(&demo, "SELECT session FROM agent").is_empty());
}

#[test]
fn plan_refuses_items_that_do_not_run_together_and_flags_that_do_not_fit_the_role() {
    let demo = project_demo("agent-run-plan");
    let one = demo.add_side(1, "agent A", "work");
    let two = demo.add_side(2, "agent B", "other");
    let brief = demo.brief("b.md");
    take(&demo, &["1"], &one, "work", &brief);
    let lists = demo.store.load().unwrap();
    let ids = [Id(1), Id(2)];
    let not_running = refusal(plan(&lists, AgentRole::Implementing, &ids, None));
    assert!(not_running.contains("#2 is not running"), "{not_running}");
    take(&demo, &["2"], &two, "other", &brief);
    let lists = demo.store.load().unwrap();
    let apart = refusal(plan(&lists, AgentRole::Implementing, &ids, None));
    assert!(apart.contains("run in different worktrees"), "{apart}");
    let with_brief = refusal(plan(
        &lists,
        AgentRole::Implementing,
        &[Id(1)],
        Some("x.md"),
    ));
    assert!(with_brief.contains("--brief"), "{with_brief}");
    let running_planned = refusal(plan(&lists, AgentRole::Planner, &[Id(1)], Some("x.md")));
    assert!(
        running_planned.contains("no agent runs"),
        "{running_planned}"
    );
    let no_brief = refusal(plan(&lists, AgentRole::Planner, &[Id(1)], None));
    assert!(no_brief.contains("--brief"), "{no_brief}");
    let single = plan(&lists, AgentRole::Implementing, &[Id(1)], None).unwrap();
    assert_eq!(single.tier, Tier::Implementation);
    assert_eq!(single.work.places().len(), 1);
    assert_eq!(single.brief, brief);
}

/// Runs an agent on item 1 in a demo whose item 2 also runs in its worktree,
/// and returns the demo with the agent's session id.
fn ended_agent(name: &str) -> (Demo, String) {
    let mut demo = project_demo(name);
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    take(&demo, &["1", "2"], &side, "work", &brief);
    let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
    fake_claude(&mut demo, &[&init, &turn(2, 100, 1000), RESULT], 0);
    let (result, out) = run_command(&demo, &["--role", "implementing", "1"]);
    result.unwrap();
    let session = out
        .split(':')
        .next()
        .unwrap()
        .trim_start_matches("agent ")
        .to_string();
    (demo, session)
}

fn resume_command(demo: &Demo, args: &[&str]) -> (Result<()>, String) {
    let mut out = Vec::new();
    let result = resume(&demo.tool, &words(&args[1..]), &demo.scratch.dir, &mut out);
    (result, String::from_utf8(out).unwrap())
}

fn record_running(demo: &Demo, session: &str, socket: &std::path::Path) {
    let sessions = demo.tool.places.claude_sessions();
    std::fs::create_dir_all(&sessions).unwrap();
    let me = std::process::id();
    std::fs::write(
        sessions.join(format!("{me}.json")),
        format!(
            r#"{{"pid":{me},"sessionId":"{session}","startedAt":{},"messagingSocketPath":"{}"}}"#,
            crate::job_claim::now_nanos() / 1_000_000,
            socket.display()
        ),
    )
    .unwrap();
}

fn message_file(demo: &Demo, text: &str) -> String {
    let path = demo.scratch.dir.join("message.txt");
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn resume_sends_the_message_and_the_new_items_to_the_inbox_of_a_running_session() {
    let (demo, session) = ended_agent("ar-inbox");
    let socket = demo.scratch.dir.join("s.sock");
    let inbox = crate::testing::FakeInbox::listen(&socket);
    record_running(&demo, &session, &socket);
    let message = message_file(&demo, "Take item 2 next.\n");
    let (result, out) = resume_command(
        &demo,
        &[
            "resume",
            &session,
            "--item",
            "2",
            "--message-file",
            &message,
        ],
    );
    result.unwrap();
    assert!(out.contains("reached its inbox"), "{out}");
    let lines = inbox.received();
    assert_eq!(lines.len(), 1, "{lines:?}");
    for part in [
        "Take item 2 next.",
        "#2 Do the second thing now",
        "--done 1 --done 2",
        "Co-Authored-By: Claude Sonnet 5.5",
    ] {
        assert!(lines[0].contains(part), "{part}: {}", lines[0]);
    }
    assert_eq!(
        rows(&demo, "SELECT item FROM agent_item ORDER BY item"),
        ["Integer(1)", "Integer(2)"]
    );
    assert_eq!(
        rows(&demo, "SELECT number FROM agent_process"),
        ["Integer(1)"]
    );
}

#[test]
fn resume_runs_claude_with_resume_when_no_session_runs_or_the_inbox_is_gone() {
    for gone in [false, true] {
        let (mut demo, session) = ended_agent(if gone { "ar-gone" } else { "ar-ended" });
        if gone {
            let socket = demo.scratch.dir.join("s.sock");
            crate::testing::stale_socket(&socket);
            record_running(&demo, &session, &socket);
        }
        let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
        let dir = fake_claude(&mut demo, &[&init, RESULT], 0);
        let message = message_file(&demo, "Go on.\n");
        let (result, out) =
            resume_command(&demo, &["resume", &session, "--message-file", &message]);
        result.unwrap();
        assert!(out.contains("Landed abc."), "{out}");
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[args.len() - 2..], ["--resume", session.as_str()]);
        assert!(!args.contains(&"--session-id"));
        assert!(
            std::fs::read_to_string(dir.join("prompt.txt"))
                .unwrap()
                .contains("Go on.")
        );
        assert_eq!(
            rows(
                &demo,
                "SELECT number, exit_status FROM agent_process ORDER BY number"
            ),
            ["Integer(1)|Integer(0)", "Integer(2)|Integer(0)"]
        );
    }
}

/// The finished agent's items close, and item 3 is taken in its worktree on a
/// new branch, as every take after a landing is.
#[test]
fn resume_takes_an_item_on_a_new_branch_of_the_agents_worktree_and_records_it() {
    for inbox in [false, true] {
        let (mut demo, session) = ended_agent(if inbox { "ar-new-in" } else { "ar-new" });
        for id in ["1", "2"] {
            cmd_item(
                &demo.tool,
                &crate::testing::no_ssh(),
                &words(&["finished", id, "--made", "the work"]),
            )
            .unwrap();
        }
        add_item(&demo, "Do the third thing now");
        let side = demo.main.parent().unwrap().join("demo-worktrees/side-1");
        crate::app_config::run_git(&side, &["switch", "-q", "-c", "next"]).unwrap();
        let brief = demo.brief("next.md");
        take(&demo, &["3"], &side, "next", &brief);
        let socket = demo.scratch.dir.join("s.sock");
        let listener = inbox.then(|| crate::testing::FakeInbox::listen(&socket));
        if inbox {
            record_running(&demo, &session, &socket);
        }
        let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
        let dir = fake_claude(&mut demo, &[&init, RESULT], 0);
        let (result, _) = resume_command(&demo, &["resume", &session, "--item", "3"]);
        result.unwrap();
        let message = match listener {
            Some(listener) => listener.received().remove(0),
            None => std::fs::read_to_string(dir.join("prompt.txt")).unwrap(),
        };
        assert!(message.contains("on branch next"), "{message}");
        assert!(message.contains("--done 3 --project demo"), "{message}");
        assert!(!message.contains("--done 1"), "{message}");
        let conn = db::open(&demo.store.database(), db::Make::Never).unwrap();
        let agent = agent_record::read(&conn, &session).unwrap().unwrap();
        let branches: Vec<&str> = agent
            .work
            .places()
            .iter()
            .map(|place| place.branch.as_str())
            .collect();
        assert_eq!(branches, ["next"]);
        assert_eq!(agent.items, [1, 3]);
    }
}

#[test]
fn resume_refuses_nothing_to_say_an_unknown_agent_and_an_item_outside_the_worktree() {
    let (demo, session) = ended_agent("ar-refuse");
    let (nothing, _) = resume_command(&demo, &["resume", &session]);
    assert!(
        nothing
            .unwrap_err()
            .text()
            .contains("--item N or --message-file F")
    );
    let message = message_file(&demo, "x");
    let (unknown, _) = resume_command(&demo, &["resume", "nope", "--message-file", &message]);
    assert!(
        unknown
            .unwrap_err()
            .text()
            .contains("records no agent nope")
    );
    add_item(&demo, "Do the third thing now");
    let (outside, _) = resume_command(&demo, &["resume", &session, "--item", "3"]);
    assert!(
        outside
            .unwrap_err()
            .text()
            .contains("does not run in a worktree")
    );
}

/// An ended agent whose items are all closed can still take a message and its
/// area's next item, so the cleanup that setup and prune run keeps its folder.
#[test]
fn resume_finds_the_folder_of_a_finished_agent_after_the_shared_prune() {
    let (mut demo, session) = ended_agent("ar-pruned");
    for id in ["1", "2"] {
        cmd_item(
            &demo.tool,
            &crate::testing::no_ssh(),
            &words(&["finished", id, "--made", "the work"]),
        )
        .unwrap();
    }
    let folder = demo.tool.places.agents().join(&session);
    std::fs::write(folder.join("scratch/note.txt"), "kept\n").unwrap();
    crate::prune::prune_shared(
        &demo.tool,
        crate::prune::Run::Real,
        crate::prune::Pruner::Prune,
    )
    .unwrap();
    let init = init_line(TOOLS_JSON, "[]", "[]", "claude-sonnet-5-5");
    fake_claude(&mut demo, &[&init, RESULT], 0);
    let message = message_file(&demo, "Go on.\n");
    let (result, _) = resume_command(&demo, &["resume", &session, "--message-file", &message]);
    result.unwrap();
    assert!(folder.join("scratch/note.txt").is_file());
}
