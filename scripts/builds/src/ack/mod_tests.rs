use super::*;
use crate::lists::fixtures::{NOON, make_store_named};
use crate::lists::store::project_of;
use crate::testing::{Scratch, repo};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "session-1";

/// The agent most tests run as, which `world` registers.
const AGENT: &str = "a1";

/// A scratch home with one rule for every file and a ~/.claude/CLAUDE.md,
/// project bae's store of repos bae and coven, a checkout of repo bae with no
/// CLAUDE.md and one of repo coven with its own, each claimed by `agent`, and
/// agent `AGENT` registered in session `SESSION_ID`.
struct World {
    scratch: Scratch,
    places: Places,
    checkout: PathBuf,
    coven: PathBuf,
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
    make_store_named(&places, "bae", &["bae", "coven"]);
    let checkout = scratch.dir.join("bae");
    let coven = scratch.dir.join("coven");
    for top in [&checkout, &coven] {
        repo(top, "target 1\n");
        let found = Checkout::at(top).unwrap().unwrap();
        crate::claim::write_claim(&found, "agent", NOON, None).unwrap();
    }
    std::fs::write(coven.join("CLAUDE.md"), "Coven rules.\n").unwrap();
    agent::register(&places, SESSION_ID, AGENT).unwrap();
    World {
        scratch,
        places,
        checkout,
        coven,
    }
}

/// The made-up words of each part of the reading <agent> started, from
/// project <project>'s store.
fn stored_words(places: &Places, project: &str, agent: &str) -> Vec<Vec<String>> {
    let store = Store::named(places, project).unwrap();
    let db = db::open_reader(&store.database()).unwrap().unwrap();
    record::reading(&db, SESSION_ID, agent)
        .unwrap()
        .unwrap()
        .words
        .into_iter()
        .map(|part| part.into_iter().map(|made| made.word).collect())
        .collect()
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
        self.ack_in(&self.checkout, agent, args, now)
    }

    /// `builds ack <args>` as <agent> runs it in <cwd>.
    fn ack_in(&self, cwd: &Path, agent: &str, args: &[&str], now: i64) -> Result<String> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let args = Args::parse(&args, &[], &["compacted"])?;
        ack(&self.places, SESSION_ID, Some(agent), &args, cwd, now)
    }

    fn check(&self, now: i64) -> Result<()> {
        self.check_as(AGENT, now)
    }

    fn check_as(&self, agent: &str, now: i64) -> Result<()> {
        self.check_in(&self.checkout, agent, now)
    }

    /// The gate for a command <agent> runs in <cwd>.
    fn check_in(&self, cwd: &Path, agent: &str, now: i64) -> Result<()> {
        check(&self.places, SESSION_ID, Some(agent), None, cwd, now)
    }

    /// Reads every part as <agent> does and returns the version and every
    /// made-up word, in order.
    fn read_all_as(&self, agent: &str, now: i64) -> (String, Vec<String>) {
        self.read_all_in(&self.checkout, agent, now)
    }

    /// Reads every part as <agent> does in <cwd> and returns the version and
    /// every made-up word, in order.
    fn read_all_in(&self, cwd: &Path, agent: &str, now: i64) -> (String, Vec<String>) {
        let started = self.ack_in(cwd, agent, &[], now).unwrap();
        let version = started
            .split("The text is version ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .to_string();
        let count = started.matches("  builds ack ").count() - 1;
        for part in 1..=count {
            self.ack_in(cwd, agent, &[&part.to_string()], now).unwrap();
        }
        (version, stored_words(&self.places, "bae", agent).concat())
    }

    fn read_all(&self, now: i64) -> (String, Vec<String>) {
        self.read_all_as(AGENT, now)
    }

    fn acknowledge(&self, now: i64) {
        self.acknowledge_as(AGENT, now);
    }

    fn acknowledge_as(&self, agent: &str, now: i64) {
        self.acknowledge_in(&self.checkout, agent, now);
    }

    /// Reads every part as <agent> does in <cwd>, and acknowledges the text.
    fn acknowledge_in(&self, cwd: &Path, agent: &str, now: i64) {
        let (version, words) = self.read_all_in(cwd, agent, now);
        let mut args = vec![version.as_str()];
        args.extend(words.iter().map(String::as_str));
        self.ack_in(cwd, agent, &args, now).unwrap();
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

/// The words of <text> as `builds ack` prints its parts: each section's text
/// and the line that names it.
fn words_of_text(text: &Text) -> std::collections::HashSet<String> {
    let count = text.sections.len();
    text.sections
        .iter()
        .enumerate()
        .flat_map(|(index, section)| {
            [
                section.text.clone(),
                format!(
                    "{} section {} of {count} continued",
                    section.name,
                    index + 1
                ),
            ]
        })
        .flat_map(|text| {
            text.split(|c: char| !c.is_ascii_alphabetic())
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A reader passes: the words a part shows that the text lacks are the part's
/// made-up words, in order, and the part shows every line of the text with
/// nothing else changed, so the answer copies no word of the text.
#[test]
fn the_words_a_part_shows_that_the_text_lacks_are_its_made_up_words() {
    let world = world("ack-made-up");
    world.ack(&[], NOON).unwrap();
    let checkout = Checkout::at(&world.checkout).unwrap();
    let text = Text::read(&world.places, "bae", checkout.as_ref()).unwrap();
    let known = words_of_text(&text);
    let stored = stored_words(&world.places, "bae", AGENT);
    let parts = text.parts(
        &text
            .sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
    );
    assert!(parts.len() >= 2, "the workflow alone fills two parts");
    for (index, lines) in parts.iter().enumerate() {
        let shown = world.ack(&[&(index + 1).to_string()], NOON).unwrap();
        let body: Vec<&str> = shown.lines().skip(1).take(lines.len()).collect();
        let unknown: Vec<String> = body
            .iter()
            .flat_map(|line| line.split(|c: char| !c.is_ascii_alphabetic()))
            .filter(|word| !word.is_empty() && !known.contains(*word))
            .map(str::to_string)
            .collect();
        assert_eq!(unknown, stored[index], "part {}", index + 1);
        let mut plain: Vec<String> = body.iter().map(|line| line.to_string()).collect();
        for word in &stored[index] {
            let line = plain
                .iter_mut()
                .find(|line| line.contains(word.as_str()))
                .unwrap();
            *line = line.replacen(&format!("{word} "), "", 1);
        }
        assert_eq!(&plain, lines, "part {}", index + 1);
    }
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
    let last = stored_words(&world.places, "bae", AGENT).len();
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
        let store = project_of(&world.places, &checkout, None).unwrap().unwrap();
        let db = db::open(
            &Store::named(&world.places, &store).unwrap().database(),
            Make::Never,
        )
        .unwrap();
        for n in 0..record::KEPT_VERSIONS {
            record::acknowledge(
                &db,
                "s2",
                "other",
                None,
                &format!("v{n}"),
                NOON + n + 1,
                &[],
            )
            .unwrap();
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
fn an_agent_that_acknowledged_two_repos_texts_runs_commands_in_either_without_reading_again() {
    let world = world("ack-two-repos");
    world.acknowledge(NOON);
    let refusal = message(world.check_in(&world.coven, AGENT, NOON + 60));
    assert!(
        refusal.contains(
            "agent a1 has not acknowledged the text that holds the CLAUDE.md of repo coven in this Claude Code session"
        ),
        "{refusal}"
    );
    world.acknowledge_in(&world.coven, AGENT, NOON + 60);
    world.check(NOON + 120).unwrap();
    world.check_in(&world.coven, AGENT, NOON + 120).unwrap();
}

#[test]
fn an_edit_of_one_repos_claude_md_asks_again_only_in_that_repo_and_shows_only_that_file() {
    let world = world("ack-two-repos-edited");
    world.acknowledge(NOON);
    world.acknowledge_in(&world.coven, AGENT, NOON);
    std::fs::write(
        world.coven.join("CLAUDE.md"),
        "Coven rules, edited with several ordinary words.\n",
    )
    .unwrap();
    world.check(NOON + 60).unwrap();
    let refusal = message(world.check_in(&world.coven, AGENT, NOON + 60));
    assert!(
        refusal.contains("the CLAUDE.md of repo coven changed"),
        "{refusal}"
    );
    let started = world.ack_in(&world.coven, AGENT, &[], NOON + 60).unwrap();
    assert!(
        started.contains("1 of the text's 5 sections changed: the CLAUDE.md of repo coven"),
        "{started}"
    );
    world.acknowledge_in(&world.coven, AGENT, NOON + 60);
    world.check_in(&world.coven, AGENT, NOON + 120).unwrap();
    world.check(NOON + 120).unwrap();
}

#[test]
fn a_compaction_clears_the_acknowledgement_of_every_repos_text() {
    let world = world("ack-two-repos-compacted");
    world.acknowledge(NOON);
    world.acknowledge_in(&world.coven, AGENT, NOON);
    world.compact(Some(AGENT)).unwrap();
    assert!(message(world.check(NOON + 60)).contains("agent a1 has not acknowledged"));
    assert!(
        message(world.check_in(&world.coven, AGENT, NOON + 60))
            .contains("agent a1 has not acknowledged")
    );
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

/// `builds ack` as the command line gives it: <args>, then the flags
/// `--project bae`.
fn ack_naming_bae(world: &World, args: &[&str]) -> Result<String> {
    let mut args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    args.extend(["--project", "bae"].map(String::from));
    ack(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        &Args::parse(&args, &[], &[]).unwrap(),
        &world.checkout,
        NOON,
    )
}

#[test]
fn project_names_the_store_in_a_checkout_of_a_repo_two_projects_hold() {
    let world = world("ack-two-projects");
    // The checkout's claim names no project, and projects bae and second both
    // hold repo bae.
    make_store_named(&world.places, "second", &["bae"]);
    let started = ack_naming_bae(&world, &[]).unwrap();
    assert!(
        started.contains("builds ack 1 --agent a1 --project bae"),
        "{started}"
    );
    let version = started
        .split("The text is version ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .to_string();
    let count = started.matches("  builds ack ").count() - 1;
    for part in 1..=count {
        ack_naming_bae(&world, &[&part.to_string()]).unwrap();
    }
    let words = stored_words(&world.places, "bae", AGENT).concat();
    // The flags follow the version and the words, as the listing prints them.
    let mut args = vec![version.as_str()];
    args.extend(words.iter().map(String::as_str));
    ack_naming_bae(&world, &args).unwrap();
    check(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        Some("bae"),
        &world.checkout,
        NOON + 60,
    )
    .unwrap();
}

#[test]
fn a_repo_two_projects_hold_is_refused_without_project_and_with_one_that_lacks_it() {
    let world = world("ack-two-projects-refused");
    make_store_named(&world.places, "second", &["bae"]);
    make_store_named(&world.places, "third", &["other"]);
    let none = message(world.ack(&[], NOON));
    assert!(none.contains("is in two projects (bae, second)"), "{none}");
    let args: Vec<String> = ["--project", "third"].map(String::from).to_vec();
    let lacking = message(ack(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        &Args::parse(&args, &[], &[]).unwrap(),
        &world.checkout,
        NOON,
    ));
    assert!(lacking.contains("does not hold repo bae"), "{lacking}");
}

/// The judge's session as `judge listen` keeps it: its main agent's id is its
/// session id.
fn judge_world(name: &str) -> World {
    let world = world(name);
    agent::register(&world.places, SESSION_ID, SESSION_ID).unwrap();
    world
}

fn outdated_at(world: &World, now: i64) -> Option<String> {
    let store = Store::named(&world.places, "bae").unwrap();
    outdated(&world.places, &store, SESSION_ID, now).unwrap()
}

#[test]
fn a_judge_that_acknowledged_the_current_text_has_nothing_to_acknowledge() {
    let world = judge_world("ack-judge-current");
    world.acknowledge_as(SESSION_ID, NOON);
    assert_eq!(outdated_at(&world, NOON + 60), None);
}

#[test]
fn a_judge_with_no_acknowledgement_is_told_so() {
    let world = judge_world("ack-judge-none");
    let why = outdated_at(&world, NOON).expect("it has acknowledged nothing");
    assert!(why.contains("has no acknowledgement"), "{why}");
}

#[test]
fn a_judge_is_told_when_its_acknowledgement_passes_its_hours() {
    let world = judge_world("ack-judge-old");
    world.acknowledge_as(SESSION_ID, NOON);
    let why = outdated_at(&world, NOON + HOURS * 3600 + 1).expect("the hours passed");
    assert!(why.contains("more than 12 hours ago"), "{why}");
}

#[test]
fn a_judge_is_told_when_the_text_changed_and_which_section() {
    let world = judge_world("ack-judge-changed");
    world.acknowledge_as(SESSION_ID, NOON);
    std::fs::write(
        world
            .scratch
            .dir
            .join("agent-instructions/rules/text/plain.md"),
        "---\npaths:\n  - '**/*'\n---\n\nWrite plain, literal sentences.\n",
    )
    .unwrap();
    let why = outdated_at(&world, NOON + 60).expect("the text changed");
    assert!(why.contains("rules/text/plain.md"), "{why}");
    // A repo's CLAUDE.md is no part of the text the judge's check reads.
    world.acknowledge_as(SESSION_ID, NOON + 120);
    assert_eq!(outdated_at(&world, NOON + 180), None);
}

#[test]
fn a_judge_that_acknowledged_in_a_repo_with_its_own_claude_md_is_current() {
    let world = judge_world("ack-judge-repo");
    world.acknowledge_in(&world.coven.clone(), SESSION_ID, NOON);
    assert_eq!(outdated_at(&world, NOON + 60), None);
}
