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
//! no CLAUDE.md: Claude Code loads ~/.claude/CLAUDE.md and the repo's
//! CLAUDE.md into every agent's context itself. builds refuses an agent that
//! has no acknowledgement of the text. An acknowledgement lasts until Claude
//! Code compacts the agent: its SessionStart hook then runs `builds ack
//! --compacted`, which deletes every acknowledgement of that agent
//! (`compacted`). An acknowledgement is of the sections of a text, by name and
//! hash, and a section that several projects' texts share counts for all of
//! them: an agent reads the sections it has not acknowledged in any project's
//! store (`Agent::known`). When the text changed since the agent acknowledged it, the
//! gate prints a notice of the changed sections once, records the agent as
//! having had it, and passes the command (`catch_up`).

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
use crate::lists::store::{Store, project_of, stores};
use crate::places::Places;
use crate::text::shell_word;
use text::{Text, random, show_part};

/// The variable that holds the Claude Code session id in an agent's shell.
pub const SESSION: &str = "CLAUDE_CODE_SESSION_ID";

/// The variable a builds command sets once it passes the gate, so that the
/// builds commands it runs (a hook's checks, install after a landing) pass
/// too.
pub const PASSED: &str = "BUILDS_ACKNOWLEDGED";

const USAGE: &str = "usage: builds ack [--agent ID] [--project P]\n       builds ack part PART TOKEN [--agent ID] [--project P]\n       builds ack VERSION TOKEN [--agent ID] [--project P]\n       builds ack --compacted";

/// Refuses `builds <command> <args>` when an agent runs it with no
/// acknowledgement, and prints the notice of a change of the text once
/// (`check`). Passes the person's commands, `ack`, which records the
/// acknowledgement, `status --json --no-folders` (`lists_checkouts`), and the commands a builds command that passed runs. The
/// gate also passes the commands that do not use the stores
/// (`install_gate::STORELESS`): the gate reads a store, and install must run
/// while the stores still have an older schema, because install migrates them
/// to its schema. The gate passes the pre-commit hook's checks in a main
/// checkout too (`commits_in_main_checkout`).
pub fn gate(
    places: &Places,
    session: &agent::Session,
    command: &str,
    args: &[String],
) -> Result<()> {
    if command == "ack"
        || install_gate::STORELESS.contains(&command)
        || std::env::var_os(PASSED).is_some()
        || lists_checkouts(command, args)
        || commits_in_main_checkout(command, args)?
    {
        return Ok(());
    }
    let Some(session) = session.claude()? else {
        return Ok(());
    };
    if let Some(notice) = check(
        places,
        session,
        agent::from_env()?.as_deref(),
        flag(args, "project"),
        &current_dir()?,
        crate::app_timing::now(),
    )? {
        eprint!("{notice}");
    }
    // SAFETY: no other thread runs yet, so none reads the environment while
    // set_var writes it.
    unsafe { std::env::set_var(PASSED, "1") };
    Ok(())
}

/// Whether `builds <command> <args>` is `status --json --no-folders`, which
/// lists the checkouts, their app lanes and their locks. That list depends on
/// no rule, and the hook that refuses an unlocked move of an app checkout
/// (no-unlocked-app-checkout-moves) reads it on every agent's command,
/// including an agent's first after a compaction, before it can acknowledge.
fn lists_checkouts(command: &str, args: &[String]) -> bool {
    command == "status" && crate::status::is_folderless_json(args)
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
            Some(checkout) => project_of(places, checkout, project),
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
            flags,
        })
    }

    /// The agent as messages name it.
    fn name(&self) -> String {
        format!("agent {}", self.id)
    }

    fn text(&self, places: &Places) -> Result<Text> {
        Text::read(places, &self.store.project)
    }

    /// The version of the text the agent last acknowledged.
    fn last(&self) -> Result<Option<String>> {
        match db::open_reader(&self.store.database())? {
            Some(db) => record::last(&db, &self.session, &self.id),
            None => Ok(None),
        }
    }

    /// The sections, each by name and hash, that the agent acknowledged in
    /// this session in any project's store: a section that several
    /// projects' texts share is read once.
    fn known(&self, places: &Places) -> Result<Vec<(String, String)>> {
        let mut known = Vec::new();
        let mut of_stores = vec![self.store.clone()];
        for store in stores(places)? {
            if store.project != self.store.project {
                of_stores.push(store);
            }
        }
        for store in of_stores {
            if let Some(db) = db::open_reader(&store.database())? {
                known.extend(record::acknowledged_sections(&db, &self.session, &self.id)?);
            }
        }
        Ok(known)
    }

    /// Where the agent stands with <text>: not acknowledged, up to date, or
    /// acknowledged an older version, which this call replaces with the
    /// current one so that the change is told once. An agent with no
    /// acknowledgement here has acknowledged the text all the same when it
    /// acknowledged every section of it in another project: this call records
    /// that.
    fn standing(&self, places: &Places, text: &Text, now: i64) -> Result<Standing> {
        let version = text.version();
        if self.last()?.as_deref() == Some(version.as_str()) {
            return Ok(Standing::Current);
        }
        let unread = text.unread(&self.known(places)?);
        let mut db = db::open(&self.store.database(), Make::Never)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        let last = record::last(&tx, &self.session, &self.id)?;
        if last.as_deref() == Some(version.as_str()) {
            return Ok(Standing::Current);
        }
        let standing = match last {
            None if !unread.sections.is_empty() => return Ok(Standing::NotAcknowledged),
            None => Standing::Current,
            Some(last) => {
                let names: Vec<String> = unread.sections.iter().map(|s| s.name.clone()).collect();
                Standing::Changed(match record::kept_sections(&tx, &last)? {
                    Some(kept) => Changes::Since {
                        changed: names,
                        removed: text.removed_since(&kept),
                    },
                    None => Changes::NotKept { sections: names },
                })
            }
        };
        record::acknowledge(&tx, &self.session, &self.id, &version, now, &text.hashes())?;
        tx.commit().sql()?;
        Ok(standing)
    }
}

/// Where an agent stands with the current text.
enum Standing {
    /// The agent has no acknowledgement of the text in this session.
    NotAcknowledged,
    /// The agent acknowledged the current version.
    Current,
    /// The text changed since the agent acknowledged it, and builds has now
    /// recorded the agent as told.
    Changed(Changes),
}

/// What changed in the text since an agent acknowledged it.
enum Changes {
    /// The sections whose text differs, and the sections that left the text.
    Since {
        changed: Vec<String>,
        removed: Vec<String>,
    },
    /// builds no longer keeps the version the agent acknowledged, so it lists
    /// every section.
    NotKept { sections: Vec<String> },
}

impl Changes {
    /// The notice that tells the agent what changed and how to read it, or
    /// None when only the order of the sections changed.
    fn notice(&self, text: &Text) -> Option<String> {
        let (changed, removed, intro) = match self {
            Changes::Since { changed, removed } => (
                changed.as_slice(),
                removed.as_slice(),
                "builds: the text every agent reads changed since you acknowledged it. This command runs as usual.",
            ),
            Changes::NotKept { sections } => (
                sections.as_slice(),
                &[][..],
                "builds: the text every agent reads changed since you acknowledged it, and builds no longer keeps the version you acknowledged, so it lists each section whose current text you have not acknowledged in this session. This command runs as usual.",
            ),
        };
        if changed.is_empty() && removed.is_empty() {
            return None;
        }
        let mut out = format!("{intro}\n");
        if !changed.is_empty() {
            out += "Read these sections:\n";
            for name in changed {
                let how = text.how_to_read(name);
                out += &format!("  {how}\n");
            }
        }
        if !removed.is_empty() {
            out += &format!("Sections removed: {}\n", removed.join(", "));
        }
        Some(out)
    }
}

/// Refuses the agent of <session> that runs a command in <cwd> with
/// `--project` <project> at <now> when it has no acknowledgement of the text.
/// Returns the notice to print once when the text changed since the agent
/// acknowledged it (`Agent::standing`).
fn check(
    places: &Places,
    session: &str,
    id: Option<&str>,
    project: Option<&str>,
    cwd: &Path,
    now: i64,
) -> Result<Option<String>> {
    let agent = Agent::resolve(places, session, id, project, cwd)?;
    let text = agent.text(places)?;
    match agent.standing(places, &text, now)? {
        Standing::Current => Ok(None),
        Standing::Changed(changes) => Ok(changes.notice(&text)),
        Standing::NotAcknowledged => fail!(
            "builds refuses this command until you read the text every agent reads before it works, and acknowledge it: {} has not acknowledged it in this Claude Code session. Run `builds ack{}`, read every part it lists, and acknowledge them; then run this command again.",
            agent.name(),
            agent.flags
        ),
    }
}

/// Why the main agent of <session>, which `builds judge listen` keeps as the
/// judge, must acknowledge the text of <store>'s project, or None when it
/// has acknowledged it. A change of the text since needs no message: the
/// judge's next command prints the notice of it.
pub fn outdated(store: &Store, session: &str) -> Result<Option<String>> {
    let last = match db::open_reader(&store.database())? {
        Some(db) => record::last(&db, session, session)?,
        None => None,
    };
    Ok(last
        .is_none()
        .then(|| format!("agent {session} has no acknowledgement of the text")))
}

/// `builds ack`.
pub fn cmd_ack(places: &Places, session: &agent::Session, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &["compacted"])?;
    if args.switch("compacted") {
        println!("{}", compacted(places, agent::HookInput::from_stdin()?)?);
        return Ok(());
    }
    let Some(session) = session.claude()? else {
        fail!(
            "builds ack records an agent's acknowledgement under its Claude Code session ({SESSION}), which this shell lacks; builds never refuses a command run outside Claude Code"
        );
    };
    print!(
        "{}",
        ack(
            places,
            session,
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
        [] => start(places, &agent, &text, now),
        [word, part, token] if word == "part" => part_of(places, &agent, &text, part, token),
        [version, token] => acknowledge(places, &agent, &text, version, token, now),
        _ => fail!("{USAGE}"),
    }
}

/// Starts a reading of the sections of <text> that the agent has not
/// acknowledged in this session, in any project: draws the tokens of its
/// parts, and gives the command for part 1. When the agent has acknowledged
/// every section, it records the acknowledgement of the text and reads
/// nothing.
fn start(places: &Places, agent: &Agent, text: &Text, now: i64) -> Result<String> {
    let known = agent.known(places)?;
    let unread = text.unread(&known);
    let version = text.version();
    let mut db = db::open(&agent.store.database(), Make::Never)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .sql()?;
    if unread.sections.is_empty() {
        record::acknowledge(
            &tx,
            &agent.session,
            &agent.id,
            &version,
            now,
            &text.hashes(),
        )?;
        tx.commit().sql()?;
        agent::set_compacted(places, &agent.session, &agent.id, false)?;
        return Ok(format!(
            "builds: {} has acknowledged every section of version {version} in this session, so there is nothing to read.\n",
            agent.name()
        ));
    }
    let parts = unread.parts();
    let mut tokens = Vec::new();
    for _ in 0..=parts.len() {
        tokens.push(token()?);
    }
    let reading_version = unread.version();
    record::start_reading(
        &tx,
        &agent.session,
        &agent.id,
        &reading_version,
        now,
        &tokens,
    )?;
    tx.commit().sql()?;
    let skipped: Vec<&str> = text
        .sections
        .iter()
        .filter(|section| !unread.sections.iter().any(|s| s.name == section.name))
        .map(|section| section.name.as_str())
        .collect();
    let skipped = if skipped.is_empty() {
        String::new()
    } else {
        format!(
            "This reading leaves out the sections you acknowledged in this session: {}.\n",
            skipped.join(", ")
        )
    };
    let compacted = if agent::was_compacted(places, &agent.session, &agent.id)? {
        format!("{COMPACTED_SENTENCE}\n")
    } else {
        String::new()
    };
    Ok(format!(
        "{WHY_SENTENCE}\n{compacted}builds ack: the text is version {reading_version}, in {} parts.\n{skipped}  builds ack part 1 {}{}\n",
        parts.len(),
        tokens[0],
        agent.flags
    ))
}

/// The first line of every reading: why builds asks for it.
const WHY_SENTENCE: &str = "builds asks every agent to read this text first, because agents that skip it work in the person's checkout, push main and poll for builds.";

/// The line after it for an agent that Claude Code compacted.
const COMPACTED_SENTENCE: &str =
    "Claude Code compacted you, so this text is no longer in your context.";

/// A token for a reading: 16 hex digits from the system's random source.
fn token() -> Result<String> {
    Ok(format!("{:016x}", random()?))
}

/// The agent's reading of <unread>, the sections it has not acknowledged,
/// refused when it has none or when those sections changed since it started.
fn reading_of(agent: &Agent, unread: &Text) -> Result<record::Reading> {
    let reading = match db::open_reader(&agent.store.database())? {
        Some(db) => record::reading(&db, &agent.session, &agent.id)?,
        None => None,
    };
    let Some(reading) = reading else {
        fail!(
            "{} has no reading; run `builds ack{}` first, which gives the command for part 1",
            agent.name(),
            agent.flags
        );
    };
    if reading.version != unread.version() {
        fail!(
            "the sections {} has not acknowledged changed since it ran `builds ack`; run `builds ack{}` again",
            agent.name(),
            agent.flags
        );
    }
    Ok(reading)
}

/// Part <part> of the agent's reading, for the agent that gives <token>, the
/// token of the part before it (the opening output's for part 1).
fn part_of(places: &Places, agent: &Agent, text: &Text, part: &str, token: &str) -> Result<String> {
    let unread = text.unread(&agent.known(places)?);
    let reading = reading_of(agent, &unread)?;
    let parts = unread.parts();
    let count = parts.len();
    let number = match part.parse::<usize>() {
        Ok(number) if (1..=count).contains(&number) => number,
        _ => fail!("there are parts 1 to {count}; {part} is none of them\n{USAGE}"),
    };
    if reading.tokens[number - 1] != token {
        fail!(
            "the token is not the one that part {number} needs: the last line of {} gives it",
            if number == 1 {
                "the output of `builds ack`".to_string()
            } else {
                format!("part {} (`builds ack part {}`)", number - 1, number - 1)
            }
        );
    }
    let next = if number == count {
        format!(
            "  builds ack {} {}{}",
            reading.version, reading.tokens[number], agent.flags
        )
    } else {
        format!(
            "  builds ack part {} {}{}",
            number + 1,
            reading.tokens[number],
            agent.flags
        )
    };
    Ok(show_part(
        &parts[number - 1],
        number,
        count,
        &reading.version,
        &next,
    ))
}

/// Records the acknowledgement of <version> for the agent that gives <token>,
/// the token of the last part; refuses a wrong token.
fn acknowledge(
    places: &Places,
    agent: &Agent,
    text: &Text,
    version: &str,
    token: &str,
    now: i64,
) -> Result<String> {
    let reading = reading_of(agent, &text.unread(&agent.known(places)?))?;
    if version != reading.version {
        fail!(
            "{} reads version {}, not {version}",
            agent.name(),
            reading.version
        );
    }
    let count = reading.tokens.len() - 1;
    if reading.tokens[count] != token {
        fail!(
            "the token is not the one that acknowledges the text: the last line of part {count} (`builds ack part {count}`) gives it, and each part needs the token of the part before it"
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
        &text.version(),
        now,
        &text.hashes(),
    )?;
    tx.commit().sql()?;
    agent::set_compacted(places, &agent.session, &agent.id, false)?;
    Ok(format!(
        "builds: {} acknowledged version {version}. The acknowledgement holds until Claude Code compacts this agent. When the text changes, builds prints the changed sections once before a command's output, and the command runs.\n",
        agent.name()
    ))
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
    agent::set_compacted(places, &hook.session_id, id, true)?;
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
