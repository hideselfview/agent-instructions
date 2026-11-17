//! What `builds land` asks of the review, before it fetches anything and after
//! it pushes:
//!
//! ```text
//! builds review check-landing DIR --project P SHA... [--rules-root DIR]
//! builds review landed DIR --project P SHA... [--rules-root DIR] [--roll N] [--reviewer PROGRAM]
//! ```
//!
//! DIR is the worktree builds land runs in, and the SHAs are the commits its
//! caller passed to `builds land`. The review reads each diff only once. Each
//! change gets one review; the coordinator reads the fixes for its confirmed
//! findings. A change's review is the one whose runs read one of the
//! commits, matched by the patch id of its added and removed lines
//! (`commits.rs`), or name one as a fix. check-landing takes the commits when
//! nothing on a path a group's rules cover changed since the last review: the
//! net change of DIR's HEAD past origin/main, without main's own commits that
//! a rebase brings in, against the net change the last review read
//! (`run::diff_since`, which `builds review` reads as its next run). So a
//! rebase that keeps every line the branch adds and removes keeps the review,
//! and so does a cherry-pick of the reviewed commits onto a moved main. land
//! checks the net change, not each commit. Every group asked over a diff a run read must have answered
//! over it (`Coverage`, which `builds review` uses too), the coordinator must
//! have settled every finding, and every confirmed finding on the commits
//! landed must name a fix among them. A finding is on a commit that changes
//! its file, and a finding on the commit messages is on the commit whose
//! message holds its quote. A confirmed finding on a changed file that names
//! no fix needs none when builds finds its quoted code in no line of the
//! branch's change past origin/main, as when the agent dropped the commit that
//! added it. check-landing and `builds review` record such a finding
//! (`CodeCheck`), and from then on it blocks no landing. A finding on a commit
//! that an earlier landing pushed belongs to that landing, so a branch can land
//! in parts. A landing whose changed paths no group covers needs no review.
//!
//! `landed` records the commits pushed. A branch lands in parts: while its
//! commits still wait to land (`--more-to-land`), its landing stays open, so
//! a later review of the branch reads only the changes since the last review,
//! and a finding on a commit not yet landed
//! still takes a fix. A finding whose fix a
//! push landed needs no fix in a later part. The push that leaves the branch
//! nothing to land records the landing as landed with the change's review
//! run, then decides whether to audit it (`audit.rs`) and, when the sample
//! picks it, starts `builds review audit` in the background. check-landing
//! and landed find the landed review of a change that landed already, so
//! both commands can run again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::store::{Store, resolve_by};
use crate::review::check;
use crate::review::commits::{self, Commit};
use crate::review::files;
use crate::review::record::{self, LandingRow};
use crate::review::render;
use crate::review::run::{self, Material};
use crate::tool::Tool;

const USAGE: &str =
    "builds review check-landing|landed DIR --project P SHA... [--rules-root DIR] [--more-to-land]";

/// What every refusal and every refused run says first.
pub const ONE_REVIEW: &str = "No diff is reviewed more than once. Each change gets one review; the coordinator reads the fixes for its confirmed findings.";

/// A commit to land, the paths it changes, and the groups whose rules cover
/// one of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub commit: Commit,
    pub paths: Vec<String>,
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
            paths,
            commit,
        })
        .collect())
}

/// The change whose review covers a landing's commits, and its review run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covered {
    pub landing: LandingRow,
    pub run: i64,
    /// The confirmed findings with no fix whose code builds found in no line
    /// of the branch's change, not yet recorded. The verdict counts them as
    /// recorded, so a command that acts on it records them first
    /// (`record_code_left`).
    pub code_left: Option<CodeLeft>,
}

/// What `judge` reads besides the records: the branch's net change, or
/// nothing.
#[derive(Clone, Copy, Debug)]
pub enum CodeCheck<'a> {
    /// Only the records: `review landed` runs after the push, which moved
    /// origin/main, and check-landing read the branch before the push.
    Records,
    /// The net change of this commit past origin/main too: whether it changed
    /// since the last review, and whether it still holds
    /// the quoted code of each confirmed finding on a changed file that names
    /// no fix. A finding whose code no line of that change holds needs no fix.
    Branch(&'a str),
}

/// The base builds reads a branch's change from.
const BRANCH_BASE: &str = "origin/main";

/// Confirmed findings with no fix whose quoted code builds found in no line of
/// the change from <base> to <head>.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeLeft {
    pub findings: Vec<i64>,
    pub base: String,
    pub head: String,
}

/// What a covering review still lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// Groups that the review asked over a diff a run read and that gave no
    /// answer over it.
    pub unanswered: Vec<String>,
    /// The paths a group's rules cover that changed since the last review.
    pub unreviewed: Vec<String>,
    /// Findings nobody settled.
    pub unsettled: Vec<record::Finding>,
    /// Confirmed findings on the commits that name no fix.
    pub unfixed: Vec<i64>,
    /// Confirmed findings on the commits whose fix is not among them, with
    /// the fix.
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
    /// No review read any of these commits or names one as a fix.
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

/// The review runs of <landing>, oldest first, each with the commits of the
/// net change it read.
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

/// What a change's review read, and the commits it names as fixes. `builds
/// review` and `builds land` both decide from it what the review has yet to
/// read, so the two never disagree.
pub struct Coverage {
    /// The review runs, oldest first, each with the commits of the net change
    /// it read.
    pub runs: Vec<(record::RunRow, Vec<Commit>)>,
    /// The run of the last review: a review shows the changes since the net
    /// change it read.
    pub last_review: record::RunRow,
    /// The patch id of each confirmed finding's named fix.
    fixes: Vec<String>,
}

impl Coverage {
    /// The coverage of the review of <landing>.
    pub fn of(db: &Connection, landing: i64) -> Result<Coverage> {
        let fixes = record::findings_of_landing(db, landing)?
            .into_iter()
            .filter(|f| f.kind == "review" && f.counts() && f.settled_as("confirmed"))
            .filter_map(|f| f.fix.map(|fix| fix.patch_id))
            .collect();
        let runs = review_runs(db, landing)?;
        let Some(reviewed) = record::landing(db, landing)?.reviewed_run else {
            fail!("review landing {landing} names no last review");
        };
        let Some((last_review, _)) = runs.iter().find(|(run, _)| run.id == reviewed) else {
            fail!(
                "review landing {landing} names run {reviewed} as its last review, which is not a review run of it"
            );
        };
        Ok(Coverage {
            last_review: last_review.clone(),
            runs,
            fixes,
        })
    }

    /// Whether <commit> belongs to this review: a run read its added and
    /// removed lines as part of a net change, or a confirmed finding names it
    /// as its fix.
    fn knows(&self, commit: &Commit) -> bool {
        self.runs
            .iter()
            .any(|(_, read)| read.iter().any(|c| c.patch_id == commit.patch_id))
            || self.fixes.contains(&commit.patch_id)
    }

    /// The commits of the net change <run> read. Fails when <run> is not a
    /// run of this review.
    pub fn commits_of(&self, run: i64) -> Result<&[Commit]> {
        match self.runs.iter().find(|(r, _)| r.id == run) {
            Some((_, read)) => Ok(read),
            None => fail!("run {run} is not a review run of this landing"),
        }
    }

    /// For each diff a run read whose groups did not all answer over it, the
    /// first run that read it and the groups asked over it that no run of
    /// that diff answered for, oldest first. <calls> are the review's calls.
    pub fn unanswered(&self, calls: &[record::Call]) -> Vec<(&record::RunRow, Vec<String>)> {
        let mut found = Vec::new();
        for (at, (first, _)) in self.runs.iter().enumerate() {
            if self.runs[..at]
                .iter()
                .any(|(earlier, _)| earlier.same_diff(first))
            {
                continue;
            }
            let over: Vec<&record::Call> = calls
                .iter()
                .filter(|c| {
                    self.runs
                        .iter()
                        .any(|(run, _)| run.id == c.run_id && run.same_diff(first))
                })
                .collect();
            let mut missing: Vec<String> = Vec::new();
            for call in &over {
                let answered = over
                    .iter()
                    .any(|c| c.group == call.group && c.failure.is_none());
                if !answered && !missing.contains(&call.group) {
                    missing.push(call.group.clone());
                }
            }
            if !missing.is_empty() {
                found.push((first, missing));
            }
        }
        found
    }
}

/// Whether <finding> is on one of the commits of <needs>, in the repo at
/// <dir>, where <read> is what its run read: a finding on the commit messages
/// is on each read commit whose message holds its quote (on every read commit
/// when no one message holds it), and any other finding is on each commit
/// that changes its file. <messages> keeps each commit message read, by SHA.
fn on_commits(
    dir: &Path,
    finding: &record::Finding,
    read: &[Commit],
    needs: &[Need],
    messages: &mut HashMap<String, String>,
) -> Result<bool> {
    if finding.file != files::COMMIT_MESSAGE {
        return Ok(needs.iter().any(|n| {
            n.paths
                .iter()
                .any(|path| check::names_file(path, &finding.file))
        }));
    }
    let quote = check::squash(&finding.code);
    let mut holders: Vec<&Commit> = Vec::new();
    for commit in read {
        if !messages.contains_key(&commit.sha) {
            let message = check::squash(&commits::message(dir, &commit.sha)?);
            messages.insert(commit.sha.clone(), message);
        }
        if messages[&commit.sha].contains(&quote) {
            holders.push(commit);
        }
    }
    if holders.is_empty() {
        holders = read.iter().collect();
    }
    Ok(holders
        .iter()
        .any(|held| needs.iter().any(|n| n.commit.patch_id == held.patch_id)))
}

/// What <landing>'s review says of <needs>, commits of the repo at <dir>,
/// reading the branch as <check> says and picking the groups of a path with
/// <groups_for>; None when its review neither read nor names as a fix any of
/// the commits that need one.
fn assess(
    db: &Connection,
    dir: &Path,
    landing: &LandingRow,
    needs: &[Need],
    check: CodeCheck,
    groups_for: &dyn Fn(&[String]) -> Vec<String>,
) -> Result<Option<(Covered, Unfinished)>> {
    let covered_paths = || needs.iter().filter(|n| !n.groups.is_empty());
    let coverage = Coverage::of(db, landing.id)?;
    if !covered_paths().any(|n| coverage.knows(&n.commit)) {
        return Ok(None);
    }
    let runs = &coverage.runs;
    let last = &coverage.last_review;
    let findings: Vec<record::Finding> = record::findings_of_landing(db, landing.id)?
        .into_iter()
        .filter(|f| f.kind == "review" && f.counts())
        .collect();
    let confirmed = || findings.iter().filter(|f| f.settled_as("confirmed"));
    let pushed = record::pushed_of_landing(db, landing.id)?;
    // The confirmed findings this landing must fix: those on its commits
    // whose fix no earlier push of the branch landed and whose code builds
    // did not record as no longer in the branch's change.
    let mut owed: Vec<&record::Finding> = Vec::new();
    let mut messages = HashMap::new();
    for finding in confirmed() {
        if finding
            .fix
            .as_ref()
            .is_some_and(|fix| pushed.contains(&fix.patch_id))
            || finding.code_left.is_some()
        {
            continue;
        }
        let Some((_, read)) = runs.iter().find(|(run, _)| run.id == finding.run_id) else {
            fail!(
                "finding {} belongs to run {}, which is not a review run of landing {}",
                finding.id,
                finding.run_id,
                landing.id
            );
        };
        if on_commits(dir, finding, read, needs, &mut messages)? {
            owed.push(finding);
        }
    }
    let mut unfinished = Unfinished::default();
    let calls = record::calls_of_landing(db, landing.id)?;
    for (_, groups) in coverage.unanswered(&calls) {
        for group in groups {
            if !unfinished.unanswered.contains(&group) {
                unfinished.unanswered.push(group);
            }
        }
    }
    if let CodeCheck::Branch(head) = check {
        let since = run::diff_since(dir, &last.base, &last.head, BRANCH_BASE, head)?;
        unfinished.unreviewed = since
            .changed
            .into_iter()
            .filter(|path| !groups_for(std::slice::from_ref(path)).is_empty())
            .collect();
    }
    let patches: Vec<&str> = needs.iter().map(|n| n.commit.patch_id.as_str()).collect();
    unfinished.unsettled = findings
        .iter()
        .filter(|f| f.settled.is_none())
        .cloned()
        .collect();
    let unfixed: Vec<&&record::Finding> = owed.iter().filter(|f| f.fix.is_none()).collect();
    let code_left = match check {
        CodeCheck::Branch(head) if unfixed.iter().any(|f| f.file != files::COMMIT_MESSAGE) => {
            let change = run::diff_between(dir, BRANCH_BASE, head)?;
            let quotes = change.quotes();
            let findings: Vec<i64> = unfixed
                .iter()
                .filter(|f| {
                    f.file != files::COMMIT_MESSAGE
                        && check::code_in(&quotes, &f.file, &f.code).is_err()
                })
                .map(|f| f.id)
                .collect();
            (!findings.is_empty()).then_some(CodeLeft {
                findings,
                base: change.base,
                head: change.head,
            })
        }
        _ => None,
    };
    let left = |id: i64| code_left.as_ref().is_some_and(|l| l.findings.contains(&id));
    unfinished.unfixed = unfixed
        .iter()
        .map(|f| f.id)
        .filter(|id| !left(*id))
        .collect();
    unfinished.fixed_elsewhere = owed
        .iter()
        .filter_map(|f| {
            let fix = f.fix.as_ref()?;
            (!patches.contains(&fix.patch_id.as_str())).then(|| (f.id, fix.sha.clone()))
        })
        .collect();
    let covered = Covered {
        landing: landing.clone(),
        run: landing.landed_run.unwrap_or(last.id),
        code_left,
    };
    Ok(Some((covered, unfinished)))
}

/// Whether the review lets the commits of <needs>, read from the checkout at
/// <dir>, land in <repo>.
///
/// The newest open change whose review takes them covers them; failing that,
/// the newest open change whose review read one of them says what it lacks.
/// A change that landed already covers them only when its review takes them
/// all, as its records say, so `landed` can run again after the push. <check>
/// says whether to read the branch's net change, and <groups_for> picks the
/// groups whose rules cover a path.
pub fn judge(
    db: &Connection,
    repo: &str,
    dir: &Path,
    needs: &[Need],
    check: CodeCheck,
    groups_for: &dyn Fn(&[String]) -> Vec<String>,
) -> Result<Verdict> {
    if needs.iter().all(|n| n.groups.is_empty()) {
        return Ok(Verdict::NotNeeded);
    }
    let mut lacking: Option<(Covered, Unfinished)> = None;
    for landing in record::landings_of_repo(db, repo, false)? {
        if let Some((covered, unfinished)) = assess(db, dir, &landing, needs, check, groups_for)? {
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
        if let Some((covered, unfinished)) =
            assess(db, dir, &landing, needs, CodeCheck::Records, groups_for)?
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
            "group {group} gave no answer over a diff the review read\n"
        ));
    }
    if !unfinished.unanswered.is_empty() {
        text.push_str(
            "Run builds review in the branch's worktree: it asks only the groups that gave no answer, over the diff the review read.\n",
        );
    }
    if !unfinished.unreviewed.is_empty() {
        text.push_str(&format!(
            "the branch changed since the last review, run {}, in {}\n",
            covered.run,
            unfinished.unreviewed.join(", ")
        ));
        text.push_str(&format!(
            "{ONE_REVIEW} Run builds review in the branch's worktree: a review shows the changes since the last review, and nothing else, fix commits included. A rebase that keeps every line the branch adds and removes leaves nothing to read.\n"
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
            "finding {id} is confirmed and names no fix: fix it in a new commit, read the fix against the finding, and name it with builds review fixed {id} <sha>. When the finding is wrong, settle it again: builds review settle {id} --again --why TEXT\n"
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

/// What both commands read: the repo, the store with its database open for
/// reading, and the commits with the groups each needs.
struct Asked {
    store: Store,
    conn: crate::db::Held,
    repo: String,
    dir: PathBuf,
    needs: Vec<Need>,
    root: PathBuf,
    material: Material,
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
    let project = args.require("project")?;
    let store = Store::named(&tool.places, project)?;
    let conn = store.open_existing()?;
    let checkout = Checkout::find(dir)?;
    let root = super::rules_root(args)?;
    let commits = commits::with_patch_ids(&checkout.top, shas)?;
    let material = Material::load(&root, project)?;
    let needs = needs(&checkout.top, commits, |paths| material.group_names(paths))?;
    Ok(Asked {
        store,
        conn,
        repo: checkout.repo(),
        dir: checkout.top.clone(),
        needs,
        root,
        material,
    })
}

/// `builds review check-landing DIR --project P SHA...`: records each
/// confirmed finding whose code is on no line of the change of DIR's HEAD past
/// origin/main, then says why
/// the commits can't land, exit 1, or that the review takes them.
pub fn check_landing_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root"], &[])?;
    let asked = ask(tool, &args)?;
    let verdict = judge(
        &asked.conn,
        &asked.repo,
        &asked.dir,
        &asked.needs,
        CodeCheck::Branch("HEAD"),
        &|paths| asked.material.group_names(paths),
    )?;
    let recorded = record_code_left(&asked.store, &verdict, || {
        Stamp::resolve(tool.now, &args, &asked.dir)
    })?;
    print!("{recorded}");
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

/// Records each confirmed finding that <verdict> found with its code gone from
/// the branch, so that the verdict holds, and returns a line for each; empty
/// when it found none. <stamp> gives who runs the command, and runs only when
/// there is something to record.
pub fn record_code_left(
    store: &Store,
    verdict: &Verdict,
    stamp: impl FnOnce() -> Result<Stamp>,
) -> Result<String> {
    let covered = match verdict {
        Verdict::Takes(covered) | Verdict::Unfinished(covered, _) => covered,
        Verdict::NotNeeded | Verdict::NoRun => return Ok(String::new()),
    };
    let Some(left) = &covered.code_left else {
        return Ok(String::new());
    };
    let stamp = stamp()?;
    let ids: Vec<String> = left.findings.iter().map(i64::to_string).collect();
    store.write(
        &stamp,
        &format!("review code not in branch {}", ids.join(" ")),
        |db| {
            for id in &left.findings {
                record::record_code_left(db, *id, &left.base, &left.head, stamp.at, &stamp.by)?;
            }
            Ok(())
        },
    )?;
    Ok(left
        .findings
        .iter()
        .map(|id| {
            format!(
                "finding {id}: builds found its code on no line of the branch's change {}..{}, so the finding needs no fix; builds recorded that\n",
                short(&left.base),
                short(&left.head)
            )
        })
        .collect())
}

/// What a verdict that lets the commits land says of them.
pub fn taken(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Takes(covered) => format!(
            "review landing {} ({}), reviewed by run {}, takes these commits: its runs read the branch's net change as it is now, and the coordinator settled every finding",
            covered.landing.id, covered.landing.key, covered.run
        ),
        _ => "no group's rules cover the changed paths: the landing needs no review".to_string(),
    }
}

/// Records the commits of <asked> as pushed, and the change as landed unless
/// <more>: commits of its branch still wait to land. Returns the landing when
/// this recorded it as landed; None when it needed no review, an earlier run
/// of landed recorded it already, or it lands in parts and has more to land.
fn record_landed(asked: &Asked, stamp: &Stamp, more: bool) -> Result<Option<i64>> {
    // Before the push, which moved origin/main, check-landing recorded each
    // finding whose code is no longer in the branch's change, so this reads
    // only the records.
    let verdict = judge(
        &asked.conn,
        &asked.repo,
        &asked.dir,
        &asked.needs,
        CodeCheck::Records,
        &|paths| asked.material.group_names(paths),
    )?;
    if let Some(why) = refusal(&asked.commits(), &verdict) {
        fail!("the review does not take these commits:\n{why}")
    }
    let Verdict::Takes(covered) = verdict else {
        return Ok(None);
    };
    if covered.landing.landed_run.is_some() {
        return Ok(None);
    }
    let landing = covered.landing.id;
    let pushed: Vec<&str> = asked
        .needs
        .iter()
        .map(|n| n.commit.patch_id.as_str())
        .collect();
    asked
        .store
        .write(stamp, &format!("review landed {landing}"), |db| {
            record::record_pushed(db, landing, &pushed, stamp.at)?;
            if more {
                return Ok(());
            }
            record::record_landed(db, landing, covered.run, stamp.at)
        })?;
    Ok((!more).then_some(landing))
}

/// Records the landing that `builds review landed` names, written by `--by`,
/// else by the claim of DIR, the worktree builds land runs in, wherever the
/// command runs.
/// Returns what it asked and the landing when this recorded it.
fn record_landing(tool: &Tool, args: &Args) -> Result<Option<(Asked, i64)>> {
    let asked = ask(tool, args)?;
    let stamp = Stamp {
        at: tool.now,
        by: resolve_by(args, &asked.dir)?,
    };
    let more = args.switch("more-to-land");
    Ok(record_landed(&asked, &stamp, more)?.map(|landing| (asked, landing)))
}

/// `builds review landed DIR --project P SHA... [--more-to-land]`: records
/// the commits pushed and, unless commits of the branch still wait to land,
/// the landing as landed, then starts the audit of a sampled one. The audit
/// never fails the landing.
pub fn landed_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "roll", "reviewer"], &["more-to-land"])?;
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
    if !super::audit::sampled(
        &asked.conn,
        landing,
        &super::group_names(&asked.root)?,
        roll,
    )? {
        return Ok(());
    }
    let program = std::env::current_exe().context(|| "finding this program".to_string())?;
    // The audit keeps running after this command exits, so this command
    // removes BUILDS_USE from the audit's environment, and the audit takes
    // its own use of the stores.
    std::process::Command::new(program)
        .env_remove(crate::install_gate::VARIABLE)
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
