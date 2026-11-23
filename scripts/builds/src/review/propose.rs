//! `builds review case propose`: builds the test set of the review from the
//! landings in the store. For a sample of landed changes across groups, each
//! proposed case is the net change the landing's first review read. Its
//! expected findings are the findings the coordinator confirmed with a reason
//! of its own (not one of `GENERIC`), each of which a second model
//! confirms against its rule's text (`model_check.rs`) before the case joins
//! the set. Its clean lines are the added lines no reviewer flagged, labelled
//! `none`. Each confirmed candidate miss of an audit becomes a case over the
//! diff the audit's reviewer read, which the store keeps, expecting the miss.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::review::audit::{candidate_misses, diff_read};
use crate::review::cases::{self, Addition, SiteLabel, same_code, same_file};
use crate::review::check::reviewers_map;
use crate::review::check::{quote_file, squash};
use crate::review::files;
use crate::review::model_check;
use crate::review::record::{self, Finding, RunRow};
use crate::review::rerun::net_change;
use crate::review::run::{self, Diff, Material};
use crate::text::plural;

/// The settle reasons that say only that a finding is real as it says, with
/// no reason of its own. The coordinator confirmed bae's findings with these
/// in bulk, so they do not show that anyone judged the finding. They are the
/// confirmed reasons of bae's store that restate the finding ("as the finding
/// says", "real in the commit its run read"). The agent that built the test
/// set found them on 2026-11-17 by counting the store's settle reasons by
/// text: together they settled 1,667 findings, and every other reason settled
/// at most 57.
pub const GENERIC: [&str; 11] = [
    "real as the finding says",
    "the sentence breaks the writing rule as the finding says",
    "the code or sentence breaks the quoted clause as the finding says",
    "real as the finding says; read against the head",
    "real as the finding says; a later commit on the branch may already fix it",
    "the sentence breaks the cited writing rule as the finding says",
    "real in the commit its run read; a later commit on the branch may already fix it",
    "real in the commit its run read, as the finding says",
    "real in the commit run 143 read; later commits on the branch may already fix it",
    "the text breaks the words rule as the finding says",
    "real in the commit run 160 read; a later commit on the branch rewrote the line",
];

/// The fewest characters (once whitespace and markdown marks are gone) an
/// added line needs to be a clean line: a shorter line, such as `}`, is in
/// the code of too many findings to name one site.
const SHORTEST_CLEAN: usize = 24;

/// How many landings with a finding of each group the set takes unless
/// `--per-group` says otherwise.
const PER_GROUP: usize = 2;

pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    /// A checkout of the repo that holds the landings' commits.
    pub repo_dir: &'a Path,
    /// The agent-instructions worktree whose rules the check reads and whose
    /// review/cases folder takes the cases.
    pub root: &'a Path,
    pub reviewer: &'a crate::review::Program,
    /// The model of the check.
    pub model: &'a super::provider::Selection,
    /// How many landings with a finding of each group the set takes.
    pub per_group: usize,
}

pub struct Proposed {
    pub text: String,
    /// Whether a check gave no answer, so propose wrote no case for its
    /// landing.
    pub failed: bool,
}

/// A landed change whose first review raised findings settled with a reason
/// of their own.
struct Candidate {
    landing: i64,
    first: RunRow,
    /// The base and head of the first review's net change, which the repo
    /// holds.
    base: String,
    head: String,
    /// The confirmed findings of the first review's diff, settled with a
    /// reason of their own.
    specific: Vec<Finding>,
    /// Every finding of the landing.
    findings: Vec<Finding>,
}

impl Candidate {
    fn name(&self) -> String {
        case_name(self.landing, &self.first)
    }
}

fn case_name(landing: i64, run: &RunRow) -> String {
    format!("landing-{landing}-run-{}", run.id)
}

/// Whether the coordinator confirmed <finding> with a reason of its own.
fn specific(finding: &Finding) -> bool {
    finding.settled.as_ref().is_some_and(|settled| {
        let last = settled.last();
        last.how == "confirmed" && !GENERIC.contains(&last.why.trim())
    })
}

/// The landed changes of <repo> in <db> that have a specific finding and
/// whose first review's commits <repo_dir> still holds, newest first, and a
/// line for each landed change with a specific finding whose commits the repo
/// lost, which gives no case.
fn candidates(
    db: &rusqlite::Connection,
    repo: &str,
    repo_dir: &Path,
) -> Result<(Vec<Candidate>, String)> {
    let mut out = Vec::new();
    let mut lost = String::new();
    for landing in record::all_landings(db)?.into_iter().rev() {
        if landing.repo != repo || landing.landed_run.is_none() {
            continue;
        }
        let runs = record::runs_of_landing(db, landing.id)?;
        let Some(first) = runs
            .iter()
            .find(|r| r.kind == "review" && r.since.is_none())
        else {
            continue;
        };
        let over: Vec<i64> = runs
            .iter()
            .filter(|r| r.kind == "review" && r.same_diff(first))
            .map(|r| r.id)
            .collect();
        let findings = record::findings_of_landing(db, landing.id)?;
        let specific: Vec<Finding> = findings
            .iter()
            .filter(|f| f.kind == "review" && f.counts() && over.contains(&f.run_id))
            .filter(|f| specific(f))
            .cloned()
            .collect();
        if specific.is_empty() {
            continue;
        }
        match net_change(repo_dir, first) {
            Ok((base, head)) => out.push(Candidate {
                landing: landing.id,
                first: first.clone(),
                base,
                head,
                specific,
                findings,
            }),
            Err(why) => lost.push_str(&format!(
                "landing {}: propose wrote no case, since {}\n",
                landing.id,
                why.text()
            )),
        }
    }
    Ok((out, lost))
}

/// For each group that raised a specific finding, in name order, the newest
/// candidates with a specific finding of that group until <per_group> picked
/// candidates have one. Returns indexes into <candidates>.
fn sample(candidates: &[Candidate], per_group: usize) -> Vec<usize> {
    let mut groups: Vec<&str> = candidates
        .iter()
        .flat_map(|c| c.specific.iter().map(|f| f.group.as_str()))
        .collect();
    groups.sort();
    groups.dedup();
    let mut picked: Vec<usize> = Vec::new();
    let raised = |c: &Candidate, group: &str| c.specific.iter().any(|f| f.group == group);
    for group in groups {
        for (i, candidate) in candidates.iter().enumerate() {
            let have = picked
                .iter()
                .filter(|&&p| raised(&candidates[p], group))
                .count();
            if have >= per_group {
                break;
            }
            if !picked.contains(&i) && raised(candidate, group) {
                picked.push(i);
            }
        }
    }
    picked.sort();
    picked
}

/// The added lines of <diff> that no finding of <findings> names, as `none`
/// sites: each line long enough to name one site and found once in its file.
fn clean_lines(diff: &Diff, findings: &[Finding]) -> Vec<SiteLabel> {
    let mut labels = Vec::new();
    for file in files::split(&diff.text) {
        let mut in_hunk = false;
        let mut added: Vec<&str> = Vec::new();
        for line in file.text.lines() {
            if line.starts_with("@@") {
                in_hunk = true;
            } else if in_hunk && let Some(code) = line.strip_prefix('+') {
                added.push(code.trim());
            }
        }
        for code in &added {
            let squashed = squash(code);
            if squashed.chars().count() < SHORTEST_CLEAN
                || added.iter().filter(|o| squash(o) == squashed).count() > 1
                || findings
                    .iter()
                    .any(|f| same_file(&f.file, &file.path) && same_code(&f.code, code))
            {
                continue;
            }
            labels.push(SiteLabel {
                file: file.path.clone(),
                code: code.to_string(),
                verdict: "none".to_string(),
                rule: None,
            });
        }
    }
    labels
}

/// The path of the file <finding> names, as <patch> names it; or why the
/// case cannot expect the finding: its code is not in <diff>, or <patch> has
/// no such file.
fn path_in(diff: &Diff, patch: &str, finding: &Finding) -> std::result::Result<String, String> {
    let quotes = diff.quotes();
    let expected = match &finding.line {
        record::FindingLine::Verified(site) => Some(site),
        record::FindingLine::Reported(_) | record::FindingLine::Legacy(_) => None,
    };
    let file = quote_file(&quotes, &finding.file, &finding.code, expected)?;
    if file == files::COMMIT_MESSAGE || files::paths(patch).iter().any(|path| path == file) {
        Ok(file.to_string())
    } else {
        Err(format!("the patch has no file {file}"))
    }
}

/// <patch> and <diff> cut to the files of <paths>, the files that hold a
/// case's expected findings, so a replay of the case sends each group only
/// those files. The commit messages stay.
fn cut(patch: &str, mut diff: Diff, paths: &[String]) -> (String, Diff) {
    diff.text = cases::keep_files(&diff.text, paths);
    diff.changed = files::paths(&diff.text);
    (cases::keep_files(patch, paths), diff)
}

/// The finding sites a case expects: the finding by rule and file, and its
/// code as a `finding` site.
fn expect<'a>(finding: &'a Finding, path: &'a str) -> [Addition<'a>; 2] {
    [
        Addition::Finding {
            rule: &finding.rule,
            file: path,
        },
        Addition::Site(SiteLabel {
            file: path.to_string(),
            code: finding.code.clone(),
            verdict: "finding".to_string(),
            rule: Some(finding.rule.clone()),
        }),
    ]
}

/// Proposes cases from the store's landings and writes each one that passes
/// the check in <inputs>' root. propose skips a landing that has a case
/// already and does not check its findings again.
pub fn propose(inputs: &Inputs) -> Result<Proposed> {
    let conn = inputs.store.open_existing()?;
    let material = Material::load(inputs.root, inputs.project)?;
    let Some(checkout) = Checkout::at(inputs.repo_dir)? else {
        fail!("{} is not inside a git checkout", inputs.repo_dir.display());
    };
    let repo = checkout.repo();
    let held: Vec<String> = cases::load(inputs.root)?
        .into_iter()
        .map(|c| c.name)
        .collect();
    let (all, lost) = candidates(&conn, &repo, inputs.repo_dir)?;
    let mut text = lost;
    let mut failed = false;
    let mut cost = Some(0);
    for i in sample(&all, inputs.per_group) {
        let candidate = &all[i];
        let name = candidate.name();
        if held.contains(&name) {
            text.push_str(&format!(
                "landing {} has a case already: {name}\n",
                candidate.landing
            ));
            continue;
        }
        let patch = cases::patch_of(inputs.repo_dir, &candidate.base, &candidate.head)?;
        let diff = run::diff_between(inputs.repo_dir, &candidate.base, &candidate.head)?.diff;
        text.push_str(&format!("landing {}:\n", candidate.landing));
        let mut asked: Vec<(&Finding, String)> = Vec::new();
        for finding in &candidate.specific {
            match path_in(&diff, &patch, finding) {
                Ok(path) => asked.push((finding, path)),
                Err(why) => text.push_str(&format!(
                    "  finding {} ({} in {}): the case leaves it out: {why}\n",
                    finding.id, finding.rule, finding.file
                )),
            }
        }
        let checked = reviewers_map(&asked, |(finding, _)| {
            model_check::check(inputs.reviewer, inputs.model, &material, finding, &diff)
        });
        let mut additions: Vec<Addition> = Vec::new();
        let mut case_failed = false;
        for ((finding, path), checked) in asked.iter().zip(&checked) {
            let verdict = match checked {
                Ok(checked) => {
                    cost = super::call::add_cost(cost, checked.usage.cost_microdollars);
                    checked.verdict.as_ref()
                }
                Err(why) => Err(why),
            };
            let head = format!(
                "  finding {} ({} in {})",
                finding.id, finding.rule, finding.file
            );
            match verdict {
                Ok((true, _)) => {
                    text.push_str(&format!("{head}: confirmed\n"));
                    additions.extend(expect(finding, path));
                }
                Ok((false, why)) => text.push_str(&format!("{head}: rejected: {why}\n")),
                Err(why) => {
                    case_failed = true;
                    text.push_str(&format!("{head}: the check gave no answer: {why}\n"));
                }
            }
        }
        if case_failed {
            failed = true;
            text.push_str("  propose wrote no case, since a check gave no answer\n");
            continue;
        }
        if additions.is_empty() {
            text.push_str("  no finding passed the check, so the landing gives no case\n");
            continue;
        }
        let expected = additions.len() / 2;
        let paths: Vec<String> = additions
            .iter()
            .filter_map(|addition| match addition {
                Addition::Finding { file, .. } => Some(file.to_string()),
                _ => None,
            })
            .collect();
        let (patch, diff) = cut(&patch, diff, &paths);
        let clean = clean_lines(&diff, &candidate.findings);
        let lines = clean.len();
        additions.extend(clean.into_iter().map(Addition::Site));
        cases::add(
            inputs.root,
            &name,
            inputs.project,
            &repo,
            &patch,
            &additions,
        )?;
        text.push_str(&format!(
            "  case {name}: {}, {}\n",
            plural(expected, "expected finding"),
            plural(lines, "clean line")
        ));
    }
    text.push_str(&misses(inputs, &conn)?);
    let cost = cost.map_or_else(
        || "The reviewers did not report a total cost".to_string(),
        |cost| format!("The checks cost {}", super::call::known_cost_text(cost)),
    );
    text.push_str(&format!(
        "{cost}. Commit the cases in {}.\n",
        inputs.root.join(cases::DIR).display()
    ));
    Ok(Proposed { text, failed })
}

/// Writes a case for each confirmed candidate miss, over the diff the audit
/// reviewer that raised it read, which the store keeps, in its landing's
/// repo. Returns what to print.
fn misses(inputs: &Inputs, conn: &rusqlite::Connection) -> Result<String> {
    let mut text = String::new();
    for landing in record::audited_landings(conn)? {
        let repo = record::landing(conn, landing)?.repo;
        let findings = record::findings_of_landing(conn, landing)?;
        let confirmed: Vec<&Finding> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled_as("confirmed"))
            .collect();
        let runs = record::runs_of_landing(conn, landing)?;
        for run in runs.iter().filter(|r| r.kind == "audit") {
            let of_run: Vec<&Finding> = confirmed
                .iter()
                .copied()
                .filter(|f| f.run_id == run.id)
                .collect();
            let Some(first) = of_run.first() else {
                continue;
            };
            let diff = match diff_read(conn, &inputs.store.texts, first) {
                Ok(diff) => diff,
                Err(why) => {
                    text.push_str(&format!(
                        "landing {landing}: propose wrote no case for run {}, since {why}\n",
                        run.id
                    ));
                    continue;
                }
            };
            let mut found: Vec<(&Finding, String)> = Vec::new();
            for miss in &of_run {
                match path_in(&diff, &diff.text, miss) {
                    Ok(path) => found.push((miss, path)),
                    Err(why) => text.push_str(&format!(
                        "landing {landing}: the case leaves out miss {}: {why}\n",
                        miss.id
                    )),
                }
            }
            let mut additions: Vec<Addition> = found
                .iter()
                .flat_map(|(miss, path)| expect(miss, path))
                .collect();
            if additions.is_empty() {
                continue;
            }
            let paths: Vec<String> = found.iter().map(|(_, path)| path.clone()).collect();
            let whole = diff.text.clone();
            let (text_cut, diff) = cut(&whole, diff, &paths);
            additions.extend(
                clean_lines(&diff, &findings)
                    .into_iter()
                    .map(Addition::Site),
            );
            let name = case_name(landing, run);
            cases::add(
                inputs.root,
                &name,
                inputs.project,
                &repo,
                &text_cut,
                &additions,
            )?;
            text.push_str(&format!(
                "landing {landing}: case {name} expects {}\n",
                crate::text::counted(found.len(), "confirmed miss", "confirmed misses")
            ));
        }
    }
    Ok(text)
}

/// `builds review case propose --repo-dir DIR --rules-root DIR [--per-group
/// N] [--model M]`: proposes cases from the store's landings, writes those
/// that pass the check, and exits 1 when a check gave no answer.
pub(super) fn propose_command(tool: &crate::tool::Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &["repo-dir", "rules-root", "per-group", "model", "reviewer"],
        &[],
    )?;
    args.exactly(0, super::USAGE)?;
    let Some(root) = args.value("rules-root")? else {
        fail!("name the agent-instructions worktree to write the cases in with --rules-root");
    };
    let model = model_check::selection(args.value("model")?);
    let per_group = match args.value("per-group")? {
        Some(n) => n
            .parse()
            .map_err(|_| crate::fail::Fail::Message(format!("'{n}' is not a number")))?,
        None => PER_GROUP,
    };
    let store = Store::resolve(&tool.places, &args, &current_dir()?)?;
    let proposed = propose(&Inputs {
        store: &store,
        project: &store.project,
        repo_dir: Path::new(args.require("repo-dir")?),
        root: Path::new(root),
        reviewer: &tool.reviewer.program_for(args.value("reviewer")?),
        model: &model,
        per_group,
    })?;
    print!("{}", proposed.text);
    if proposed.failed {
        return Err(crate::fail::Fail::Exit(1));
    }
    Ok(())
}

#[cfg(test)]
#[path = "propose_tests.rs"]
mod tests;
