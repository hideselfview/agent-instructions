//! Checking shots against their boards: hashes, the record's skips, rendering
//! the boards that need it, the comparison, the record's update and the lists.

use crate::compare::{self, Finding, Side, Warning};
use crate::declared::Declared;
use crate::design::{self, DesignSystem};
use crate::lint::{self, Problem};
use crate::lists::{self, Lists};
use crate::names::{Name, Platform};
use crate::record::{self, Entry, Record, Standing};
use crate::render::serve::Tree;
use crate::render::{self, runtime};
use crate::settings::Settings;
use crate::shots::{ElementList, Picture, Shot};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a verification looks at and may write.
pub(crate) struct Verification<'a> {
    pub(crate) root: &'a Path,
    /// The repo's screenshot settings.
    pub(crate) settings: &'a Settings,
    /// The program's cache folder, where the design system is built.
    pub(crate) cache: &'a Path,
    /// The shots to check.
    pub(crate) names: &'a [Name],
    /// Declared shots this run left out because they need a window.
    pub(crate) not_taken: &'a [Name],
    /// Every shot the harnesses declare.
    pub(crate) declared: &'a [Declared],
    /// Where each platform's captured files are.
    pub(crate) shots_for: &'a dyn Fn(Platform) -> PathBuf,
    /// Where the boards are rendered, in `boards/`.
    pub(crate) out: &'a Path,
    /// The branch whose record the pairs are marked new or changed against.
    pub(crate) base: Option<&'a str>,
    /// Where to write the record this run's results make: every pair checked,
    /// with its findings and warnings. Never the repository's record: only
    /// `builds shots record` writes that.
    pub(crate) results: Option<&'a Path>,
    /// Which pairs' findings fail the run.
    pub(crate) gate: Gate,
}

/// Which pairs' hard findings fail a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Every pair's: the check on a pull request or main, where the boards and
    /// apps are expected to agree.
    EveryPair,
    /// Only pairs new or changed against the base record: a local run, where
    /// the other pairs' findings belong to work the branch did not touch. A
    /// base with no record has no pair that is unchanged against it, so it
    /// gates none.
    NewOrChanged,
}

/// One pair that was compared.
pub(crate) struct Checked {
    pub(crate) name: Name,
    pub(crate) board: ElementList,
    pub(crate) shot: ElementList,
    /// The matched elements: (board index, shot index).
    pub(crate) pairs: Vec<(usize, usize)>,
    /// How far the shot's frame sits right and down of the board's.
    pub(crate) offset: (f64, f64),
    pub(crate) board_png: PathBuf,
    pub(crate) shot_png: PathBuf,
    pub(crate) findings: Vec<Finding>,
    pub(crate) warnings: Vec<Warning>,
    pub(crate) against_base: Option<Standing>,
}

/// One pair the record skipped.
pub(crate) struct Skipped {
    pub(crate) name: Name,
    /// The findings the record holds for it: the check it skips found them.
    pub(crate) findings: Vec<String>,
    /// The warnings the record holds for it.
    pub(crate) warnings: Vec<String>,
    pub(crate) against_base: Option<Standing>,
}

pub(crate) struct Outcome {
    /// What the run says of the runtime copy it drew the boards with.
    pub(crate) runtime: String,
    pub(crate) checked: Vec<Checked>,
    pub(crate) skipped: Vec<Skipped>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) lists: Lists,
    /// Declared shots the run was not given the means to take (they need a
    /// window); neither checked nor failing.
    pub(crate) not_taken: Vec<Name>,
    pub(crate) gate: Gate,
    /// The base was given and its record has no entries.
    pub(crate) base_empty: bool,
}

impl Outcome {
    /// Whether a pair's findings fail the run.
    fn gates(&self, has_findings: bool, against_base: Option<Standing>) -> bool {
        has_findings
            && match self.gate {
                Gate::EveryPair => true,
                Gate::NewOrChanged => {
                    matches!(against_base, Some(Standing::New | Standing::Changed))
                }
            }
    }

    fn gated(&self, pair: &Checked) -> bool {
        self.gates(!pair.findings.is_empty(), pair.against_base)
    }

    fn gated_skip(&self, pair: &Skipped) -> bool {
        self.gates(!pair.findings.is_empty(), pair.against_base)
    }

    /// The pairs whose findings fail the run.
    pub(crate) fn failing(&self) -> impl Iterator<Item = &Checked> {
        self.checked.iter().filter(|c| self.gated(c))
    }

    /// The pairs with findings that are listed and do not fail the run.
    pub(crate) fn listed(&self) -> impl Iterator<Item = &Checked> {
        self.checked
            .iter()
            .filter(|c| !c.findings.is_empty() && !self.gated(c))
    }

    /// The skipped pairs whose recorded findings fail the run.
    pub(crate) fn failing_skipped(&self) -> impl Iterator<Item = &Skipped> {
        self.skipped.iter().filter(|s| self.gated_skip(s))
    }

    /// The skipped pairs with recorded findings that are listed and do not
    /// fail the run.
    pub(crate) fn listed_skipped(&self) -> impl Iterator<Item = &Skipped> {
        self.skipped
            .iter()
            .filter(|s| !s.findings.is_empty() && !self.gated_skip(s))
    }

    /// Whether nothing fails: no gated finding, checked now or recorded, and
    /// no shot or record entry without a board.
    pub(crate) fn passed(&self) -> bool {
        self.problems.is_empty()
            && self.failing().next().is_none()
            && self.failing_skipped().next().is_none()
    }
}

/// Splits `names` into the pairs whose record entry has these hashes and the
/// pairs to check.
pub(crate) fn partition(
    names: &[Name],
    board_hashes: &BTreeMap<Name, String>,
    shot_hashes: &BTreeMap<Name, String>,
    record: &Record,
) -> (Vec<Name>, Vec<Name>) {
    names.iter().cloned().partition(|name| {
        record.standing(&name.to_string(), &board_hashes[name], &shot_hashes[name])
            == Standing::Unchanged
    })
}

pub(crate) fn verify(job: &Verification) -> Result<Outcome, String> {
    let root = job.root;
    let runtime = runtime::check(root).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(root, job.cache).map_err(|e| e.to_string())?;
    let record_path = root.join(record::FILE);
    let record = Record::load(&record_path).map_err(|e| e.to_string())?;
    let base = job
        .base
        .map(|r| Record::at(root, r))
        .transpose()
        .map_err(|e| e.to_string())?;

    let problems = lint::problems(root, &job.settings.preview_calls, job.declared, &record);
    let boardless = |name: &Name| {
        problems
            .iter()
            .any(|p| p.declared && p.name == name.to_string())
    };
    let hashed: Vec<Name> = job
        .declared
        .iter()
        .map(|d| d.name.clone())
        .filter(|n| !boardless(n))
        .collect();
    let board_hashes = render::hashes(root, &hashed, &design, runtime.copy.sha256)?;
    let names: Vec<Name> = job
        .names
        .iter()
        .filter(|n| !boardless(n))
        .cloned()
        .collect();

    let mut shots = BTreeMap::new();
    for name in &names {
        let shot =
            Shot::load(&(job.shots_for)(name.platform()), name).map_err(|e| e.to_string())?;
        shots.insert(name.clone(), shot);
    }
    let shot_hashes: BTreeMap<Name, String> = shots
        .iter()
        .map(|(n, s)| (n.clone(), s.hash.clone()))
        .collect();
    let (confirmed, pending) = partition(&names, &board_hashes, &shot_hashes, &record);
    // A base with no record says nothing of any pair, so no pair is marked
    // new or changed against it.
    let base_empty = base.as_ref().is_some_and(Record::is_empty);
    let standing = |name: &Name| {
        base.as_ref()
            .filter(|b| !b.is_empty())
            .map(|b| b.standing(&name.to_string(), &board_hashes[name], &shot_hashes[name]))
    };

    let boards = job.out.join("boards");
    let tree = Tree {
        root,
        design: &design,
        runtime: runtime.copy.text,
    };
    render::boards(tree, job.settings, &pending, &boards)?;
    let mut checked = Vec::new();
    for name in &pending {
        let shot = &shots[name];
        let list =
            ElementList::load(&boards.join(format!("{name}.json"))).map_err(|e| e.to_string())?;
        let picture_file = boards.join(format!("{name}.png"));
        let picture = std::fs::read(&picture_file)
            .map_err(|e| format!("cannot read {}: {e}", picture_file.display()))?;
        let picture =
            Picture::decode(&picture).map_err(|e| format!("{}: {e}", picture_file.display()))?;
        let result = compare::compare(&list.elements, &shot.list.elements);
        let warnings = compare::warnings(
            &Side {
                elements: &list.elements,
                picture: &picture,
                scale: list.scale,
                unshot: &list.unshot,
            },
            &Side {
                elements: &shot.list.elements,
                picture: &shot.picture,
                scale: shot.list.scale,
                unshot: &shot.list.unshot,
            },
            &result.pairs,
        );
        checked.push(Checked {
            name: name.clone(),
            offset: compare::geometry::frame_offset(
                &list.elements,
                &shot.list.elements,
                &result.pairs,
            ),
            pairs: result
                .pairs
                .iter()
                .map(|(b, s)| (b.index, s.index))
                .collect(),
            board: list,
            shot: shot.list.clone(),
            board_png: picture_file,
            shot_png: (job.shots_for)(name.platform()).join(name.file_name("png")),
            findings: result.findings,
            warnings,
            against_base: standing(name),
        });
    }
    if let Some(results_path) = job.results {
        let mut results = record.clone();
        for pair in &checked {
            results.set(
                &pair.name.to_string(),
                Entry {
                    board: board_hashes[&pair.name].clone(),
                    shot: shot_hashes[&pair.name].clone(),
                    findings: pair.findings.iter().map(Finding::sentence).collect(),
                    warnings: pair.warnings.iter().map(Warning::sentence).collect(),
                },
            );
        }
        std::fs::create_dir_all(job.out)
            .map_err(|e| format!("cannot prepare {}: {e}", job.out.display()))?;
        results.save(results_path).map_err(|e| e.to_string())?;
    }
    let skipped = confirmed
        .into_iter()
        .map(|name| {
            let entry = record
                .get(&name.to_string())
                .expect("partition skips only recorded pairs");
            Skipped {
                findings: entry.findings.clone(),
                warnings: entry.warnings.clone(),
                against_base: standing(&name),
                name,
            }
        })
        .collect();
    let ready = design::ready_boards(root).map_err(|e| e.to_string())?;
    let lists = lists::lists(
        &hashed_all(job.declared),
        &ready,
        &record,
        &board_hashes,
        &problems,
    );
    Ok(Outcome {
        runtime: runtime.said,
        checked,
        skipped,
        problems,
        lists,
        not_taken: job.not_taken.to_vec(),
        gate: job.gate,
        base_empty,
    })
}

fn hashed_all(declared: &[Declared]) -> Vec<Name> {
    declared.iter().map(|d| d.name.clone()).collect()
}

fn mark(standing: Option<Standing>) -> &'static str {
    match standing {
        Some(Standing::New) => " (new against the base)",
        Some(Standing::Changed) => " (changed against the base)",
        Some(Standing::Unchanged) | None => "",
    }
}

/// Prints what the run found: the pairs that fail it first, then the pairs
/// whose findings are only listed, then a summary naming both.
pub(crate) fn print(outcome: &Outcome) {
    for problem in &outcome.problems {
        eprintln!("{}", problem.sentence);
    }
    for (label, pairs) in [
        ("FAIL ", outcome.failing().collect::<Vec<_>>()),
        (
            "listed, does not fail the run: ",
            outcome.listed().collect(),
        ),
    ] {
        for pair in pairs {
            for finding in &pair.findings {
                eprintln!(
                    "{label}{}{}: {}",
                    pair.name,
                    mark(pair.against_base),
                    finding.sentence()
                );
            }
        }
    }
    for pair in &outcome.checked {
        for warning in &pair.warnings {
            eprintln!(
                "{}{}: warning: {}",
                pair.name,
                mark(pair.against_base),
                warning.sentence()
            );
        }
    }
    for (label, pairs) in [
        ("FAIL ", outcome.failing_skipped().collect::<Vec<_>>()),
        (
            "listed, does not fail the run: ",
            outcome.listed_skipped().collect(),
        ),
    ] {
        for pair in pairs {
            for finding in &pair.findings {
                eprintln!(
                    "{label}{}{}: {finding} (from the record)",
                    pair.name,
                    mark(pair.against_base)
                );
            }
        }
    }
    for pair in outcome.skipped.iter().filter(|s| s.findings.is_empty()) {
        eprintln!("{}{}: confirmed", pair.name, mark(pair.against_base));
        for warning in &pair.warnings {
            eprintln!("{}: confirmed with the warning: {warning}", pair.name);
        }
    }
    for name in &outcome.not_taken {
        eprintln!("{name}: {}", crate::capture::NEEDS_WINDOW);
    }
    for (title, list) in [
        ("Boards with no shot (to build)", &outcome.lists.to_build),
        (
            "Shots whose board changed since confirmed (to update)",
            &outcome.lists.to_update,
        ),
        ("Shots with no board (to delete)", &outcome.lists.to_delete),
    ] {
        if !list.is_empty() {
            eprintln!("{title}:");
            for name in list {
                eprintln!("  {name}");
            }
        }
    }
    summarize(outcome);
}

fn names_of<'a>(pairs: impl Iterator<Item = &'a Name>) -> String {
    pairs.map(Name::to_string).collect::<Vec<_>>().join(", ")
}

/// Says plainly which pairs fail the run and which have findings that are
/// only listed.
fn summarize(outcome: &Outcome) {
    let failing: Vec<&Name> = outcome
        .failing()
        .map(|c| &c.name)
        .chain(outcome.failing_skipped().map(|s| &s.name))
        .collect();
    let listed: Vec<&Name> = outcome
        .listed()
        .map(|c| &c.name)
        .chain(outcome.listed_skipped().map(|s| &s.name))
        .collect();
    if !failing.is_empty() {
        eprintln!(
            "builds shots: {} pair(s) fail the run: {}",
            failing.len(),
            names_of(failing.iter().copied())
        );
    }
    if !outcome.problems.is_empty() {
        eprintln!(
            "builds shots: {} shot or record entry(ies) with no board fail the run",
            outcome.problems.len()
        );
    }
    if !listed.is_empty() {
        let why = if outcome.base_empty {
            "the base has no record, so no pair is new or changed against it"
        } else {
            "they are neither new nor changed against the base"
        };
        eprintln!(
            "builds shots: {} pair(s) have findings that are only listed ({why}): {}",
            listed.len(),
            names_of(listed.iter().copied())
        );
    }
    if outcome.passed() {
        eprintln!("builds shots: no pair fails the run");
    }
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
