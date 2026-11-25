use super::*;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
use crate::lists::{Id, Running};
use std::path::PathBuf;

fn argv(words: &str) -> Vec<String> {
    words.split_whitespace().map(str::to_string).collect()
}

fn agent(pid: i32, place: usize, command: &str) -> Agent {
    Agent {
        pid,
        provider: Provider::Claude,
        place,
        argv: argv(command),
    }
}

/// side-1 (place 0) and side-2 (place 1) of repo bae.
fn places() -> Vec<Place> {
    ["side-1", "side-2"]
        .map(|name| Place {
            label: format!("bae/{name}"),
            repo: "bae".into(),
            top: PathBuf::from(format!("/Users/x/dev/bae-worktrees/{name}")),
            root: PathBuf::from("/Users/x/dev/bae"),
        })
        .to_vec()
}

/// Lists where items run in side-1: one item per entry of <count>.
fn running_in_side_1(count: usize) -> Lists {
    let db = db();
    for _ in 0..count {
        let id = item(&db, vec![]);
        crate::lists::running::start(
            &db,
            &[id],
            &[Running {
                worktree: "/Users/x/dev/bae-worktrees/side-1".into(),
                branch: "work".into(),
                brief: "b.md".into(),
                since: stamp(NOON),
            }],
        )
        .unwrap();
    }
    read(&db)
}

fn judged(agents: &[Agent], lists: &Lists) -> Vec<Advice> {
    findings(agents, &places(), &[lists])
}

#[test]
fn an_implementing_agent_on_the_strongest_tier_gets_a_line() {
    let lists = running_in_side_1(1);
    let id = lists.items[0].id;
    let lines = judged(
        &[agent(7, 0, "claude --model claude-opus-5-5 -p brief")],
        &lists,
    );
    assert_eq!(
        lines,
        [Advice::said(format!(
            "model: the agent (pid 7) in bae/side-1 for item {} runs claude-opus-5-5, and an implementing agent takes claude-sonnet-5-5. Stop the agent at its next clean point and continue the item with an agent that runs claude-sonnet-5-5",
            id.0
        ))]
    );
}

#[test]
fn an_implementing_agent_that_names_no_model_gets_a_line() {
    let lines = judged(&[agent(7, 0, "claude -p brief")], &running_in_side_1(1));
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
        assert!(judged(&[agent(7, 0, command)], &running_in_side_1(1)).is_empty());
    }
}

#[test]
fn a_process_in_a_worktree_where_no_item_runs_gets_no_line() {
    let lines = judged(
        &[agent(9, 1, "claude --model claude-opus-5-5")],
        &running_in_side_1(1),
    );
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn codex_takes_its_own_implementation_model() {
    let codex = |command: &str| Agent {
        provider: Provider::Codex,
        ..agent(7, 0, command)
    };
    let lists = running_in_side_1(1);
    assert!(judged(&[codex("codex -m gpt-6.2-sol exec")], &lists).is_empty());
    let lines = judged(&[codex("codex --model gpt-6.2-astra exec")], &lists);
    assert!(lines[0].text.contains("runs gpt-6.2-astra"), "{lines:?}");
    assert!(lines[0].text.contains("takes gpt-6.2-sol"), "{lines:?}");
}

#[test]
fn several_items_in_one_worktree_are_all_named() {
    let lists = running_in_side_1(2);
    let ids: Vec<Id> = lists.items.iter().map(|i| i.id).collect();
    let lines = judged(&[agent(7, 0, "claude")], &lists);
    assert!(
        lines[0]
            .text
            .contains(&format!("items {}, {}", ids[0].0, ids[1].0)),
        "{lines:?}"
    );
}
