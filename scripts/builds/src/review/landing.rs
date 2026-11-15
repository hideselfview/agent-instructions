//! What `builds land` asks of the review, before it fetches anything and after
//! it pushes:
//!
//! ```text
//! builds review check-landing DIR --project P SHA... [--rules-root DIR]
//! builds review landed DIR --project P SHA... [--rules-root DIR] [--roll N] [--reviewer PROGRAM]
//! ```
//!
//! DIR is the worktree the commits land from and the SHAs are the commits
//! `builds land` was given. No diff is reviewed more than once. Each change gets one review; the
//! coordinator reads the fixes for its confirmed findings. So a landing's
//! commits are taken when one change's review covers them: each commit whose
//! paths a group's rules cover is either one the review read, matched by patch
//! id (`commits.rs`) so that a rebase or a cherry-pick that keeps its change
//! keeps its review, or the fix the coordinator named for one of the review's
//! confirmed findings. Every group that covers a path a reviewed commit changes
//! must have answered over it, every finding must be settled, and every
//! confirmed one must name a fix among the commits landed. A commit that is
//! neither reviewed nor a named fix is new work, which lands as a change of its
//! own. A landing whose changed paths no group covers needs no review.
//!
//! `landed` records the landing as landed with the change's review run, then
//! decides whether to audit it (`audit.rs`) and, when it is sampled, starts
//! `builds review audit` in the background. A landing whose change landed
//! already is matched by its landed review, so both commands can run again.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::checkout::Checkout;
use crate::db;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::store::{Store, resolve_by};
use crate::review::commits::{self, Commit};
use crate::review::record::{self, LandingRow};
use crate::review::render;
use crate::review::run::Material;
use crate::tool::Tool;

const USAGE: &str = "builds review check-landing|landed DIR --project P SHA... [--rules-root DIR]";

/// What every refusal and every refused run says first.
pub const ONE_REVIEW: &str = "No diff is reviewed more than once. Each change gets one review; the coordinator reads the fixes for its confirmed findings.";

/// A commit to land and the groups whose rules cover a path it changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub commit: Commit,
    pub groups: Vec<String>,
}

/// Each of <commits> of the repo at <dir> with the groups that <groups_for>
/// picks for the paths it changes.
pub fn needs(
    dir: &Path,
    commits: Vec<Commit>,
    groups_for: impl Fn(&[String]) -> Vec<String>,
) -> Result<Vec<Need>> {
    let touched = commits::paths_of_each(dir, &commits)?;
    Ok(commits
        .into_iter()
        .zip(touched)
        .map(|(commit, paths)| Need {
            groups: groups_for(&paths),
            commit,
        })
        .collect())
}

/// The change whose review covers a landing's commits, and its review run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covered {
    pub landing: LandingRow,
    pub run: i64,
}

/// What a covering review still lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// Groups that cover a path a reviewed commit changes and gave no answer
    /// over it.
    pub unanswered: Vec<String>,
    /// Commits the review did not read that no confirmed finding names as
    /// its fix.
    pub unreviewed: Vec<Commit>,
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
            && self.unreviewed.is_empty()
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
    /// No review read any of these commits.
    NoRun,
    /// A change's review read some of these commits and does not take them.
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

/// The review runs of <landing>, oldest first, each with the commits it read.
/// A change has one; a store from before that rule may hold more, and a run
/// that asks only the groups that gave no answer reads the review's commits.
pub fn review_runs(db: &Connection, landing: i64) -> Result<Vec<(record::RunRow, Vec<Commit>)>> {
    let mut runs = Vec::new();
    for run in record::runs_of_landing(db, landing)? {
        if run.kind == "review" {
            let read = record::commits_of_run(db, run.id)?;
            runs.push((run, read));
        }
    }
    Ok(runs)
}

/// What <landing>'s review says of <needs>; None when its review read none of
/// the commits that need one.
fn assess(
    db: &Connection,
    landing: &LandingRow,
    needs: &[Need],
) -> Result<Option<(Covered, Unfinished)>> {
    let covered_paths = || needs.iter().filter(|n| !n.groups.is_empty());
    let runs = review_runs(db, landing.id)?;
    let read_by = |commit: &Commit| -> Vec<i64> {
        runs.iter()
            .filter(|(_, read)| read.iter().any(|c| c.patch_id == commit.patch_id))
            .map(|(run, _)| run.id)
            .collect()
    };
    if covered_paths().all(|n| read_by(&n.commit).is_empty()) {
        return Ok(None);
    }
    let Some(newest) = runs.last().map(|(run, _)| run.id) else {
        return Ok(None);
    };
    let answers: Vec<record::Call> = record::calls_of_landing(db, landing.id)?
        .into_iter()
        .filter(|c| c.failure.is_none())
        .collect();
    let findings: Vec<record::Finding> = record::findings_of_landing(db, landing.id)?
        .into_iter()
        .filter(|f| f.kind == "review" && f.counts())
        .collect();
    let confirmed = || findings.iter().filter(|f| f.settled_as("confirmed"));
    let fixes: Vec<&str> = confirmed()
        .filter_map(|f| f.fix.as_ref().map(|fix| fix.patch_id.as_str()))
        .collect();
    let mut unfinished = Unfinished::default();
    for need in covered_paths() {
        let runs = read_by(&need.commit);
        if runs.is_empty() {
            if !fixes.contains(&need.commit.patch_id.as_str()) {
                unfinished.unreviewed.push(need.commit.clone());
            }
            continue;
        }
        for group in &need.groups {
            let answered = answers
                .iter()
                .any(|c| &c.group == group && runs.contains(&c.run_id));
            if !answered && !unfinished.unanswered.contains(group) {
                unfinished.unanswered.push(group.clone());
            }
        }
    }
    let patches: Vec<&str> = needs.iter().map(|n| n.commit.patch_id.as_str()).collect();
    unfinished.unsettled = findings
        .iter()
        .filter(|f| f.settled.is_none())
        .cloned()
        .collect();
    unfinished.unfixed = confirmed()
        .filter(|f| f.fix.is_none())
        .map(|f| f.id)
        .collect();
    unfinished.fixed_elsewhere = confirmed()
        .filter_map(|f| {
            let fix = f.fix.as_ref()?;
            (!patches.contains(&fix.patch_id.as_str())).then(|| (f.id, fix.sha.clone()))
        })
        .collect();
    let covered = Covered {
        landing: landing.clone(),
        run: landing.landed_run.unwrap_or(newest),
    };
    Ok(Some((covered, unfinished)))
}

/// Whether the review lets the commits of <needs> land in <repo>. <db> is
/// None when the store has no database yet.
///
/// The newest open change whose review takes them covers them; failing that,
/// the newest open change whose review read one of them says what it lacks.
/// A change that landed already covers them only when its review takes them
/// all, so `landed` can run again after the push.
pub fn judge(db: Option<&Connection>, repo: &str, needs: &[Need]) -> Result<Verdict> {
    if needs.iter().all(|n| n.groups.is_empty()) {
        return Ok(Verdict::NotNeeded);
    }
    let Some(db) = db else {
        return Ok(Verdict::NoRun);
    };
    let mut lacking: Option<(Covered, Unfinished)> = None;
    for landing in record::landings_of_repo(db, repo, false)? {
        if let Some((covered, unfinished)) = assess(db, &landing, needs)? {
            if unfinished.is_empty() {
                return Ok(Verdict::Takes(covered));
            }
            lacking.get_or_insert((covered, unfinished));
        }
    }
    if let Some((covered, unfinished)) = lacking {
        return Ok(Verdict::Unfinished(covered, unfinished));
    }
    for landing in record::landings_of_repo(db, repo, true)? {
        if let Some((covered, unfinished)) = assess(db, &landing, needs)?
            && unfinished.is_empty()
        {
            return Ok(Verdict::Takes(covered));
        }
    }
    Ok(Verdict::NoRun)
}

/// Why <verdict> does not let <commits> land, in words; None when it does.
pub fn refusal(commits: &[Commit], verdict: &Verdict) -> Option<String> {
    let (covered, unfinished) = match verdict {
        Verdict::NotNeeded | Verdict::Takes(_) => return None,
        Verdict::NoRun => {
            return Some(format!(
                "no review read the commits this lands ({}).\n{ONE_REVIEW}\nReview them as a change (builds review <worktree> [<base>] in the branch's worktree), settle every finding, name the fix of each confirmed one, and land them.\n",
                shas(commits)
            ));
        }
        Verdict::Unfinished(covered, unfinished) => (covered, unfinished),
    };
    let mut text = format!(
        "review landing {} ({}), reviewed by run {}, does not take these commits ({}) yet:\n",
        covered.landing.id,
        covered.landing.key,
        covered.run,
        shas(commits)
    );
    for group in &unfinished.unanswered {
        text.push_str(&format!(
            "group {group} covers a path a reviewed commit changes and gave no answer over it\n"
        ));
    }
    if !unfinished.unanswered.is_empty() {
        text.push_str(
            "Run builds review in the branch's worktree: it asks only the groups that gave no answer, over the diff the review read.\n",
        );
    }
    if !unfinished.unreviewed.is_empty() {
        for commit in &unfinished.unreviewed {
            text.push_str(&format!(
                "commit {} is not one the review read, and no confirmed finding names it as its fix\n",
                short(&commit.sha)
            ));
        }
        text.push_str(&format!(
            "{ONE_REVIEW} A commit added after the review, or one whose patch id a rebase changed, is new work: land the reviewed commits and their fixes without it, then review it as a change of its own. When later commits need it, ask the person how to land them.\n"
        ));
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
            "finding {id} is confirmed and names no fix: fix it in a new commit, read the fix against the finding, and name it with builds review fixed {id} <sha>\n"
        ));
    }
    for (id, sha) in &unfinished.fixed_elsewhere {
        text.push_str(&format!(
            "finding {id} is fixed by {}, which is not among the commits this lands\n",
            short(sha)
        ));
    }
    Some(text)
}

/// What both commands read: the repo, the store, and the commits with the
/// groups each needs.
struct Asked {
    store: Store,
    repo: String,
    dir: PathBuf,
    needs: Vec<Need>,
    root: PathBuf,
}

impl Asked {
    fn commits(&self) -> Vec<Commit> {
        self.needs.iter().map(|n| n.commit.clone()).collect()
    }
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
    let material = Material::load(&root, project)?;
    let needs = needs(&checkout.top, commits, |paths| material.group_names(paths))?;
    Ok(Asked {
        store,
        repo: checkout.repo(),
        dir: checkout.top.clone(),
        needs,
        root,
    })
}

/// `builds review check-landing DIR --project P SHA...`: says why the commits
/// can't land, exit 1, or that the review takes them.
pub fn check_landing_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root"], &[])?;
    let asked = ask(tool, &args)?;
    let conn = db::open_reader(&asked.store.database())?;
    let verdict = judge(conn.as_ref(), &asked.repo, &asked.needs)?;
    match refusal(&asked.commits(), &verdict) {
        Some(why) => {
            print!("{why}");
            Err(Fail::Exit(1))
        }
        None => {
            println!("{}", taken(&verdict));
            Ok(())
        }
    }
}

/// What a verdict that lets the commits land says of them.
pub fn taken(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Takes(covered) => format!(
            "review landing {} ({}), reviewed by run {}, takes these commits: each is reviewed or the named fix of a confirmed finding, and every finding is settled",
            covered.landing.id, covered.landing.key, covered.run
        ),
        _ => "no group's rules cover the changed paths: the landing needs no review".to_string(),
    }
}

/// Records the change of <asked> as landed; returns the landing when this
/// recorded it, None when it needed no review or was recorded already.
fn record_landed(asked: &Asked, stamp: &Stamp) -> Result<Option<i64>> {
    let conn = db::open_reader(&asked.store.database())?;
    let verdict = judge(conn.as_ref(), &asked.repo, &asked.needs)?;
    if let Some(why) = refusal(&asked.commits(), &verdict) {
        fail!("the review does not take these commits:\n{why}")
    }
    let Verdict::Takes(covered) = verdict else {
        return Ok(None);
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

/// Records the landing that `builds review landed` names, written by `--by`,
/// else by the claim of DIR, the worktree the commits land from, wherever the
/// command runs.
/// Returns what it asked and the landing when this recorded it.
fn record_landing(tool: &Tool, args: &Args) -> Result<Option<(Asked, i64)>> {
    let asked = ask(tool, args)?;
    let stamp = Stamp {
        at: tool.now,
        by: resolve_by(args, &asked.dir)?,
    };
    Ok(record_landed(&asked, &stamp)?.map(|landing| (asked, landing)))
}

/// `builds review landed DIR --project P SHA...`: records the landing, then
/// starts the audit of a sampled one. The audit never fails the landing.
pub fn landed_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "roll", "reviewer"], &[])?;
    let Some((asked, landing)) = record_landing(tool, &args)? else {
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
