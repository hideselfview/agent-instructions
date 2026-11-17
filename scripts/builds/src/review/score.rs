//! How well reviews do, counted from what the coordinator settled. A finding
//! the coordinator confirmed is a true positive (TP) and one it dropped a false
//! positive (FP). A finding counts as its last settle, so one the coordinator
//! settled again as dropped is an FP. A confirmed finding whose code left its
//! branch stays a TP: the code it named was there when the reviewer read it. A
//! candidate miss that an audit raised (`audit.rs`) and the
//! coordinator confirmed is a false negative (FN). A rule that a reviewer
//! answered "none" for, over a diff in which no confirmed miss is in that rule,
//! is a true negative (TN). Precision is TP / (TP + FP), recall TP / (TP + FN).
//!
//! Each count goes to the setup of the call it belongs to: the group, its model
//! and its prompt (`prompt::setup`). `builds review scores` counts the store's
//! reviews this way. `builds review replay --landing` counts a replay of a
//! landing's diffs by pairing each settled finding with a finding of the replay
//! that names the same site (`same_site`), and prints the replay's counts next
//! to the recorded reviews' counts.

use rusqlite::Connection;

use crate::fail::Result;
use crate::review::audit::candidate_misses;
use crate::review::cases::{same_code, same_file};
use crate::review::record::{self, Call, Finding, Kind, RunRow, Verdict};

/// How many lines apart two findings may be and still name one site when
/// neither quote of code holds the other.
pub const NEAR_LINES: i64 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub true_pos: usize,
    pub false_pos: usize,
    pub false_neg: usize,
    pub true_neg: usize,
    /// Findings nobody settled; in a replay, the findings that no settled
    /// finding pairs with.
    pub unsettled: usize,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.true_pos += other.true_pos;
        self.false_pos += other.false_pos;
        self.false_neg += other.false_neg;
        self.true_neg += other.true_neg;
        self.unsettled += other.unsettled;
    }

    /// TP / (TP + FP); None when there is neither.
    pub fn precision(&self) -> Option<f64> {
        ratio(self.true_pos, self.true_pos + self.false_pos)
    }

    /// TP / (TP + FN); None when there is neither.
    pub fn recall(&self) -> Option<f64> {
        ratio(self.true_pos, self.true_pos + self.false_neg)
    }
}

fn ratio(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

/// What a group was asked with. The counts of one setup are kept apart from
/// another's.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Setup {
    pub group: String,
    pub model: String,
    /// The hash its prompt is kept under in `review_text`; empty for the
    /// misses in a rule that no group answered for.
    pub prompt: String,
}

impl Setup {
    fn of(call: &Call) -> Setup {
        Setup {
            group: call.group.clone(),
            model: call.model.clone(),
            prompt: call.prompt.clone(),
        }
    }

    /// Where a confirmed miss goes when no call over its landing answered for
    /// its rule.
    fn unasked() -> Setup {
        Setup {
            group: "(no group answered for the rule)".to_string(),
            model: String::new(),
            prompt: String::new(),
        }
    }
}

/// What the calls of one setup cost and how they scored, by rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupScore {
    pub setup: Setup,
    pub spend: Spend,
    /// The commits of the rules checkouts its runs read.
    pub rules_versions: Vec<String>,
    /// By rule, in the order first counted.
    pub rules: Vec<(String, Tally)>,
}

impl GroupScore {
    fn rule(&mut self, rule: &str) -> &mut Tally {
        let at = match self.rules.iter().position(|(r, _)| r == rule) {
            Some(at) => at,
            None => {
                self.rules.push((rule.to_string(), Tally::default()));
                self.rules.len() - 1
            }
        };
        &mut self.rules[at].1
    }

    fn spent(&mut self, call: &Call, rules_version: &str) {
        self.spend.add(call);
        if !self.rules_versions.iter().any(|v| v == rules_version) {
            self.rules_versions.push(rules_version.to_string());
        }
    }

    /// The counts of every rule together.
    pub fn total(&self) -> Tally {
        let mut total = Tally::default();
        for (_, tally) in &self.rules {
            total.add(tally);
        }
        total
    }
}

/// The calls of some runs: how many, their tokens and their cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spend {
    pub calls: usize,
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// In millionths of a US dollar.
    pub cost_microdollars: i64,
    /// Calls made before builds recorded the cost.
    pub uncosted: usize,
}

impl Spend {
    fn add(&mut self, call: &Call) {
        self.calls += 1;
        self.tokens_in += call.tokens_in;
        self.tokens_out += call.tokens_out;
        match call.cost_microdollars {
            Some(cost) => self.cost_microdollars += cost,
            None => self.uncosted += 1,
        }
    }

    fn join(&mut self, other: &Spend) {
        self.calls += other.calls;
        self.tokens_in += other.tokens_in;
        self.tokens_out += other.tokens_out;
        self.cost_microdollars += other.cost_microdollars;
        self.uncosted += other.uncosted;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scores {
    pub groups: Vec<GroupScore>,
}

impl Scores {
    fn group(&mut self, setup: Setup) -> &mut GroupScore {
        let at = match self.groups.iter().position(|g| g.setup == setup) {
            Some(at) => at,
            None => {
                self.groups.push(GroupScore {
                    setup,
                    ..GroupScore::default()
                });
                self.groups.len() - 1
            }
        };
        &mut self.groups[at]
    }
}

/// Counts what review <calls> did from how their findings were settled.
/// <runs> holds every run the calls and misses belong to, so a miss is matched
/// to the calls over the same diff; <findings> are the calls' findings that
/// builds' checks accepted, and <misses> the confirmed candidate misses.
pub fn count_settled(
    scores: &mut Scores,
    runs: &[RunRow],
    calls: &[&Call],
    verdicts: &[Verdict],
    findings: &[&Finding],
    misses: &[&Finding],
) {
    let run_of = |id: i64| {
        runs.iter()
            .find(|r| r.id == id)
            .expect("every call and miss belongs to one of the runs")
    };
    let same_diff = |a: i64, b: i64| {
        let (a, b) = (run_of(a), run_of(b));
        a.base == b.base && a.head == b.head
    };
    for call in calls {
        let group = scores.group(Setup::of(call));
        group.spent(call, &run_of(call.run_id).rules_version);
        if call.failure.is_some() {
            continue;
        }
        for verdict in verdicts.iter().filter(|v| v.call_id == call.id) {
            let missed = misses
                .iter()
                .any(|m| m.rule == verdict.rule && same_diff(m.run_id, call.run_id));
            let tally = group.rule(&verdict.rule);
            if verdict.verdict == "none" && !missed {
                tally.true_neg += 1;
            }
        }
    }
    for finding in findings {
        let call = call_of(calls.iter().copied(), finding);
        let tally = scores.group(Setup::of(call)).rule(&finding.rule);
        match &finding.settled {
            Some(settled) if settled.how == "confirmed" => tally.true_pos += 1,
            Some(_) => tally.false_pos += 1,
            None => tally.unsettled += 1,
        }
    }
    for miss in misses {
        let answered = |c: &&&Call| c.failure.is_none() && c.rules.contains(&miss.rule);
        let call = calls
            .iter()
            .filter(answered)
            .rfind(|c| same_diff(c.run_id, miss.run_id))
            .or_else(|| calls.iter().rfind(answered));
        let setup = call.map(|c| Setup::of(c)).unwrap_or_else(Setup::unasked);
        scores.group(setup).rule(&miss.rule).false_neg += 1;
    }
}

/// The call of <calls> that raised <finding>: one call per group per run.
fn call_of<'a>(mut calls: impl Iterator<Item = &'a Call>, finding: &Finding) -> &'a Call {
    calls
        .find(|c| c.run_id == finding.run_id && c.group == finding.group)
        .expect("a finding belongs to a call of its run")
}

/// Whether two findings name one site: the same rule, the same file (one path
/// may be shortened from the left), and code where one quote holds the other
/// once whitespace and markdown marks are gone, or, when <near> allows it,
/// lines at most `NEAR_LINES` apart.
fn same_site(a: &Finding, b: &Finding, near: bool) -> bool {
    a.rule == b.rule
        && same_file(&a.file, &b.file)
        && (same_code(&a.code, &b.code) || (near && (a.line - b.line).abs() <= NEAR_LINES))
}

/// Pairs each of <truths> with at most one of <raised>, and each of <raised>
/// with at most one truth: first where their code names one site, then where
/// their lines are near.
fn pair(truths: &[&Finding], raised: &[&Finding]) -> Vec<Option<usize>> {
    let mut paired: Vec<Option<usize>> = vec![None; truths.len()];
    let mut used = vec![false; raised.len()];
    for near in [false, true] {
        for (i, truth) in truths.iter().enumerate() {
            if paired[i].is_some() {
                continue;
            }
            let found = raised
                .iter()
                .enumerate()
                .position(|(j, f)| !used[j] && same_site(truth, f, near));
            if let Some(j) = found {
                used[j] = true;
                paired[i] = Some(j);
            }
        }
    }
    paired
}

/// Counts a replay's <calls> over one diff against <truths>, the settled
/// findings of that diff (the review's, and the audits' candidate misses). A
/// confirmed finding the replay raises again is a TP and one it does not raise
/// an FN; a dropped finding it raises again is an FP; a rule it answered "none"
/// for, with no confirmed finding in it, is a TN. A truth in a rule whose call
/// failed counts nowhere. Returns the replay's findings that pair with no
/// truth, which count as unsettled.
pub fn count_replay<'a>(
    scores: &mut Scores,
    calls: &[Call],
    verdicts: &[Verdict],
    raised: &'a [Finding],
    truths: &[&Finding],
    rules_version: &str,
) -> Vec<&'a Finding> {
    for call in calls {
        scores.group(Setup::of(call)).spent(call, rules_version);
    }
    let accepted: Vec<&Finding> = raised.iter().filter(|f| f.counts()).collect();
    let paired = pair(truths, &accepted);
    for (truth, partner) in truths.iter().zip(&paired) {
        let real = truth.settled_as("confirmed");
        match partner {
            Some(j) => {
                let call = call_of(calls.iter(), accepted[*j]);
                let tally = scores.group(Setup::of(call)).rule(&truth.rule);
                if real {
                    tally.true_pos += 1;
                } else {
                    tally.false_pos += 1;
                }
            }
            None if real => {
                let answered = calls
                    .iter()
                    .rfind(|c| c.failure.is_none() && c.rules.contains(&truth.rule));
                if let Some(call) = answered {
                    scores.group(Setup::of(call)).rule(&truth.rule).false_neg += 1;
                }
            }
            None => {}
        }
    }
    for call in calls.iter().filter(|c| c.failure.is_none()) {
        let group = scores.group(Setup::of(call));
        for verdict in verdicts.iter().filter(|v| v.call_id == call.id) {
            let confirmed = truths
                .iter()
                .any(|t| t.rule == verdict.rule && t.settled_as("confirmed"));
            let tally = group.rule(&verdict.rule);
            if verdict.verdict == "none" && !confirmed {
                tally.true_neg += 1;
            }
        }
    }
    let new: Vec<&Finding> = accepted
        .iter()
        .enumerate()
        .filter(|(j, _)| !paired.contains(&Some(*j)))
        .map(|(_, f)| *f)
        .collect();
    for finding in &new {
        let call = call_of(calls.iter(), finding);
        scores.group(Setup::of(call)).rule(&finding.rule).unsettled += 1;
    }
    new
}

/// The scores of a store's reviews, and what its audits and replays cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoreScores {
    pub reviews: Scores,
    pub landings: usize,
    pub review_runs: usize,
    pub audits: Spend,
    pub replays: Spend,
}

/// Counts every review in the store from how its findings and its audits'
/// candidate misses were settled.
pub fn of_store(db: &Connection) -> Result<StoreScores> {
    let mut scores = StoreScores::default();
    for landing in record::all_landings(db)? {
        let runs = record::runs_of_landing(db, landing.id)?;
        let calls = record::calls_of_landing(db, landing.id)?;
        let verdicts = record::verdicts_of_landing(db, landing.id)?;
        let findings = record::findings_of_landing(db, landing.id)?;
        let misses: Vec<&Finding> = candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled_as("confirmed"))
            .collect();
        let reviews: Vec<&Call> = calls.iter().filter(|c| c.kind == "review").collect();
        let raised: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.kind == "review" && f.counts())
            .collect();
        count_settled(
            &mut scores.reviews,
            &runs,
            &reviews,
            &verdicts,
            &raised,
            &misses,
        );
        scores.landings += 1;
        scores.review_runs += runs.iter().filter(|r| r.kind == "review").count();
    }
    for call in record::calls_of_kind(db, Kind::Audit)? {
        scores.audits.add(&call);
    }
    for call in record::calls_of_kind(db, Kind::Replay)? {
        scores.replays.add(&call);
    }
    Ok(scores)
}

/// What every report of scores says first: what the counts mean.
const LEGEND: &str = "TP: a confirmed finding, also one whose code left its branch. FP: a dropped finding, also one settled again as dropped. FN: a candidate miss an audit raised and the coordinator confirmed. TN: a rule answered \"none\" over a diff with no confirmed miss in it. Precision is TP / (TP + FP), recall TP / (TP + FN).\n";

fn percent(value: Option<f64>) -> String {
    match value {
        Some(value) => format!("{:.0}%", value * 100.0),
        None => "n/a".to_string(),
    }
}

fn tally_text(t: &Tally, unsettled: &str) -> String {
    format!(
        "TP {}, FP {}, FN {}, TN {}, {unsettled} {}; precision {}, recall {}",
        t.true_pos,
        t.false_pos,
        t.false_neg,
        t.true_neg,
        t.unsettled,
        percent(t.precision()),
        percent(t.recall())
    )
}

fn spend_text(s: &Spend) -> String {
    let cost = if s.calls > 0 && s.uncosted == s.calls {
        "cost not recorded".to_string()
    } else if s.uncosted == 0 {
        format!("${:.2}", dollars(s.cost_microdollars))
    } else {
        format!(
            "${:.2} (not recorded for {} of {} calls)",
            dollars(s.cost_microdollars),
            s.uncosted,
            s.calls
        )
    };
    format!(
        "{} calls, {} tokens in, {} out, {cost}",
        s.calls, s.tokens_in, s.tokens_out
    )
}

fn dollars(microdollars: i64) -> f64 {
    microdollars as f64 / 1_000_000.0
}

fn setup_text(setup: &Setup) -> String {
    if setup.prompt.is_empty() {
        return "no call".to_string();
    }
    format!(
        "{}, prompt {}",
        setup.model,
        &setup.prompt[..setup.prompt.len().min(8)]
    )
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

/// The names of <groups>' groups, sorted, each once.
fn group_names<'a>(groups: impl Iterator<Item = &'a GroupScore>) -> Vec<&'a str> {
    let mut names: Vec<&str> = groups.map(|g| g.setup.group.as_str()).collect();
    names.sort();
    names.dedup();
    names
}

/// `builds review scores`: per group, each setup's calls and counts, then each
/// rule's counts; then what audits and replays cost.
pub fn store_report(project: &str, scores: &StoreScores) -> String {
    let mut out = format!(
        "review scores of {project}: {} review runs over {} landings\n{LEGEND}",
        scores.review_runs, scores.landings
    );
    let mut groups: Vec<&GroupScore> = scores.reviews.groups.iter().collect();
    groups.sort_by(|a, b| a.setup.cmp(&b.setup));
    for name in group_names(groups.iter().copied()) {
        out.push_str(&format!("\n{name}\n"));
        for group in groups.iter().filter(|g| g.setup.group == name) {
            let versions: Vec<&str> = group.rules_versions.iter().map(|v| short(v)).collect();
            out.push_str(&format!(
                "  {}, rules {}: {}\n    {}\n",
                setup_text(&group.setup),
                if versions.is_empty() {
                    "-".to_string()
                } else {
                    versions.join(" ")
                },
                spend_text(&group.spend),
                tally_text(&group.total(), "unsettled")
            ));
            let mut rules: Vec<&(String, Tally)> = group.rules.iter().collect();
            rules.sort_by(|a, b| a.0.cmp(&b.0));
            for (rule, tally) in rules {
                out.push_str(&format!("    {rule}: {}\n", tally_text(tally, "unsettled")));
            }
        }
    }
    if groups.is_empty() {
        out.push_str("no review is recorded yet\n");
    }
    out.push_str(&format!("\naudits: {}\n", spend_text(&scores.audits)));
    out.push_str(&format!("replays: {}\n", spend_text(&scores.replays)));
    out
}

/// One side's counts of <rule> over its groups named <name>; None when none
/// of them answered for it.
fn rule_tally(scores: &Scores, name: &str, rule: &str) -> Option<Tally> {
    let mut found = None;
    for group in scores.groups.iter().filter(|g| g.setup.group == name) {
        if let Some((_, tally)) = group.rules.iter().find(|(r, _)| r == rule) {
            found.get_or_insert_with(Tally::default).add(tally);
        }
    }
    found
}

/// A replay's counts next to the recorded reviews' counts over the same
/// diffs: per group, each side's setups, then each rule.
pub fn comparison(in_use: &Scores, replay: &Scores) -> String {
    let mut out = format!(
        "\nscores over the diffs replayed. \"in use\" is what the recorded reviews did; \"replay\" is this replay, whose findings that pair with no settled finding are new.\n{LEGEND}"
    );
    let sides = [("in use", in_use, "unsettled"), ("replay", replay, "new")];
    for name in group_names(in_use.groups.iter().chain(&replay.groups)) {
        out.push_str(&format!("\n{name}\n"));
        for (word, scores, unsettled) in sides {
            for group in scores.groups.iter().filter(|g| g.setup.group == name) {
                out.push_str(&format!(
                    "  {word}  {}: {}\n    {}\n",
                    setup_text(&group.setup),
                    spend_text(&group.spend),
                    tally_text(&group.total(), unsettled)
                ));
            }
        }
        let mut rules: Vec<&str> = in_use
            .groups
            .iter()
            .chain(&replay.groups)
            .filter(|g| g.setup.group == name)
            .flat_map(|g| g.rules.iter().map(|(r, _)| r.as_str()))
            .collect();
        rules.sort();
        rules.dedup();
        for rule in rules {
            out.push_str(&format!("    {rule}\n"));
            for (word, scores, unsettled) in sides {
                let line = match rule_tally(scores, name, rule) {
                    Some(tally) => tally_text(&tally, unsettled),
                    None => "not asked".to_string(),
                };
                out.push_str(&format!("      {word}  {line}\n"));
            }
        }
    }
    out.push_str("\nall groups\n");
    for (word, scores, unsettled) in sides {
        let mut tally = Tally::default();
        let mut spend = Spend::default();
        for group in &scores.groups {
            tally.add(&group.total());
            spend.join(&group.spend);
        }
        out.push_str(&format!(
            "  {word}  {}; {}\n",
            tally_text(&tally, unsettled),
            spend_text(&spend)
        ));
    }
    out
}

#[cfg(test)]
#[path = "score_tests.rs"]
mod tests;
