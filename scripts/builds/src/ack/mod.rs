//! `builds ack`, and the gate that refuses an agent's builds commands until
//! the agent has read and acknowledged the text builds asks every agent to
//! read (text.rs).
//!
//! builds distinguishes an agent from the person's terminal by the session
//! identity in its environment. Codex gives builds the coordinator's session
//! and the caller's thread. Claude Code shares a session id among its agents
//! and names the caller with `--agent` or `BUILDS_AGENT` (agent.rs).
//!
//! builds keeps the acknowledgement in the store of the project whose repo
//! the command runs in, else of the project `--project` names. builds refuses an agent that
//! has no acknowledgement of the text. An acknowledgement lasts until Claude
//! Code compacts the agent: its SessionStart hook then runs `builds ack
//! --compacted`, which deletes every acknowledgement of that agent
//! (`compacted`). An acknowledgement is of the sections of a text, by name and
//! hash, and a section that several projects' texts share counts for all of
//! them: an agent reads the sections it has not acknowledged in any project's
//! store (`Agent::known`). When the text changed since the agent acknowledged it, the
//! gate prints one line that gives `builds ack`, passes the command, and
//! acknowledges nothing it must read: only `builds ack` acknowledges sections
//! that changed, so a command whose output nobody reads (a pipe, a hook) leaves
//! them unread.

pub mod agent;
pub mod record;
pub mod text;

use std::path::Path;

use rusqlite::TransactionBehavior;

use crate::checkout::Checkout;
use crate::command::Parsed;
use crate::db::{self, Make, Sql};
use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of, stores};
use crate::places::Places;
use crate::text::shell_word;
use text::{Role, Text, random, show_part};

/// The variable that holds the Claude Code session id in an agent's shell.
pub const SESSION: &str = "CLAUDE_CODE_SESSION_ID";

/// The variable a builds command sets once it passes the gate, so that the
/// builds commands it runs (a hook's checks, install after a landing) pass
/// too.
pub const PASSED: &str = "BUILDS_ACKNOWLEDGED";

/// Refuses <parsed> when an agent runs it with no acknowledgement, and
/// prints the notice of a change of the text on each command until the agent
/// has read it with `builds ack` (`check`). Passes the person's commands, the
/// commands a builds command that passed runs, and the commands that need no
/// acknowledgement (`Parsed::needs_acknowledgement`).
pub fn gate(
    places: &Places,
    session: &agent::Session,
    parsed: &Parsed<'_, String>,
    project: Option<&str>,
) -> Result<()> {
    if std::env::var_os(PASSED).is_some() || !parsed.needs_acknowledgement()? {
        return Ok(());
    }
    let Some(identity) = session.identity()? else {
        return Ok(());
    };
    if let Some(notice) = check(places, &identity, project, &current_dir()?)? {
        crate::output::warn(notice.trim_end_matches('\n'));
    }
    // SAFETY: no other thread runs yet, so none reads the environment while
    // set_var writes it.
    unsafe { std::env::set_var(PASSED, "1") };
    Ok(())
}

/// An agent as builds names it, and where builds keeps its acknowledgement.
struct Agent {
    caller: agent::Caller,
    claude: bool,
    store: Store,
    /// The flags that name this agent to `builds ack` again.
    flags: String,
}

impl Agent {
    fn acknowledged(&self, places: &Places) -> Result<()> {
        if self.claude {
            agent::set_compacted(places, &self.caller.session, &self.caller.id, false)?;
        }
        Ok(())
    }

    /// The agent <id> of <session> that runs a command in <cwd> with
    /// `--project` <project>. Fails when no id names the agent, or when no
    /// hook registered the id in <session>.
    fn resolve(
        places: &Places,
        identity: &agent::Identity,
        project: Option<&str>,
        cwd: &Path,
    ) -> Result<Agent> {
        let caller = identity.caller();
        let claude = matches!(identity, agent::Identity::Claude(_));
        if claude && !agent::is_registered(places, &caller.session, &caller.id)? {
            fail!(
                "builds has registered no agent {} in session {}. Use the id that your Claude Code hook printed.",
                caller.id,
                caller.session
            );
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
        let mut flags = format!(" --agent {}", shell_word(&caller.id));
        if project.is_some() {
            flags += &format!(" --project {}", shell_word(&store.project));
        }
        Ok(Agent {
            caller,
            claude,
            store,
            flags,
        })
    }

    /// The agent as messages name it.
    fn name(&self) -> String {
        format!("agent {}", self.caller.id)
    }

    /// The version of the text the agent last acknowledged.
    fn last(&self) -> Result<Option<String>> {
        match db::open_reader(&self.store.database())? {
            Some(db) => record::last(&db, &self.caller.session, &self.caller.id),
            None => Ok(None),
        }
    }

    /// The text the agent reads: the sections of every agent, and those of
    /// each role it works as, which are the roles whose sections it
    /// acknowledged and the <requested> ones.
    fn text(&self, places: &Places, requested: &[Role]) -> Result<Text> {
        let mut roles = Text::roles_held(&self.known(places)?);
        roles.extend(requested);
        Ok(Text::for_roles(&roles))
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
                known.extend(record::acknowledged_sections(
                    &db,
                    &self.caller.session,
                    &self.caller.id,
                )?);
            }
        }
        Ok(known)
    }

    /// Where the agent stands with <text>: not acknowledged, up to date, or
    /// acknowledged an older version, which this call leaves as it is: only
    /// `builds ack` records an acknowledgement of changed sections. A text
    /// whose sections the agent acknowledged all the same (another project's
    /// store holds them, or only their order changed) has nothing to read, and
    /// this call records the agent's acknowledgement of it.
    fn standing(&self, places: &Places, text: &Text) -> Result<Standing> {
        let version = text.version();
        if self.last()?.as_deref() == Some(version.as_str()) {
            return Ok(Standing::Current);
        }
        let unread = text.unread(&self.known(places)?);
        let mut db = db::open(&self.store.database(), Make::Never)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        let last = record::last(&tx, &self.caller.session, &self.caller.id)?;
        if last.as_deref() == Some(version.as_str()) {
            return Ok(Standing::Current);
        }
        match last {
            None if !unread.sections.is_empty() => return Ok(Standing::NotAcknowledged),
            None => {}
            Some(_) => {
                let kept =
                    record::acknowledged_sections(&tx, &self.caller.session, &self.caller.id)?;
                if !unread.sections.is_empty() || !text.removed_since(&kept).is_empty() {
                    return Ok(Standing::Changed);
                }
            }
        }
        record::acknowledge(
            &tx,
            &self.caller.session,
            &self.caller.id,
            &version,
            &text.hashes(),
        )?;
        tx.commit().sql()?;
        Ok(Standing::Current)
    }
}

/// Where an agent stands with the current text.
enum Standing {
    /// The agent has no acknowledgement of the text in this session.
    NotAcknowledged,
    /// The agent acknowledged the current version.
    Current,
    /// The text changed since the agent acknowledged it, and the agent has
    /// not read the change.
    Changed,
}

/// Refuses the agent of <session> that runs a command in <cwd> with
/// `--project` <project> when it has no acknowledgement of the text.
/// Returns the notice to print when the text changed since the agent
/// acknowledged it (`Agent::standing`).
fn check(
    places: &Places,
    identity: &agent::Identity,
    project: Option<&str>,
    cwd: &Path,
) -> Result<Option<String>> {
    let agent = Agent::resolve(places, identity, project, cwd)?;
    let text = agent.text(places, &[])?;
    match agent.standing(places, &text)? {
        Standing::Current => Ok(None),
        Standing::Changed => Ok(Some(format!(
            "builds: the text every agent reads changed since you acknowledged it. This command runs as usual. Run `builds ack{}` to read the changed sections.\n",
            agent.flags
        ))),
        Standing::NotAcknowledged => fail!(
            "builds refuses this command until you read the text every agent reads before it works, and acknowledge it: {} has not acknowledged it in this session. Run `builds ack{}`, read every part it lists, and acknowledge them; then run this command again. A coordinator, a design session or a judge adds `--role coordinator`, `--role design` or `--role judge` to read the sections of its role.",
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
    let args = Args::parse(args, &["role"], &["compacted", "codex-compacted"])?;
    if args.switch("compacted") {
        crate::output::answer_line(compacted(places, agent::HookInput::from_stdin()?)?);
        return Ok(());
    }
    if args.switch("codex-compacted") {
        crate::output::answer_line(codex_compacted(places, agent::HookInput::from_stdin()?)?);
        return Ok(());
    }
    let Some(identity) = session.identity()? else {
        fail!("Run builds ack from the Codex or Claude Code session reading the rules.");
    };
    crate::output::answer(ack(
        places,
        &identity,
        &args,
        &current_dir()?,
        crate::app_timing::now(),
    )?);
    Ok(())
}

/// What `builds ack` with <args> does for agent <id> of <session> in <cwd>
/// at <now>, and the text it prints.
fn ack(
    places: &Places,
    identity: &agent::Identity,
    args: &Args,
    cwd: &Path,
    now: i64,
) -> Result<String> {
    let mut agent = Agent::resolve(places, identity, args.value("project")?, cwd)?;
    let mut requested = Vec::new();
    for name in args.values("role") {
        let role = Role::parse(name)?;
        if !requested.contains(&role) {
            requested.push(role);
        }
    }
    for role in &requested {
        agent.flags += &format!(" --role {}", role.name());
    }
    let text = agent.text(places, &requested)?;
    match args.positional() {
        [] => start(places, &agent, &text, now),
        [word, part, token] if word == "part" => part_of(places, &agent, &text, part, token),
        [version, token] => acknowledge(places, &agent, &text, version, token),
        _ => fail!("{}", crate::command::Command::Ack.usage_text()),
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
        let removed = text.removed_since(&record::acknowledged_sections(
            &tx,
            &agent.caller.session,
            &agent.caller.id,
        )?);
        record::acknowledge(
            &tx,
            &agent.caller.session,
            &agent.caller.id,
            &version,
            &text.hashes(),
        )?;
        tx.commit().sql()?;
        agent.acknowledged(places)?;
        let removed = if removed.is_empty() {
            String::new()
        } else {
            format!("Sections removed: {}\n", removed.join(", "))
        };
        return Ok(format!(
            "builds: {} has acknowledged every section of version {version} in this session, so there is nothing to read.\n{removed}",
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
        &agent.caller.session,
        &agent.caller.id,
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
    let compacted =
        if agent.claude && agent::was_compacted(places, &agent.caller.session, &agent.caller.id)? {
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
        Some(db) => record::reading(&db, &agent.caller.session, &agent.caller.id)?,
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
        _ => fail!(
            "there are parts 1 to {count}; {part} is none of them\n{}",
            crate::command::Command::Ack.usage_text()
        ),
    };
    if reading.tokens[number - 1] != token {
        fail!(
            "the token is not the one that part {number} needs: the last line of {} gives it",
            if number == 1 {
                "the output of `builds ack`".to_string()
            } else {
                format!("part {} (`builds ack part {}`)", number - 1, number - 1)
            },
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
        &agent.caller.session,
        &agent.caller.id,
        &text.version(),
        &text.hashes(),
    )?;
    tx.commit().sql()?;
    agent.acknowledged(places)?;
    Ok(format!(
        "builds: {} acknowledged version {version}. The acknowledgement holds until the session reports a compaction. When the text changes, builds prints one line before a command's output that gives `builds ack`, and the command runs.\n",
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
    agent::forget_rules(places, &hook.session_id, id)?;
    forget_compacted(places, &hook.session_id, id)?;
    Ok(agent::hook_output(
        "SessionStart",
        &format!(
            "Claude Code compacted your context, so builds ended your acknowledgements of the text every agent reads. {} Run `builds ack --agent {id}` before your next builds command, adding `--role coordinator`, `--role design` or `--role judge` for each role you work as.",
            agent::told(id)
        ),
    ))
}

/// Codex's PostCompact payload names the worker as agent_id and the root
/// as session_id. The hook process can inherit another conversation's shell
/// environment, so only the event identifies the compacted agent.
fn codex_compacted(places: &Places, hook: agent::HookInput) -> Result<String> {
    if hook.hook_event_name != "PostCompact" {
        fail!(
            "Builds expected a Codex PostCompact event, but received {}.",
            hook.hook_event_name
        );
    }
    let session = agent::ThreadId::parse(&hook.session_id)?;
    let worker = hook
        .agent_id
        .as_deref()
        .map(agent::ThreadId::parse)
        .transpose()?;
    let thread = worker.as_ref().unwrap_or(&session);
    forget_compacted(places, session.id(), thread.id())?;
    Ok(serde_json::json!({
        "systemMessage": "Codex compacted this thread. Read and acknowledge the builds rules before your next builds command."
    }).to_string())
}

fn forget_compacted(places: &Places, session: &str, thread: &str) -> Result<()> {
    for store in stores(places)? {
        let mut db = db::open(&store.database(), Make::Never)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .sql()?;
        record::forget(&tx, session, thread)?;
        tx.commit().sql()?;
    }
    Ok(())
}

impl crate::command::Parsed<'_, String> {
    /// Whether the acknowledgement check (ack/mod.rs `gate`) covers the
    /// command. It passes `ack` itself; the hook that gives an agent its
    /// rules; the commands that use no store, since the check reads one and
    /// install must run while the stores still have an older schema;
    /// `status --json --no-folders`, which lists the checkouts, their app
    /// lanes and their locks for the hook that refuses an unlocked move of an
    /// app checkout, which reads it on every agent's command, including an
    /// agent's first after a compaction, before it can acknowledge; and the
    /// pre-commit hook's `checks run` in a repo's main checkout, since no
    /// agent commits there (`builds help workflow`, WORKTREES), so a commit
    /// there is the person's.
    pub fn needs_acknowledgement(&self) -> Result<bool> {
        use crate::command::Command::*;
        use crate::command::*;
        Ok(match self.command {
            Ack | RulesOnEdit => false,
            Status => !crate::status::is_folderless_json(self.rest),
            Checks(checks::Run) => match crate::checks_run::run_worktree(self.rest) {
                Some(worktree) => !crate::checkout::Checkout::at(worktree)?
                    .is_some_and(|checkout| checkout.is_main()),
                None => true,
            },
            Help
            | Setup
            | Lake
            | Tool
            | New
            | Claim
            | Note
            | Release
            | Retire
            | Cargo
            | AppBuild
            | BuildApp
            | Lock
            | Programs
            | Queue
            | Takeover
            | AppClone
            | SccacheTmpdir
            | Prune
            | Du
            | Health
            | InstallHook
            | Waits
            | Land
            | Brief
            | Install
            | Version
            | Next
            | Agent(agent::Start | agent::Run | agent::Resume | agent::List | agent::Show)
            | Shots(shots::Findings | shots::Picture | shots::CheckProgram | shots::Checker)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            )
            | Remote(remote::Run | remote::Step)
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Dashboard(dashboard::Stage | dashboard::Serve)
            | Checks(checks::Whole | checks::Stats | checks::List | checks::Tests)
            | Review(
                review::Run
                | review::Copy
                | review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Replay
                | review::Audit
                | review::Misses
                | review::Rates
                | review::Scores
                | review::CasePropose
                | review::CaseAdd,
            )
            | Judge(judge::Listen)
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::Take
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            )
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            ) => self.command.stores() != crate::install_gate::Stores::Storeless,
        })
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
