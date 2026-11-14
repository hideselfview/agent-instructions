//! The test set of the review: `review/cases/<name>/` holds `change.patch`
//! (`git format-patch` output) and `expected.toml`, which names the findings
//! the change must produce (rule and file) and the rules that must raise no
//! blocking finding on it. `replay` runs a grouping over every case.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::review::check::parse_diff;
use crate::review::groups::Groups;
use crate::review::record::{Kind, NewCall, NewRun};
use crate::review::run::{Diff, Material, ask_groups};

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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFile {
    project: String,
    /// Rules that must raise no blocking finding on this change.
    #[serde(default)]
    clean_for: Vec<String>,
    #[serde(default)]
    finding: Vec<Expect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub project: String,
    pub patch: String,
    pub clean_for: Vec<String>,
    pub expected: Vec<Expect>,
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
pub fn replay_case(material: &Material, reviewer: &Path, case: &Case) -> CaseResult {
    let files = parse_diff(&case.patch);
    let diff = Diff {
        base: String::new(),
        head: String::new(),
        text: case.patch.clone(),
        changed: files.iter().map(|f| f.path.clone()).collect(),
    };
    let picked = material.groups.pick(&material.rules, &diff.changed);
    let calls = ask_groups(material, reviewer, &picked, &diff, None, &|_| Vec::new());
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
        calls,
    }
}

/// Whether two paths name one file, one possibly shortened from the left.
fn same_file(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// Every case of <cases> replayed, in order. <alternative> stands in for the
/// checkout's own groups when given.
pub fn replay_all(
    root: &Path,
    cases: &[Case],
    reviewer: &Path,
    alternative: Option<&Groups>,
) -> Result<Vec<CaseResult>> {
    let mut materials: Vec<(String, Material)> = Vec::new();
    let mut results = Vec::new();
    for case in cases {
        if !materials
            .iter()
            .any(|(project, _)| *project == case.project)
        {
            let mut material = Material::load(root, &case.project)?;
            if let Some(groups) = alternative {
                material = material.with_groups(groups.clone())?;
            }
            materials.push((case.project.clone(), material));
        }
        let (_, material) = materials
            .iter()
            .find(|(project, _)| *project == case.project)
            .expect("loaded above");
        results.push(replay_case(material, reviewer, case));
    }
    Ok(results)
}

/// The run of a replayed case, as the store records it.
pub fn run_of<'a>(case: &'a Case, at: i64) -> NewRun<'a> {
    NewRun {
        landing: None,
        kind: Kind::Replay,
        label: Some(&case.name),
        at,
        base: "",
        head: "",
        grouping: "",
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
}

pub fn totals(results: &[CaseResult]) -> Totals {
    let mut t = Totals {
        expected: 0,
        found: 0,
        false_blocking: 0,
        failed_calls: 0,
        tokens_in: 0,
        tokens_out: 0,
    };
    for r in results {
        t.expected += r.expected;
        t.found += r.found;
        t.false_blocking += r.false_blocking;
        t.failed_calls += r.calls.iter().filter(|c| c.failure.is_some()).count();
        let (i, o) = r.tokens();
        t.tokens_in += i;
        t.tokens_out += o;
    }
    t
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
        "recall {} of {} expected findings ({percent}%), {} false blocking findings on clean cases, {} failed calls, {} tokens in, {} out\n",
        t.found, t.expected, t.false_blocking, t.failed_calls, t.tokens_in, t.tokens_out
    ));
    out
}

#[cfg(test)]
#[path = "cases_tests.rs"]
mod tests;
