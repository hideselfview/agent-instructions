use super::*;
use crate::lists::fixtures::{NOON, make_store_named};
use crate::lists::store::project_of;
use crate::testing::{Scratch, repo};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "session-1";

/// The agent most tests run as, which `world` registers.
const AGENT: &str = "a1";

/// A scratch home with one rule for every file and a ~/.claude/CLAUDE.md that
/// the text leaves out, project bae's store of repos bae and coven, a checkout of repo bae with no
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

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        fail::Fail::Message(message) => message,
        fail::Fail::Exit(code) => panic!("exit {code}"),
    }
}

/// The arguments of the command in the last line of <shown>, without its
/// flags.
fn next_command(shown: &str) -> Vec<String> {
    let line = shown
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("builds ack "))
        .unwrap();
    line.trim_start()
        .strip_prefix("builds ack ")
        .unwrap()
        .split_whitespace()
        .take_while(|word| !word.starts_with("--"))
        .map(str::to_string)
        .collect()
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

    /// Reads every part as <agent> does and returns the version and the
    /// last part's token.
    fn read_all_as(&self, agent: &str, now: i64) -> (String, String) {
        self.read_all_in(&self.checkout, agent, now)
    }

    /// Reads every part as <agent> does in <cwd>, running each command the
    /// last line of the output before gives, and returns the version and the
    /// token the acknowledging command holds.
    fn read_all_in(&self, cwd: &Path, agent: &str, now: i64) -> (String, String) {
        let mut shown = self.ack_in(cwd, agent, &[], now).unwrap();
        let version = shown
            .split("The text is version ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .to_string();
        loop {
            let command = next_command(&shown);
            if command[0] != "part" {
                assert_eq!(command[0], version);
                return (version, command[1].clone());
            }
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            shown = self.ack_in(cwd, agent, &args, now).unwrap();
        }
    }

    fn read_all(&self, now: i64) -> (String, String) {
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
        let (version, token) = self.read_all_in(cwd, agent, now);
        self.ack_in(cwd, agent, &[&version, &token], now).unwrap();
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
fn an_agent_that_fetched_every_part_in_order_and_gave_the_last_token_passes() {
    let world = world("ack-passes");
    world.acknowledge(NOON);
    world.check(NOON + 60).unwrap();
}

/// The opening output says what the text is and gives part 1's command; each
/// part shows every line of the text unchanged, then ends with the command for
/// the next step, with a token only that part's output holds.
#[test]
fn each_part_shows_the_text_unchanged_and_ends_with_the_next_commands_token() {
    let world = world("ack-chain");
    let started = world.ack(&[], NOON).unwrap();
    let text = Text::read(&world.places, "bae").unwrap();
    let names: Vec<String> = text.sections.iter().map(|s| s.name.clone()).collect();
    let parts = text.parts(&names);
    assert!(parts.len() >= 2, "the workflow alone fills two parts");
    assert!(
        started.contains(&format!("in {} parts", parts.len())),
        "{started}"
    );
    let mut command = next_command(&started);
    let mut tokens = vec![command[2].clone()];
    for (index, lines) in parts.iter().enumerate() {
        assert_eq!(command[..2], ["part", &(index + 1).to_string()]);
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        let shown = world.ack(&args, NOON).unwrap();
        let body: Vec<&str> = shown.lines().skip(1).take(lines.len()).collect();
        assert_eq!(body, *lines, "part {}", index + 1);
        command = next_command(&shown);
        let token = command.last().unwrap().clone();
        assert!(!tokens.contains(&token), "part {}", index + 1);
        assert_eq!(shown.matches(&token).count(), 1);
        tokens.push(token);
    }
    assert_eq!(command[0], text.version());
}

#[test]
fn a_part_needs_the_token_of_the_part_before_it() {
    let world = world("ack-order");
    let started = world.ack(&[], NOON).unwrap();
    let first = next_command(&started);
    let refusal = message(world.ack(&["part", "2", &first[2]], NOON));
    assert!(refusal.contains("part 2 needs"), "{refusal}");
    let refusal = message(world.ack(&["part", "1", "wrong"], NOON));
    assert!(refusal.contains("part 1 needs"), "{refusal}");
    world.ack(&["part", "1", &first[2]], NOON).unwrap();
}

#[test]
fn the_acknowledgement_needs_the_last_parts_token() {
    let world = world("ack-wrong-token");
    let started = world.ack(&[], NOON).unwrap();
    let version = next_command_version(&started);
    let (_, token) = world.read_all(NOON);
    let refusal = message(world.ack(&[&version, "wrong"], NOON));
    assert!(refusal.contains("the last line of part"), "{refusal}");
    assert!(message(world.check(NOON)).contains("has not acknowledged"));
    // A reading's first token does not acknowledge either.
    let first = next_command(&world.ack(&[], NOON).unwrap())[2].clone();
    assert_ne!(first, token);
    assert!(world.ack(&[&version, &first], NOON).is_err());
}

#[test]
fn reading_a_part_twice_shows_it_again_and_a_new_reading_draws_new_tokens() {
    let world = world("ack-tokens-kept");
    let started = world.ack(&[], NOON).unwrap();
    let first = next_command(&started);
    let args: Vec<&str> = first.iter().map(String::as_str).collect();
    let shown = world.ack(&args, NOON).unwrap();
    assert_eq!(world.ack(&args, NOON).unwrap(), shown);
    let again = world.ack(&[], NOON).unwrap();
    assert_ne!(next_command(&again)[2], first[2]);
    assert!(world.ack(&args, NOON).is_err());
}

#[test]
fn every_part_stays_well_under_the_size_claude_code_shows_whole() {
    let world = world("ack-part-size");
    let mut shown = world.ack(&[], NOON).unwrap();
    let mut part = 0;
    while next_command(&shown)[0] == "part" {
        part += 1;
        let command = next_command(&shown);
        let args: Vec<&str> = command.iter().map(String::as_str).collect();
        shown = world.ack(&args, NOON).unwrap();
        assert!(
            shown.len() < 25_000,
            "part {part} has {} bytes",
            shown.len()
        );
    }
    assert!(part >= 2);
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
        let started = self.ack(&[], now).unwrap();
        started
            .split("in ")
            .filter_map(|piece| piece.split(" parts").next()?.parse().ok())
            .next()
            .unwrap()
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
        started.contains("1 of the text's 3 sections changed: rules/text/plain.md"),
        "{started}"
    );
    let only = world
        .ack(&["part", "1", &next_command(&started)[2]], NOON + 60)
        .unwrap();
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
    std::fs::remove_file(
        world
            .scratch
            .dir
            .join("agent-instructions/rules/text/plain.md"),
    )
    .unwrap();
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
fn an_acknowledgement_in_one_repo_holds_in_another_repo_of_the_project() {
    let world = world("ack-two-repos");
    world.acknowledge(NOON);
    world.check_in(&world.coven, AGENT, NOON + 60).unwrap();
}

#[test]
fn an_edit_of_a_claude_md_leaves_every_acknowledgement_current() {
    let world = world("ack-claude-md-edited");
    world.acknowledge(NOON);
    std::fs::write(
        world.coven.join("CLAUDE.md"),
        "Coven rules, edited with several ordinary words.\n",
    )
    .unwrap();
    std::fs::write(
        world.places.home.join(".claude/CLAUDE.md"),
        "Global rules, edited with several ordinary words.\n",
    )
    .unwrap();
    world.check(NOON + 60).unwrap();
    world.check_in(&world.coven, AGENT, NOON + 60).unwrap();
}

#[test]
fn a_compaction_clears_the_acknowledgement_in_every_repo() {
    let world = world("ack-two-repos-compacted");
    world.acknowledge(NOON);
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
        started.contains("builds ack part 1 ") && started.contains(" --agent a1 --project bae"),
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
        started.contains("builds ack part 1 ") && started.contains(" --agent a1 --project bae"),
        "{started}"
    );
    let mut shown = started;
    let version = loop {
        let command = next_command(&shown);
        if command[0] != "part" {
            // The flags follow the version and the token, as the output
            // prints them.
            ack_naming_bae(&world, &[&command[0], &command[1]]).unwrap();
            break command[0].clone();
        }
        shown = ack_naming_bae(&world, &[&command[0], &command[1], &command[2]]).unwrap();
    };
    assert!(!version.is_empty());
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
    world.acknowledge_as(SESSION_ID, NOON + 120);
    assert_eq!(outdated_at(&world, NOON + 180), None);
}

#[test]
fn a_judge_whose_text_lost_a_section_and_changed_no_other_is_told_so() {
    let world = judge_world("ack-judge-section-left");
    world.acknowledge_as(SESSION_ID, NOON);
    std::fs::remove_file(
        world
            .scratch
            .dir
            .join("agent-instructions/rules/text/plain.md"),
    )
    .unwrap();
    let why = outdated_at(&world, NOON + 60).expect("a section left the text");
    assert!(why.contains("moved or left"), "{why}");
}

fn next_command_version(started: &str) -> String {
    started
        .split("The text is version ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .to_string()
}
