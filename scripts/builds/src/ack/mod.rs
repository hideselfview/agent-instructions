//! `builds ack`, and the gate that refuses an agent's builds commands until
//! the agent has read and acknowledged the text builds asks every agent to
//! read (text.rs).
//!
//! builds tells an agent's command from the person's by the Claude Code
//! session id in its environment (`CLAUDE_CODE_SESSION_ID`): Claude Code sets
//! it in the shells of a session and of every agent the session dispatches,
//! and the person's terminal lacks it, so builds never refuses the person's
//! own commands. The agents a session dispatches share its session id, so
//! builds names an agent by the session id and the agent id that `--agent` or
//! `BUILDS_AGENT` gives, which a hook registered when Claude Code started the
//! agent (agent.rs).
//!
//! builds keeps the acknowledgement in the store of the project whose repo
//! the command runs in, else of the project `--project` names. The text holds
//! the CLAUDE.md of the repo the command runs in, so builds keeps an agent's
//! acknowledgement of each repo's text apart, and an agent that works in two
//! repos of a project reads each repo's text once. builds refuses an agent
//! whose acknowledgement of the text there is missing, is of another version
//! of the text, or is more than `HOURS` hours old. After Claude Code compacts
//! an agent, its SessionStart hook runs `builds ack --compacted`, which
//! deletes every acknowledgement of that agent (`compacted`).

pub mod agent;
pub mod record;
pub mod text;

use std::path::Path;

use rusqlite::TransactionBehavior;

use crate::checkout::Checkout;
use crate::db::{self, Make, Sql};
use crate::fail;
use crate::fail::Result;
use crate::install_gate;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of_or_flag, stores};
use crate::places::Places;
use crate::text::{local_minute, shell_word};
use text::{Text, WORDS_PER_PART, new_words, show_part};

/// The variable that holds the Claude Code session id in an agent's shell.
pub const SESSION: &str = "CLAUDE_CODE_SESSION_ID";

/// The variable a builds command sets once it passes the gate, so that the
/// builds commands it runs (a hook's checks, install after a landing) pass
/// too.
pub const PASSED: &str = "BUILDS_ACKNOWLEDGED";

/// How many hours an acknowledgement holds.
pub const HOURS: i64 = 12;

const USAGE: &str = "usage: builds ack [--agent ID] [--project P]\n       builds ack PART [--agent ID] [--project P]\n       builds ack VERSION WORD... [--agent ID] [--project P]\n       builds ack --compacted";

/// Refuses `builds <command> <args>` when an agent runs it without a current
/// acknowledgement. Passes the person's commands, `ack`, which records the
/// acknowledgement, and the commands a builds command that passed runs. The
/// gate also passes the commands that do not use the stores
/// (`install_gate::STORELESS`): the gate reads a store, and install must run
/// while the stores still have an older schema, because install migrates them
/// to its schema. The gate passes the pre-commit hook's checks in a main
/// checkout too (`commits_in_main_checkout`).
pub fn gate(places: &Places, command: &str, args: &[String]) -> Result<()> {
    if command == "ack"
        || install_gate::STORELESS.contains(&command)
        || std::env::var_os(PASSED).is_some()
        || commits_in_main_checkout(command, args)?
    {
        return Ok(());
    }
    let Some(session) = session() else {
        return Ok(());
    };
    check(
        places,
        &session,
        agent::from_env()?.as_deref(),
        flag(args, "project"),
        &current_dir()?,
        crate::app_timing::now(),
    )?;
    // SAFETY: no other thread runs yet, so none reads the environment while
    // set_var writes it.
    unsafe { std::env::set_var(PASSED, "1") };
    Ok(())
}

/// Whether `builds <command> <args>` is the pre-commit hook's `checks run`
/// in a repo's main checkout. No agent commits in a main checkout (`builds
/// help workflow`, WORKTREES), so a commit there is the person's, even when
/// the person's Claude Code session makes it, and the gate, which makes
/// agents read the rules, does not apply to its hook.
fn commits_in_main_checkout(command: &str, args: &[String]) -> Result<bool> {
    if command != "checks" {
        return Ok(false);
    }
    let Some(worktree) = crate::checks_run::run_worktree(args) else {
        return Ok(false);
    };
    Ok(Checkout::at(worktree)?.is_some_and(|checkout| checkout.is_main()))
}

/// The session id of the agent that runs this command, or None for the
/// person.
fn session() -> Option<String> {
    std::env::var(SESSION).ok().filter(|id| !id.is_empty())
}

/// The value of flag <name> in a command's arguments <args>, before any `--`.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let wanted = format!("--{name}");
    args.iter()
        .take_while(|arg| *arg != "--")
        .position(|arg| *arg == wanted)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

/// An agent as builds names it, and where builds keeps its acknowledgement.
struct Agent {
    session: String,
    /// The id Claude Code gives the agent (agent.rs).
    id: String,
    store: Store,
    /// The checkout the command runs in, whose CLAUDE.md the text holds.
    checkout: Option<Checkout>,
    /// The flags that name this agent to `builds ack` again.
    flags: String,
}

impl Agent {
    /// The agent <id> of <session> that runs a command in <cwd> with
    /// `--project` <project>. Fails when no id names the agent, or when no
    /// hook registered the id in <session>.
    fn resolve(
        places: &Places,
        session: &str,
        id: Option<&str>,
        project: Option<&str>,
        cwd: &Path,
    ) -> Result<Agent> {
        let how = format!(
            "builds' hooks told you your id when Claude Code started you, in the line \"Your builds agent id is ...\"; the main agent's id is its session id, in {SESSION}"
        );
        let Some(id) = id else {
            fail!(
                "builds names each agent by the id Claude Code gives it, and this command names none: pass --agent ID to every builds command, or set {}=ID in front of a script that runs builds. {how}.",
                agent::VARIABLE
            );
        };
        if !agent::is_registered(places, session, id)? {
            fail!("builds has registered no agent {id} in session {session}. {how}.");
        }
        let checkout = Checkout::at(cwd)?;
        let of_checkout = match &checkout {
            Some(checkout) => project_of_or_flag(places, checkout, project),
            None => Ok(None),
        };
        let store = match (of_checkout?, project) {
            (Some(found), _) => Store::named(places, &found)?,
            (None, Some(named)) => Store::named(places, named)?,
            (None, None) => fail!(
                "builds keeps an agent's acknowledgement in the store of the project it works in, and {} is in no repo of a project; run builds from a checkout of a project's repo, or name the project with --project",
                cwd.display()
            ),
        };
        let mut flags = format!(" --agent {}", shell_word(id));
        if project.is_some() {
            flags += &format!(" --project {}", shell_word(&store.project));
        }
        Ok(Agent {
            session: session.to_string(),
            id: id.to_string(),
            store,
            checkout,
            flags,
        })
    }

    /// The agent as messages name it.
    fn name(&self) -> String {
        format!("agent {}", self.id)
    }

    fn text(&self, places: &Places) -> Result<Text> {
        Text::read(places, &self.store.project, self.checkout.as_ref())
    }

    /// The agent's last acknowledgement of <text>.
    fn last(&self, text: &Text) -> Result<Option<record::Ack>> {
        match db::open_reader(&self.store.database())? {
            Some(db) => record::last(&db, &self.session, &self.id, text.repo.as_deref()),
            None => Ok(None),
        }
    }

    /// What the agent's reading of <text> at <now> holds.
    fn scope(&self, text: &Text, now: i64) -> Result<Scope> {
        let Some(ack) = self.last(text)? else {
            return Ok(Scope::Whole(Whole::NotAcknowledged));
        };
        if now - ack.at > HOURS * 3600 {
            return Ok(Scope::Whole(Whole::TooOld(ack)));
        }
        if ack.version == text.version() {
            return Ok(Scope::Whole(Whole::Current));
        }
        let database = self.store.database();
        let Some(db) = db::open_reader(&database)? else {
            fail!(
                "builds read {}'s acknowledgement from {}, and the file is gone now",
                self.name(),
                database.display()
            );
        };
        let Some(kept) = record::kept_sections(&db, &ack.version)? else {
            return Ok(Scope::Whole(Whole::NotKept(ack)));
        };
        let sections = text.changed_since(&kept);
        // A reading of no parts has no check words to give.
        if sections.is_empty() {
            return Ok(Scope::Whole(Whole::NoTextChanged(ack)));
        }
        Ok(Scope::Changed {
            since: ack,
            sections,
        })
    }
}

/// What a reading holds.
enum Scope {
    /// Every section of the text, for the reason the `Whole` gives.
    Whole(Whole),
    /// Only <sections>, those that changed since the agent's last
    /// acknowledgement <since>, of a version builds keeps.
    Changed {
        since: record::Ack,
        sections: Vec<String>,
    },
}

/// Why a reading holds every section of the text, with the agent's last
/// acknowledgement where it has one.
enum Whole {
    /// The agent has not acknowledged the text in this session.
    NotAcknowledged,
    /// The agent acknowledged the text more than `HOURS` hours ago.
    TooOld(record::Ack),
    /// The agent acknowledged the current version at most `HOURS` hours ago,
    /// and reads the text again.
    Current,
    /// builds no longer keeps the version the agent acknowledged.
    NotKept(record::Ack),
    /// The version changed, but no section's text did: sections moved, or
    /// a section left the text.
    NoTextChanged(record::Ack),
}

impl Scope {
    /// The names of the sections a reading of <text> holds, in the text's
    /// order.
    fn sections(&self, text: &Text) -> Vec<String> {
        match self {
            Scope::Whole(_) => text.sections.iter().map(|s| s.name.clone()).collect(),
            Scope::Changed { sections, .. } => sections.clone(),
        }
    }
}

/// Refuses the agent of <session> that runs a command in <cwd> with `--by`
/// <by> and `--project` <project> at <now>, unless its acknowledgement is of
/// the current version of the text and at most `HOURS` hours old.
fn check(
    places: &Places,
    session: &str,
    id: Option<&str>,
    project: Option<&str>,
    cwd: &Path,
    now: i64,
) -> Result<()> {
    let agent = Agent::resolve(places, session, id, project, cwd)?;
    let text = agent.text(places)?;
    let who = agent.name();
    let why = match agent.scope(&text, now)? {
        Scope::Whole(Whole::Current) => return Ok(()),
        Scope::Whole(Whole::NotAcknowledged) => match &text.repo {
            Some(repo) => format!(
                "{who} has not acknowledged the text that holds the CLAUDE.md of repo {repo} in this Claude Code session"
            ),
            None => format!("{who} has not acknowledged it in this Claude Code session"),
        },
        Scope::Whole(Whole::TooOld(ack)) => format!(
            "{who} acknowledged it at {}, more than {HOURS} hours ago, so `builds ack` shows the whole text",
            local_minute(ack.at)
        ),
        Scope::Whole(Whole::NotKept(ack)) => format!(
            "the text changed since {who} acknowledged version {} at {}, and builds no longer keeps that version, so `builds ack` shows the whole text",
            ack.version,
            local_minute(ack.at)
        ),
        Scope::Whole(Whole::NoTextChanged(ack)) => format!(
            "the text changed since {who} acknowledged version {} at {}, but no section's text changed: sections moved or left the text, so `builds ack` shows the whole text",
            ack.version,
            local_minute(ack.at)
        ),
        Scope::Changed { since, sections } => format!(
            "the text changed since {who} acknowledged it at {}: {} changed, and `builds ack` shows only those sections",
            local_minute(since.at),
            sections.join(", ")
        ),
    };
    fail!(
        "builds refuses this command until you read the text every agent reads before it works, and acknowledge it: {why}. Run `builds ack{}`, read every part it lists, and acknowledge them; then run this command again.",
        agent.flags
    )
}

/// `builds ack`.
pub fn cmd_ack(places: &Places, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &["compacted"])?;
    if args.switch("compacted") {
        println!("{}", compacted(places, agent::HookInput::from_stdin()?)?);
        return Ok(());
    }
    let Some(session) = session() else {
        fail!(
            "builds ack records an agent's acknowledgement under its Claude Code session ({SESSION}), which this shell lacks; builds never refuses a command run outside Claude Code"
        );
    };
    print!(
        "{}",
        ack(
            places,
            &session,
            agent::from_env()?.as_deref(),
            &args,
            &current_dir()?,
            crate::app_timing::now()
        )?
    );
    Ok(())
}

/// What `builds ack` with <args> does for agent <id> of <session> in <cwd>
/// at <now>, and the text it prints.
fn ack(
    places: &Places,
    session: &str,
    id: Option<&str>,
    args: &Args,
    cwd: &Path,
    now: i64,
) -> Result<String> {
    let agent = Agent::resolve(places, session, id, args.value("project")?, cwd)?;
    let text = agent.text(places)?;
    match args.positional() {
        [] => start(&agent, &text, now),
        [part] => part_of(&agent, &text, part),
        [version, words @ ..] => acknowledge(&agent, &text, version, words, now),
    }
}

/// Starts a reading: picks the sections it holds, draws each part's check
/// words and says how to read the parts and acknowledge them.
fn start(agent: &Agent, text: &Text, now: i64) -> Result<String> {
    let scope = agent.scope(text, now)?;
    let shown = scope.sections(text);
    let parts = text.parts(&shown);
    let mut words = Vec::new();
    for _ in &parts {
        words.push(new_words()?);
    }
    let version = text.version();
    let mut db = db::open(&agent.store.database(), Make::Never)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .sql()?;
    record::start_reading(
        &tx,
        &agent.session,
        &agent.id,
        &version,
        now,
        &shown,
        &words,
    )?;
    tx.commit().sql()?;
    let names: Vec<&str> = text.sections.iter().map(|s| s.name.as_str()).collect();
    let mut out = format!(
        "builds asks every agent to read this text in full before it runs any other builds command: {}.\n\nThe text is version {version}, in {} parts.\n",
        names.join(", "),
        parts.len()
    );
    if let Scope::Changed { since, sections } = &scope {
        out += &format!(
            "Since {} acknowledged version {} at {}, {} of the text's {} sections changed: {}. The reading holds only those sections, each in full.\n",
            agent.name(),
            since.version,
            local_minute(since.at),
            sections.len(),
            text.sections.len(),
            sections.join(", ")
        );
    }
    out += &format!(
        "\nRead each part in full, one command each, and note its {WORDS_PER_PART} check words:\n\n"
    );
    for number in 1..=parts.len() {
        out += &format!("  builds ack {number}{}\n", agent.flags);
    }
    out += &format!(
        "\nThen acknowledge the text with every check word of every part, in order:\n\n  builds ack {version} WORD...{}\n\nRunning `builds ack` again draws new words.\n",
        agent.flags
    );
    Ok(out)
}

/// The agent's reading, refused when it has none or the text changed since
/// it started.
fn reading_of(agent: &Agent, text: &Text) -> Result<record::Reading> {
    let reading = match db::open_reader(&agent.store.database())? {
        Some(db) => record::reading(&db, &agent.session, &agent.id)?,
        None => None,
    };
    let Some(reading) = reading else {
        fail!(
            "{} has no reading; run `builds ack{}` first, which draws each part's check words",
            agent.name(),
            agent.flags
        );
    };
    if reading.version != text.version() {
        fail!(
            "the text changed since {} ran `builds ack`; run `builds ack{}` again",
            agent.name(),
            agent.flags
        );
    }
    Ok(reading)
}

/// Part <part> of the agent's reading.
fn part_of(agent: &Agent, text: &Text, part: &str) -> Result<String> {
    let reading = reading_of(agent, text)?;
    let parts = text.parts(&reading.sections);
    let count = parts.len();
    let number = match part.parse::<usize>() {
        Ok(number) if (1..=count).contains(&number) => number,
        _ => fail!("there are parts 1 to {count}; {part} is none of them\n{USAGE}"),
    };
    Ok(show_part(
        &parts[number - 1],
        number,
        count,
        &reading.version,
        &reading.words[number - 1],
    ))
}

/// Records the acknowledgement of <version> with <words>, every check word of
/// every part in order; refuses a missing or wrong word, naming its part.
fn acknowledge(
    agent: &Agent,
    text: &Text,
    version: &str,
    words: &[String],
    now: i64,
) -> Result<String> {
    let reading = reading_of(agent, text)?;
    if version != reading.version {
        fail!(
            "{} reads version {}, not {version}",
            agent.name(),
            reading.version
        );
    }
    let expected: usize = reading.words.iter().map(Vec::len).sum();
    if words.len() > expected {
        fail!(
            "you gave {} check words, and the {} parts hold {expected}",
            words.len(),
            reading.words.len()
        );
    }
    let wrong = wrong_parts(&reading.words, words);
    if !wrong.is_empty() {
        let numbers: Vec<String> = wrong.iter().map(usize::to_string).collect();
        let commands: Vec<String> = wrong
            .iter()
            .map(|part| format!("`builds ack {part}{}`", agent.flags))
            .collect();
        fail!(
            "the check words of part {} are missing or wrong; read {} again, then acknowledge with every check word of every part, in order",
            numbers.join(", "),
            commands.join(", ")
        );
    }
    let mut db = db::open(&agent.store.database(), Make::Never)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .sql()?;
    record::acknowledge(
        &tx,
        &agent.session,
        &agent.id,
        text.repo.as_deref(),
        version,
        now,
        &text.hashes(),
    )?;
    tx.commit().sql()?;
    Ok(format!(
        "builds: {} acknowledged version {version}. The acknowledgement holds for {HOURS} hours, until the text changes or Claude Code compacts this agent.\n",
        agent.name()
    ))
}

/// The parts, counted from 1, whose check words in <given> are missing or
/// differ from <expected>, each part's words in order.
fn wrong_parts(expected: &[Vec<String>], given: &[String]) -> Vec<usize> {
    let mut given = given.iter();
    expected
        .iter()
        .enumerate()
        .filter(|(_, words)| {
            let mut wrong = false;
            for word in words.iter() {
                if given.next() != Some(word) {
                    wrong = true;
                }
            }
            wrong
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// What `builds ack --compacted` does with the input <hook> Claude Code's
/// SessionStart hook passes after it compacts an agent: deletes every
/// acknowledgement and the reading of that agent in every store, and returns
/// the hook's output, which tells the agent its id again. The input names the
/// agent (`agent_id`, absent when the main agent compacted). Fails, deleting
/// nothing, for an agent no hook registered.
fn compacted(places: &Places, hook: agent::HookInput) -> Result<String> {
    if hook.hook_event_name != "SessionStart" || hook.source.as_deref() != Some("compact") {
        fail!(
            "`builds ack --compacted` runs from the SessionStart hook after a compaction, and this input is of {} ({})",
            hook.hook_event_name,
            hook.source.as_deref().unwrap_or("no source")
        );
    }
    let id = hook.agent();
    if !agent::is_registered(places, &hook.session_id, id)? {
        fail!(
            "Claude Code compacted agent {id} of session {}, and no hook registered that agent, so builds deleted no acknowledgement",
            hook.session_id
        );
    }
    for store in stores(places)? {
        let mut db = db::open(&store.database(), Make::Never)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        record::forget(&tx, &hook.session_id, id)?;
        tx.commit().sql()?;
    }
    Ok(agent::hook_output(
        "SessionStart",
        &format!(
            "Claude Code compacted your context, so builds ended your acknowledgements of the text every agent reads. {} Run `builds ack --agent {id}` before your next builds command.",
            agent::told(id)
        ),
    ))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
