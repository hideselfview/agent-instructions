//! What `builds land` asks of the review, before it fetches anything and after
//! it pushes:
//!
//! ```text
//! builds review check-landing DIR --project P SHA... [--landing KEY] [--rules-root DIR]
//! builds review landed DIR --project P SHA... [--landing KEY] [--more-to-land] [--rules-root DIR]
//! ```
//!
//! DIR is the worktree builds land runs in, the SHAs are the commits its
//! caller passed to `builds land`, and KEY names the change in DIR's repo, by
//! default DIR's branch as `builds review` names it. The review reads each
//! diff only once. Each change gets one review, the open landing of its repo
//! and key. check-landing takes the commits when nothing is unreviewed and no
//! confirmed finding is open: nothing on a path a group's rules cover, and no
//! commit message a group reads, changed since the last review
//! (`run::diff_since`, which `builds review` reads as its next run), every
//! group asked over a diff a run read answered over it (`Coverage`, which
//! `builds review` uses too), and the coordinator settled every finding.
//! A confirmed finding stays open until a later review closes it, because the
//! changes that review read fix it, or the coordinator settles it again as
//! dropped. A rebase that keeps every line the branch adds and removes leaves
//! nothing unreviewed, and so does a cherry-pick of the reviewed commits onto a
//! moved main. A landing whose changed paths no group covers needs no review.
//!
//! `landed` records the landing as landed with its last review, unless commits
//! of the branch still wait to land (`--more-to-land`), which keeps it open.
//! `landed` then decides whether to audit it (`audit.rs`) and, when the
//! sample picks it, starts `builds review audit` in the background, never for
//! a landing of design files only (`builds design only`). A key
//! with no open landing has nothing to record, so `landed` can run again.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::short;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::store::{Store, resolve_by};
use crate::review::commits;
use crate::review::files;
use crate::review::record::{self, Change, LandingRow};
use crate::review::render;
use crate::review::run::{self, Material};
use crate::tool::Tool;

const USAGE: &str = "builds review check-landing|landed DIR --project P SHA... [--landing KEY] [--rules-root DIR] [--more-to-land]";

/// What every refusal and every refused run says first.
pub const ONE_REVIEW: &str = "builds review never sends a reviewer the same diff twice. Each change gets one review: a review shows the changes since the last review, and nothing else.";

/// A commit to land, the paths it changes, and the groups whose rules cover
/// one of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub sha: String,
    pub paths: Vec<String>,
    pub groups: Vec<String>,
}

/// Each of <shas> of the repo at <dir> with the groups that <groups_for>
/// picks for the paths it changes.
pub fn needs(
    dir: &Path,
    shas: Vec<String>,
    groups_for: impl Fn(&[String]) -> Vec<String>,
) -> Result<Vec<Need>> {
    let touched = commits::paths_of_each(dir, &shas)?;
    Ok(shas
        .into_iter()
        .zip(touched)
        .map(|(sha, paths)| Need {
            groups: groups_for(&paths),
            paths,
            sha,
        })
        .collect())
}

/// The change whose review covers a landing's commits, and its last review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covered {
    pub landing: LandingRow,
    pub run: i64,
}

/// What `judge` reads besides the records: the branch's net change, or
/// nothing.
#[derive(Clone, Copy, Debug)]
pub enum CodeCheck<'a> {
    /// Only the records: `review landed` runs after the push, which moved
    /// origin/main, and check-landing read the branch before the push.
    Records,
    /// The net change of this commit past origin/main too: whether it changed
    /// since the last review.
    Branch(&'a str),
}

/// The base builds reads a branch's change from.
const BRANCH_BASE: &str = "origin/main";

/// What a covering review still lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// Groups that the review asked over a diff a run read and that gave no
    /// answer over it.
    pub unanswered: Vec<String>,
    /// The paths a group's rules cover that changed since the last review,
    /// and `files::COMMIT_MESSAGE` when a commit message that a group reads
    /// changed since then.
    pub unreviewed: Vec<String>,
    /// Findings nobody settled.
    pub unsettled: Vec<record::Finding>,
    /// Confirmed findings that no review closed.
    pub open: Vec<record::Finding>,
}

impl Unfinished {
    fn is_empty(&self) -> bool {
        self.unanswered.is_empty()
            && self.unreviewed.is_empty()
            && self.unsettled.is_empty()
            && self.open.is_empty()
    }
}

/// What the review says of a landing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No group's rules cover a path the commits change.
    NotNeeded,
    /// A finished review covers the commits.
    Takes(Covered),
    /// The change's key has no open review.
    NoRun,
    /// The change's review does not take the commits yet.
    Unfinished(Covered, Unfinished),
}

fn shas(needs: &[Need]) -> String {
    needs
        .iter()
        .map(|n| short(&n.sha))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The review runs of <landing>, oldest first.
pub fn review_runs(db: &Connection, landing: i64) -> Result<Vec<record::RunRow>> {
    Ok(record::runs_of_landing(db, landing)?
        .into_iter()
        .filter(|run| run.kind == "review")
        .collect())
}

/// What a change's review read. `builds review` and `builds land` both decide
/// from it what the review has yet to read, so the two never disagree.
pub struct Coverage {
    /// The review runs, oldest first.
    pub runs: Vec<record::RunRow>,
    /// The run of the last review: a review shows the changes since the net
    /// change it read.
    pub last_review: record::RunRow,
}

impl Coverage {
    /// The coverage of the review of <landing>, a row the caller read.
    pub fn of(db: &Connection, landing: &LandingRow) -> Result<Coverage> {
        let runs = review_runs(db, landing.id)?;
        let Some(reviewed) = landing.reviewed_run else {
            fail!("review landing {} names no last review", landing.id);
        };
        let Some(last_review) = runs.iter().find(|run| run.id == reviewed) else {
            fail!(
                "review landing {} names run {reviewed} as its last review, which is not a review run of it",
                landing.id
            );
        };
        Ok(Coverage {
            last_review: last_review.clone(),
            runs,
        })
    }

    /// For each diff a run read whose groups did not all answer over it, the
    /// first run that read it and the groups asked over it that no run of
    /// that diff answered for, oldest first. <calls> are the review's calls.
    pub fn unanswered(&self, calls: &[record::Call]) -> Vec<(&record::RunRow, Vec<String>)> {
        let mut found = Vec::new();
        for (at, first) in self.runs.iter().enumerate() {
            if self.runs[..at]
                .iter()
                .any(|earlier| earlier.same_diff(first))
            {
                continue;
            }
            let over: Vec<&record::Call> = calls
                .iter()
                .filter(|c| {
                    self.runs
                        .iter()
                        .any(|run| run.id == c.run_id && run.same_diff(first))
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

    /// The run up to whose net change group <group> has read every change
    /// with <finding> among its open findings: the run that raised the
    /// finding, then each run that read the changes since the net change
    /// reached so far and over whose diff the group answered once the judge
    /// had confirmed the finding. <calls> are the review's calls, and
    /// <asking> the diffs this review asks again with the groups it asks
    /// over each (`unanswered`), which count as answered: the finding is
    /// confirmed now. A later run asks the group over the changes since that
    /// run, so that the group can close the finding whatever file the fix
    /// changed.
    pub fn read_with(
        &self,
        calls: &[record::Call],
        asking: &[(&record::RunRow, Vec<String>)],
        finding: &record::Finding,
        group: &str,
    ) -> Result<&record::RunRow> {
        let Some(raised) = self.runs.iter().find(|run| run.id == finding.run_id) else {
            fail!(
                "finding {} comes from run {}, which is not a review run of its landing",
                finding.id,
                finding.run_id
            );
        };
        let confirmed = finding.settled.as_ref().map(|settled| settled.last().at);
        // Whether the group answered over <run>'s diff with the finding given.
        let answered = |run: &record::RunRow| {
            let given = |over: &record::RunRow| confirmed.is_some_and(|at| at <= over.at);
            let ran = self.runs.iter().any(|over| {
                over.same_diff(run)
                    && given(over)
                    && calls.iter().any(|call| {
                        call.run_id == over.id && call.group == group && call.failure.is_none()
                    })
            });
            let asks = asking.iter().any(|(over, groups)| {
                over.same_diff(run) && groups.iter().any(|name| name == group)
            });
            ran || asks
        };
        let mut reached = raised;
        let mut passed = vec![raised];
        while let Some(next) = self.runs.iter().rev().find(|run| {
            let from_reached = run.since.is_some_and(|since| {
                self.runs
                    .iter()
                    .any(|earlier| earlier.id == since && earlier.reads_up_to(reached))
            });
            from_reached && !passed.iter().any(|p| p.reads_up_to(run)) && answered(run)
        }) {
            passed.push(next);
            reached = next;
        }
        Ok(reached)
    }
}

/// What the review of <landing> lacks, in the repo at <dir>, reading the
/// branch as <check> says, against the groups and rules of <material>.
fn assess(
    db: &Connection,
    dir: &Path,
    landing: &LandingRow,
    check: CodeCheck,
    material: &Material,
) -> Result<(Covered, Unfinished)> {
    let coverage = Coverage::of(db, landing)?;
    let last = &coverage.last_review;
    let findings = record::findings_of_landing(db, landing.id)?;
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
        let branch = commits::paths(dir, &commits::in_range(dir, &since.base, &since.head)?)?;
        let messages_unread = !material.commit_text_groups(&since.diff, &branch).is_empty();
        unfinished.unreviewed = since
            .diff
            .changed
            .into_iter()
            .filter(|path| !material.group_names(std::slice::from_ref(path)).is_empty())
            .collect();
        if messages_unread {
            unfinished
                .unreviewed
                .push(files::COMMIT_MESSAGE.to_string());
        }
    }
    unfinished.unsettled = findings
        .iter()
        .filter(|f| f.is_unsettled())
        .cloned()
        .collect();
    unfinished.open = findings.into_iter().filter(|f| f.is_open()).collect();
    let covered = Covered {
        landing: landing.clone(),
        run: last.id,
    };
    Ok((covered, unfinished))
}

/// Whether the review lets the commits of <needs>, read from the checkout at
/// <dir>, land: the open review of <change> must take them. <check> says whether
/// to read the branch's net change, and <material> holds the groups and the
/// rules that say which group reads a path or a commit message.
pub fn judge(
    db: &Connection,
    change: Change,
    dir: &Path,
    needs: &[Need],
    check: CodeCheck,
    material: &Material,
) -> Result<Verdict> {
    if needs.iter().all(|n| n.groups.is_empty()) {
        return Ok(Verdict::NotNeeded);
    }
    let Some(open) = record::open_landing(db, change)? else {
        return Ok(Verdict::NoRun);
    };
    let landing = record::landing(db, open)?;
    let (covered, unfinished) = assess(db, dir, &landing, check, material)?;
    if unfinished.is_empty() {
        Ok(Verdict::Takes(covered))
    } else {
        Ok(Verdict::Unfinished(covered, unfinished))
    }
}

/// Why <verdict> does not let the commits of <needs> land, in words; None
/// when it does.
pub fn refusal(needs: &[Need], verdict: &Verdict) -> Option<String> {
    let (covered, unfinished) = match verdict {
        Verdict::NotNeeded | Verdict::Takes(_) => return None,
        Verdict::NoRun => {
            return Some(format!(
                "no review of this change is open, so no review read the commits this lands ({}).\n{ONE_REVIEW}\nReview them as a change (builds review <worktree> [<base>] in the branch's worktree), settle every finding, and land them once no confirmed finding is open.\n",
                shas(needs)
            ));
        }
        Verdict::Unfinished(covered, unfinished) => (covered, unfinished),
    };
    let mut text = format!(
        "review landing {} ({}), last reviewed by run {}, does not take these commits ({}) yet:\n",
        covered.landing.id,
        covered.landing.key,
        covered.run,
        shas(needs)
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
            "{ONE_REVIEW} Run builds review in the branch's worktree: it reads those changes, fix commits included, and closes each open confirmed finding they fix. A rebase that keeps every line the branch adds and removes leaves nothing to read.\n"
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
            "The judge settles each, or the coordinator when no judge listens: builds review settle <finding> confirmed|dropped --why TEXT\n",
        );
    }
    if !unfinished.open.is_empty() {
        let count = unfinished.open.len();
        text.push_str(&format!(
            "{} open:\n",
            if count == 1 {
                "1 confirmed finding is".to_string()
            } else {
                format!("{count} confirmed findings are")
            }
        ));
        for finding in &unfinished.open {
            text.push_str(&render::finding_text(finding));
        }
        text.push_str(
            "Fix each in a new commit and run builds review in the branch's worktree: the review closes each open finding the changes fix. When a finding is wrong, settle it again: builds review settle <finding> --again --why TEXT\n",
        );
    }
    Some(text)
}

/// What keeps the commits of <verdict> from landing, one clause each, naming
/// who acts: empty when nothing blocks them. `builds health` says these of an
/// idle worktree, from the same records `refusal` reads.
pub fn blockers(verdict: &Verdict) -> Vec<String> {
    let unfinished = match verdict {
        Verdict::NotNeeded | Verdict::Takes(_) => return Vec::new(),
        Verdict::NoRun => {
            return vec![
                "no review of the branch is open: its agent must run builds review".to_string(),
            ];
        }
        Verdict::Unfinished(_, unfinished) => unfinished,
    };
    let mut found = Vec::new();
    for group in &unfinished.unanswered {
        found.push(format!(
            "group {group} gave no answer over a diff the review read: its agent must run builds review"
        ));
    }
    if !unfinished.unreviewed.is_empty() {
        found.push(format!(
            "no review has read the changes to {}: its agent must run builds review",
            unfinished.unreviewed.join(", ")
        ));
    }
    for finding in &unfinished.unsettled {
        found.push(format!(
            "finding {} waits for the judge to settle it (the coordinator when no judge listens)",
            finding.id
        ));
    }
    for finding in &unfinished.open {
        found.push(format!(
            "finding {} is confirmed and open: its agent must fix it, or the person must allow it",
            finding.id
        ));
    }
    found
}

/// What both commands read: the store with its database open for reading,
/// the change's repo and key, and the commits with the groups each needs.
struct Asked {
    store: Store,
    conn: crate::db::Held,
    repo: String,
    key: String,
    dir: PathBuf,
    needs: Vec<Need>,
    root: PathBuf,
    material: Material,
}

impl Asked {
    fn change(&self) -> Change<'_> {
        Change {
            repo: &self.repo,
            key: &self.key,
        }
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
    let key = match args.value("landing")? {
        Some(key) => key.to_string(),
        None => super::default_key(&checkout.top)?,
    };
    let root = super::rules_root(args)?;
    let material = Material::load(&root, project)?;
    let needs = needs(&checkout.top, shas.to_vec(), |paths| {
        material.group_names(paths)
    })?;
    Ok(Asked {
        store,
        conn,
        repo: checkout.repo(),
        key,
        dir: checkout.top.clone(),
        needs,
        root,
        material,
    })
}

/// `builds review check-landing DIR --project P SHA...`: says why the commits
/// can't land, exit 1, or that the review takes them.
pub fn check_landing_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["landing", "rules-root"], &[])?;
    let asked = ask(tool, &args)?;
    let verdict = judge(
        &asked.conn,
        asked.change(),
        &asked.dir,
        &asked.needs,
        CodeCheck::Branch("HEAD"),
        &asked.material,
    )?;
    match refusal(&asked.needs, &verdict) {
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
            "review landing {} ({}), last reviewed by run {}, takes these commits: nothing changed since the last review, and no confirmed finding is open",
            covered.landing.id, covered.landing.key, covered.run
        ),
        _ => "no group's rules cover the changed paths: the landing needs no review".to_string(),
    }
}

/// Records the change of <asked> as landed with its last review, unless
/// <more>: commits of its branch still wait to land. Returns the landing when
/// this recorded it as landed; None when it needed no review, the change has
/// no open landing (an earlier run of landed recorded it already), or it has
/// more to land.
fn record_landed(asked: &Asked, stamp: &Stamp, more: bool) -> Result<Option<i64>> {
    // Before the push, which moved origin/main, check-landing read the
    // branch, so this reads only the records.
    let verdict = judge(
        &asked.conn,
        asked.change(),
        &asked.dir,
        &asked.needs,
        CodeCheck::Records,
        &asked.material,
    )?;
    let covered = match &verdict {
        Verdict::NotNeeded => return Ok(None),
        Verdict::NoRun => {
            // A rerun after review landed recorded the landing finds no open
            // review, and so does a key other than the one check-landing
            // judged.
            crate::text::note(format!(
                "review landed: no review of {} in {} is open, so builds records no landing",
                asked.key, asked.repo
            ));
            return Ok(None);
        }
        Verdict::Takes(covered) => covered,
        Verdict::Unfinished(..) => {
            let why = refusal(&asked.needs, &verdict)
                .expect("a verdict that does not take the commits says why");
            fail!("the review does not take these commits:\n{why}")
        }
    };
    if more {
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
/// the landing as landed unless commits of the branch still wait to land,
/// then starts the audit of a sampled one, never of a landing of design files
/// only. The audit never fails the landing.
pub fn landed_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["landing", "rules-root"], &["more-to-land"])?;
    let Some((asked, landing)) = record_landing(tool, &args)? else {
        return Ok(());
    };
    let roll = (super::clock_bits() % 1_000_000) as f64 / 1_000_000.0;
    if let Err(why) = start_audit(tool, &asked, landing, roll) {
        eprintln!(
            "builds: the audit of review landing {landing} was not started: {}",
            why.text()
        );
    }
    Ok(())
}

/// Does the landing hold design files only? An audit of one would run
/// reviewers over every board it changes, so none is started.
fn holds_design_only(tool: &Tool, asked: &Asked) -> Result<bool> {
    let shas: Vec<String> = asked.needs.iter().map(|need| need.sha.clone()).collect();
    crate::lists::change_is_design_only(
        tool,
        &asked.store.project,
        &asked.dir.display().to_string(),
        &shas,
    )
}

/// Starts `builds review audit` on <landing> in the background when the
/// sample picks it.
fn start_audit(tool: &Tool, asked: &Asked, landing: i64, roll: f64) -> Result<()> {
    if holds_design_only(tool, asked)? {
        return Ok(());
    }
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
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(|| "starting the audit".to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;
