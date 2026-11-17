//! The test set of the review: `review/cases/<name>/` holds `change.patch`
//! (`git format-patch` output) and `expected.toml`, which names the findings
//! the change must produce (rule and file) and the rules that must raise no
//! blocking finding on it. `replay` runs a grouping over every case.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::review::check::{parse_diff, squash};
use crate::review::files::{self, Source};
use crate::review::record::{Kind, NewCall, NewRun};
use crate::review::run::{Diff, Material, Tokens, ask_groups};

pub const DIR: &str = "review/cases";
const PATCH: &str = "change.patch";
const EXPECTED: &str = "expected.toml";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub rule: String,
    /// The file the finding is in, as the patch names it.
    pub file: String,
}

/// The true verdict of one site a reviewer lists: a line of code (as much of
/// it as identifies it) in a file, and whether it breaks a rule.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SiteLabel {
    pub file: String,
    /// The start of the site's code, enough to tell it from its neighbours.
    pub code: String,
    /// `finding` or `none`.
    pub verdict: String,
    /// The rule a finding breaks; a `none` site names none and breaks none of
    /// its group's rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFile {
    project: String,
    /// Rules that must raise no blocking finding on this change.
    #[serde(default)]
    clean_for: Vec<String>,
    #[serde(default)]
    finding: Vec<Expect>,
    /// The true verdict of each site reviewers listed on this change.
    #[serde(default)]
    site: Vec<SiteLabel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub project: String,
    pub patch: String,
    pub clean_for: Vec<String>,
    pub expected: Vec<Expect>,
    pub labels: Vec<SiteLabel>,
}

/// Every case under <root>/review/cases, by name.
pub fn load(root: &Path) -> Result<Vec<Case>> {
    let dir = root.join(DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut cases = Vec::new();
    for name in names {
        let case_dir = dir.join(&name);
        let patch_path = case_dir.join(PATCH);
        let patch = std::fs::read_to_string(&patch_path)
            .context(|| format!("reading {}", patch_path.display()))?;
        let expected_path = case_dir.join(EXPECTED);
        let text = std::fs::read_to_string(&expected_path)
            .context(|| format!("reading {}", expected_path.display()))?;
        let file: ExpectedFile = toml::from_str(&text).map_err(|error| {
            crate::fail::Fail::Message(format!("{}: {error}", expected_path.display()))
        })?;
        if file.finding.is_empty() && file.clean_for.is_empty() {
            fail!(
                "{} expects nothing: name a finding, or the rules that must stay quiet (clean_for)",
                expected_path.display()
            );
        }
        let paths: Vec<String> = parse_diff(&patch).into_iter().map(|f| f.path).collect();
        for expect in &file.finding {
            if !paths.iter().any(|p| p == &expect.file) {
                fail!(
                    "{} expects a finding in {}, which the patch does not change",
                    expected_path.display(),
                    expect.file
                );
            }
        }
        cases.push(Case {
            name,
            project: file.project,
            patch,
            clean_for: file.clean_for,
            expected: file.finding,
            labels: file.site,
        });
    }
    Ok(cases)
}

/// What a case gains.
pub enum Addition<'a> {
    /// A finding the change must produce.
    Finding { rule: &'a str, file: &'a str },
    /// A rule that must raise no blocking finding on the change.
    Clean { rule: &'a str },
}

/// Adds <addition> to the case <name> of <root>, making it from <patch> when
/// it is new; an existing case must hold the same patch.
pub fn add(
    root: &Path,
    name: &str,
    project: &str,
    patch: &str,
    addition: Addition,
) -> Result<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        fail!("'{name}' is not a case name: it is a folder name");
    }
    if let Addition::Finding { file, .. } = addition
        && !parse_diff(patch).iter().any(|f| f.path == file)
    {
        fail!("the patch does not change {file}");
    }
    let dir = root.join(DIR).join(name);
    let patch_path = dir.join(PATCH);
    let expected_path = dir.join(EXPECTED);
    let mut expected = if expected_path.exists() {
        let held = std::fs::read_to_string(&patch_path)
            .context(|| format!("reading {}", patch_path.display()))?;
        if held != patch {
            fail!("case {name} holds another change; name this one differently with --name");
        }
        let text = std::fs::read_to_string(&expected_path)
            .context(|| format!("reading {}", expected_path.display()))?;
        toml::from_str::<ExpectedFile>(&text).map_err(|error| {
            crate::fail::Fail::Message(format!("{}: {error}", expected_path.display()))
        })?
    } else {
        std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&patch_path, patch)
            .context(|| format!("writing {}", patch_path.display()))?;
        ExpectedFile {
            project: project.to_string(),
            clean_for: Vec::new(),
            finding: Vec::new(),
            site: Vec::new(),
        }
    };
    match addition {
        Addition::Finding { rule, file } => {
            let new = Expect {
                rule: rule.to_string(),
                file: file.to_string(),
            };
            if !expected.finding.contains(&new) {
                expected.finding.push(new);
            }
        }
        Addition::Clean { rule } => {
            if !expected.clean_for.iter().any(|r| r == rule) {
                expected.clean_for.push(rule.to_string());
            }
        }
    }
    let text = toml::to_string(&expected).expect("the expectations serialize");
    std::fs::write(&expected_path, text)
        .context(|| format!("writing {}", expected_path.display()))?;
    Ok(dir)
}

/// What a grouping did on one case.
#[derive(Clone, Debug)]
pub struct CaseResult {
    pub name: String,
    pub expected: usize,
    /// Expected findings some group raised, in the right rule and file.
    pub found: usize,
    pub missed: Vec<Expect>,
    /// Blocking findings on a rule the case must keep quiet.
    pub false_blocking: usize,
    pub calls: Vec<NewCall>,
    /// The tokens of the passes after each group's first.
    pub repeat: Tokens,
    /// How the case's site labels scored, by rule.
    pub sites: Vec<SiteScore>,
}

/// How reviewers did on the labelled sites of one rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteScore {
    pub group: String,
    pub rule: String,
    /// Labelled finding sites some accepted finding of the rule names.
    pub found: usize,
    /// Labelled finding sites no accepted finding names.
    pub missed: usize,
    /// Accepted findings that land on a site labelled `none`.
    pub wrong: usize,
    /// Accepted findings that land on no labelled site.
    pub unlabelled: usize,
}

/// Whether two quotes of code (a reviewer's finding and a label, or two
/// findings) name one site: one holds the other, once whitespace and markdown
/// marks are gone.
pub(super) fn same_code(a: &str, b: &str) -> bool {
    let (a, b) = (squash(a), squash(b));
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

/// The place of <rule>'s score in <scores>, added when it has none.
fn score_index(scores: &mut Vec<SiteScore>, material: &Material, rule: &str) -> usize {
    if let Some(i) = scores.iter().position(|s| s.rule == rule) {
        return i;
    }
    let group = material
        .groups
        .groups
        .iter()
        .find(|g| g.rules.iter().any(|r| r == rule))
        .map(|g| g.name.clone())
        .unwrap_or_default();
    scores.push(SiteScore {
        group,
        rule: rule.to_string(),
        ..SiteScore::default()
    });
    scores.len() - 1
}

fn score_sites(
    material: &Material,
    labels: &[SiteLabel],
    accepted: &[&crate::review::record::NewFinding],
) -> Vec<SiteScore> {
    let mut scores: Vec<SiteScore> = Vec::new();
    for label in labels.iter().filter(|l| l.verdict == "finding") {
        let rule = label.rule.as_deref().unwrap_or_default();
        let hit = accepted.iter().any(|f| {
            f.rule == rule && same_file(&f.file, &label.file) && same_code(&f.code, &label.code)
        });
        let i = score_index(&mut scores, material, rule);
        if hit {
            scores[i].found += 1;
        } else {
            scores[i].missed += 1;
        }
    }
    for f in accepted {
        let on =
            |label: &&SiteLabel| same_file(&f.file, &label.file) && same_code(&f.code, &label.code);
        let right = labels
            .iter()
            .filter(on)
            .any(|l| l.verdict == "finding" && l.rule.as_deref() == Some(f.rule.as_str()));
        if right {
            continue;
        }
        let i = score_index(&mut scores, material, &f.rule);
        if labels.iter().filter(on).any(|l| l.verdict == "none") {
            scores[i].wrong += 1;
        } else {
            scores[i].unlabelled += 1;
        }
    }
    scores
}

impl CaseResult {
    pub fn tokens(&self) -> (i64, i64) {
        (
            self.calls.iter().map(|c| c.tokens_in).sum(),
            self.calls.iter().map(|c| c.tokens_out).sum(),
        )
    }
}

/// Runs <material>'s groups over <case> the way a landing's review does.
pub fn replay_case(
    material: &Material,
    reviewer: &Path,
    case: &Case,
    source_repo: Option<&Path>,
) -> CaseResult {
    let diff = Diff {
        messages: messages_of_patch(&case.patch),
        source: source_repo.and_then(|repo| {
            Some(Source {
                repo: repo.to_path_buf(),
                rev: commit_of(&case.patch)?,
                in_place: false,
            })
        }),
        base: String::new(),
        head: String::new(),
        text: case.patch.clone(),
        changed: files::paths(&case.patch),
    };
    let picked = material.groups.pick(&material.rules, &diff.changed);
    let asked = ask_groups(material, reviewer, &picked, &diff, None, &[]);
    let (calls, repeat) = (asked.calls, asked.repeat);
    let accepted: Vec<&crate::review::record::NewFinding> = calls
        .iter()
        .flat_map(|c| c.findings.iter())
        .filter(|f| f.check_failed.is_none())
        .collect();
    let mut missed = Vec::new();
    for expect in &case.expected {
        let hit = accepted
            .iter()
            .any(|f| f.rule == expect.rule && same_file(&f.file, &expect.file));
        if !hit {
            missed.push(expect.clone());
        }
    }
    let false_blocking = accepted
        .iter()
        .filter(|f| f.blocking && case.clean_for.contains(&f.rule))
        .count();
    CaseResult {
        name: case.name.clone(),
        expected: case.expected.len(),
        found: case.expected.len() - missed.len(),
        missed,
        false_blocking,
        sites: score_sites(material, &case.labels, &accepted),
        calls,
        repeat,
    }
}

/// The commit messages of a `git format-patch` file: for each mail, the
/// subject (without its `[PATCH]` mark) and the body up to the `---` line.
pub fn messages_of_patch(patch: &str) -> String {
    let mut out = String::new();
    let mut in_message = false;
    let mut in_header = false;
    for line in patch.lines() {
        let starts_mail = line.starts_with("From ")
            && line
                .split_whitespace()
                .nth(1)
                .is_some_and(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()));
        if starts_mail {
            in_header = true;
            in_message = false;
            continue;
        }
        if in_header {
            if let Some(subject) = line.strip_prefix("Subject: ") {
                let subject = subject.strip_prefix("[PATCH] ").unwrap_or(subject);
                out.push_str(subject);
                out.push('\n');
                in_message = true;
            } else if line.is_empty() {
                in_header = false;
                in_message = true;
                out.push('\n');
            }
            continue;
        }
        if in_message {
            if line == "---" {
                in_message = false;
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// The commit a `git format-patch` file was made from, named on its first line.
fn commit_of(patch: &str) -> Option<String> {
    let rest = patch.strip_prefix("From ")?;
    let sha = rest.split_whitespace().next()?;
    (sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).then(|| sha.to_string())
}

/// Whether two paths name one file, one possibly shortened from the left.
pub(super) fn same_file(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// Every case of <cases> replayed, in order. <alternative> stands in for the
/// every group is asked in <model> when given. A reviewer of one file at a time reads
/// that file's new text from <source_repo> at the commit the case's patch was
/// made from.
pub fn replay_all(
    root: &Path,
    cases: &[Case],
    reviewer: &Path,
    source_repo: Option<&Path>,
    model: Option<&str>,
) -> Result<Vec<CaseResult>> {
    let mut materials: Vec<(String, Material)> = Vec::new();
    let mut results = Vec::new();
    for case in cases {
        if !materials
            .iter()
            .any(|(project, _)| *project == case.project)
        {
            let mut material = Material::load(root, &case.project)?;
            if let Some(model) = model {
                material = material.with_model(model);
            }
            materials.push((case.project.clone(), material));
        }
        let (_, material) = materials
            .iter()
            .find(|(project, _)| *project == case.project)
            .expect("loaded above");
        results.push(replay_case(material, reviewer, case, source_repo));
    }
    Ok(results)
}

/// The run of a replayed case, as the store records it.
pub fn run_of<'a>(case: &'a Case, at: i64, rules_version: &'a str) -> NewRun<'a> {
    NewRun {
        landing: None,
        kind: Kind::Replay,
        label: Some(&case.name),
        at,
        base: "",
        head: "",
        since: None,
        grouping: "",
        rules_version,
    }
}

/// The numbers of a replay: recall, false blocking findings and tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Totals {
    pub expected: usize,
    pub found: usize,
    pub false_blocking: usize,
    pub failed_calls: usize,
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// Of tokens_in, the part read from the prompt cache and the part written to it.
    pub tokens_cached: i64,
    pub tokens_written: i64,
    /// Of those, what the passes after each group's first used.
    pub repeat: Tokens,
    /// What `claude -p` reported the calls cost, in millionths of a US dollar.
    pub cost_microdollars: i64,
}

pub fn totals(results: &[CaseResult]) -> Totals {
    let mut t = Totals {
        expected: 0,
        found: 0,
        false_blocking: 0,
        failed_calls: 0,
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        tokens_written: 0,
        repeat: Tokens::default(),
        cost_microdollars: 0,
    };
    for r in results {
        t.expected += r.expected;
        t.found += r.found;
        t.false_blocking += r.false_blocking;
        t.failed_calls += r.calls.iter().filter(|c| c.failure.is_some()).count();
        let (i, o) = r.tokens();
        t.tokens_in += i;
        t.tokens_out += o;
        t.tokens_cached += r.calls.iter().map(|c| c.tokens_cached).sum::<i64>();
        t.tokens_written += r.calls.iter().map(|c| c.tokens_written).sum::<i64>();
        t.cost_microdollars += r.calls.iter().map(|c| c.cost_microdollars).sum::<i64>();
        t.repeat.input += r.repeat.input;
        t.repeat.cached += r.repeat.cached;
        t.repeat.written += r.repeat.written;
        t.repeat.output += r.repeat.output;
    }
    t
}

/// The site labels' scores, per rule and per group: recall (labelled finding
/// sites found), precision (found over found plus findings on `none` sites)
/// and the findings on unlabelled sites.
fn site_report(results: &[CaseResult]) -> String {
    let mut by_rule: Vec<SiteScore> = Vec::new();
    for score in results.iter().flat_map(|r| r.sites.iter()) {
        match by_rule.iter_mut().find(|s| s.rule == score.rule) {
            Some(held) => {
                held.found += score.found;
                held.missed += score.missed;
                held.wrong += score.wrong;
                held.unlabelled += score.unlabelled;
            }
            None => by_rule.push(score.clone()),
        }
    }
    if by_rule.is_empty() {
        return String::new();
    }
    by_rule.sort_by(|a, b| (&a.group, &a.rule).cmp(&(&b.group, &b.rule)));
    let ratio = |n: usize, d: usize| match d {
        0 => "n/a".to_string(),
        d => format!("{}%", n * 100 / d),
    };
    let line = |name: &str, s: &SiteScore| {
        format!(
            "  {name}: sites found {} of {} (recall {}), {} on `none` sites (precision {}), {} on unlabelled sites\n",
            s.found,
            s.found + s.missed,
            ratio(s.found, s.found + s.missed),
            s.wrong,
            ratio(s.found, s.found + s.wrong),
            s.unlabelled
        )
    };
    let mut out = String::from("site labels, per group and per rule:\n");
    let mut groups: Vec<&str> = by_rule.iter().map(|s| s.group.as_str()).collect();
    groups.dedup();
    for group in groups {
        let mut total = SiteScore::default();
        for s in by_rule.iter().filter(|s| s.group == group) {
            total.found += s.found;
            total.missed += s.missed;
            total.wrong += s.wrong;
            total.unlabelled += s.unlabelled;
        }
        out.push_str(&line(&format!("group {group}"), &total));
        for s in by_rule.iter().filter(|s| s.group == group) {
            out.push_str(&line(&format!("  rule {}", s.rule), s));
        }
    }
    out
}

pub fn report(results: &[CaseResult]) -> String {
    let mut out = String::new();
    for r in results {
        out.push_str(&format!(
            "{}: found {} of {} expected, {} false blocking, {} tokens in, {} out\n",
            r.name,
            r.found,
            r.expected,
            r.false_blocking,
            r.tokens().0,
            r.tokens().1
        ));
        for miss in &r.missed {
            out.push_str(&format!("  missed {} in {}\n", miss.rule, miss.file));
        }
        for call in r.calls.iter().filter(|c| c.failure.is_some()) {
            out.push_str(&format!(
                "  group {} failed: {}\n",
                call.group,
                call.failure.as_deref().unwrap_or("")
            ));
        }
    }
    let t = totals(results);
    let percent = (t.found * 100).checked_div(t.expected).unwrap_or(100);
    out.push_str(&format!(
        "recall {} of {} expected findings ({percent}%), {} false blocking findings on clean cases, {} failed calls, {} tokens in ({} uncached, {} read from the cache, {} written to it), {} out, ${:.2}\n",
        t.found,
        t.expected,
        t.false_blocking,
        t.failed_calls,
        t.tokens_in,
        t.tokens_in - t.tokens_cached - t.tokens_written,
        t.tokens_cached,
        t.tokens_written,
        t.tokens_out,
        t.cost_microdollars as f64 / 1_000_000.0
    ));
    if t.repeat != Tokens::default() {
        out.push_str(&format!(
            "repeat passes: {} tokens in ({} uncached, {} read from the cache, {} written to it), {} out\n",
            t.repeat.input,
            t.repeat.input - t.repeat.cached - t.repeat.written,
            t.repeat.cached,
            t.repeat.written,
            t.repeat.output
        ));
    }
    out.push_str(&site_report(results));
    out
}

#[cfg(test)]
#[path = "cases_tests.rs"]
mod tests;
