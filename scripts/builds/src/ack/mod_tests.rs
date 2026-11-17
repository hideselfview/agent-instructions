use super::*;
use crate::lists::fixtures::{NOON, make_store_named};
use crate::testing::{Scratch, repo};
use std::path::PathBuf;

const SESSION_ID: &str = "session-1";

/// The agent most tests run as, which `world` registers.
const AGENT: &str = "a1";

/// A scratch home with one rule for every file and a ~/.claude/CLAUDE.md,
/// project bae's store, a checkout of repo bae claimed by `agent`, and agent
/// `AGENT` registered in session `SESSION_ID`.
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
    agent::register(&places, SESSION_ID, AGENT).unwrap();
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
        self.ack_as(AGENT, args, now)
    }

    fn ack_as(&self, agent: &str, args: &[&str], now: i64) -> Result<String> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let args = Args::parse(&args, &[], &["compacted"])?;
        ack(
            &self.places,
            SESSION_ID,
            Some(agent),
            &args,
            &self.checkout,
            now,
        )
    }

    fn check(&self, now: i64) -> Result<()> {
        self.check_as(AGENT, now)
    }

    fn check_as(&self, agent: &str, now: i64) -> Result<()> {
        check(
            &self.places,
            SESSION_ID,
            Some(agent),
            None,
            &self.checkout,
            now,
        )
    }

    /// Reads every part as <agent> does and returns the version and every
    /// check word, in order.
    fn read_all_as(&self, agent: &str, now: i64) -> (String, Vec<String>) {
        let started = self.ack_as(agent, &[], now).unwrap();
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
            let shown = self.ack_as(agent, &[&part.to_string()], now).unwrap();
            for line in shown.lines() {
                if let Some((_, word)) = line.split_once(&format!("IN PART {part}: ")) {
                    words.push(word.to_string());
                }
            }
        }
        (version, words)
    }

    fn read_all(&self, now: i64) -> (String, Vec<String>) {
        self.read_all_as(AGENT, now)
    }

    fn acknowledge(&self, now: i64) {
        self.acknowledge_as(AGENT, now);
    }

    fn acknowledge_as(&self, agent: &str, now: i64) {
        let (version, words) = self.read_all_as(agent, now);
        let mut args = vec![version.as_str()];
        args.extend(words.iter().map(String::as_str));
        self.ack_as(agent, &args, now).unwrap();
    }

    /// What `builds ack --compacted` returns for the SessionStart input of a
    /// compaction of <agent>, or of the main agent when <agent> is None.
    fn compact(&self, agent: Option<&str>) -> Result<String> {
        let agent_id = agent
            .map(|id| format!(r#", "agent_id": "{id}""#))
            .unwrap_or_default();
        compacted(
            &self.places,
            agent::HookInput::parse(&format!(
                r#"{{"session_id": "{SESSION_ID}", "hook_event_name": "SessionStart", "source": "compact"{agent_id}}}"#
            ))?,
        )
    }
}

#[test]
fn an_agent_that_has_not_acknowledged_is_refused_with_the_ack_command() {
    let world = world("ack-refused");
    let refusal = message(world.check(NOON));
    assert!(
        refusal.contains("agent a1 has not acknowledged it"),
        "{refusal}"
    );
    assert!(refusal.contains("`builds ack --agent a1`"), "{refusal}");
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

impl World {
    /// Edits the rule for every file, so the text gets a new version.
    fn edit_rule(&self, body: &str) {
        std::fs::write(
            self.scratch
                .dir
                .join("agent-instructions/rules/text/plain.md"),
            format!("---\npaths:\n  - '**/*'\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    /// The number of parts in the reading that `builds ack` starts at <now>.
    fn parts(&self, now: i64) -> usize {
        self.ack(&[], now).unwrap().matches("  builds ack ").count() - 1
    }
}

#[test]
fn an_edit_of_a_rule_refuses_the_agent_naming_the_rule_and_its_reading_holds_only_that_rule() {
    let world = world("ack-edited");
    world.acknowledge(NOON);
    world.edit_rule("Write plain, literal sentences.");
    let refusal = message(world.check(NOON + 60));
    assert!(
        refusal.contains("rules/text/plain.md changed, and `builds ack` shows only those sections"),
        "{refusal}"
    );
    let started = world.ack(&[], NOON + 60).unwrap();
    assert!(
        started.contains("1 of the text's 4 sections changed: rules/text/plain.md"),
        "{started}"
    );
    let only = world.ack(&["1"], NOON + 60).unwrap();
    assert!(
        only.contains("===== rules/text/plain.md (section 1 of 1) ====="),
        "{only}"
    );
    assert!(only.contains("part 1 of 1"), "{only}");
    world.acknowledge(NOON + 60);
    world.check(NOON + 120).unwrap();
}

#[test]
fn a_reader_whose_version_builds_no_longer_keeps_reads_the_whole_text() {
    let world = world("ack-not-kept");
    world.acknowledge(NOON);
    let whole = world.parts(NOON);
    {
        let checkout = Checkout::at(&world.checkout).unwrap().unwrap();
        let store = project_of(&world.places, &checkout).unwrap().unwrap();
        let db = db::open(
            &Store::named(&world.places, &store).unwrap().database(),
            Make::Never,
        )
        .unwrap();
        for n in 0..record::KEPT_VERSIONS {
            record::acknowledge(&db, "s2", "other", &format!("v{n}"), NOON + n + 1, &[]).unwrap();
        }
    }
    world.edit_rule("Write plain, literal sentences.");
    let refusal = message(world.check(NOON + 60));
    assert!(
        refusal.contains("builds no longer keeps that version"),
        "{refusal}"
    );
    assert_eq!(world.parts(NOON + 60), whole);
}

#[test]
fn a_reader_whose_text_lost_a_section_and_changed_no_other_reads_the_whole_text() {
    let world = world("ack-section-left");
    world.acknowledge(NOON);
    std::fs::remove_file(world.places.home.join(".claude/CLAUDE.md")).unwrap();
    let refusal = message(world.check(NOON + 60));
    assert!(
        refusal.contains("no section's text changed: sections moved or left the text"),
        "{refusal}"
    );
    let started = world.ack(&[], NOON + 60).unwrap();
    assert!(!started.contains("sections changed"), "{started}");
}

#[test]
fn a_reader_whose_acknowledgement_is_older_than_twelve_hours_reads_the_whole_text() {
    let world = world("ack-old-whole");
    world.acknowledge(NOON);
    let whole = world.parts(NOON);
    world.edit_rule("Write plain, literal sentences.");
    assert_eq!(world.parts(NOON + HOURS * 3600 + 1), whole);
}

#[test]
fn two_agents_of_one_session_keep_separate_acknowledgements() {
    let world = world("ack-two-agents");
    agent::register(&world.places, SESSION_ID, "a2").unwrap();
    world.acknowledge(NOON);
    world.check(NOON + 60).unwrap();
    let refusal = message(world.check_as("a2", NOON + 60));
    assert!(
        refusal.contains("agent a2 has not acknowledged"),
        "{refusal}"
    );
    world.acknowledge_as("a2", NOON + 60);
    world.check_as("a2", NOON + 120).unwrap();
}

#[test]
fn a_compaction_of_one_agent_clears_its_acknowledgement_and_leaves_the_others() {
    let world = world("ack-compacted");
    agent::register(&world.places, SESSION_ID, "a2").unwrap();
    world.acknowledge(NOON);
    world.acknowledge_as("a2", NOON);
    let told = world.compact(Some("a2")).unwrap();
    let output: serde_json::Value = serde_json::from_str(&told).unwrap();
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("Your builds agent id is a2."), "{context}");
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    assert!(message(world.check_as("a2", NOON + 60)).contains("agent a2 has not acknowledged"));
    world.check(NOON + 60).unwrap();
}

#[test]
fn a_compaction_of_an_agent_no_hook_registered_fails_and_deletes_nothing() {
    let world = world("ack-compacted-unknown");
    world.acknowledge(NOON);
    let refusal = message(world.compact(Some("stranger")));
    assert!(
        refusal.contains("builds deleted no acknowledgement"),
        "{refusal}"
    );
    world.check(NOON + 60).unwrap();
}

#[test]
fn an_unregistered_or_missing_agent_id_is_refused_saying_how_to_find_it() {
    let world = world("ack-unregistered");
    let refusal = message(world.check_as("stranger", NOON));
    assert!(
        refusal.contains("no agent stranger in session session-1"),
        "{refusal}"
    );
    assert!(refusal.contains("Your builds agent id is"), "{refusal}");
    let refusal = message(check(
        &world.places,
        SESSION_ID,
        None,
        None,
        &world.checkout,
        NOON,
    ));
    assert!(refusal.contains("pass --agent ID"), "{refusal}");
    assert!(refusal.contains("BUILDS_AGENT=ID"), "{refusal}");
}

#[test]
fn the_main_agent_is_named_by_its_session_id_and_its_compaction_clears_it() {
    let world = world("ack-main-agent");
    let told = agent::start(
        &world.places,
        agent::HookInput::parse(&format!(
            r#"{{"session_id": "{SESSION_ID}", "hook_event_name": "SessionStart", "source": "startup"}}"#
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(
        told.contains("Your builds agent id is session-1."),
        "{told}"
    );
    world.acknowledge_as(SESSION_ID, NOON);
    world.acknowledge(NOON);
    world.check_as(SESSION_ID, NOON + 60).unwrap();
    world.compact(None).unwrap();
    assert!(message(world.check_as(SESSION_ID, NOON + 60)).contains("has not acknowledged"));
    world.check(NOON + 60).unwrap();
}

#[test]
fn an_agent_outside_a_checkout_names_the_project_with_project() {
    let world = world("ack-named");
    let elsewhere = world.scratch.dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let args: Vec<String> = ["--project", "bae"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let started = ack(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        &Args::parse(&args, &[], &[]).unwrap(),
        &elsewhere,
        NOON,
    )
    .unwrap();
    assert!(
        started.contains("builds ack 1 --agent a1 --project bae"),
        "{started}"
    );
    let refused = message(check(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        Some("bae"),
        &elsewhere,
        NOON,
    ));
    assert!(
        refused.contains("agent a1 has not acknowledged"),
        "{refused}"
    );
}

#[test]
fn the_gate_reads_project_before_the_end_of_the_flags() {
    let args: Vec<String> = ["show", "--project", "p", "--", "--project", "x"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    assert_eq!(flag(&args, "project"), Some("p"));
}

#[test]
fn a_claim_that_names_a_project_without_the_repo_is_refused_even_with_project() {
    let world = world("ack-bad-claim");
    let checkout = Checkout::at(&world.checkout).unwrap().unwrap();
    std::fs::write(checkout.project_file(), "other\n").unwrap();
    let refusal = message(check(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        Some("bae"),
        &world.checkout,
        NOON,
    ));
    assert!(refusal.contains("does not hold repo bae"), "{refusal}");
}
