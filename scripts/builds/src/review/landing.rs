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
//! findings. So check-landing takes a landing's commits when one change's
//! review covers them: each commit whose paths a group's rules cover is either
//! one the review read, matched by the patch id of its added and removed lines
//! (`commits.rs`) so that a rebase or a cherry-pick that keeps those lines
//! keeps its review, or the fix the coordinator named for one of the review's
//! confirmed findings on a changed file (`Coverage`, which `builds review`
//! uses too). A fix of a finding on the commit messages is a reviewed commit
//! with a new message, so the review covers it only when a run read its added
//! and removed lines. Every group that covers a path a reviewed commit
//! changes must have answered over it, the coordinator must have settled every
//! finding, and every confirmed finding on the commits landed must name a fix
//! among them. A finding is on a commit that changes its file, and a finding
//! on the commit messages is on the commit whose message holds its quote. A
//! confirmed finding on a changed file that names no fix needs none when
//! builds finds its quoted code in no line of the branch's change past
//! origin/main: the commit that held the code left the branch. check-landing
//! and `builds review` record such a finding (`CodeCheck`), and from then on
//! it blocks no landing. A
//! finding on a commit that an earlier landing pushed belongs to that landing,
//! so a branch can land in parts. A commit that the review did not read and
//! that no finding on a changed file names as its fix waits for `builds
//! review`, which reads it as a new run of the review. A landing whose changed
//! paths no group covers needs no review.
//!
//! `landed` records the commits pushed. A branch lands in parts: while its
//! commits still wait to land (`--more-to-land`), its landing stays open, so
//! a later review of the branch reads only the commits no run read and a
//! finding on a commit not yet landed still takes a fix. A finding whose fix a
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

/// Where `judge` looks for the quoted code of a confirmed finding on a changed
/// file that names no fix.
#[derive(Clone, Copy, Debug)]
pub enum CodeCheck<'a> {
    /// Only in the records: `review landed` runs after the push, which moved
    /// origin/main.
    Records,
    /// In the change of this commit past origin/main too: a finding whose code
    /// no line of that change holds needs no fix.
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
    /// Groups that cover a path a reviewed commit changes and gave no answer
    /// over it.
    pub unanswered: Vec<String>,
    /// Commits the review did not read that no confirmed finding on a changed
    /// file names as its fix.
    pub unreviewed: Vec<Commit>,
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

/// What a change's review read, and the commits it names as fixes. `builds
/// review` and `builds land` both decide from it which commits need a new run
/// of the review, so the two never disagree.
pub struct Coverage {
    /// The review runs, oldest first, each with the commits it read.
    pub runs: Vec<(record::RunRow, Vec<Commit>)>,
    /// The patch id of each confirmed finding's named fix, and whether the
    /// finding is on the commit messages.
    fixes: Vec<(String, bool)>,
}

impl Coverage {
    /// The coverage of the review of <landing>.
    pub fn of(db: &Connection, landing: i64) -> Result<Coverage> {
        let fixes = record::findings_of_landing(db, landing)?
            .into_iter()
            .filter(|f| f.kind == "review" && f.counts() && f.settled_as("confirmed"))
            .filter_map(|f| {
                let on_message = f.file == files::COMMIT_MESSAGE;
                f.fix.map(|fix| (fix.patch_id, on_message))
            })
            .collect();
        Ok(Coverage {
            runs: review_runs(db, landing)?,
            fixes,
        })
    }

    /// The runs that read <commit>'s added and removed lines.
    pub fn runs_reading(&self, commit: &Commit) -> Vec<i64> {
        self.runs
            .iter()
            .filter(|(_, read)| read.iter().any(|c| c.patch_id == commit.patch_id))
            .map(|(run, _)| run.id)
            .collect()
    }

    /// Whether a new run of the review must read <commit>: no run read its
    /// added and removed lines, and no confirmed finding on a changed file
    /// names it as its fix. A fix of a finding on the commit messages is a reviewed commit
    /// with a new message, so it needs a review when a rebase changed the
    /// lines it adds or removes, as any reviewed commit does.
    pub fn needs_review(&self, commit: &Commit) -> bool {
        self.runs_reading(commit).is_empty()
            && !self
                .fixes
                .iter()
                .any(|(fix, on_message)| !on_message && *fix == commit.patch_id)
    }

    /// Whether a run read <commit> or a confirmed finding names it as its fix.
    fn knows(&self, commit: &Commit) -> bool {
        !self.runs_reading(commit).is_empty()
            || self.fixes.iter().any(|(fix, _)| *fix == commit.patch_id)
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
/// looking for the code of confirmed findings with no fix as <check> says;
/// None when its review neither read nor names as a fix any of the commits
/// that need one.
fn assess(
    db: &Connection,
    dir: &Path,
    landing: &LandingRow,
    needs: &[Need],
    check: CodeCheck,
) -> Result<Option<(Covered, Unfinished)>> {
    let covered_paths = || needs.iter().filter(|n| !n.groups.is_empty());
    let coverage = Coverage::of(db, landing.id)?;
    if !covered_paths().any(|n| coverage.knows(&n.commit)) {
        return Ok(None);
    }
    let runs = &coverage.runs;
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
    let pushed = record::pushed_of_landing(db, landing.id)?;
    // The confirmed findings this landing must fix: those on its commits
    // whose fix no earlier push of the branch landed and whose code builds
    // did not record as gone from the branch.
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
    for need in covered_paths() {
        if coverage.needs_review(&need.commit) {
            unfinished.unreviewed.push(need.commit.clone());
            continue;
        }
        let runs = coverage.runs_reading(&need.commit);
        if runs.is_empty() {
            // A named fix that no run read: no group answers over it.
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
        run: landing.landed_run.unwrap_or(newest),
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
/// says where to look for the code of a confirmed finding with no fix.
pub fn judge(
    db: &Connection,
    repo: &str,
    dir: &Path,
    needs: &[Need],
    check: CodeCheck,
) -> Result<Verdict> {
    if needs.iter().all(|n| n.groups.is_empty()) {
        return Ok(Verdict::NotNeeded);
    }
    let mut lacking: Option<(Covered, Unfinished)> = None;
    for landing in record::landings_of_repo(db, repo, false)? {
        if let Some((covered, unfinished)) = assess(db, dir, &landing, needs, check)? {
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
        if let Some((covered, unfinished)) = assess(db, dir, &landing, needs, CodeCheck::Records)?
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
                "commit {} is not one the review read, and no confirmed finding on a changed file names it as its fix\n",
                short(&commit.sha)
            ));
        }
        text.push_str(&format!(
            "{ONE_REVIEW} No run of the review read a commit added after the review, or one whose added or removed lines a rebase changed. Run builds review in the branch's worktree: it reads those commits as a new run of the review. A commit that fixes a confirmed finding on a changed file needs no review: read it against the finding and name it with builds review fixed <finding> <sha>. A commit named as the fix of a finding on its message needs a review when no run read its added and removed lines.\n"
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
    })
}

/// `builds review check-landing DIR --project P SHA...`: records each
/// confirmed finding whose code left the branch at DIR's HEAD, then says why
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
        &format!("review code left {}", ids.join(" ")),
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
                "finding {id}: builds found no line of the branch's change {}..{} that holds its code, so its code left the branch and the finding needs no fix; builds recorded that\n",
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
            "review landing {} ({}), reviewed by run {}, takes these commits: a run of the review read each one, or the coordinator named it as the fix of a confirmed finding on a changed file, and the coordinator settled every finding",
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
    // check-landing recorded each finding whose code left the branch before
    // the push, which moved origin/main, so this reads only the records.
    let verdict = judge(
        &asked.conn,
        &asked.repo,
        &asked.dir,
        &asked.needs,
        CodeCheck::Records,
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
