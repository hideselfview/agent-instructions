//! What a card's steps caught: each run of a hook check, a cargo run or an
//! app build step that failed, linked to what followed it on the same card,
//! and each review finding with how it was settled. `builds item times`
//! counts them over the cards that landed lately (card_profile.rs).

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::step_failure::RunResult;

/// The kinds of step whose runs pass or fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StepKind {
    /// A check of a commit hook run.
    Hook,
    /// A `builds cargo` run.
    Cargo,
    /// A step of an app build.
    AppBuild,
}

impl StepKind {
    pub fn words(self) -> &'static str {
        match self {
            StepKind::Hook => "hook checks",
            StepKind::Cargo => "cargo runs",
            StepKind::AppBuild => "app build steps",
        }
    }

    /// What its runs name as failed.
    pub fn failed_words(self) -> &'static str {
        match self {
            StepKind::Hook => "staged files that hook checks failed on",
            StepKind::Cargo => "tests that cargo runs reported failed",
            StepKind::AppBuild => "shots that app build steps failed",
        }
    }
}

/// One run of a step that passed or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub kind: StepKind,
    /// The step, the same for each of its runs: a hook check's name, cargo's
    /// subcommand with the crates it named, or an app build's lane and step.
    pub name: String,
    /// When it started, seconds since 1970.
    pub at: i64,
    /// How it ended. A failed run carries what it said: the excerpt of its
    /// output, and the tests a cargo run reported failed, the staged files a
    /// hook check's output named, or the shots an app build's shots run said
    /// fail it.
    pub result: RunResult,
    /// What the step ran on: the commit and the uncommitted changes, or for a
    /// hook check the commit and the staged files; None when builds did not
    /// record it.
    pub state: Option<String>,
}

/// What became of a failed run, from the next run of the same step on the
/// same card that passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It passed on something else: the agent fixed what it caught.
    Fixed,
    /// It passed on the same commit and changes.
    Flaky,
    /// No later run passed.
    Stood,
}

/// The outcome of each failed run of <attempts>, which are in time order.
pub fn failure_outcomes(attempts: &[Attempt]) -> Vec<(&Attempt, Outcome)> {
    let mut found = Vec::new();
    for (at, run) in attempts
        .iter()
        .enumerate()
        .filter(|(_, run)| !run.result.passed())
    {
        let next = attempts[at + 1..].iter().find(|later| {
            later.kind == run.kind && later.name == run.name && later.result.passed()
        });
        let outcome = match next {
            None => Outcome::Stood,
            Some(later) if later.state.is_some() && later.state == run.state => Outcome::Flaky,
            Some(_) => Outcome::Fixed,
        };
        found.push((run, outcome));
    }
    found
}

/// A finding of a review run on a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub group: String,
    /// `confirmed` or `dropped` as it counts now (a second settle as
    /// dropped counts), or None while nobody settled it.
    pub settled: Option<String>,
    /// Did a later review run close it, the change having fixed it?
    pub closed: bool,
}

/// How many steps of each name to list.
const LISTED: usize = 15;

/// What the steps of the cards caught: for each kind of step and each step,
/// its runs, its failed runs and how many of the failures a fix followed,
/// passed again on the same commit, or stood, the steps that fail most first;
/// then for each kind the names its runs failed most (tests, files, shots);
/// then each review group's findings and
/// how they were settled. <cards> gives each card's attempts, in time order,
/// and its review findings.
pub fn text(cards: &[(&[Attempt], &[Finding])]) -> String {
    let mut out = String::new();
    let mut runs: BTreeMap<(StepKind, &str), usize> = BTreeMap::new();
    let mut failed: BTreeMap<(StepKind, &str), [usize; 3]> = BTreeMap::new();
    let mut names_failed: BTreeMap<(StepKind, &str), usize> = BTreeMap::new();
    let mut not_kept: BTreeMap<StepKind, usize> = BTreeMap::new();
    for (attempts, _) in cards {
        for attempt in attempts.iter() {
            if attempt.result == RunResult::Failed(None) {
                *not_kept.entry(attempt.kind).or_default() += 1;
            }
            *runs
                .entry((attempt.kind, attempt.name.as_str()))
                .or_default() += 1;
            for name in attempt
                .result
                .failure()
                .iter()
                .flat_map(|said| &said.failed)
            {
                *names_failed
                    .entry((attempt.kind, name.as_str()))
                    .or_default() += 1;
            }
        }
        for (attempt, outcome) in failure_outcomes(attempts) {
            failed
                .entry((attempt.kind, attempt.name.as_str()))
                .or_default()[outcome as usize] += 1;
        }
    }
    let _ = writeln!(
        out,
        "steps that failed (runs, failures, then failures a fix followed, that passed again on the same commit and changes, and that stood, then failures with no output kept)"
    );
    for kind in [StepKind::Hook, StepKind::Cargo, StepKind::AppBuild] {
        let all: usize = runs
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|(_, n)| n)
            .sum();
        let mut names: Vec<(&str, &[usize; 3])> = failed
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|((_, name), counts)| (*name, counts))
            .collect();
        let sum = |counts: &[usize; 3]| counts.iter().sum::<usize>();
        let total: [usize; 3] = names.iter().fold([0; 3], |mut total, (_, counts)| {
            for (slot, n) in total.iter_mut().zip(counts.iter()) {
                *slot += n;
            }
            total
        });
        let _ = writeln!(
            out,
            "  {}: {all} runs, {} failed, {} fixed, {} flaky, {} stood, {} with no output kept",
            kind.words(),
            sum(&total),
            total[Outcome::Fixed as usize],
            total[Outcome::Flaky as usize],
            total[Outcome::Stood as usize],
            not_kept.get(&kind).copied().unwrap_or(0),
        );
        names.sort_by_key(|(_, counts)| std::cmp::Reverse(sum(counts)));
        for (name, counts) in names.into_iter().take(LISTED) {
            let _ = writeln!(
                out,
                "    {:>5} runs  {:>4} failed  {:>4} fixed  {:>4} flaky  {:>4} stood  {name}",
                runs[&(kind, name)],
                sum(counts),
                counts[Outcome::Fixed as usize],
                counts[Outcome::Flaky as usize],
                counts[Outcome::Stood as usize],
            );
        }
    }
    for kind in [StepKind::Hook, StepKind::Cargo, StepKind::AppBuild] {
        let mut names: Vec<(&str, usize)> = names_failed
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|((_, name), n)| (*name, *n))
            .collect();
        if names.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{} most often (failed runs)", kind.failed_words());
        names.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (name, n) in names.into_iter().take(LISTED) {
            let _ = writeln!(out, "  {n:>5}  {name}");
        }
    }
    let mut groups: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for (_, findings) in cards {
        for finding in findings.iter() {
            let counts = groups.entry(finding.group.as_str()).or_default();
            match finding.settled.as_deref() {
                Some("confirmed") => counts[0] += 1,
                Some(_) => counts[1] += 1,
                None => counts[2] += 1,
            }
            if finding.closed {
                counts[3] += 1;
            }
        }
    }
    let _ = writeln!(
        out,
        "review findings by rule group (confirmed, dropped, not settled, and confirmed ones a later review closed after a fix)"
    );
    let mut groups: Vec<(&str, [usize; 4])> = groups.into_iter().collect();
    groups.sort_by_key(|(_, counts)| std::cmp::Reverse(counts[0]));
    for (group, counts) in groups {
        let _ = writeln!(
            out,
            "  {:>4} confirmed  {:>4} dropped  {:>4} not settled  {:>4} closed  {group}",
            counts[0], counts[1], counts[2], counts[3],
        );
    }
    out
}

#[cfg(test)]
#[path = "step_outcomes_tests.rs"]
mod tests;
