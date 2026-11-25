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
    running_in_side_1_at(&vec![Tier::Implementation; count])
}

/// Lists where one item runs in side-1 for each of <tiers>, at its tier.
fn running_in_side_1_at(tiers: &[Tier]) -> Lists {
    let db = db();
    for tier in tiers {
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
        crate::lists::running::set_tier(&db, id, *tier).unwrap();
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
            "model: the agent (pid 7) in bae/side-1 for item {} runs claude-opus-5-5, and an item of the implementation tier takes claude-sonnet-5-5. Stop the agent at its next clean point and continue the item with an agent that runs claude-sonnet-5-5",
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

#[test]
fn read_places_a_session_process_by_its_working_folder() {
    let here = std::env::current_dir().unwrap();
    let place = Place {
        label: "bae/side-1".into(),
        repo: "bae".into(),
        top: here.clone(),
        root: here,
    };
    let own = std::process::id() as i32;
    let processes = Processes::read().unwrap();
    let (agents, unread) = super::read(&processes, &HashSet::from([own]), &[place]);
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(
        agents
            .iter()
            .map(|a| (a.pid, a.provider, a.place))
            .collect::<Vec<_>>(),
        [(own, Provider::Claude, 0)]
    );
    // A process outside every place, or in no session, is no agent here.
    assert!(
        super::read(&processes, &HashSet::from([own]), &[])
            .0
            .is_empty()
    );
    assert!(
        super::read(&processes, &HashSet::new(), &places())
            .0
            .is_empty()
    );
}

#[test]
fn model_findings_judge_the_agents_a_machine_read() {
    let lists = running_in_side_1(1);
    let loaded = Ok(vec![crate::lists::store::Loaded {
        project: "demo".into(),
        lists: Ok(lists),
    }]);
    let found = crate::health::model_findings(
        &[agent(7, 0, "claude --model claude-opus-5-5")],
        &places(),
        &loaded,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, crate::health::FindingKind::Model);
    assert!(crate::health::model_findings(&[], &places(), &loaded).is_empty());
}

fn process(pid: i32, ppid: i32, name: &str) -> crate::procs::Process {
    crate::procs::Process {
        pid,
        ppid,
        uid: 501,
        name: name.into(),
        started: 0,
    }
}

#[test]
fn a_model_call_that_builds_started_is_no_agent() {
    let here = std::env::current_dir().unwrap();
    let place = Place {
        label: "bae/side-1".into(),
        repo: "bae".into(),
        top: here.clone(),
        root: here,
    };
    let own = std::process::id() as i32;
    let sessions = HashSet::from([own]);
    // `builds review` (pid 10) runs this process as a reviewer.
    let reviewer = Processes::of(vec![process(10, 1, "builds"), process(own, 10, "2.1.332")]);
    assert!(
        super::read(&reviewer, &sessions, std::slice::from_ref(&place))
            .0
            .is_empty()
    );
    // A coordinator's Claude Code session (pid 20) runs it as an agent.
    let agent = Processes::of(vec![process(20, 1, "2.1.332"), process(own, 20, "2.1.332")]);
    assert_eq!(super::read(&agent, &sessions, &[place]).0.len(), 1);
}

#[test]
fn an_agent_on_the_strongest_tier_works_an_item_that_takes_it() {
    let lists = running_in_side_1_at(&[Tier::Strongest]);
    assert!(judged(&[agent(7, 0, "claude --model claude-opus-5-5")], &lists).is_empty());
    let lines = judged(&[agent(7, 0, "claude --model claude-sonnet-5-5")], &lists);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0]
            .text
            .contains("an item of the strongest tier takes claude-opus-5-5"),
        "{lines:?}"
    );
}

#[test]
fn a_worktree_whose_items_name_two_tiers_is_judged_by_the_implementation_tier() {
    let lists = running_in_side_1_at(&[Tier::Strongest, Tier::Implementation]);
    assert!(judged(&[agent(7, 0, "claude --model claude-sonnet-5-5")], &lists).is_empty());
    assert_eq!(
        judged(&[agent(7, 0, "claude --model claude-opus-5-5")], &lists).len(),
        1
    );
}
