//! `builds review case propose`: builds the test set of the review from the
//! landings in the store. For a sample of landed changes across groups, each
//! proposed case is the net change the landing's first review read. Its
//! expected findings are the review's confirmed findings whose settle gives a
//! reason of its own (not one of `GENERIC`), each of which a second model
//! confirms against its rule's text (`model_check.rs`) before the case joins
//! the set. Its clean lines are the added lines no reviewer flagged, labelled
//! `none`. Each confirmed candidate miss of an audit becomes a case over the
//! diff the audit's reviewer read, which the store keeps, expecting the miss.

use std::path::Path;

use crate::checkout::{Checkout, parallel_map};
use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::Store;
use crate::review::audit::{candidate_misses, diff_read};
use crate::review::cases::{self, Addition, SiteLabel, same_code, same_file};
use crate::review::check::{code_in, names_file, parse_diff, squash};
use crate::review::files;
use crate::review::model_check;
use crate::review::record::{self, Finding, RunRow};
use crate::review::render;
use crate::review::rerun::net_change;
use crate::review::run::{self, Diff, Material};

/// The settle reasons that say only that a finding is real as it says, with
/// no reason of its own. The coordinator confirmed bae's findings with these
/// in bulk, so they do not show that anyone judged the finding. They are the
/// confirmed reasons of bae's store that restate the finding ("as the finding
/// says", "real in the commit its run read"), found by counting the store's
/// settle reasons by text on 2026-11-17: together they settled 1,667
/// findings, and every other reason settled at most 57.
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
    pub reviewer: &'a Path,
    /// The model of the check.
    pub model: &'a str,
    /// How many landings with a finding of each group the set takes.
    pub per_group: usize,
}

pub struct Proposed {
    pub text: String,
    /// Whether a check gave no answer, so its case was not written.
    pub failed: bool,
}

/// A landed change whose first review raised findings settled with a reason
/// of their own.
struct Candidate {
    landing: i64,
    first: RunRow,
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

/// The landed changes of <repo> in <db> that have a specific finding, newest
/// first.
fn candidates(db: &rusqlite::Connection, repo: &str) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
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
        if !specific.is_empty() {
            out.push(Candidate {
                landing: landing.id,
                first: first.clone(),
                specific,
                findings,
            });
        }
    }
    Ok(out)
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

/// The path of the file <finding> names, as <patch> names it; None when the
/// finding's code is not in <diff>.
fn path_in(diff: &Diff, patch: &str, finding: &Finding) -> Option<String> {
    code_in(&diff.quotes(), &finding.file, &finding.code).ok()?;
    if finding.file == files::COMMIT_MESSAGE {
        return Some(finding.file.clone());
    }
    parse_diff(patch)
        .into_iter()
        .map(|f| f.path)
        .find(|path| names_file(path, &finding.file))
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
/// the check in <inputs>' root. A landing that has a case already is left as
/// it is, and its findings are not checked again.
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
    let all = candidates(&conn, &repo)?;
    let mut text = String::new();
    let mut failed = false;
    let mut cost = 0;
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
        let Some((patch, diff)) = change_of(
            inputs.repo_dir,
            &candidate.first,
            &mut text,
            candidate.landing,
        )?
        else {
            continue;
        };
        let asked: Vec<(&Finding, String)> = candidate
            .specific
            .iter()
            .filter_map(|f| path_in(&diff, &patch, f).map(|path| (f, path)))
            .collect();
        let checked = parallel_map(&asked, |(finding, _)| {
            model_check::check(inputs.reviewer, inputs.model, &material, finding, &diff)
        });
        let mut additions: Vec<Addition> = Vec::new();
        let mut case_failed = false;
        text.push_str(&format!("landing {}:\n", candidate.landing));
        for ((finding, path), checked) in asked.iter().zip(&checked) {
            cost += checked.usage.cost_microdollars;
            let head = format!(
                "  finding {} ({} in {})",
                finding.id, finding.rule, finding.file
            );
            match &checked.verdict {
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
            text.push_str("  not written, since a check gave no answer\n");
            continue;
        }
        if additions.is_empty() {
            text.push_str("  no finding passed the check, so the landing gives no case\n");
            continue;
        }
        let expected = additions.len() / 2;
        let clean = clean_lines(&diff, &candidate.findings);
        let lines = clean.len();
        additions.extend(clean.into_iter().map(Addition::Site));
        cases::add(inputs.root, &name, inputs.project, &patch, &additions)?;
        text.push_str(&format!(
            "  case {name}: {}, {}\n",
            render::plural(expected, "expected finding"),
            render::plural(lines, "clean line")
        ));
    }
    text.push_str(&misses(inputs, &conn)?);
    text.push_str(&format!(
        "the checks cost ${:.2}; commit the cases in {}\n",
        cost as f64 / 1_000_000.0,
        inputs.root.join(cases::DIR).display()
    ));
    Ok(Proposed { text, failed })
}

/// The patch and diff of <run>'s net change in <repo_dir>, or None with why
/// in <text> when the repo lacks its commits.
fn change_of(
    repo_dir: &Path,
    run: &RunRow,
    text: &mut String,
    landing: i64,
) -> Result<Option<(String, Diff)>> {
    let (base, head) = match net_change(repo_dir, run) {
        Ok(change) => change,
        Err(why) => {
            text.push_str(&format!("landing {landing}: no case: {}\n", why.text()));
            return Ok(None);
        }
    };
    let patch = cases::patch_of(repo_dir, &base, &head)?;
    let diff = run::diff_between(repo_dir, &base, &head)?;
    Ok(Some((patch, diff)))
}

/// Writes a case for each confirmed candidate miss, over the diff the audit
/// reviewer that raised it read, which the store keeps. Returns what to
/// print.
fn misses(inputs: &Inputs, conn: &rusqlite::Connection) -> Result<String> {
    let mut text = String::new();
    for landing in record::audited_landings(conn)? {
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
            let diff = match diff_read(conn, first) {
                Ok(diff) => diff,
                Err(why) => {
                    text.push_str(&format!(
                        "landing {landing}: no case of run {}: {why}\n",
                        run.id
                    ));
                    continue;
                }
            };
            let mut found: Vec<(&Finding, String)> = Vec::new();
            for miss in &of_run {
                match path_in(&diff, &diff.text, miss) {
                    Some(path) => found.push((miss, path)),
                    None => text.push_str(&format!(
                        "landing {landing}: miss {} quotes code that is not in the diff\n",
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
            additions.extend(
                clean_lines(&diff, &findings)
                    .into_iter()
                    .map(Addition::Site),
            );
            let name = case_name(landing, run);
            cases::add(inputs.root, &name, inputs.project, &diff.text, &additions)?;
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
    let model = args.value("model")?.unwrap_or(model_check::MODEL);
    if !["opus", "sonnet", "haiku"].contains(&model) {
        fail!("--model is opus, sonnet or haiku, not {model}");
    }
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
        reviewer: Path::new(args.value("reviewer")?.unwrap_or("claude")),
        model,
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
