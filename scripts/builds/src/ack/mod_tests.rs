use super::*;
use crate::lists::fixtures::{NOON, make_store_named};
use crate::testing::{Scratch, repo};
use std::path::PathBuf;

const SESSION_ID: &str = "session-1";

/// A scratch home with one rule for every file and a ~/.claude/CLAUDE.md,
/// project bae's store, and a checkout of repo bae claimed by `agent`.
struct World {
    scratch: Scratch,
    places: Places,
    checkout: PathBuf,
}

fn world(name: &str) -> World {
    let scratch = Scratch::new(name);
    let places = scratch.places();
    let root = scratch.dir.join("agent-instructions");
    std::fs::create_dir_all(root.join("rules/text")).unwrap();
    std::fs::write(
        root.join("rules/text/plain.md"),
        "---\npaths:\n  - '**/*'\n---\n\nWrite plain sentences.\n",
    )
    .unwrap();
    std::fs::create_dir_all(places.home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&root, places.home.join(".codex/agent-instructions")).unwrap();
    std::fs::create_dir_all(places.home.join(".claude")).unwrap();
    std::fs::write(places.home.join(".claude/CLAUDE.md"), "Global rules.\n").unwrap();
    make_store_named(&places, "bae", &["bae"]);
    let checkout = scratch.dir.join("bae");
    repo(&checkout, "target 1\n");
    let found = Checkout::at(&checkout).unwrap().unwrap();
    crate::claim::write_claim(&found, "agent", NOON, None, None).unwrap();
    World {
        scratch,
        places,
        checkout,
    }
}

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        fail::Fail::Message(message) => message,
        fail::Fail::Exit(code) => panic!("exit {code}"),
    }
}

impl World {
    fn ack(&self, args: &[&str], now: i64) -> Result<String> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let args = Args::parse(&args, &[], &["compacted"])?;
        ack(&self.places, SESSION_ID, &args, &self.checkout, now)
    }

    fn check(&self, now: i64) -> Result<()> {
        check(&self.places, SESSION_ID, None, None, &self.checkout, now)
    }

    /// Reads every part as an agent does and returns the version and every
    /// check word, in order.
    fn read_all(&self, now: i64) -> (String, Vec<String>) {
        let started = self.ack(&[], now).unwrap();
        let version = started
            .split("The text is version ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .to_string();
        let count = started.matches("  builds ack ").count() - 1;
        let mut words = Vec::new();
        for part in 1..=count {
            let shown = self.ack(&[&part.to_string()], now).unwrap();
            for line in shown.lines() {
                if let Some((_, word)) = line.split_once(&format!("IN PART {part}: ")) {
                    words.push(word.to_string());
                }
            }
        }
        (version, words)
    }

    fn acknowledge(&self, now: i64) {
        let (version, words) = self.read_all(now);
        let mut args = vec![version.as_str()];
        args.extend(words.iter().map(String::as_str));
        self.ack(&args, now).unwrap();
    }
}

#[test]
fn an_agent_that_has_not_acknowledged_is_refused_with_the_ack_command() {
    let world = world("ack-refused");
    let refusal = message(world.check(NOON));
    assert!(
        refusal.contains("agent has not acknowledged it"),
        "{refusal}"
    );
    assert!(refusal.contains("`builds ack --by agent`"), "{refusal}");
}

#[test]
fn an_agent_that_read_every_part_and_gave_every_word_passes() {
    let world = world("ack-passes");
    world.acknowledge(NOON);
    world.check(NOON + 60).unwrap();
}

#[test]
fn a_wrong_or_missing_word_is_refused_naming_its_part() {
    let world = world("ack-wrong-word");
    let (version, mut words) = world.read_all(NOON);
    assert!(words.len() >= 3 * 2, "the workflow alone fills two parts");
    words[1] = "wrongword".to_string();
    words.truncate(words.len() - 1);
    let mut args = vec![version.as_str()];
    args.extend(words.iter().map(String::as_str));
    let last = words.len() / 3 + 1;
    let refusal = message(world.ack(&args, NOON));
    assert!(
        refusal.contains(&format!("part 1, {last} are missing or wrong")),
        "{refusal}"
    );
    assert!(message(world.check(NOON)).contains("has not acknowledged"));
}

#[test]
fn reading_a_part_twice_keeps_its_words_and_a_new_reading_draws_new_ones() {
    let world = world("ack-words-kept");
    world.ack(&[], NOON).unwrap();
    let first = world.ack(&["1"], NOON).unwrap();
    assert_eq!(world.ack(&["1"], NOON).unwrap(), first);
    world.ack(&[], NOON).unwrap();
    assert_ne!(world.ack(&["1"], NOON).unwrap(), first);
}

#[test]
fn every_part_stays_well_under_the_size_claude_code_shows_whole() {
    let world = world("ack-part-size");
    let started = world.ack(&[], NOON).unwrap();
    let count = started.matches("  builds ack ").count() - 1;
    for part in 1..=count {
        let shown = world.ack(&[&part.to_string()], NOON).unwrap();
        assert!(
            shown.len() < 25_000,
            "part {part} has {} bytes",
            shown.len()
        );
    }
}

#[test]
fn an_acknowledgement_older_than_twelve_hours_is_refused() {
    let world = world("ack-old");
    world.acknowledge(NOON);
    world.check(NOON + HOURS * 3600).unwrap();
    let refusal = message(world.check(NOON + HOURS * 3600 + 1));
    assert!(refusal.contains("more than 12 hours ago"), "{refusal}");
}

#[test]
fn an_edit_of_a_rule_refuses_the_agent_naming_the_rule_and_its_reading_shows_it_first() {
    let world = world("ack-edited");
    world.acknowledge(NOON);
    std::fs::write(
        world
            .scratch
            .dir
            .join("agent-instructions/rules/text/plain.md"),
        "---\npaths:\n  - '**/*'\n---\n\nWrite plain, literal sentences.\n",
    )
    .unwrap();
    let refusal = message(world.check(NOON + 60));
    assert!(refusal.contains("rules/text/plain.md changed"), "{refusal}");
    world.ack(&[], NOON + 60).unwrap();
    let first = world.ack(&["1"], NOON + 60).unwrap();
    assert!(
        first.contains("===== rules/text/plain.md (section 1 of"),
        "{first}"
    );
}

#[test]
fn a_compaction_ends_the_sessions_acknowledgements() {
    let world = world("ack-compacted");
    world.acknowledge(NOON);
    let told = compacted(
        &world.places,
        &format!(r#"{{"session_id": "{SESSION_ID}", "source": "compact"}}"#),
    )
    .unwrap();
    assert!(told.contains("builds ack"), "{told}");
    assert!(message(world.check(NOON + 60)).contains("has not acknowledged"));
}

#[test]
fn an_agent_outside_a_claimed_worktree_is_named_by_by_or_its_last_reading() {
    let world = world("ack-named");
    let elsewhere = world.scratch.dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let refused = message(check(
        &world.places,
        SESSION_ID,
        None,
        Some("bae"),
        &elsewhere,
        NOON,
    ));
    assert!(refused.contains("say who you are"), "{refused}");
    let args: Vec<String> = ["--by", "coordinator", "--project", "bae"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let started = ack(
        &world.places,
        SESSION_ID,
        &Args::parse(&args, &[], &[]).unwrap(),
        &elsewhere,
        NOON,
    )
    .unwrap();
    assert!(
        started.contains("builds ack 1 --by coordinator --project bae"),
        "{started}"
    );
    let refused = message(check(
        &world.places,
        SESSION_ID,
        None,
        Some("bae"),
        &elsewhere,
        NOON,
    ));
    assert!(
        refused.contains("coordinator has not acknowledged"),
        "{refused}"
    );
}

#[test]
fn the_gate_reads_by_and_project_before_the_end_of_the_flags() {
    let args: Vec<String> = ["show", "--by", "me", "--", "--project", "x"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    assert_eq!(flag(&args, "by"), Some("me"));
    assert_eq!(flag(&args, "project"), None);
}

#[test]
fn the_dashboard_lists_each_agents_last_acknowledgement() {
    let world = world("ack-dashboard");
    world.acknowledge(NOON);
    let json: serde_json::Value = serde_json::from_str(&dashboard_json(&world.places)).unwrap();
    assert_eq!(json["hours"], HOURS);
    let acks = &json["projects"][0]["acks"];
    assert_eq!(acks[0]["who"], "agent");
    assert_eq!(acks[0]["at"], NOON);
}

#[test]
fn a_claim_that_names_a_project_without_the_repo_is_refused_even_with_project() {
    let world = world("ack-bad-claim");
    let checkout = Checkout::at(&world.checkout).unwrap().unwrap();
    std::fs::write(checkout.project_file(), "other\n").unwrap();
    let refusal = message(check(
        &world.places,
        SESSION_ID,
        None,
        Some("bae"),
        &world.checkout,
        NOON,
    ));
    assert!(refusal.contains("does not hold repo bae"), "{refusal}");
}
