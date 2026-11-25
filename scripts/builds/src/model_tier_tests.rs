use super::*;

fn argv(words: &str) -> Vec<String> {
    words.split_whitespace().map(str::to_string).collect()
}

fn agent(pid: i32, cwd: &str, command: &str) -> Agent {
    Agent {
        pid,
        client: Client::Claude,
        cwd: PathBuf::from(cwd),
        argv: argv(command),
    }
}

fn worktrees() -> Vec<Worktree> {
    vec![
        Worktree {
            top: PathBuf::from("/Users/x/dev/builds-worktrees/side-1"),
            label: "builds/side-1".into(),
            items: vec![Id(662)],
        },
        Worktree {
            top: PathBuf::from("/Users/x/dev/builds-worktrees/side-2"),
            label: "builds/side-2".into(),
            items: Vec::new(),
        },
    ]
}

const SIDE_1: &str = "/Users/x/dev/builds-worktrees/side-1";

#[test]
fn an_implementing_agent_on_the_strongest_tier_gets_a_line() {
    let lines = findings(
        &[agent(7, SIDE_1, "claude --model claude-opus-5-5 -p brief")],
        &worktrees(),
    );
    assert_eq!(
        lines,
        [Advice::said(
            "model: the agent (pid 7) in builds/side-1 for item 662 runs claude-opus-5-5, and an implementing agent takes claude-sonnet-5-5. Stop the agent at its next clean point and continue the item with an agent that runs claude-sonnet-5-5"
                .to_string()
        )]
    );
}

#[test]
fn an_implementing_agent_that_names_no_model_gets_a_line() {
    let lines = findings(&[agent(7, SIDE_1, "claude -p brief")], &worktrees());
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.contains("names no model"), "{lines:?}");
    assert!(lines[0].text.contains("claude-sonnet-5-5"), "{lines:?}");
}

#[test]
fn an_implementing_agent_on_the_implementation_tier_gets_no_line() {
    for command in [
        "claude --model claude-sonnet-5-5 -p brief",
        "claude --model=claude-sonnet-5-5",
    ] {
        assert!(findings(&[agent(7, SIDE_1, command)], &worktrees()).is_empty());
    }
}

#[test]
fn a_process_outside_a_worktree_where_an_item_runs_gets_no_line() {
    let outside = agent(8, "/Users/x/dev/builds", "claude --model claude-opus-5-5");
    let idle = agent(
        9,
        "/Users/x/dev/builds-worktrees/side-2/scripts",
        "claude --model claude-opus-5-5",
    );
    assert!(findings(&[outside, idle], &worktrees()).is_empty());
}

#[test]
fn codex_takes_its_own_implementation_model() {
    let codex = |command: &str| Agent {
        client: Client::Codex,
        ..agent(7, SIDE_1, command)
    };
    assert!(findings(&[codex("codex -m gpt-6.2-sol exec")], &worktrees()).is_empty());
    let lines = findings(&[codex("codex --model gpt-6.2-astra exec")], &worktrees());
    assert!(lines[0].text.contains("runs gpt-6.2-astra"), "{lines:?}");
    assert!(lines[0].text.contains("takes gpt-6.2-sol"), "{lines:?}");
}

#[test]
fn several_items_in_one_worktree_are_all_named() {
    let mut many = worktrees();
    many[0].items.push(Id(700));
    let lines = findings(&[agent(7, SIDE_1, "claude")], &many);
    assert!(lines[0].text.contains("items 662, 700"), "{lines:?}");
}
