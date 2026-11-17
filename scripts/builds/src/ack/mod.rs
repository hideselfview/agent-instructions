//! `builds ack`, and the gate that refuses an agent's builds commands until
//! the agent has read and acknowledged the text builds asks every agent to
//! read (text.rs).
//!
//! builds tells an agent's command from the person's by the Claude Code
//! session id in its environment (`CLAUDE_CODE_SESSION_ID`): Claude Code sets
//! it in the shells of a session and of every agent the session dispatches,
//! and the person's terminal lacks it, so builds never refuses the person's
//! own commands. The agents a session dispatches share its session id, so
//! builds names an agent by the session id and a name: `--by WHO`, else the
//! claim of the worktree the command runs in, else the name the session last
//! acknowledged or started a reading under.
//!
//! builds keeps the acknowledgement in the store of the project whose repo
//! the command runs in, else of the project `--project` names. builds refuses
//! an agent whose acknowledgement there is missing, is of another version of
//! the text, or is more than `HOURS` hours old. After Claude Code compacts a
//! session, its SessionStart hook runs `builds ack --compacted`, and that
//! command deletes the session's acknowledgements.

pub mod record;
pub mod text;

use std::path::Path;

use rusqlite::TransactionBehavior;

use crate::checkout::Checkout;
use crate::claim::Claim;
use crate::db::{self, Make, Sql};
use crate::fail;
use crate::fail::{Context, Result};
use crate::install_gate;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of, stores};
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

const USAGE: &str = "usage: builds ack [--by WHO] [--project P]\n       builds ack PART [--by WHO] [--project P]\n       builds ack VERSION WORD... [--by WHO] [--project P]\n       builds ack --compacted";

/// Refuses `builds <command> <args>` when an agent runs it without a current
/// acknowledgement. Passes the person's commands, `ack`, which records the
/// acknowledgement, and the commands a builds command that passed runs. The
/// gate also passes the commands that take no use of the stores
/// (`install_gate::STORELESS`): the gate reads a store, and install must run
/// while the stores still carry an older schema, because install moves them
/// to its own.
pub fn gate(places: &Places, command: &str, args: &[String]) -> Result<()> {
    if command == "ack"
        || install_gate::STORELESS.contains(&command)
        || std::env::var_os(PASSED).is_some()
    {
        return Ok(());
    }
    let Some(session) = session() else {
        return Ok(());
    };
    check(
        places,
        &session,
        flag(args, "by"),
        flag(args, "project"),
        &current_dir()?,
        crate::app_timing::now(),
    )?;
    // SAFETY: no other thread runs yet, so none reads the environment while
    // set_var writes it.
    unsafe { std::env::set_var(PASSED, "1") };
    Ok(())
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
    who: String,
    store: Store,
    /// The checkout the command runs in, whose CLAUDE.md the text holds.
    checkout: Option<Checkout>,
    /// The flags that name this agent to `builds ack` again.
    flags: String,
}

impl Agent {
    /// The agent of <session> that runs a command in <cwd> with `--by` <by>
    /// and `--project` <project>.
    fn resolve(
        places: &Places,
        session: &str,
        by: Option<&str>,
        project: Option<&str>,
        cwd: &Path,
    ) -> Result<Agent> {
        let checkout = Checkout::at(cwd)?;
        let of_checkout = match &checkout {
            Some(checkout) => project_of(places, checkout),
            None => Ok(None),
        };
        let (store, named) = match (of_checkout?, project) {
            (Some(found), _) => (Store::named(places, &found)?, false),
            (None, Some(named)) => (Store::named(places, named)?, true),
            (None, None) => fail!(
                "builds keeps an agent's acknowledgement in the store of the project it works in, and {} is in no repo of a project; run builds from a checkout of a project's repo, or name the project with --project",
                cwd.display()
            ),
        };
        let claimed = match &checkout {
            Some(checkout) => Claim::read(&checkout.claim_file())?
                .map(|claim| claim.who)
                .filter(|who| !who.is_empty()),
            None => None,
        };
        let who = match (by, claimed) {
            (Some(by), _) if !by.trim().is_empty() => by.to_string(),
            (Some(_), _) => fail!("--by is empty"),
            (None, Some(claimed)) => claimed,
            (None, None) => match db::open_reader(&store.database())? {
                Some(db) => record::last_who(&db, session)?,
                None => None,
            }
            .ok_or_else(|| {
                fail::Fail::Message(
                    "say who you are: run `builds ack --by WHO`, with the name your coordinator gave you or `coordinator`"
                        .to_string(),
                )
            })?,
        };
        let mut flags = format!(" --by {}", shell_word(&who));
        if named {
            flags += &format!(" --project {}", shell_word(&store.project));
        }
        Ok(Agent {
            session: session.to_string(),
            who,
            store,
            checkout,
            flags,
        })
    }

    fn text(&self, places: &Places) -> Result<Text> {
        Text::read(places, &self.store.project, self.checkout.as_ref())
    }

    fn last(&self) -> Result<Option<record::Ack>> {
        match db::open_reader(&self.store.database())? {
            Some(db) => record::last(&db, &self.session, &self.who),
            None => Ok(None),
        }
    }

    /// The sections that changed since the agent's last acknowledgement,
    /// which its reading shows first: none when it has no acknowledgement or
    /// acknowledged the current version of the text.
    fn changed(&self, text: &Text) -> Result<Vec<String>> {
        Ok(match self.last()? {
            Some(ack) if ack.version != text.version() => text.changed_since(&ack.sections),
            _ => Vec::new(),
        })
    }
}

/// Refuses the agent of <session> that runs a command in <cwd> with `--by`
/// <by> and `--project` <project> at <now>, unless its acknowledgement is of
/// the current version of the text and at most `HOURS` hours old.
fn check(
    places: &Places,
    session: &str,
    by: Option<&str>,
    project: Option<&str>,
    cwd: &Path,
    now: i64,
) -> Result<()> {
    let agent = Agent::resolve(places, session, by, project, cwd)?;
    let text = agent.text(places)?;
    let who = &agent.who;
    let why = match agent.last()? {
        None => format!(
            "{who} has not acknowledged it in this Claude Code session since the session started or Claude Code last compacted it"
        ),
        Some(ack) if ack.version != text.version() => format!(
            "the text changed since {who} acknowledged it at {}: {} changed, and `builds ack` shows those sections first",
            local_minute(ack.at),
            text.changed_since(&ack.sections).join(", ")
        ),
        Some(ack) if now - ack.at > HOURS * 3600 => format!(
            "{who} acknowledged it at {}, more than {HOURS} hours ago",
            local_minute(ack.at)
        ),
        Some(_) => return Ok(()),
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
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .context(|| "reading the hook's input".to_string())?;
        print!("{}", compacted(places, &input)?);
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
            &args,
            &current_dir()?,
            crate::app_timing::now()
        )?
    );
    Ok(())
}

/// What `builds ack` with <args> does for the agent of <session> in <cwd> at
/// <now>, and the text it prints.
fn ack(places: &Places, session: &str, args: &Args, cwd: &Path, now: i64) -> Result<String> {
    let agent = Agent::resolve(
        places,
        session,
        args.value("by")?,
        args.value("project")?,
        cwd,
    )?;
    let text = agent.text(places)?;
    match args.positional() {
        [] => start(&agent, &text, now),
        [part] => part_of(&agent, &text, part),
        [version, words @ ..] => acknowledge(&agent, &text, version, words, now),
    }
}

/// Starts a reading: draws each part's check words and says how to read the
/// parts and acknowledge them.
fn start(agent: &Agent, text: &Text, now: i64) -> Result<String> {
    let changed = agent.changed(text)?;
    let parts = text.parts(&changed);
    let mut words = Vec::new();
    for _ in &parts {
        words.push(new_words()?);
    }
    let version = text.version();
    let mut db = db::open(&agent.store.database(), Make::Never)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .sql()?;
    record::start_reading(&tx, &agent.session, &agent.who, &version, now, &words)?;
    tx.commit().sql()?;
    let names: Vec<&str> = text.sections.iter().map(|s| s.name.as_str()).collect();
    let mut out = format!(
        "builds asks every agent to read this text in full before it runs any other builds command: {}.\n\nThe text is version {version}, in {} parts.\n",
        names.join(", "),
        parts.len()
    );
    if !changed.is_empty() {
        out += &format!(
            "Since {}'s last acknowledgement, {} changed; the reading shows them first.\n",
            agent.who,
            changed.join(", ")
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
        Some(db) => record::reading(&db, &agent.session, &agent.who)?,
        None => None,
    };
    let Some(reading) = reading else {
        fail!(
            "{} has no reading; run `builds ack{}` first, which draws each part's check words",
            agent.who,
            agent.flags
        );
    };
    if reading.version != text.version() {
        fail!(
            "the text changed since {} ran `builds ack`; run `builds ack{}` again",
            agent.who,
            agent.flags
        );
    }
    Ok(reading)
}

/// Part <part> of the agent's reading.
fn part_of(agent: &Agent, text: &Text, part: &str) -> Result<String> {
    let reading = reading_of(agent, text)?;
    let parts = text.parts(&agent.changed(text)?);
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
            agent.who,
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
        &agent.who,
        version,
        now,
        &text.hashes(),
    )?;
    tx.commit().sql()?;
    Ok(format!(
        "builds: {} acknowledged version {version}. The acknowledgement holds for {HOURS} hours, until the text changes or Claude Code compacts this session.\n",
        agent.who
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

/// What `builds ack --compacted` does with the input Claude Code's
/// SessionStart hook passes after a compaction: deletes the session's
/// acknowledgements and readings in every store, and returns what the hook
/// tells the session.
fn compacted(places: &Places, input: &str) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct HookInput {
        session_id: String,
    }
    let hook: HookInput = serde_json::from_str(input).map_err(|error| {
        fail::Fail::Message(format!(
            "the hook's input is not Claude Code's SessionStart input: {error}"
        ))
    })?;
    for store in stores(places)? {
        let mut db = db::open(&store.database(), Make::Never)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        record::forget_session(&tx, &hook.session_id)?;
        tx.commit().sql()?;
    }
    Ok(
        "builds: the compaction ended this session's acknowledgement of the text every agent reads. builds refuses your next builds command until you run `builds ack`, read every part it lists in full, and acknowledge them.\n"
            .to_string(),
    )
}

/// Every project's acknowledgements as the dashboard reads them: each
/// agent's name, session, version and time, and the hours an
/// acknowledgement holds, from which the page tells which ones ended.
pub fn dashboard_json(places: &Places) -> String {
    let value = match stores(places) {
        Ok(stores) => {
            let projects: Vec<serde_json::Value> = stores
                .iter()
                .map(|store| {
                    let read = db::open_reader(&store.database()).and_then(|db| match db {
                        Some(db) => record::all(&db),
                        None => Ok(Vec::new()),
                    });
                    match read {
                        Ok(acks) => serde_json::json!({
                            "project": store.project,
                            "acks": acks.iter().map(|ack| serde_json::json!({
                                "who": ack.who,
                                "version": ack.version,
                                "at": ack.at,
                            })).collect::<Vec<_>>(),
                        }),
                        Err(error) => serde_json::json!({
                            "project": store.project,
                            "error": error.text(),
                        }),
                    }
                })
                .collect();
            serde_json::json!({ "hours": HOURS, "projects": projects })
        }
        Err(error) => serde_json::json!({ "error": error.text() }),
    };
    value.to_string()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
