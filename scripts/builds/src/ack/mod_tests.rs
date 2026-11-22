use super::*;
use crate::lists::fixtures::{NOON, make_store_named};
use crate::lists::store::project_of;
use crate::testing::{Scratch, repo};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "session-1";

/// The agent most tests run as, which `world` registers.
const AGENT: &str = "a1";

/// A scratch home with a ~/.claude/CLAUDE.md, project bae's store of repos
/// bae and coven, a checkout of repo bae with no
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

fn test_identity(session: &str, id: Option<&str>) -> Result<agent::Identity> {
    Ok(
        agent::Session::of(Some(session.into()), None, None, id.map(str::to_string))
            .identity()?
            .expect("a Claude test session"),
    )
}

fn check(
    places: &Places,
    session: &str,
    id: Option<&str>,
    project: Option<&str>,
    cwd: &Path,
    now: i64,
) -> Result<Option<String>> {
    super::check(places, &test_identity(session, id)?, project, cwd, now)
}

fn ack(
    places: &Places,
    session: &str,
    id: Option<&str>,
    args: &Args,
    cwd: &Path,
    now: i64,
) -> Result<String> {
    super::ack(places, &test_identity(session, id)?, args, cwd, now)
}

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        fail::Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
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

    fn check(&self, now: i64) -> Result<Option<String>> {
        self.check_as(AGENT, now)
    }

    fn check_as(&self, agent: &str, now: i64) -> Result<Option<String>> {
        self.check_in(&self.checkout, agent, now)
    }

    /// The gate for a command <agent> runs in <cwd>.
    fn check_in(&self, cwd: &Path, agent: &str, now: i64) -> Result<Option<String>> {
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
        let version = next_command_version(&shown);
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
    let text = Text::read();
    let parts = text.parts();
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

impl World {
    fn record_acknowledgement(&self, project: &str, agent: &str, text: &Text) {
        let store = Store::named(&self.places, project).unwrap();
        let db = db::open(&store.database(), Make::Never).unwrap();
        record::acknowledge(
            &db,
            SESSION_ID,
            agent,
            &text.version(),
            NOON,
            &text.hashes(),
        )
        .unwrap();
    }
}

fn older_lists_overview() -> Text {
    let mut text = Text::read();
    text.sections[1].text.push_str("\nEarlier lists help.");
    text
}

/// The notice for an agent whose text changed: the check passes and returns it
/// once.
#[test]
fn an_edit_of_the_lists_overview_passes_the_command_with_one_notice_naming_the_changed_section() {
    let world = world("ack-edited");
    world.record_acknowledgement("bae", AGENT, &older_lists_overview());
    let notice = world.check(NOON + 120).unwrap().expect("the text changed");
    assert!(
        notice.contains("changed since you acknowledged it. This command runs as usual."),
        "{notice}"
    );
    assert!(notice.contains("  builds help lists\n"), "{notice}");
    assert!(!notice.contains("builds help workflow"), "{notice}");
    assert_eq!(world.check(NOON + 180).unwrap(), None);
}

#[test]
fn each_section_says_how_to_read_it() {
    let text = Text::read();
    assert_eq!(
        text.how_to_read("builds help workflow"),
        "builds help workflow"
    );
    assert_eq!(
        text.how_to_read("builds help lists overview"),
        "builds help lists"
    );
}

#[test]
fn an_agent_whose_version_builds_no_longer_keeps_gets_a_notice_listing_every_section() {
    let world = world("ack-not-kept");
    world.record_acknowledgement("bae", AGENT, &older_lists_overview());
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
    let notice = world.check(NOON + 60).unwrap().expect("the text changed");
    assert!(
        notice.contains("builds no longer keeps the version you acknowledged"),
        "{notice}"
    );
    assert!(notice.contains("builds help workflow"), "{notice}");
    assert!(notice.contains("builds help lists"), "{notice}");
    assert_eq!(world.check(NOON + 120).unwrap(), None);
}

#[test]
fn a_section_that_left_the_text_is_named_as_removed() {
    let world = world("ack-section-left");
    let mut older = Text::read();
    older.sections.push(text::Section {
        name: "rules/text/plain.md".to_string(),
        text: "Write plain sentences.".to_string(),
        how_to_read: "rules/text/plain.md".to_string(),
    });
    world.record_acknowledgement("bae", AGENT, &older);
    let notice = world.check(NOON + 60).unwrap().expect("a section left");
    assert!(
        notice.contains("Sections removed: rules/text/plain.md"),
        "{notice}"
    );
    assert!(!notice.contains("Read these sections"), "{notice}");
    assert_eq!(world.check(NOON + 120).unwrap(), None);
}

#[test]
fn an_acknowledgement_never_expires() {
    let world = world("ack-never-expires");
    world.acknowledge(NOON);
    world.check(NOON + 365 * 24 * 3600).unwrap();
}

#[test]
fn a_reading_after_a_change_holds_only_the_changed_sections() {
    let world = world("ack-changed-only");
    world.record_acknowledgement("bae", AGENT, &older_lists_overview());
    // A command the agent runs first passes with a notice and records the
    // change, so the agent reads the change again only by starting here.
    let started = world.ack(&[], NOON + 60).unwrap();
    assert!(started.contains("in 1 parts"), "{started}");
    assert!(
        started.contains("leaves out the sections you acknowledged in this session"),
        "{started}"
    );
    let first = world
        .ack(&["part", "1", &next_command(&started)[2]], NOON + 60)
        .unwrap();
    assert!(
        first.contains("===== builds help lists overview"),
        "{first}"
    );
    assert!(!first.contains("===== builds help workflow"), "{first}");
}

impl World {
    /// Adds a second project and returns a folder outside both repos.
    fn with_second_project(&self) -> PathBuf {
        make_store_named(&self.places, "second", &["other"]);
        let elsewhere = self.scratch.dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        elsewhere
    }

    /// `builds ack <args> --project second` as the agent runs it in <cwd>.
    fn ack_second(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut args = args.to_vec();
        args.extend(["--project", "second"]);
        self.ack_in(cwd, AGENT, &args, NOON)
    }

    fn check_second(&self, cwd: &Path) -> Result<Option<String>> {
        check(
            &self.places,
            SESSION_ID,
            Some(AGENT),
            Some("second"),
            cwd,
            NOON,
        )
    }
}

#[test]
fn a_project_whose_sections_the_agent_acknowledged_elsewhere_needs_no_reading() {
    let world = world("ack-second-project-shared-only");
    let elsewhere = world.with_second_project();
    world.acknowledge(NOON);
    assert_eq!(world.check_second(&elsewhere).unwrap(), None);
    let shown = world.ack_second(&elsewhere, &[]).unwrap();
    assert!(shown.contains("nothing to read"), "{shown}");
}

#[test]
fn a_change_of_a_shared_section_is_told_once_across_projects() {
    let world = world("ack-shared-notice");
    let elsewhere = world.with_second_project();
    let older = older_lists_overview();
    world.record_acknowledgement("bae", AGENT, &older);
    world.record_acknowledgement("second", AGENT, &older);
    assert!(world.check(NOON + 60).unwrap().is_some());
    assert_eq!(world.check_second(&elsewhere).unwrap(), None);
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
fn an_unregistered_agent_id_is_refused_saying_how_to_find_it() {
    let world = world("ack-unregistered");
    let refusal = message(world.check_as("stranger", NOON));
    assert!(
        refusal.contains("no agent stranger in session session-1"),
        "{refusal}"
    );
    assert!(
        refusal.contains("Use the id that your Claude Code hook printed."),
        "{refusal}"
    );
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

fn outdated_at(world: &World) -> Option<String> {
    let store = Store::named(&world.places, "bae").unwrap();
    outdated(&store, SESSION_ID).unwrap()
}

#[test]
fn a_judge_that_acknowledged_the_current_text_has_nothing_to_acknowledge() {
    let world = judge_world("ack-judge-current");
    world.acknowledge_as(SESSION_ID, NOON);
    assert_eq!(outdated_at(&world), None);
}

#[test]
fn a_judge_with_no_acknowledgement_is_told_so() {
    let world = judge_world("ack-judge-none");
    let why = outdated_at(&world).expect("it has acknowledged nothing");
    assert!(why.contains("has no acknowledgement"), "{why}");
}

#[test]
fn a_judge_whose_acknowledged_text_changed_has_nothing_to_acknowledge() {
    let world = judge_world("ack-judge-changed");
    world.record_acknowledgement("bae", SESSION_ID, &older_lists_overview());
    assert_eq!(outdated_at(&world), None);
}

fn next_command_version(started: &str) -> String {
    started
        .split("the text is version ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn every_reading_opens_with_why_builds_asks_for_it() {
    let world = world("ack-why");
    let started = world.ack(&[], NOON).unwrap();
    assert_eq!(
        started.lines().next().unwrap(),
        "builds asks every agent to read this text first, because agents that skip it work in the person's checkout, push main and poll for builds."
    );
    assert!(!started.contains("compacted you"), "{started}");
}

#[test]
fn a_reading_after_a_compaction_says_the_text_left_the_agents_context_until_it_acknowledges() {
    let world = world("ack-compacted-opening");
    world.acknowledge(NOON);
    world.compact(Some(AGENT)).unwrap();
    let started = world.ack(&[], NOON + 60).unwrap();
    let lines: Vec<&str> = started.lines().collect();
    assert_eq!(
        lines[1],
        "Claude Code compacted you, so this text is no longer in your context."
    );
    world.acknowledge(NOON + 120);
    let again = world.ack(&[], NOON + 180).unwrap();
    assert!(!again.contains("compacted you"), "{again}");
}

#[test]
fn a_compaction_of_another_agent_leaves_the_opening_of_this_one_as_it_was() {
    let world = world("ack-compacted-other");
    agent::register(&world.places, SESSION_ID, "a2").unwrap();
    world.compact(Some("a2")).unwrap();
    assert!(!world.ack(&[], NOON).unwrap().contains("compacted you"));
}

#[test]
fn a_reading_whose_sections_changed_before_a_part_is_refused_even_with_the_same_part_count() {
    let world = world("ack-reading-changed");
    let agent = Agent::resolve(
        &world.places,
        SESSION_ID,
        Some(AGENT),
        None,
        &world.checkout,
    )
    .unwrap();
    let older = older_lists_overview();
    assert_eq!(older.parts().len(), Text::read().parts().len());
    let started = start(&world.places, &agent, &older, NOON).unwrap();
    let refusal = message(world.ack(&["part", "1", &next_command(&started)[2]], NOON));
    assert!(refusal.contains("changed since"), "{refusal}");
}

#[test]
fn codex_compaction_clears_only_the_thread_named_in_the_event() {
    let world = world("codex-compaction");
    let store = Store::named(&world.places, "bae").unwrap();
    let mut db = db::open(&store.database(), Make::Never).unwrap();
    let session = "00000000-0000-4000-8000-000000000001";
    let worker = "00000000-0000-4000-8000-000000000002";
    let other = "00000000-0000-4000-8000-000000000003";
    for thread in [session, worker, other] {
        let tx = db.transaction().unwrap();
        record::acknowledge(&tx, session, thread, "v1", NOON, &[]).unwrap();
        tx.commit().unwrap();
    }
    let event = |agent_id: Option<&str>| {
        agent::HookInput::parse(&serde_json::json!({"hook_event_name": "PostCompact", "session_id": session, "agent_id": agent_id}).to_string()).unwrap()
    };
    codex_compacted(&world.places, event(Some(worker))).unwrap();
    assert_eq!(record::last(&db, session, worker).unwrap(), None);
    assert!(record::last(&db, session, session).unwrap().is_some());
    codex_compacted(&world.places, event(None)).unwrap();
    assert_eq!(record::last(&db, session, session).unwrap(), None);
    assert!(record::last(&db, session, other).unwrap().is_some());
}

#[test]
fn the_gate_refuses_a_claude_agent_that_names_no_id() {
    let world = world("ack-gate-no-id");
    // SAFETY: each test of this suite runs in a process of its own, and this
    // one starts no thread that reads the environment.
    unsafe { std::env::remove_var(PASSED) };
    let session = agent::Session::of(Some(SESSION_ID.into()), None, None, None);
    let refusal = message(gate(&world.places, &session, "du", &[]));
    assert!(refusal.contains("pass --agent ID"), "{refusal}");
    assert!(refusal.contains("BUILDS_AGENT=ID"), "{refusal}");
}
