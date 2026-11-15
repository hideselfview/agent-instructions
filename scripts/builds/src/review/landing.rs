//! What `builds land` asks of the review, before it fetches anything and after
//! it pushes:
//!
//! ```text
//! builds review check-landing DIR --project P SHA... [--rules-root DIR]
//! builds review landed DIR --project P SHA... [--rules-root DIR] [--roll N] [--reviewer PROGRAM]
//! ```
//!
//! DIR is the landing worktree and the SHAs are the commits `builds land` was
//! given. A landing whose changed paths no group's rules cover needs no
//! review. Any other landing needs a review run that read exactly its changes:
//! the same patch ids in the same order (`commits.rs`), so a rebase or a
//! cherry-pick that keeps each change keeps the review. The newest such run
//! names the change's landing under review, and that review must be finished:
//! every group that covers a changed path answered over these commits, every
//! finding of every review run of the landing is settled, and every confirmed
//! one names a fix whose patch id is among the commits landed.
//!
//! `landed` records the landing as landed with the run that covered it, then
//! decides whether to audit it (`audit.rs`) and, when it is sampled, starts
//! `builds review audit` in the background. A landing whose change landed
//! already is matched by its landed run, so both commands can run again.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::checkout::Checkout;
use crate::db;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::store::Store;
use crate::review::commits::{self, Commit, same_change};
use crate::review::record::{self, LandingRow};
use crate::review::render;
use crate::review::run::Material;
use crate::tool::Tool;

const USAGE: &str = "builds review check-landing|landed DIR --project P SHA... [--rules-root DIR]";

/// The run that covers a landing's commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covered {
    pub landing: LandingRow,
    pub run: i64,
}

/// What a covering review still lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// Groups that cover a changed path and have no answer over the commits.
    pub unanswered: Vec<String>,
    /// Findings nobody settled.
    pub unsettled: Vec<record::Finding>,
    /// Confirmed findings that name no fix.
    pub unfixed: Vec<i64>,
    /// Confirmed findings whose fix is not among the commits, with the fix.
    pub fixed_elsewhere: Vec<(i64, String)>,
}

impl Unfinished {
    fn is_empty(&self) -> bool {
        self.unanswered.is_empty()
            && self.unsettled.is_empty()
            && self.unfixed.is_empty()
            && self.fixed_elsewhere.is_empty()
    }
}

/// What the review says of a landing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No group's rules cover a path the commits change.
    NotNeeded,
    /// A finished review covers the commits.
    Takes(Covered),
    /// No review run read these commits.
    NoRun,
    /// A run read these commits and the review is not finished.
    Unfinished(Covered, Unfinished),
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn shas(commits: &[Commit]) -> String {
    commits
        .iter()
        .map(|c| short(&c.sha))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The review runs of <landing> that read the same changes as <commits>,
/// newest first.
fn runs_over(db: &Connection, landing: i64, commits: &[Commit]) -> Result<Vec<i64>> {
    let mut found = Vec::new();
    for run in record::runs_of_landing(db, landing)?.into_iter().rev() {
        if run.kind == "review" && same_change(&record::commits_of_run(db, run.id)?, commits) {
            found.push(run.id);
        }
    }
    Ok(found)
}

/// The newest run that read the same changes as <commits> in <repo>: a run
/// of a landing still open, else the run a landed change landed with.
fn matching(db: &Connection, repo: &str, commits: &[Commit]) -> Result<Option<Covered>> {
    for landing in record::landings_of_repo(db, repo, false)? {
        if let Some(&run) = runs_over(db, landing.id, commits)?.first() {
            return Ok(Some(Covered { landing, run }));
        }
    }
    for landing in record::landings_of_repo(db, repo, true)? {
        let run = landing.landed_run.expect("a landed landing names its run");
        if same_change(&record::commits_of_run(db, run)?, commits) {
            return Ok(Some(Covered { landing, run }));
        }
    }
    Ok(None)
}

/// The groups of <landing> that answered, with no failure, in one of <runs>.
fn answered(db: &Connection, landing: i64, runs: &[i64]) -> Result<Vec<String>> {
    Ok(record::calls_of_landing(db, landing)?
        .into_iter()
        .filter(|c| runs.contains(&c.run_id) && c.failure.is_none())
        .map(|c| c.group)
        .collect())
}

/// The groups among <needed> that already answered over <commits> in
/// <landing>: a review of those commits need not ask them again.
pub fn groups_answered(
    db: &Connection,
    landing: i64,
    commits: &[Commit],
    needed: &[String],
) -> Result<Vec<String>> {
    let done = answered(db, landing, &runs_over(db, landing, commits)?)?;
    Ok(needed
        .iter()
        .filter(|g| done.contains(g))
        .cloned()
        .collect())
}

/// Whether the review lets <commits> of <repo> land, when <needed> names the
/// groups whose rules cover a path they change. <db> is None when the store
/// has no database yet.
pub fn judge(
    db: Option<&Connection>,
    repo: &str,
    commits: &[Commit],
    needed: &[String],
) -> Result<Verdict> {
    if needed.is_empty() {
        return Ok(Verdict::NotNeeded);
    }
    let Some(db) = db else {
        return Ok(Verdict::NoRun);
    };
    let Some(covered) = matching(db, repo, commits)? else {
        return Ok(Verdict::NoRun);
    };
    if covered.landing.landed_run.is_some() {
        return Ok(Verdict::Takes(covered));
    }
    let landing = covered.landing.id;
    let done = answered(db, landing, &runs_over(db, landing, commits)?)?;
    let findings: Vec<record::Finding> = record::findings_of_landing(db, landing)?
        .into_iter()
        .filter(|f| f.kind == "review" && f.counts())
        .collect();
    let patches: Vec<&str> = commits.iter().map(|c| c.patch_id.as_str()).collect();
    let confirmed = || findings.iter().filter(|f| f.settled_as("confirmed"));
    let unfinished = Unfinished {
        unanswered: needed
            .iter()
            .filter(|g| !done.contains(g))
            .cloned()
            .collect(),
        unsettled: findings
            .iter()
            .filter(|f| f.settled.is_none())
            .cloned()
            .collect(),
        unfixed: confirmed()
            .filter(|f| f.fix.is_none())
            .map(|f| f.id)
            .collect(),
        fixed_elsewhere: confirmed()
            .filter_map(|f| {
                let fix = f.fix.as_ref()?;
                (!patches.contains(&fix.patch_id.as_str())).then(|| (f.id, fix.sha.clone()))
            })
            .collect(),
    };
    if unfinished.is_empty() {
        return Ok(Verdict::Takes(covered));
    }
    Ok(Verdict::Unfinished(covered, unfinished))
}

/// Why <verdict> does not let <commits> land, in words; None when it does.
pub fn refusal(
    db: Option<&Connection>,
    repo: &str,
    commits: &[Commit],
    verdict: &Verdict,
) -> Result<Option<String>> {
    let (covered, unfinished) = match verdict {
        Verdict::NotNeeded | Verdict::Takes(_) => return Ok(None),
        Verdict::NoRun => return Ok(Some(no_run(db, repo, commits)?)),
        Verdict::Unfinished(covered, unfinished) => (covered, unfinished),
    };
    let mut text = format!(
        "review landing {} ({}), run {}, covers these commits ({}), and is not finished:\n",
        covered.landing.id,
        covered.landing.key,
        covered.run,
        shas(commits)
    );
    for group in &unfinished.unanswered {
        text.push_str(&format!(
            "group {group} covers a changed path and has no answer over these commits\n"
        ));
    }
    if !unfinished.unanswered.is_empty() {
        text.push_str("Run builds review again on these commits.\n");
    }
    if !unfinished.unsettled.is_empty() {
        let count = unfinished.unsettled.len();
        text.push_str(&format!(
            "{} not settled:\n",
            if count == 1 {
                "1 finding is".to_string()
            } else {
                format!("{count} findings are")
            }
        ));
        for finding in &unfinished.unsettled {
            text.push_str(&render::finding_text(finding));
        }
        text.push_str(
            "Judge each and settle it: builds review settle <finding> confirmed|dropped --why TEXT\n",
        );
    }
    for id in &unfinished.unfixed {
        text.push_str(&format!(
            "finding {id} is confirmed and names no fix: fix it in a new commit, name it with builds review fixed {id} <sha>, and review the commits again\n"
        ));
    }
    for (id, sha) in &unfinished.fixed_elsewhere {
        text.push_str(&format!(
            "finding {id} is fixed by {}, which is not among the commits this lands\n",
            short(sha)
        ));
    }
    Ok(Some(text))
}

/// The refusal when no run read the same changes as <commits>, naming the
/// newest run that read some of them and how its commits differ.
fn no_run(db: Option<&Connection>, repo: &str, commits: &[Commit]) -> Result<String> {
    let mut text = format!(
        "no review run covers the commits this lands ({}).\n",
        shas(commits)
    );
    if let Some(db) = db {
        let patches =
            |list: &[Commit]| -> Vec<String> { list.iter().map(|c| c.patch_id.clone()).collect() };
        let wanted = patches(commits);
        'landings: for landing in record::landings_of_repo(db, repo, false)? {
            for run in record::runs_of_landing(db, landing.id)?.into_iter().rev() {
                if run.kind != "review" {
                    continue;
                }
                let read = record::commits_of_run(db, run.id)?;
                let had = patches(&read);
                if !had.iter().any(|p| wanted.contains(p)) {
                    continue;
                }
                let lacks: Vec<Commit> = commits
                    .iter()
                    .filter(|c| !had.contains(&c.patch_id))
                    .cloned()
                    .collect();
                let extra: Vec<Commit> = read
                    .iter()
                    .filter(|c| !wanted.contains(&c.patch_id))
                    .cloned()
                    .collect();
                text.push_str(&format!(
                    "The newest review that read some of them, run {} of landing {} ({}), read {}",
                    run.id,
                    landing.id,
                    landing.key,
                    shas(&read)
                ));
                match (lacks.is_empty(), extra.is_empty()) {
                    (true, true) => text.push_str(", the same changes in another order.\n"),
                    _ => {
                        text.push(':');
                        if !lacks.is_empty() {
                            text.push_str(&format!(" it lacks {}", shas(&lacks)));
                        }
                        if !extra.is_empty() {
                            text.push_str(&format!(
                                "{} it also read {}, which this does not land",
                                if lacks.is_empty() { "" } else { ";" },
                                shas(&extra)
                            ));
                        }
                        text.push_str(".\n");
                    }
                }
                break 'landings;
            }
        }
    }
    text.push_str(
        "Review exactly these commits (builds review <worktree> [<base>] in the branch's worktree), settle every finding, and land again.\n",
    );
    Ok(text)
}

/// What both commands read: the repo, the store, the commits and the groups
/// they need.
struct Asked {
    store: Store,
    repo: String,
    dir: PathBuf,
    commits: Vec<Commit>,
    needed: Vec<String>,
    root: PathBuf,
}

fn ask(tool: &Tool, args: &Args) -> Result<Asked> {
    let Some((dir, shas)) = args.positional().split_first() else {
        fail!("usage: {USAGE}")
    };
    if shas.is_empty() {
        fail!("name the commits: {USAGE}");
    }
    let checkout = Checkout::find(dir)?;
    let project = args.require("project")?;
    let store = Store::named(&tool.places, project)?;
    let root = super::rules_root(args)?;
    let commits = commits::with_patch_ids(&checkout.top, shas)?;
    let changed = commits::paths(&checkout.top, shas)?;
    let material = Material::load(&root, project)?;
    let needed = material
        .groups
        .pick(&material.rules, &changed)
        .into_iter()
        .map(|(group, _)| group.name.clone())
        .collect();
    Ok(Asked {
        store,
        repo: checkout.repo(),
        dir: checkout.top.clone(),
        commits,
        needed,
        root,
    })
}

/// `builds review check-landing DIR --project P SHA...`: says why the commits
/// can't land, exit 1, or that the review takes them.
pub fn check_landing_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root"], &[])?;
    let asked = ask(tool, &args)?;
    let conn = db::open_reader(&asked.store.database())?;
    let verdict = judge(conn.as_ref(), &asked.repo, &asked.commits, &asked.needed)?;
    match &verdict {
        Verdict::NotNeeded => {
            println!("no group's rules cover the changed paths: the landing needs no review");
            Ok(())
        }
        Verdict::Takes(covered) => {
            println!(
                "review landing {} ({}), run {}, covers these commits and is finished",
                covered.landing.id, covered.landing.key, covered.run
            );
            Ok(())
        }
        Verdict::NoRun | Verdict::Unfinished(..) => {
            let why = refusal(conn.as_ref(), &asked.repo, &asked.commits, &verdict)?
                .expect("an unfinished review says why");
            print!("{why}");
            Err(Fail::Exit(1))
        }
    }
}

/// Records the change of <asked> as landed; returns the landing when this
/// recorded it, None when it needed no review or was recorded already.
fn record_landed(asked: &Asked, stamp: &Stamp) -> Result<Option<i64>> {
    let conn = db::open_reader(&asked.store.database())?;
    let verdict = judge(conn.as_ref(), &asked.repo, &asked.commits, &asked.needed)?;
    let covered = match verdict {
        Verdict::NotNeeded => return Ok(None),
        Verdict::Takes(covered) => covered,
        Verdict::NoRun | Verdict::Unfinished(..) => {
            let why = refusal(conn.as_ref(), &asked.repo, &asked.commits, &verdict)?
                .expect("an unfinished review says why");
            fail!("the review does not take these commits:\n{why}")
        }
    };
    drop(conn);
    if covered.landing.landed_run.is_some() {
        return Ok(None);
    }
    let landing = covered.landing.id;
    asked
        .store
        .write(stamp, &format!("review landed {landing}"), |db| {
            record::record_landed(db, landing, covered.run, stamp.at)
        })?;
    Ok(Some(landing))
}

/// `builds review landed DIR --project P SHA...`: records the landing, then
/// starts the audit of a sampled one. The audit never fails the landing.
pub fn landed_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "roll", "reviewer"], &[])?;
    let asked = ask(tool, &args)?;
    let stamp = Stamp {
        at: tool.now,
        by: args.value("by")?.unwrap_or("builds land").to_string(),
    };
    let Some(landing) = record_landed(&asked, &stamp)? else {
        return Ok(());
    };
    let roll = match args.value("roll")? {
        Some(roll) => roll
            .parse::<f64>()
            .map_err(|_| Fail::Message(format!("'{roll}' is not a number")))?,
        None => (super::clock_bits() % 1_000_000) as f64 / 1_000_000.0,
    };
    let reviewer = args.value("reviewer")?.unwrap_or("claude");
    if let Err(why) = start_audit(&asked, landing, roll, reviewer) {
        eprintln!(
            "builds: the audit of review landing {landing} was not started: {}",
            why.text()
        );
    }
    Ok(())
}

/// Starts `builds review audit` on <landing> in the background when the
/// sample picks it.
fn start_audit(asked: &Asked, landing: i64, roll: f64, reviewer: &str) -> Result<()> {
    let Some(conn) = db::open_reader(&asked.store.database())? else {
        return Ok(());
    };
    if !super::audit::sampled(&conn, landing, &super::group_names(&asked.root)?, roll)? {
        return Ok(());
    }
    let program = std::env::current_exe().context(|| "finding this program".to_string())?;
    std::process::Command::new(program)
        .args(["review", "audit", &landing.to_string()])
        .args(["--project", &asked.store.project])
        .arg("--repo-dir")
        .arg(&asked.dir)
        .arg("--rules-root")
        .arg(&asked.root)
        .args(["--reviewer", reviewer])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(|| "starting the audit".to_string())?;
    Ok(())
}

/// The commit <sha> of the repo at <dir>, with its patch id; a commit that
/// changes nothing can fix nothing.
pub fn fix_commit(dir: &Path, sha: &str) -> Result<Commit> {
    match commits::with_patch_ids(dir, &[sha.to_string()])?.pop() {
        Some(commit) => Ok(commit),
        None => fail!("commit {sha} changes nothing, so it fixes nothing"),
    }
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;
