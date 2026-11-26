//! `builds review`: reviews a change against the rules of agent-instructions,
//! one reviewer per group of rules (one folder of `rules/` each), and records
//! every review in the project's store.
//!
//! ```text
//! builds review [run] WORKTREE [BASE] --kind code|mechanical [--checked-by TEXT] [--landing KEY] [--item N]... [--head SHA] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review copy WORKTREE [BASE] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review wait LANDING
//! builds review settle FINDING confirmed|dropped --why TEXT
//! builds review settle FINDING --again --why TEXT
//! builds review settle FINDING allowed --by-person --why TEXT
//! builds review dispute FINDING --why TEXT
//! builds review keep FINDING --why TEXT
//! builds review keep FINDING --again --why TEXT
//! builds review show LANDING
//! builds review check-landing | landed DIR --project P SHA... [--landing KEY]
//! builds review groups [--rules-root DIR] [--project P --repo R --changed PATH...]
//! builds review replay [--case NAME]... [--source REPO] [--model M] [--rules-root DIR] [--compare] [--reviewer PROGRAM]
//! builds review replay --landing N... --repo-dir DIR [--group NAME]... [--model M] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review audit LANDING --repo-dir DIR
//! builds review misses [--check]
//! builds review rates | scores
//! builds review case propose | add ...
//! ```
//!
//! `run` reviews the worktree's net change past BASE (default `origin/main`)
//! before it lands. builds review reads no diff more than once. Each change
//! gets one review. The groups whose rules cover a changed path each get one
//! `claude -p`. The run
//! records the net change it read under the change's KEY, then says what
//! `builds land` would say of the branch's commits (`landing.rs`), and exits
//! 1 unless land would take them. The change's landing names the run of its
//! last review, and a review shows the changes since the last review, and
//! nothing else (`run::diff_since`): fix commits and reworded commit messages
//! are part of them, and main's own commits that a rebase brings in are not.
//! A run on a KEY reviewed already asks the groups that gave the review no
//! answer, over the diff they did not answer, and reads the changes since the
//! last review in a new run, which becomes the last review. Each reviewer also gets the open confirmed
//! findings of its group's rules, checks the code at each one's place, and
//! closes each one whose problem is gone. A group that holds open findings
//! reads every change since the run that raised them, whatever files the
//! changes touch, so that it sees a fix in any file: it reads every file of
//! the changes since the last review, and when it has not read every change
//! up to the last review with its findings given, a run of its own reads the
//! changes since the last net change it did read so. Of the other groups of
//! a run, none reads a file where a group of the run holds open findings,
//! and each reads every other changed file its rules cover
//! (`run::ask_groups`). New findings in design and capture files require
//! rules that judge those kinds of files (`rules::Rule::judges`). A group
//! also reads capture file changes to decide whether its open findings are
//! fixed. `landing::Coverage`
//! says what the review read, for this run and for `builds land`. The run
//! refuses when the review lacks nothing, no confirmed finding is open and no
//! closed finding goes back to its group (`landing::Coverage::rechecks`).
//!
//! `run` asks no copy group (`groups::Group::copy`), whose rules judge word
//! choice, grammar and style. `copy` asks only those, over the worktree's net
//! change, records the run and its findings, and prints them (`copy.rs`). The
//! person runs it by hand; no landing waits on it.
//!
//! Every call records its setup (its group's model and prompt), its tokens and
//! its cost. `scores` counts how each setup did from the settled findings, and
//! `replay --landing` asks again over recorded diffs and counts the replay the
//! same way (`score.rs`).

mod audit;
pub mod audit_item;
pub mod call;
mod cases;
pub mod check;
pub mod commits;
pub mod copy;
pub mod design_review;
pub mod ended;
pub mod exchange_time;
#[cfg(test)]
pub mod fakes;
mod files;
pub mod frequent;
pub mod groups;
pub mod idle;
pub mod judge;
pub mod judge_ack;
pub mod judge_told;
mod kept_correction;
pub mod kept_open;
mod key;
pub mod landing;
pub mod marked_mechanical;
pub mod mechanical;
mod model_check;
mod prompt;
mod propose;
pub mod provider;
pub use key::landing_key;
pub use provider::{Program, Selection};
pub mod record;
mod render;
mod rerun;
pub mod rules;
pub mod run;
mod score;
pub mod texts;
pub mod verdicts;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ack::agent::Caller;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::inbox::{Messaging, Recipient};
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::design_paths::Globs;
use crate::lists::spec_check::Brief;
use crate::lists::store::{Store, project_of};
use crate::tool::Tool;
use boards::git;
use rusqlite::Connection;

use record::Kind;

const USAGE: &str = "usage: builds review [run] WORKTREE [BASE] --kind code|mechanical [--checked-by TEXT] [--landing KEY] [--item N]... [--head SHA] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review copy WORKTREE [BASE] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review wait LANDING\n       builds review settle FINDING confirmed|dropped --why TEXT\n       builds review settle FINDING --again --why TEXT\n       builds review settle FINDING allowed --by-person --why TEXT\n       builds review dispute FINDING --why TEXT\n       builds review keep FINDING --why TEXT\n       builds review keep FINDING --again --why TEXT\n       builds review show LANDING
       builds review key DIR --project P
       builds review check-landing DIR --project P SHA... [--landing KEY] [--squashed] [--rules-root DIR]\n       builds review landed DIR --project P SHA... [--landing KEY] [--more-to-land] [--rules-root DIR]\n       builds review groups [--rules-root DIR] [--project P --repo R --changed PATH...]\n       builds review replay [--case NAME]... [--source REPO] [--model MODEL] [--rules-root DIR] [--compare] [--reviewer PROGRAM]\n       builds review replay --landing N... --repo-dir DIR [--group NAME]... [--model MODEL] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review audit LANDING --repo-dir DIR [--rules-root DIR] [--reviewer PROGRAM]\n       builds review misses [--check [--model MODEL] [--rules-root DIR] [--reviewer PROGRAM]]\n       builds review rates [--rules-root DIR]\n       builds review scores\n       builds review case propose --repo-dir DIR --rules-root DIR [--per-group N] [--model MODEL] [--reviewer PROGRAM]\n       builds review case add LANDING FINDING --repo-dir DIR --rules-root DIR [--name NAME]\n       builds review case add --rule SLUG --file PATH --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR\n       builds review case add --clean-for SLUG --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR";

/// The checkout the rules and groups are read from: `--rules-root`, else the
/// one `install.sh` links at ~/.codex/agent-instructions.
fn rules_root(args: &Args) -> Result<PathBuf> {
    match args.value("rules-root")? {
        Some(root) => Ok(PathBuf::from(root)),
        None => default_rules_root(),
    }
}

/// The checkout `install.sh` links at ~/.codex/agent-instructions.
pub fn default_rules_root() -> Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => Ok(rules::installed_root(Path::new(&home))),
        None => fail!("HOME is not set; name the checkout with --rules-root"),
    }
}

/// The reviewer program a command asks and the rules checkout it reads, for
/// the commands that check text before they write it (`lists::spec_check`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reviewer {
    pub program: Program,
    pub rules_root: PathBuf,
}

impl Reviewer {
    pub fn program_for(&self, explicit: Option<&str>) -> Program {
        explicit.map_or_else(
            || self.program.clone(),
            |path| self.program.with_executable(path.into()),
        )
    }

    /// The session's reviewer and the rules checkout install links in the home folder.
    pub fn installed(
        places: &crate::places::Places,
        session: &crate::ack::agent::Session,
        explicit: Option<std::ffi::OsString>,
    ) -> Reviewer {
        let executable = provider::default_program(explicit, session);
        let temporary = places
            .tmpdir
            .as_ref()
            .or(places.user_temp.as_ref())
            .unwrap_or(&places.private_tmp)
            .clone();
        Reviewer {
            program: Program::new(executable, temporary),
            rules_root: rules::installed_root(&places.home),
        }
    }
}

/// The rules the coordinator confirmed findings of most often on the first
/// reviews of the recent branches of <project> and of the other projects
/// (`frequent.rs`), counted over <rules>, the rules of <project>.
pub fn confirmed_often(
    places: &crate::places::Places,
    project: &str,
    rules: &[rules::Rule],
) -> Result<Vec<frequent::Frequent>> {
    let conn = Store::named(places, project)?.open_existing()?;
    let others = crate::lists::store::stores(places)?
        .into_iter()
        .filter(|store| store.project != project)
        .map(|store| store.open_existing())
        .collect::<Result<Vec<_>>>()?;
    let others: Vec<&rusqlite::Connection> = others.iter().map(|held| &**held).collect();
    frequent::confirmed_often(&conn, &others, rules)
}

/// The file a review of the commits up to <head> holds locked while it runs,
/// in the store's folder; the dashboard reads a held one as a review running.
pub fn running_file(store_dir: &Path, head: &str) -> PathBuf {
    store_dir.join("reviews-running").join(head)
}

/// Whether a review of the commits up to <head> runs now.
pub fn is_running(store_dir: &Path, head: &str) -> bool {
    crate::flock::is_held(&running_file(store_dir, head)).unwrap_or(false)
}

/// Holds <path> locked until dropped, then removes it.
struct Running {
    path: PathBuf,
    fd: libc::c_int,
}

impl Running {
    fn start(path: PathBuf) -> Result<Running> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
        }
        let Some(fd) = crate::queue::try_lock(&path)? else {
            fail!("a review of these commits runs already");
        };
        Ok(Running { path, fd })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        unsafe { libc::close(self.fd) };
    }
}

pub fn cmd_review(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "groups" => list_groups(tool, rest),
        "wait" => wait_command(tool, rest),
        "settle" => settle(tool, rest, tool.session.caller()?.as_ref()),
        "dispute" => dispute_command(
            tool,
            rest,
            tool.session.caller()?.as_ref(),
            Messaging::for_session(&tool.session, Recipient::Coordinator)?.as_ref(),
        ),
        "keep" => keep_command(tool, rest, tool.session.caller()?.as_ref()),
        "show" => show(tool, rest),
        "run" => run_command(tool, rest),
        "copy" => copy_command(tool, rest),
        "key" => key::key_command(tool, rest),
        "check-landing" => landing::check_landing_command(tool, rest),
        "landed" => landing::landed_command(tool, rest),
        "replay" => replay(tool, rest),
        "audit" => audit_command(tool, rest),
        "misses" => misses(tool, rest),
        "rates" => rates(tool, rest),
        "scores" => scores(tool, rest),
        "case" => case_command(tool, rest),
        _ => run_command(tool, args),
    }
}

/// What one review needs.
pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    pub repo: &'a str,
    pub worktree: &'a Path,
    pub base: &'a str,
    /// The commit to review, read from a copy of its files; the worktree's
    /// HEAD, read in place, when none.
    pub head: Option<&'a str>,
    /// Names the change, and so its landing under review.
    pub key: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a Program,
    pub stamp: &'a Stamp,
    /// The landing's brief.
    pub brief: Option<crate::lists::spec_check::Brief>,
    /// The agent that runs the review; None for the person.
    pub runner: Option<&'a Caller>,
}

/// What a review found.
#[derive(Debug)]
pub struct Reported {
    pub landing: Option<i64>,
    /// Everything to print.
    pub text: String,
    /// What `builds land` would refuse these commits for; None when it
    /// would take them.
    pub refusal: Option<String>,
}

/// Reviews the worktree's net change past the base and records it. A change
/// gets one review: when the change, its repo and key, has one already, this
/// asks only what the review lacks (`later_asks`), and, when it lacks nothing, asks the groups that hold open confirmed findings
/// again (`later_asks`); it refuses when none is open either.
pub fn review(inputs: &Inputs) -> Result<Reported> {
    let conn = inputs.store.open_existing()?;
    let change = match inputs.head {
        Some(head) => {
            let mut change = run::diff_between(inputs.worktree, inputs.base, head)?;
            if let Some(source) = &mut change.diff.source {
                source.in_place = false;
            }
            change
        }
        None => run::diff_of(inputs.worktree, inputs.base)?,
    };
    if change.diff.patch.text().trim().is_empty() {
        return Ok(Reported {
            landing: None,
            text: format!("nothing to review: no changes past {}\n", inputs.base),
            refusal: None,
        });
    }
    let shas = crate::git::commits_in_range(inputs.worktree, &change.base, &change.head)?;
    let material = run::Material::load(inputs.root, inputs.project)?;
    let design = Globs::read(&conn, inputs.repo)?.with_capture_folders(inputs.worktree)?;
    let scope = run::Scope {
        material: &material,
        files: &design,
    };
    if scope.group_names(&change.diff.changed).is_empty() {
        return Ok(Reported {
            landing: None,
            text: "no group's rules cover the changed paths: nothing to review, and builds land needs no review\n".to_string(),
            refusal: None,
        });
    }
    let under_review = record::Change {
        repo: inputs.repo,
        key: inputs.key,
    };
    let open = record::open_landing(&conn, under_review)?;
    // The confirmed findings no review closed, and the closed ones that go
    // back to their group (`landing::Coverage::rechecks`), which each reviewer
    // checks against the code as it is now.
    let open_findings: Vec<record::Finding> = match open {
        None => Vec::new(),
        Some(landing) => {
            let mut findings: Vec<record::Finding> = record::findings_of_landing(&conn, landing)?
                .into_iter()
                .filter(record::Finding::is_open)
                .collect();
            let coverage = landing::Coverage::of(&conn, &record::landing(&conn, landing)?)?;
            findings.extend(
                coverage
                    .rechecks(
                        &conn,
                        landing,
                        scope,
                        inputs.worktree,
                        &change.base,
                        &change.head,
                    )?
                    .into_iter()
                    .map(|recheck| recheck.finding),
            );
            findings.sort_by_key(|finding| finding.id);
            findings
        }
    };
    // A finding in a file the branch deleted needs no reviewer to say its
    // code is gone: the run closes it.
    let (open_findings, deleted) =
        run::split_deleted_findings(inputs.worktree, &change.head, open_findings)?;
    let (asks, previous_runs) = match open {
        None => (
            vec![Ask {
                groups: scope.group_names(&change.diff.changed),
                change,
                since: None,
                last_review: true,
                note: None,
                open: Vec::new(),
                at_head: true,
                deleted,
            }],
            Vec::new(),
        ),
        Some(landing) => later_asks(
            &conn,
            inputs,
            scope,
            &change,
            landing,
            &open_findings,
            deleted,
        )?,
    };
    let mut running: Vec<(String, Running)> = Vec::new();
    let mut answers = Vec::new();
    for ask in &asks {
        if !running.iter().any(|(head, _)| *head == ask.change.head) {
            let file = running_file(&inputs.store.dir, &ask.change.head);
            running.push((ask.change.head.clone(), Running::start(file)?));
        }
        let picked = material.groups.named(&material.rules, &ask.groups);
        answers.push(run::ask_groups(
            scope,
            inputs.reviewer,
            run::ReviewKind::Branch,
            &picked,
            &ask.change.diff,
            inputs.brief.as_ref(),
            &ask.open,
        ));
    }
    let still_found = account_for_repeated_findings(
        inputs.worktree,
        &asks,
        &mut answers,
        &open_findings,
        &previous_runs,
    )?;
    let stamp = inputs.stamp;
    let (landing, runs) = inputs
        .store
        .write(stamp, &format!("review {}", inputs.key), |db| {
            let landing = match open {
                Some(id) => id,
                None => record::add_landing(db, under_review, stamp.at)?,
            };
            let mut runs = Vec::new();
            for (ask, asked) in asks.iter().zip(&answers) {
                let (run_id, _) = record::add_run(
                    db,
                    &inputs.store.texts,
                    &record::NewRun {
                        landing: Some(landing),
                        kind: Kind::Review {
                            changed_lines: check::changed_lines(ask.change.diff.patch.text()),
                        },
                        label: None,
                        at: stamp.at,
                        base: &ask.change.base,
                        head: &ask.change.head,
                        since: ask.since,
                        rules_version: &run::rules_version(inputs.root),
                    },
                    &asked.calls,
                )?;
                for &finding in &ask.deleted {
                    record::close(db, finding, run_id, Some(landing))?;
                }
                if ask.last_review {
                    record::record_reviewed(db, landing, run_id)?;
                }
                if let Some(runner) = inputs.runner {
                    judge::record_branch_agent(db, run_id, runner)?;
                }
                judge_told::begin(
                    db,
                    &record::findings_of_run(db, run_id)?
                        .iter()
                        .filter(|finding| finding.is_unsettled())
                        .map(|finding| (finding.id, judge_told::Matter::Settle))
                        .collect::<Vec<_>>(),
                )?;
                runs.push((
                    run_id,
                    record::calls_of_run(db, run_id)?,
                    record::findings_of_run(db, run_id)?,
                ));
            }
            Ok((landing, runs))
        })?;
    drop(running);
    let mut text = String::new();
    for (at, ((ask, asked), (run_id, calls, findings))) in
        asks.iter().zip(&answers).zip(&runs).enumerate()
    {
        text.push_str(&format!(
            "review of {} (landing {landing}, run {run_id}) over {}...{}\n",
            inputs.key,
            short(&ask.change.base),
            short(&ask.change.head)
        ));
        if let Some(note) = &ask.note {
            text.push_str(note);
        }
        for group in &asked.nothing_to_read {
            text.push_str(&format!(
                "{group}: the diff holds none of the files its rules cover (a commit changed one and a later commit changed it back, or the diff changes only design files, which its rules do not judge), so it had nothing to read: its answer is no findings\n"
            ));
        }
        for (group, paths) in &asked.not_read {
            text.push_str(&format!(
                "{group} did not read {}: other groups hold the open findings in those files, so only those groups read them\n",
                paths.join(", ")
            ));
        }
        text.push_str(&render::run_report(calls, findings));
        for finding in &still_found[at] {
            text.push_str(&format!(
                "{} still finds finding {} ({}, {}): the code at its place has not changed, so builds records no new finding and tells the judge nothing\n",
                finding.group, finding.id, finding.rule, finding.file
            ));
        }
        for call in &asked.calls {
            for (id, verdict) in &call.follow_up {
                let closed_by = open_findings
                    .iter()
                    .find(|finding| finding.id == *id)
                    .and_then(|finding| finding.closed_by);
                match (verdict, closed_by) {
                    (call::FollowUp::Fixed, None) => {
                        text.push_str(&render::closed_text(*run_id, *id));
                    }
                    (call::FollowUp::Fixed, Some(closing)) => {
                        text.push_str(&render::rechecked_text(&call.group, *id, closing, *run_id));
                    }
                    (call::FollowUp::StillOpen(reason), None) => {
                        text.push_str(&kept_open::text(&call.group, *id, reason));
                    }
                    (call::FollowUp::StillOpen(reason), Some(closing)) => {
                        text.push_str(&render::reopened_text(&call.group, *id, closing, reason));
                    }
                }
            }
        }
        for &finding in &ask.deleted {
            text.push_str(&render::closed_text(*run_id, finding));
        }
    }
    // What builds land would say of the branch's commits now.
    let needs = landing::needs(inputs.worktree, shas, |paths| scope.group_names(paths))?;
    let verdict = landing::judge(
        &conn,
        under_review,
        inputs.worktree,
        &needs,
        landing::CodeCheck::Branch(inputs.head.unwrap_or("HEAD")),
        scope,
    )?;
    let refusal = landing::refusal(&needs, &verdict);
    if refusal.is_none() {
        text.push_str(&format!("builds land: {}\n", landing::taken(&verdict)));
    }
    Ok(Reported {
        landing: Some(landing),
        text,
        refusal,
    })
}

/// A group can raise the same problem again on unchanged code. Record it as
/// another observation of the open finding. A repeat at the head contradicts
/// any fixed verdict the group gives there, even in a separate ask.
fn account_for_repeated_findings<'a>(
    repo: &Path,
    asks: &[Ask],
    answers: &mut [run::Asked],
    open: &'a [record::Finding],
    previous_runs: &[record::RunRow],
) -> Result<Vec<Vec<&'a record::Finding>>> {
    let read: Vec<_> = previous_runs.iter().collect();
    let mut repeated_at_head = BTreeMap::new();
    let mut repeated = Vec::new();
    for (ask, answer) in asks.iter().zip(answers.iter_mut()) {
        let mut still: Vec<&record::Finding> = Vec::new();
        for call in &mut answer.calls {
            for raised in std::mem::take(&mut call.findings) {
                if raised.check_failed.is_some() {
                    call.findings.push(raised);
                    continue;
                }
                let candidates = open
                    .iter()
                    .filter(|held| held.group == call.group && held.could_match_quote(&raised));
                let mut repeated = None;
                for held in candidates {
                    if same_finding_site(repo, &ask.change, &read, held, &raised)? {
                        repeated = Some(held);
                        break;
                    }
                }
                let Some(finding) = repeated else {
                    call.findings.push(raised);
                    continue;
                };
                if ask.at_head {
                    repeated_at_head.entry(finding.id).or_insert(raised.problem);
                }
                if !still.iter().any(|seen| seen.id == finding.id) {
                    still.push(finding);
                }
            }
        }
        repeated.push(still);
    }
    for (ask, answer) in asks.iter().zip(answers) {
        if ask.at_head {
            for call in &mut answer.calls {
                let disagreements: Vec<_> = call
                    .follow_up
                    .iter()
                    .filter_map(|(id, verdict)| match verdict {
                        call::FollowUp::Fixed => repeated_at_head
                            .get(id)
                            .map(|reason| call::disagreement(*id, reason)),
                        call::FollowUp::StillOpen(_) => None,
                    })
                    .collect();
                if !disagreements.is_empty() {
                    call.failure = Some(disagreements.join(" "));
                    call.follow_up.clear();
                }
            }
        }
    }
    Ok(repeated)
}

/// Whether the checked report identifies the open finding's site.
fn same_finding_site(
    repo: &Path,
    change: &run::NetChange,
    read: &[&record::RunRow],
    held: &record::Finding,
    raised: &record::NewFinding,
) -> Result<bool> {
    use record::FindingLine;

    let Some(run) = read.iter().find(|run| run.id == held.run_id) else {
        fail!(
            "Builds found no review run {} for open finding {}.",
            held.run_id,
            held.id
        );
    };
    let previous = || {
        run::diff_of_run(repo, read, run, |run| {
            Ok((run.base.clone(), run.head.clone()))
        })
    };
    let mut earlier = None;
    let recovered;
    let original = match &held.line {
        FindingLine::Verified(site) => site,
        FindingLine::Reported(_) => return Ok(false),
        FindingLine::Legacy(line) => {
            let previous = previous()?;
            let quotes = previous.diff.quotes();
            let site = check::code_site(&quotes, &held.file, *line, &held.code).map_err(|reason| {
                Fail::Message(format!(
                    "Builds could not verify the quote of open finding {} from run {} because {reason}.",
                    held.id, held.run_id
                ))
            })?;
            if site.file != raised.file {
                return Ok(false);
            }
            earlier = Some(previous);
            recovered = site.source.owned();
            &recovered
        }
    };
    let path = &raised.file;
    let FindingLine::Verified(current) = &raised.line else {
        return Ok(false);
    };
    if original.position.side != current.position.side || original.path != current.path {
        return Ok(false);
    }
    if path == files::COMMIT_MESSAGE {
        return Ok(original.position.line == current.position.line);
    }
    let removal;
    let (from, to) = match current.position.side {
        check::Side::After => (run.head.as_str(), change.head.as_str()),
        check::Side::Before => {
            removal = earlier.map_or_else(previous, Ok)?;
            (removal.before(path), change.before(path))
        }
    };
    Ok(
        files::line_at(repo, from, to, &current.path, original.position.line)?
            == Some(current.position.line),
    )
}

/// One run of a review: the diff its reviewers read and the groups it asks.
struct Ask {
    change: run::NetChange,
    /// The earlier run, when the diff holds the changes since that run's net
    /// change; None for a diff of the whole net change.
    since: Option<i64>,
    /// Whether the run reads the changes since the last review, so the
    /// landing records the run as its last review. A run that asks again the
    /// groups that gave an earlier run no answer reads that run's diff, and a
    /// run that asks the groups that hold open findings over the changes
    /// since an earlier run reads up to the branch's head from there.
    last_review: bool,
    groups: Vec<String>,
    /// Findings whose groups decide their status in this ask. Each group
    /// does so in one ask over the head under review.
    open: Vec<record::Finding>,
    at_head: bool,
    /// Findings that this run closes because their files are absent at the head.
    deleted: Vec<i64>,
    /// What the run asked, for a run of a change that has a review already.
    note: Option<String>,
}

/// The runs a change whose review <landing> holds gets now, so that the
/// review reads <change>, the branch's net change: for each diff a run read,
/// the groups asked over it that gave no answer over it, asked over that diff
/// again; for each group that holds a finding of <open> and has not read
/// every change since the run that raised it with that finding given
/// (`landing::Coverage::read_with`), that group asked over the changes since
/// the last net change it read so; and, when the changes since the last
/// review touch a path a group covers or a commit message, or change
/// anything while a group that holds a finding of <open> has read up to the
/// last review, the groups that cover a changed path, the groups that read
/// the changed messages and those groups that hold findings, asked over
/// those changes (`run::diff_since`). No reviewer reads a diff twice. Fails
/// with why builds review reviews no change twice when the review lacks
/// nothing.
fn later_asks(
    conn: &rusqlite::Connection,
    inputs: &Inputs,
    scope: run::Scope,
    change: &run::NetChange,
    landing: i64,
    open: &[record::Finding],
    deleted: Vec<i64>,
) -> Result<(Vec<Ask>, Vec<record::RunRow>)> {
    let coverage = landing::Coverage::of(conn, &record::landing(conn, landing)?)?;
    let calls = record::calls_of_landing(conn, landing)?;
    let read: Vec<&record::RunRow> = coverage.runs.iter().collect();
    let unanswered = coverage.unanswered(&calls);
    let mut asks = Vec::new();
    for (run, missing) in &unanswered {
        asks.push(Ask {
            change: run::diff_of_run(inputs.worktree, &read, run, |run| {
                Ok((run.base.clone(), run.head.clone()))
            })?,
            since: run.since,
            last_review: false,
            note: Some(format!(
                "asked only the groups that gave run {} no answer, over the diff it read: {}\n",
                run.id,
                missing.join(", ")
            )),
            groups: missing.clone(),
            open: Vec::new(),
            at_head: run.head == change.head,
            deleted: Vec::new(),
        });
    }
    let last = &coverage.last_review;
    // The changes since <from>'s net change, read in place when the
    // branch's net change is.
    let since_run = |from: &record::RunRow| -> Result<run::NetChange> {
        let mut since = run::diff_since(
            inputs.worktree,
            &from.base,
            &from.head,
            &change.base,
            &change.head,
        )?;
        if let (Some(source), Some(read)) = (&mut since.diff.source, &change.diff.source) {
            source.in_place = read.in_place;
        }
        Ok(since)
    };
    // Each group that holds open findings, with the run since which it reads
    // the changes: the earliest net change it read every change up to with
    // each of its findings.
    let mut holders: Vec<(String, &record::RunRow)> = Vec::new();
    let group_findings = scope.material.holding_groups(open)?;
    let (kept, closings) = if group_findings.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            kept_open::of_landing(conn, landing)?,
            record::closings_of_landing(conn, landing)?,
        )
    };
    for (group, findings) in &group_findings {
        let mut from: Option<&record::RunRow> = None;
        for finding in findings {
            let reached = coverage.read_with(&kept, &closings, finding, group)?;
            if from.is_none_or(|from| reached.id < from.id) {
                from = Some(reached);
            }
        }
        if let Some(from) = from {
            holders.push((group.clone(), from));
        }
    }
    let behind: Vec<&(String, &record::RunRow)> = holders
        .iter()
        .filter(|(_, from)| !from.reads_up_to(last))
        .collect();
    let mut froms: Vec<&record::RunRow> = Vec::new();
    for (_, from) in &behind {
        if !froms.iter().any(|f| f.id == from.id) {
            froms.push(from);
        }
    }
    let catch_up_start = asks.len();
    for from in froms {
        let since = since_run(from)?;
        if since.diff.is_empty() {
            continue;
        }
        let groups: Vec<String> = behind
            .iter()
            .filter(|(_, f)| f.id == from.id)
            .map(|(group, _)| group.clone())
            .filter(|group| {
                !asks.iter().any(|ask| {
                    ask.at_head
                        && ask.change.base == since.base
                        && ask.since == Some(from.id)
                        && ask.groups.contains(group)
                })
            })
            .collect();
        if groups.is_empty() {
            continue;
        }
        asks.push(Ask {
            note: Some(format!(
                "asked the groups that hold open findings over the changes since run {} over {}...{}, the last net change they read with those findings: {}\n",
                from.id,
                short(&from.base),
                short(&from.head),
                groups.join(", ")
            )),
            change: since,
            since: Some(from.id),
            last_review: false,
            groups,
            open: Vec::new(),
            at_head: true,
            deleted: Vec::new(),
        });
    }
    let since = since_run(last)?;
    let mut groups = scope.group_names(&since.diff.changed);
    for group in scope.commit_text_groups(&since.diff, &change.diff.changed) {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    if !since.diff.is_empty() {
        for (group, _) in &holders {
            if !groups.contains(group) {
                groups.push(group.clone());
            }
        }
    }
    let needs_review = !groups.is_empty();
    groups.retain(|group| {
        !asks[catch_up_start..]
            .iter()
            .any(|ask| ask.groups.contains(group))
    });
    if groups.is_empty() && needs_review {
        // The catch-up asks together cover every group that needed to read
        // the changes since the last review. They all read the same head.
        asks[catch_up_start..]
            .last_mut()
            .expect("the groups that needed review got catch-up asks")
            .last_review = true;
    }
    let mut latest = Ask {
        note: Some(format!(
            "read only the changes since the last review, run {} over {}...{}\n",
            last.id,
            short(&last.base),
            short(&last.head)
        )),
        change: since,
        since: Some(last.id),
        last_review: true,
        groups,
        open: Vec::new(),
        at_head: true,
        deleted: Vec::new(),
    };
    // A group decides the status of its findings against the head, even
    // when another ask retries an older diff for that group.
    let mut recheck = Vec::new();
    let mut recheck_findings = Vec::new();
    for (group, findings) in group_findings {
        if let Some(ask) = std::iter::once(&mut latest)
            .chain(asks.iter_mut().rev())
            .find(|ask| ask.at_head && ask.groups.contains(&group))
        {
            ask.open.extend(findings.into_iter().cloned());
        } else {
            recheck.push(group);
            recheck_findings.extend(findings.into_iter().cloned());
        }
    }
    if !recheck.is_empty() {
        asks.push(Ask {
            note: Some(format!(
                "asked {} again over the whole net change to check their open findings against the branch head\n",
                recheck.join(", ")
            )),
            change: change.clone(),
            since: None,
            last_review: false,
            groups: recheck,
            open: recheck_findings,
            at_head: true,
            deleted: Vec::new(),
        });
    }
    if !latest.groups.is_empty() {
        latest.deleted = deleted;
        asks.push(latest);
    } else if !deleted.is_empty() {
        if let Some(ask) = asks.iter_mut().rfind(|ask| ask.at_head) {
            ask.deleted = deleted;
        } else {
            latest.note = Some(format!(
                "closed the findings in files the branch deleted, with no reviewer, after run {}\n",
                last.id
            ));
            latest.last_review = false;
            latest.deleted = deleted;
            asks.push(latest);
        }
    }
    if asks.is_empty() {
        fail!(
            "{}\nrun {} of review landing {landing} ({}) is the last review, over {}...{}; every group answered, nothing changed since the last review on a path a group's rules cover or in a commit message a group reads, no confirmed finding is open and no closed finding goes back to its group, so builds review does not review it again.\nbuilds review show {landing} lists the review and its findings.",
            landing::ONE_REVIEW,
            last.id,
            inputs.key,
            short(&last.base),
            short(&last.head)
        );
    }
    Ok((asks, coverage.runs))
}

/// The first 10 characters of commit <rev>, as the review's reports and
/// `builds item times` print it.
pub fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

/// What a review run says of its kind: code, which the reviewers read, or
/// mechanical work that the agent names the check of.
enum ReviewKind {
    Code,
    Mechanical { checked_by: String },
}

/// The kind `--kind` names. With none, builds knows the answer of a pure
/// rename, of a design-only change and of a capture-files-only change, which
/// need no `--kind`, and refuses the rest.
fn kind_of(tool: &Tool, args: &Args) -> Result<Option<ReviewKind>> {
    let checked_by = args.value("checked-by")?;
    match (args.value("kind")?, checked_by) {
        (Some("code"), None) => Ok(Some(ReviewKind::Code)),
        (Some("code"), Some(_)) => fail!("--checked-by goes with --kind mechanical only"),
        (Some("mechanical"), Some(check)) if !check.trim().is_empty() => {
            Ok(Some(ReviewKind::Mechanical {
                checked_by: check.trim().to_string(),
            }))
        }
        (Some("mechanical"), _) => fail!(
            "--kind mechanical needs --checked-by TEXT naming the check that covers the change, such as \"compiler\" or \"shots run\""
        ),
        (Some(other), _) => fail!("--kind is code or mechanical, not {other}"),
        (None, _) => {
            let (worktree, base) = worktree_and_base(args)?;
            let checkout = Checkout::find(worktree)?;
            if known_answer(tool, args, &checkout, base)? {
                return Ok(None);
            }
            fail!(
                "builds review needs --kind code, which runs the reviewers, or --kind mechanical --checked-by TEXT, for a change that only moves or renames code, regenerates files or applies a codemod and that a check covers (REVIEW in builds help workflow). Only a pure rename, a design-only change and a change of capture files only need no --kind."
            )
        }
    }
}

/// Whether the net change of <checkout> past <base> is one whose kind builds
/// knows: only renamed identifiers, design files only, or capture files only.
fn known_answer(tool: &Tool, args: &Args, checkout: &Checkout, base: &str) -> Result<bool> {
    let net = run::diff_between(&checkout.top, base, "HEAD")?;
    if mechanical::only_renames(net.diff.patch.text()) {
        return Ok(true);
    }
    let Some(project) = project_given(tool, args, checkout)? else {
        return Ok(false);
    };
    let store = Store::named(&tool.places, &project)?;
    let globs = Globs::read(&*store.open_existing()?, &checkout.repo())?
        .with_capture_folders(&checkout.top)?;
    if !net.diff.changed.is_empty()
        && net
            .diff
            .changed
            .iter()
            .all(|path| globs.kind_of(path) == Some(crate::lists::design_paths::FileKind::Capture))
    {
        return Ok(true);
    }
    let shas = crate::git::commits_in_range(&checkout.top, &net.base, &net.head)?;
    crate::lists::change_is_design_only(&store, &checkout.top, &checkout.repo(), &shas)
}

/// Records the call of mechanical work that `--kind mechanical` makes, and
/// starts no reviewer.
fn call_mechanical(
    tool: &Tool,
    args: &Args,
    checked_by: &str,
    runner: Option<&Caller>,
) -> Result<()> {
    let (worktree, base) = worktree_and_base(args)?;
    let checkout = Checkout::find(worktree)?;
    let Some(project) = project_given(tool, args, &checkout)? else {
        fail!(
            "repo {} is in no project of the lists, so there is no store to record the call in; say which with --project",
            checkout.repo()
        )
    };
    let store = Store::named(&tool.places, &project)?;
    let key = match args.value("landing")? {
        Some(key) => key.to_string(),
        None => {
            let db = store.open_existing()?;
            landing_key(&db, &checkout.repo(), &checkout.top)?
        }
    };
    let stamp = Stamp::resolve(tool.now, args, &checkout.top)?;
    let by = runner.map_or("the person", |caller| caller.id.as_str());
    print!(
        "{}",
        marked_mechanical::call_mechanical(
            &store,
            &checkout.repo(),
            &checkout.top,
            base,
            &key,
            (by, checked_by),
            &stamp,
        )?
    );
    Ok(())
}

fn run_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "landing",
            "item",
            "rules-root",
            "reviewer",
            "head",
            "kind",
            "checked-by",
        ],
        &[],
    )?;
    let caller = tool.session.caller()?;
    if let Some(ReviewKind::Mechanical { checked_by }) = kind_of(tool, &args)? {
        return call_mechanical(tool, &args, &checked_by, caller.as_ref());
    }
    let (reported, to_settle) = run_review(tool, &args, caller.as_ref())?;
    print!("{}", reported.text);
    if let Some(refusal) = &reported.refusal {
        print!("{refusal}");
        if let Some(landing) = reported.landing {
            println!("builds review show {landing} lists every run of this landing");
        }
    }
    if let Some(Pending {
        store,
        to_settle,
        stamp,
    }) = to_settle
    {
        print!(
            "{}",
            tell_settlers(
                tool,
                &store,
                &stamp,
                &to_settle,
                caller.as_ref(),
                Messaging::for_session(&tool.session, Recipient::Coordinator)?.as_ref(),
            )?
        );
    }
    match reported.refusal {
        Some(_) => Err(Fail::Exit(1)),
        None => Ok(()),
    }
}

/// Reviews the branch that the worktree <worktree> of <repo> has checked out
/// past origin/main, as `builds review <worktree>` does with the rules and
/// reviewer of <tool>, as <runner>, whose session's inbox is <own>; tells
/// the judge, or the coordinator, of the findings nobody has settled; and
/// returns what the review and the telling said. `builds design record` and
/// `amend` run it for a design change whose review has a change to read
/// (`design_review::start`).
pub fn review_branch(
    tool: &Tool,
    store: &Store,
    repo: &str,
    worktree: &Path,
    stamp: &Stamp,
    runner: Option<&Caller>,
    own: Option<&crate::inbox::Messaging>,
) -> Result<String> {
    let db = store.open_existing()?;
    let key = landing_key(&db, repo, worktree)?;
    let reported = review(&Inputs {
        store,
        project: &store.project,
        repo,
        worktree,
        base: "origin/main",
        head: None,
        key: &key,
        root: &tool.reviewer.rules_root,
        reviewer: &tool.reviewer.program,
        stamp,
        brief: None,
        runner,
    })?;
    let mut text = reported.text;
    if let Some(refusal) = &reported.refusal {
        text.push_str(refusal);
    }
    let Some(landing) = reported.landing else {
        return Ok(text);
    };
    text.push_str(&format!(
        "builds review show {landing} lists every run of this landing, and builds review wait {landing} waits until no finding waits for a settle\n"
    ));
    let to_settle = judge::ToSettle {
        project: store.project.clone(),
        landing,
        key,
        worktree: Some(worktree.to_path_buf()),
        findings: unsettled(store, landing)?,
    };
    text.push_str(&tell_settlers(tool, store, stamp, &to_settle, runner, own)?);
    Ok(text)
}

/// The findings of review landing <landing> in <store> that nobody has
/// settled.
fn unsettled(store: &Store, landing: i64) -> Result<Vec<i64>> {
    Ok(
        record::findings_of_landing(&*store.open_existing()?, landing)?
            .into_iter()
            .filter(record::Finding::is_unsettled)
            .map(|finding| finding.id)
            .collect(),
    )
}

/// Tells the judge, or the coordinator, of the findings of <to_settle>, as
/// <caller>, whose session's inbox is <own>, then sends the judge each
/// finding that waits for it and that builds has not told the judge's
/// session of (`tell_judge_waiting`), and returns what both said.
fn tell_settlers(
    tool: &Tool,
    store: &Store,
    stamp: &Stamp,
    to_settle: &judge::ToSettle,
    caller: Option<&Caller>,
    own: Option<&crate::inbox::Messaging>,
) -> Result<String> {
    let sessions = tool.places.sessions();
    let running = judge::running_claude(&sessions)?;
    let mut text = match to_settle.notice() {
        Some(notice) => judge::tell(store, stamp, &sessions, &running, &notice, caller, own)?,
        None => String::new(),
    };
    for line in judge_told::tell_waiting(store, stamp, &sessions, &running)? {
        text.push_str(&format!("{line}\n"));
    }
    Ok(text)
}

/// Sends the project's judge each finding that waits for it and that builds
/// has not told the judge's session of, and prints what it sent. A settle,
/// dispute or answer can put a finding there that no review run told of
/// (judge_told.rs).
fn tell_judge_waiting(
    store: &Store,
    stamp: &Stamp,
    sessions: &crate::inbox::Sessions,
    running: &[crate::claude_sessions::Running],
) -> Result<()> {
    for line in judge_told::tell_waiting(store, stamp, sessions, running)? {
        println!("{line}");
    }
    Ok(())
}

/// `builds review wait LANDING`: waits until every finding of the review
/// landing is settled, then prints how each was settled (judge.rs).
fn wait_command(tool: &Tool, args: &[String]) -> Result<()> {
    let (store, landing) = wait_target(&tool.places, args, &tool.dir)?;
    drop(store.open_existing()?);
    print!("{}", judge::wait(&store, landing, judge::WAIT_LIMIT)?);
    Ok(())
}

pub fn wait_target(
    places: &crate::places::Places,
    args: &[String],
    cwd: &Path,
) -> Result<(Store, i64)> {
    let args = Args::parse(args, &[], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let landing: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    let store = Store::resolve(places, &args, cwd)?;
    Ok((store, landing))
}

/// `builds review copy WORKTREE [BASE]`: the copy review, which asks the
/// copy groups over the worktree's net change past BASE, records the run and
/// prints its findings. Exits 1 when a group raised a finding or gave no
/// answer.
fn copy_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "reviewer"], &[])?;
    let (worktree, base) = worktree_and_base(&args)?;
    let checkout = Checkout::find(worktree)?;
    let Some(project) = project_given(tool, &args, &checkout)? else {
        fail!(
            "repo {} is in no project of the lists, so builds does not know whose rules to read or which store records the review; say which with --project",
            checkout.repo()
        )
    };
    let stamp = Stamp::resolve(tool.now, &args, &checkout.top)?;
    let checked = copy::check(&copy::Inputs {
        store: &Store::named(&tool.places, &project)?,
        project: &project,
        repo: &checkout.repo(),
        worktree: &checkout.top,
        base,
        label: &default_key(&checkout.top)?,
        root: &rules_root(&args)?,
        reviewer: &tool.reviewer.program_for(args.value("reviewer")?),
        stamp: &stamp,
    })?;
    print!("{}", checked.text);
    if checked.clean {
        Ok(())
    } else {
        Err(Fail::Exit(1))
    }
}

/// The worktree and base a review command names: WORKTREE [BASE], with
/// origin/main the base when it names none.
fn worktree_and_base(args: &Args) -> Result<(&str, &str)> {
    match args.positional() {
        [worktree] => Ok((worktree.as_str(), "origin/main")),
        [worktree, base] => Ok((worktree.as_str(), base.as_str())),
        _ => fail!("{USAGE}"),
    }
}

/// The project `--project` names, else the project of <checkout>'s repo;
/// None when the repo is in no project.
fn project_given(tool: &Tool, args: &Args, checkout: &Checkout) -> Result<Option<String>> {
    match args.value("project")? {
        Some(project) => Ok(Some(project.to_string())),
        None => project_of(&tool.places, checkout, None),
    }
}

/// What a review run leaves to tell the judge or the coordinator of: the
/// store, the findings nobody has settled and the stamp of the run.
struct Pending {
    store: Store,
    to_settle: judge::ToSettle,
    stamp: Stamp,
}

/// Runs the review `builds review run` names, as <runner>, and returns what
/// it found, with the findings it left that nobody has settled.
fn run_review(
    tool: &Tool,
    args: &Args,
    runner: Option<&Caller>,
) -> Result<(Reported, Option<Pending>)> {
    let (worktree, base) = worktree_and_base(args)?;
    let checkout = Checkout::find(worktree)?;
    let worktree = checkout.top.clone();
    refuse_uncommitted(&worktree)?;
    let Some(project) = project_given(tool, args, &checkout)? else {
        fail!(
            "repo {} is in no project of the lists, so there is no store to record a review in; say which with --project",
            checkout.repo()
        )
    };
    let store = Store::named(&tool.places, &project)?;
    let key = match args.value("landing")? {
        Some(key) => key.to_string(),
        None => {
            let db = store.open_existing()?;
            landing_key(&db, &checkout.repo(), &worktree)?
        }
    };
    let stamp = Stamp::resolve(tool.now, args, &worktree)?;
    let brief = brief_of(&tool.places, &store, args)?;
    let root = rules_root(args)?;
    let reviewer = tool.reviewer.program_for(args.value("reviewer")?);
    let head = args.value("head")?;
    let reported = review(&Inputs {
        store: &store,
        project: &project,
        repo: &checkout.repo(),
        worktree: &worktree,
        base,
        head,
        key: &key,
        root: &root,
        reviewer: &reviewer,
        stamp: &stamp,
        brief,
        runner,
    })?;
    let Some(landing) = reported.landing else {
        return Ok((reported, None));
    };
    let to_settle = judge::ToSettle {
        project,
        landing,
        key,
        worktree: Some(worktree),
        findings: unsettled(&store, landing)?,
    };
    Ok((
        reported,
        Some(Pending {
            store,
            to_settle,
            stamp,
        }),
    ))
}

/// Refuses a worktree with uncommitted changes, untracked files included,
/// before any reviewer starts: a review reads commits only, so it would raise
/// again the findings that the agent has fixed in files it has not committed.
fn refuse_uncommitted(worktree: &Path) -> Result<()> {
    let paths = match crate::side_git::uncommitted_paths(worktree) {
        Ok(paths) => paths,
        Err(said) => fail!("git status failed in {}: {said}", worktree.display()),
    };
    if paths.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = paths.iter().map(|path| format!("  {path}")).collect();
    fail!(
        "refusing: {} has uncommitted changes, and a review reads commits only, so it would raise again the findings these changes fix. Commit or remove them, then run builds review again:\n{}",
        worktree.display(),
        names.join("\n")
    )
}

/// The key of the change the worktree holds: the branch it is on, else its
/// HEAD.
pub fn default_key(worktree: &Path) -> Result<String> {
    let text = |args: &[&str]| -> Option<String> {
        let output = git(worktree).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    if let Some(branch) = text(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        return Ok(format!("branch:{branch}"));
    }
    match text(&["rev-parse", "--short", "HEAD"]) {
        Some(head) => Ok(format!("detached:{head}")),
        None => fail!("{} has no HEAD to review", worktree.display()),
    }
}

/// The brief of the first `--item` that has one, read from the main checkout
/// of one of the project's repos (`find_brief`): briefs are in git-ignored
/// plans/, so no worktree holds them.
fn brief_of(
    places: &crate::places::Places,
    store: &Store,
    args: &Args,
) -> Result<Option<crate::lists::spec_check::Brief>> {
    let ids = args.values("item");
    if ids.is_empty() {
        return Ok(None);
    }
    let lists = store.load()?;
    for id in ids {
        let id = crate::lists::Id::parse(id)?;
        let Some(item) = lists.items.iter().find(|item| item.id == id) else {
            fail!("--item {}: there is no open item {}", id.0, id.0);
        };
        if let Some(brief) = &item.brief {
            let file = crate::lists::draft::find_brief(places, &lists.repos, brief)?;
            return Ok(Some(Brief::read(brief, &file)?));
        }
    }
    Ok(None)
}

/// `builds review settle FINDING confirmed|dropped --why TEXT`;
/// `builds review settle FINDING --again --why TEXT`, which changes a
/// confirmed finding that no review closed to dropped and keeps its first
/// settle; or `builds review settle FINDING allowed --by-person --why TEXT`,
/// which records that the person allowed an open confirmed finding to land.
/// <caller> runs it: the agent of the finding's branch never settles it,
/// and while a judge listens, only the judge gives a verdict (judge.rs).
fn settle(tool: &Tool, args: &[String], caller: Option<&Caller>) -> Result<()> {
    let args = Args::parse(args, &["why"], &["again", "by-person"])?;
    let again = args.switch("again");
    let by_person = args.switch("by-person");
    let (finding, how) = match (args.positional(), again) {
        ([finding], true) => (finding, "dropped"),
        ([finding, how], false) => (finding, how.as_str()),
        _ => fail!("{USAGE}"),
    };
    match (how == "allowed", by_person) {
        (true, false) => fail!(
            "only the person can allow a finding to land: pass --by-person once they have said to land finding {finding} as it is, with their words in --why, never on your own judgment"
        ),
        (false, true) => fail!("--by-person goes with allowed: {USAGE}"),
        _ => {}
    }
    let id: i64 = finding
        .parse()
        .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
    let why = args.why()?;
    let (store, _) = store_of(tool, &args)?;
    let stamp = Stamp::resolve(tool.now, &args, &tool.dir)?;
    let sessions = tool.places.sessions();
    let running = judge::running_claude(&sessions)?;
    if again {
        store.write(&stamp, &format!("review settle {id} again"), |db| {
            judge::may_judge(db, &sessions, &running, id, caller)?;
            record::settle_again(db, id, why, stamp.at, &stamp.by)
        })?;
        judge::tell_waits(&store.dir)?;
        println!(
            "You settled finding {id} again, as dropped; the finding keeps its first settle beside the second."
        );
        return tell_judge_waiting(&store, &stamp, &sessions, &running);
    }
    if by_person {
        store.write(&stamp, &format!("review allow {id}"), |db| {
            judge::may_record(db, id, caller)?;
            record::allow(db, id, why, stamp.at, &stamp.by)
        })?;
        judge::tell_waits(&store.dir)?;
        println!(
            "finding {id} stays confirmed, and the person allowed it to land: builds land no longer waits on it"
        );
        return tell_judge_waiting(&store, &stamp, &sessions, &running);
    }
    let held = store.write(&stamp, &format!("review settle {id}"), |db| {
        judge::may_judge(db, &sessions, &running, id, caller)?;
        record::settle(db, id, how, why, stamp.at, &stamp.by)?;
        let Some(landing) = record::finding_of(db, id)?.landing_id else {
            return Ok(None);
        };
        match audit_item::Ready::of(db, landing)? {
            Some(ready) => audit_item::hold(db, &ready, &stamp).map(Some),
            None => Ok(None),
        }
    })?;
    judge::tell_waits(&store.dir)?;
    println!("finding {id} settled {how}");
    if let Some(held) = held {
        println!("{}", held_words(&held));
    }
    tell_judge_waiting(&store, &stamp, &sessions, &running)
}

/// What the settle of an audit's last finding made, in words.
fn held_words(held: &audit_item::Held) -> String {
    match held {
        audit_item::Held::NothingToFix => {
            "the audit has no confirmed finding left to fix, so its review ended".to_string()
        }
        audit_item::Held::Joined { item } => format!(
            "the audit has no finding left to settle: its confirmed findings joined item {item}, which holds the fixes of the audit's other reviews"
        ),
        audit_item::Held::Item { item } => format!(
            "the audit has no finding left to settle: its confirmed findings became item {item}, which waits in the backlog until the person starts it"
        ),
    }
}

/// The finding a command names: its one positional argument.
fn finding_named(args: &Args) -> Result<i64> {
    let [finding] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    finding
        .parse()
        .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))
}

/// `builds review dispute FINDING --why TEXT`, run by <caller>, whose
/// session's inbox is <own>: records that <caller> thinks the judge
/// confirmed the finding wrongly, and tells the judge, or the coordinator,
/// once (judge.rs).
fn dispute_command(
    tool: &Tool,
    args: &[String],
    caller: Option<&Caller>,
    own: Option<&crate::inbox::Messaging>,
) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let id = finding_named(&args)?;
    let why = args.why()?;
    let (store, conn) = store_of(tool, &args)?;
    let stamp = Stamp::resolve(tool.now, &args, &tool.dir)?;
    let sessions = tool.places.sessions();
    let running = judge::running_claude(&sessions)?;
    let finding = store.write(&stamp, &format!("review dispute {id}"), |db| {
        judge::dispute(db, id, why, &stamp)
    })?;
    print!(
        "{}",
        judge::tell(
            &store,
            &stamp,
            &sessions,
            &running,
            &judge::dispute_notice(&conn, &store.project, &finding)?,
            caller,
            own,
        )?
    );
    tell_judge_waiting(&store, &stamp, &sessions, &running)
}

/// `builds review keep FINDING --why TEXT`, run by <caller>: the judge's
/// answer to a dispute that keeps the finding confirmed. With `--again`, the
/// judge corrects the reason it gave when it kept the finding. Only whoever
/// may give a verdict on the finding answers (judge.rs).
fn keep_command(tool: &Tool, args: &[String], caller: Option<&Caller>) -> Result<()> {
    let args = Args::parse(args, &["why"], &["again"])?;
    let again = args.switch("again");
    let id = finding_named(&args)?;
    let why = args.why()?;
    let (store, _) = store_of(tool, &args)?;
    let stamp = Stamp::resolve(tool.now, &args, &tool.dir)?;
    let sessions = tool.places.sessions();
    let running = judge::running_claude(&sessions)?;
    store.write(&stamp, &format!("review keep {id}"), |db| {
        judge::may_judge(db, &sessions, &running, id, caller)?;
        if again {
            judge::correct_keep(db, id, why, &stamp)
        } else {
            judge::keep(db, id, why, &stamp)
        }
    })?;
    judge::tell_waits(&store.dir)?;
    if again {
        println!("finding {id} stays confirmed: you corrected the reason you kept it");
    } else {
        println!("finding {id} stays confirmed: you kept it after its dispute");
    }
    tell_judge_waiting(&store, &stamp, &sessions, &running)
}

/// `builds review show LANDING`.
fn show(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    let (_, conn) = store_of(tool, &args)?;
    print!("{}", show_text(&conn, id)?);
    Ok(())
}

/// What `builds review show` prints for landing <id>.
fn show_text(conn: &Connection, id: i64) -> Result<String> {
    let landing = record::landing(conn, id)?;
    let kept = kept_open::of_landing(conn, id)?;
    let closings = record::closings_of_landing(conn, id)?;
    let mut text = format!(
        "landing {id}: {} ({}), last review: {}, {}\n",
        landing.key,
        landing.repo,
        match landing.reviewed_run {
            Some(run) => format!("run {run}"),
            None => "none".to_string(),
        },
        match (landing.landed_run(), record::abandoned_why(conn, id)?) {
            (Some(run), _) => format!("landed with run {run}"),
            (None, Some(why)) => format!("abandoned: {why}"),
            (None, None) => "not landed".to_string(),
        }
    );
    for record::RunRow {
        id: run,
        kind,
        base,
        head,
        since,
        ..
    } in record::runs_of_landing(conn, id)?
    {
        match since {
            Some(since) => text.push_str(&format!(
                "{kind} {run} over {}...{}, read as the changes since the last review, run {since}\n",
                short(&base),
                short(&head)
            )),
            None => text.push_str(&format!(
                "{kind} {run} over {}...{}\n",
                short(&base),
                short(&head)
            )),
        }
        text.push_str(&render::run_report(
            &record::calls_of_run(conn, run)?,
            &record::findings_of_run(conn, run)?,
        ));
        for closing in closings.iter().filter(|closing| closing.run == run) {
            text.push_str(&render::closed_text(run, closing.finding));
        }
        for kept in kept.iter().filter(|kept| kept.run == run) {
            let closed_before = closings
                .iter()
                .filter(|closing| closing.finding == kept.finding && closing.run < run)
                .map(|closing| closing.run)
                .max();
            text.push_str(&match closed_before {
                Some(closing) => {
                    render::reopened_text(&kept.group, kept.finding, closing, &kept.reason)
                }
                None => kept_open::text(&kept.group, kept.finding, &kept.reason),
            });
        }
    }
    Ok(text)
}

/// `builds review replay`: runs a grouping over every case of the checkout's
/// `review/cases` and prints the recall, the false blocking findings and the
/// tokens. Each replayed case is recorded in its project's store. With
/// `--landing`, it replays the diffs of recorded reviews instead (`rerun.rs`).
fn replay(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "case",
            "rules-root",
            "reviewer",
            "source",
            "model",
            "landing",
            "repo-dir",
            "group",
        ],
        &["compare"],
    )?;
    args.exactly(0, USAGE)?;
    if !args.values("landing").is_empty() {
        if args.switch("compare") {
            fail!("--compare goes with the cases, not --landing\n{USAGE}");
        }
        return replay_landings(tool, &args);
    }
    if args.value("repo-dir")?.is_some() || !args.values("group").is_empty() {
        fail!("--repo-dir and --group go with --landing\n{USAGE}");
    }
    let root = rules_root(&args)?;
    let reviewer = tool.reviewer.program_for(args.value("reviewer")?);
    let source = args.value("source")?.map(PathBuf::from);
    let mut all = cases::load(&root)?;
    let wanted = args.values("case");
    if !wanted.is_empty() {
        for name in &wanted {
            if !all.iter().any(|c| c.name == *name) {
                fail!("there is no case {name}");
            }
        }
        all.retain(|c| wanted.contains(&c.name.as_str()));
    }
    if all.is_empty() {
        fail!("{} holds no cases", root.join(cases::DIR).display());
    }
    // replay records each case's run in its project's store, so the command
    // checks that every store exists before any reviewer runs. The store
    // holds the design files of the case's repo.
    let mut stores = Vec::new();
    let mut designs = Vec::new();
    for case in &all {
        let store = Store::named(&tool.places, &case.project)?;
        designs.push(Globs::read(&*store.open_existing()?, &case.repo)?);
        stores.push(store);
    }
    let stamp = Stamp::resolve(tool.now, &args, &tool.dir)?;
    // Runs the cases under the rules and groups of <root>, every group in
    // <model> when given, and records each case's run.
    let replay_under = |root: &Path, model: Option<&str>| -> Result<Vec<cases::CaseResult>> {
        let rules_version = run::rules_version(root);
        let results = cases::replay_all(root, &all, &designs, &reviewer, source.as_deref(), model)?;
        for ((case, result), store) in all.iter().zip(&results).zip(&stores) {
            store.write(&stamp, &format!("review replay {}", case.name), |db| {
                record::add_run(
                    db,
                    &store.texts,
                    &cases::run_of(case, stamp.at, &rules_version),
                    &result.calls,
                )
                .map(|_| ())
            })?;
        }
        Ok(results)
    };
    let results = replay_under(&root, args.value("model")?)?;
    let mut failed = cases::totals(&results).failed_calls > 0;
    if args.switch("compare") {
        let current = replay_under(&default_rules_root()?, None)?;
        failed |= cases::totals(&current).failed_calls > 0;
        let name = match args.value("model")? {
            Some(model) => format!("{model}, rules of {}", root.display()),
            None => format!("rules of {}", root.display()),
        };
        print!("{}", cases::comparison(&current, &results, &name));
    } else {
        print!("{}", cases::report(&results));
    }
    if failed {
        return Err(Fail::Exit(1));
    }
    Ok(())
}

/// `builds review replay --landing N...`: replays the diffs the reviews of
/// those landings read and scores the replay against their settled findings.
fn replay_landings(tool: &Tool, args: &Args) -> Result<()> {
    if !args.values("case").is_empty() || args.value("source")?.is_some() {
        fail!(
            "--landing replays recorded reviews, and --case and --source replay cases: give one or the other"
        );
    }
    let landings: Vec<i64> = args
        .values("landing")
        .into_iter()
        .map(|landing| {
            landing
                .parse()
                .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))
        })
        .collect::<Result<_>>()?;
    let store = Store::resolve(&tool.places, args, &tool.dir)?;
    // Fails when the project has no store, before anything asks who writes.
    drop(store.open_existing()?);
    let root = rules_root(args)?;
    let reviewer = tool.reviewer.program_for(args.value("reviewer")?);
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let stamp = Stamp::resolve(tool.now, args, &tool.dir)?;
    let replayed = rerun::replay(&rerun::Inputs {
        store: &store,
        project: &store.project,
        repo_dir: &repo_dir,
        landings: &landings,
        root: &root,
        reviewer: &reviewer,
        model: args.value("model")?,
        groups: &args.values("group"),
        stamp: &stamp,
    })?;
    print!("{}", replayed.text);
    if replayed.failed {
        return Err(Fail::Exit(1));
    }
    Ok(())
}

/// The store of a command that names no worktree, with its database open
/// for reading: `--project`, else the project of the current directory's
/// repo. Fails when that project has no store, so that no review command
/// answers from an empty one.
fn store_of(tool: &Tool, args: &Args) -> Result<(Store, crate::db::Held)> {
    let store = Store::resolve(&tool.places, args, &tool.dir)?;
    let conn = store.open_existing()?;
    Ok((store, conn))
}

/// `rule` mapped to the name of its group in the checkout's group folders.
pub(super) fn group_names(root: &Path) -> Result<impl Fn(&str) -> Option<String>> {
    let groups = groups::Groups::load(root)?;
    Ok(move |rule: &str| {
        groups
            .groups
            .iter()
            .find(|g| g.rules.iter().any(|r| r == rule))
            .map(|g| g.name.clone())
    })
}

/// `builds review audit LANDING --repo-dir DIR`: audits a landing that landed.
fn audit_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["repo-dir", "rules-root", "reviewer", "seed"], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let landing: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    // audit::audit opens the store, and fails when the project has none.
    let store = Store::resolve(&tool.places, &args, &tool.dir)?;
    let root = rules_root(&args)?;
    let reviewer = tool.reviewer.program_for(args.value("reviewer")?);
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let stamp = Stamp {
        at: tool.now,
        by: args.value("by")?.unwrap_or("builds audit").to_string(),
    };
    let seed = match args.value("seed")? {
        Some(seed) => seed
            .parse()
            .map_err(|_| Fail::Message(format!("'{seed}' is not a number")))?,
        None => clock_bits(),
    };
    let text = audit::audit(&audit::Inputs {
        store: &store,
        project: &store.project,
        repo_dir: &repo_dir,
        landing,
        root: &root,
        reviewer: &reviewer,
        stamp: &stamp,
        seed,
    })?;
    print!("{text}");
    Ok(())
}

/// Bits that differ from one run to the next.
pub(super) fn clock_bits() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    (now as u64) ^ (u64::from(std::process::id()) << 32)
}

/// `builds review misses`: the candidate misses nobody has settled. With
/// `--check`, the model check settles each one (`audit::check_open_misses`).
fn misses(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "model", "reviewer"], &["check"])?;
    args.exactly(0, USAGE)?;
    if args.switch("check") {
        return audit::check_misses_command(tool, &args);
    }
    let (_, conn) = store_of(tool, &args)?;
    let open = audit::open_misses(&conn)?;
    for (landing, finding) in &open {
        println!("landing {landing}:");
        print!("{}", render::finding_text(finding));
    }
    println!(
        "builds found {} that nobody has settled",
        crate::text::counted(open.len(), "candidate miss", "candidate misses")
    );
    Ok(())
}

/// `builds review rates`: each group's audited miss rate.
fn rates(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root"], &[])?;
    args.exactly(0, USAGE)?;
    let (_, conn) = store_of(tool, &args)?;
    let root = rules_root(&args)?;
    for rate in audit::rates(&conn, &group_names(&root)?)? {
        println!(
            "{}: {} audits, {} confirmed misses; the latest {}: {}{}",
            rate.group,
            rate.audits,
            rate.misses,
            rate.recent_audits,
            rate.recent_misses,
            if rate.risen() { " (risen)" } else { "" }
        );
    }
    Ok(())
}

/// `builds review scores`: precision and recall per group and rule, for each
/// setup, from the settled findings and audits in the store.
fn scores(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let (store, conn) = store_of(tool, &args)?;
    print!(
        "{}",
        score::store_report(&store.project, &score::of_store(&conn)?)
    );
    Ok(())
}

/// `builds review case add`: the case a confirmed finding or a confirmed miss
/// makes, written into the checkout named by --rules-root (never a default:
/// the main checkout is the person's).
fn case_command(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((sub, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match sub.as_str() {
        "add" => {}
        "propose" => return propose::propose_command(tool, rest),
        _ => fail!("unknown review case command {sub}\n{USAGE}"),
    }
    let args = Args::parse(
        rest,
        &[
            "repo-dir",
            "rules-root",
            "name",
            "rule",
            "file",
            "commits",
            "clean-for",
        ],
        &[],
    )?;
    let Some(root) = args.value("rules-root")? else {
        fail!("name the agent-instructions worktree to write the case in with --rules-root");
    };
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let repo = Checkout::find(args.require("repo-dir")?)?.repo();
    let patch_of = |base: &str, head: &str| cases::patch_of(&repo_dir, base, head);
    let (project, patch, addition, default_name) = match args.positional() {
        [landing, finding] => {
            let (store, conn) = store_of(tool, &args)?;
            let landing: i64 = landing
                .parse()
                .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
            let finding: i64 = finding
                .parse()
                .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
            let found = record::finding_of(&conn, finding)?;
            if found.landing_id != Some(landing) {
                fail!("finding {finding} is not in landing {landing}");
            }
            if !found.settled_as("confirmed") {
                fail!(
                    "finding {finding} is not settled as confirmed; only a confirmed finding becomes a case"
                );
            }
            let run = record::runs_of_landing(&conn, landing)?
                .into_iter()
                .find(|r| r.id == found.run_id)
                .expect("a finding's run belongs to its landing");
            (
                store.project.clone(),
                patch_of(&run.base, &run.head)?,
                (found.rule, Some(found.file)),
                format!("{}-{}", short(&run.head), found.group.replace(' ', "-")),
            )
        }
        [] => {
            // builds review replay records this case's run in the project's
            // store, so the project must have a store.
            let store = Store::named(&tool.places, args.require("project")?)?;
            store.open_existing()?;
            let commits = args.require("commits")?;
            let Some((base, head)) = commits.split_once("..") else {
                fail!("--commits is BASE..HEAD");
            };
            let (rule, file, word) = match args.value("clean-for")? {
                Some(rule) => (rule.to_string(), None, "clean"),
                None => (
                    args.require("rule")?.to_string(),
                    Some(args.require("file")?.to_string()),
                    "miss",
                ),
            };
            (
                store.project,
                patch_of(base, head)?,
                (rule, file),
                format!("{}-{word}", short(head)),
            )
        }
        _ => fail!("{USAGE}"),
    };
    let name = args
        .value("name")?
        .map(str::to_string)
        .unwrap_or(default_name);
    let (rule, file) = addition;
    let addition = match &file {
        Some(file) => cases::Addition::Finding { rule: &rule, file },
        None => cases::Addition::Clean { rule: &rule },
    };
    let dir = cases::add(Path::new(root), &name, &project, &repo, &patch, &[addition])?;
    println!("case {name} written in {}; commit it", dir.display());
    Ok(())
}

/// Prints each group, and fails when the grouping is not whole. With
/// `--changed`, prints only the groups a change to those paths in repo
/// `--repo` of project `--project` runs, reading the repo's design files from
/// the project's store.
fn list_groups(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "changed", "repo"], &[])?;
    args.exactly(0, USAGE)?;
    let root = rules_root(&args)?;
    let groups = groups::Groups::load(&root)?;
    let every = rules::load(&root, None)?;
    groups::require_whole(&groups, &every)?;
    let changed: Vec<String> = args
        .values("changed")
        .into_iter()
        .map(str::to_string)
        .collect();
    if changed.is_empty() {
        for group in &groups.groups {
            print_group(group, group.rules.iter().map(String::as_str));
        }
        return Ok(());
    }
    let project = args.require("project")?;
    let design = Globs::read(
        &*Store::named(&tool.places, project)?.open_existing()?,
        args.require("repo")?,
    )?;
    let project_rules = rules::load(&root, Some(project))?;
    for (group, members) in groups.pick(&project_rules, &changed, &design) {
        print_group(group, members.iter().map(|rule| rule.slug.as_str()));
    }
    Ok(())
}

fn print_group<'a>(group: &groups::Group, slugs: impl Iterator<Item = &'a str>) {
    let kind = if group.copy { ", copy review only" } else { "" };
    println!("{} ({}{kind}): {}", group.name, group.model, group.question);
    for slug in slugs {
        println!("  {slug}");
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
