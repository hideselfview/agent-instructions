//! `builds review optimize`: an Opus agent reads the record, a replay of the
//! current grouping and `review/groups.toml`, and proposes up to three
//! groupings. builds replays each over the test set and adopts the one with
//! the fewest tokens that still finds every expected finding and raises no
//! more false blocking findings. Recall is the hard constraint; tokens are
//! what the optimizer lowers.

use std::path::Path;

use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::review::audit::Rate;
use crate::review::call::invoke;
use crate::review::cases::{self, Case, CaseResult, Totals};
use crate::review::groups::{self, Groups};
use crate::review::record::GroupStat;
use crate::review::rules::{self, Rule};

const SYSTEM: &str = "You improve how a code review groups its rules. You have no tools: everything you need is in the message. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.";

/// At most this many groupings are tried.
const MOST: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The whole text of a `review/groups.toml`.
    pub groups_toml: String,
    pub why: String,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["proposals"],
        "additionalProperties": false,
        "properties": {
            "proposals": {
                "type": "array",
                "maxItems": MOST,
                "items": {
                    "type": "object",
                    "required": ["groups_toml", "why"],
                    "additionalProperties": false,
                    "properties": {
                        "groups_toml": {"type": "string"},
                        "why": {"type": "string"}
                    }
                }
            }
        }
    })
}

pub fn parse_proposals(body: &Value) -> std::result::Result<Vec<Proposal>, String> {
    let list = body["proposals"]
        .as_array()
        .ok_or("the answer has no proposals list")?;
    let mut proposals = Vec::new();
    for entry in list.iter().take(MOST) {
        proposals.push(Proposal {
            groups_toml: entry["groups_toml"]
                .as_str()
                .ok_or("a proposal has no groups_toml")?
                .to_string(),
            why: entry["why"]
                .as_str()
                .ok_or("a proposal has no why")?
                .to_string(),
        });
    }
    Ok(proposals)
}

pub fn prompt(
    groups_text: &str,
    rules: &[Rule],
    stats: &[GroupStat],
    rates: &[Rate],
    baseline: &str,
) -> String {
    let mut out = String::from("Task: propose groupings\n\n");
    out.push_str(
        "A review hands each group of rules to one reviewer, which answers for every rule in its group. \
Every rule is in exactly one group. A group runs when one of its rules covers a changed path. \
Your job is to propose up to three groupings that cost fewer tokens over the test set while the reviewers still find every expected finding and raise no more false blocking findings.\n\n\
Moves to consider:\n\
- Merge two groups whose rules are never missed together.\n\
- Split a group where a rule's recall drops when it is grouped with the others.\n\
- Move a rule to the group whose question is closest to it.\n\n\
Each proposal is the whole text of a new groups.toml, in the format of the current one, with every rule listed below in exactly one group, and a sentence saying why it should cost less.\n",
    );
    out.push_str(&format!(
        "\n# The current groups.toml\n\n```toml\n{}\n```\n",
        groups_text.trim_end()
    ));
    out.push_str("\n# Every rule, with its digest\n\n");
    for rule in rules {
        out.push_str(&format!("- {}: {}\n", rule.slug, rule.digest()));
    }
    out.push_str("\n# What the landing reviews recorded, by group\n\n");
    if stats.is_empty() {
        out.push_str("Nothing is recorded yet.\n");
    }
    for s in stats {
        out.push_str(&format!(
            "- {}: {} calls ({} failed), {} tokens in, {} out, {} findings accepted ({} confirmed, {} dropped by the coordinator), {} dropped by builds' own check\n",
            s.group, s.calls, s.failed_calls, s.tokens_in, s.tokens_out, s.findings, s.confirmed, s.dropped, s.dropped_by_builds
        ));
    }
    out.push_str(
        "\n# Audited miss rates by group\n\nAn audit asks again, two other ways, and a confirmed miss is a finding the landing's review did not raise.\n\n",
    );
    if rates.is_empty() {
        out.push_str("No landing is audited yet.\n");
    }
    for r in rates {
        out.push_str(&format!(
            "- {}: {} audits, {} confirmed misses; in the latest {}, {}\n",
            r.group, r.audits, r.misses, r.recent_audits, r.recent_misses
        ));
    }
    out.push_str(&format!(
        "\n# The replay of the current grouping over the test set\n\n{baseline}\n"
    ));
    out
}

/// What the replay of a grouping costs, tokens in and out together.
pub fn tokens(totals: &Totals) -> i64 {
    totals.tokens_in + totals.tokens_out
}

/// Whether <candidate> may replace <baseline>: every expected finding found,
/// no failed call, no more false blocking findings, and fewer tokens.
pub fn qualifies(baseline: &Totals, candidate: &Totals) -> bool {
    candidate.found == candidate.expected
        && candidate.failed_calls == 0
        && candidate.false_blocking <= baseline.false_blocking
        && tokens(candidate) < tokens(baseline)
}

/// The index of the qualifying candidate with the fewest tokens, the first
/// of equals.
pub fn best(baseline: &Totals, candidates: &[Totals]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| qualifies(baseline, c))
        .min_by_key(|(i, c)| (tokens(c), *i))
        .map(|(i, _)| i)
}

/// A grouping to commit and the words of its commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adoption {
    pub groups_toml: String,
    pub message: String,
}

pub struct Decision {
    /// What to print.
    pub text: String,
    pub adopt: Option<Adoption>,
}

fn numbers(groups: usize, t: &Totals) -> String {
    format!(
        "{groups} groups, {} of {} expected findings found, {} false blocking findings, {} tokens in, {} out",
        t.found, t.expected, t.false_blocking, t.tokens_in, t.tokens_out
    )
}

/// Replays the current grouping and the proposer's groupings and decides.
pub fn optimize(
    root: &Path,
    reviewer: &Path,
    proposer: &Path,
    source_repo: Option<&Path>,
    stats: &[GroupStat],
    rates: &[Rate],
) -> Result<Decision> {
    let all_cases: Vec<Case> = cases::load(root)?;
    if all_cases.is_empty() {
        fail!(
            "{} holds no cases to replay",
            root.join(cases::DIR).display()
        );
    }
    let groups_path = root.join(groups::FILE);
    let groups_text = std::fs::read_to_string(&groups_path)
        .context(|| format!("reading {}", groups_path.display()))?;
    let current = Groups::parse(&groups_text)?;
    let baseline_results: Vec<CaseResult> =
        cases::replay_all(root, &all_cases, reviewer, None, source_repo)?;
    let baseline = cases::totals(&baseline_results);
    let mut text = format!(
        "current grouping: {}\n",
        numbers(current.groups.len(), &baseline)
    );
    if baseline.found != baseline.expected || baseline.failed_calls > 0 {
        text.push_str("the current grouping misses an expected finding or has a failed call, so no grouping can qualify against it; nothing changes\n");
        text.push_str(&cases::report(&baseline_results));
        return Ok(Decision { text, adopt: None });
    }
    let every = rules::load(root, None)?;
    let asked = invoke(
        proposer,
        "opus",
        &schema(),
        SYSTEM,
        &prompt(
            &groups_text,
            &every,
            stats,
            rates,
            &cases::report(&baseline_results),
        ),
    );
    let proposals = match asked.body.and_then(|body| parse_proposals(&body)) {
        Ok(proposals) => proposals,
        Err(why) => fail!("the proposer gave no groupings: {why}"),
    };
    let mut tried: Vec<(Proposal, Groups, Totals)> = Vec::new();
    for (n, proposal) in proposals.into_iter().enumerate() {
        let n = n + 1;
        let parsed = Groups::parse(&proposal.groups_toml).and_then(|g| {
            groups::require_whole(&g, &every)?;
            Ok(g)
        });
        match parsed {
            Err(why) => text.push_str(&format!("proposal {n} discarded: {}\n", why.text())),
            Ok(groups) => {
                let results =
                    cases::replay_all(root, &all_cases, reviewer, Some(&groups), source_repo)?;
                let totals = cases::totals(&results);
                text.push_str(&format!(
                    "proposal {n}: {} ({})\n",
                    numbers(groups.groups.len(), &totals),
                    if qualifies(&baseline, &totals) {
                        "qualifies"
                    } else {
                        "does not qualify"
                    }
                ));
                tried.push((proposal, groups, totals));
            }
        }
    }
    let totals: Vec<Totals> = tried.iter().map(|(_, _, t)| t.clone()).collect();
    let Some(index) = best(&baseline, &totals) else {
        text.push_str(
            "no proposal finds every expected finding with fewer tokens; nothing changes\n",
        );
        return Ok(Decision { text, adopt: None });
    };
    let (proposal, groups, after) = &tried[index];
    let message = format!(
        "Regroup the review's rules: {} tokens over the test set, down from {}\n\n\
         The optimizer proposed this grouping, and builds replayed it over every\n\
         case of review/cases against the grouping it replaces.\n\n\
         Before: {}.\n\
         After: {}.\n\n\
         Why: {}\n",
        tokens(after),
        tokens(&baseline),
        numbers(current.groups.len(), &baseline),
        numbers(groups.groups.len(), after),
        proposal.why.trim()
    );
    text.push_str(&format!("adopting proposal {}\n", index + 1));
    Ok(Decision {
        text,
        adopt: Some(Adoption {
            groups_toml: proposal.groups_toml.clone(),
            message,
        }),
    })
}

/// Writes the grouping into the checkout and commits it. The checkout's
/// groups file must have no uncommitted change.
pub fn adopt(root: &Path, adoption: &Adoption) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let git = |args: &[&str]| -> Result<std::process::Output> {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .context(|| format!("running git {}", args.join(" ")))
    };
    let status = git(&["status", "--porcelain", "--", groups::FILE])?;
    if !status.stdout.is_empty() {
        fail!(
            "{} has changes not committed; commit or discard them first",
            groups::FILE
        );
    }
    let path = root.join(groups::FILE);
    std::fs::write(&path, &adoption.groups_toml)
        .context(|| format!("writing {}", path.display()))?;
    let added = git(&["add", "--", groups::FILE])?;
    if !added.status.success() {
        fail!(
            "git add failed: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        );
    }
    let mut commit = Command::new("git");
    commit
        .arg("-C")
        .arg(root)
        .args(["commit", "-q", "-F", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = commit
        .spawn()
        .context(|| "running git commit".to_string())?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(adoption.message.as_bytes())
        .context(|| "writing the commit message".to_string())?;
    let output = child
        .wait_with_output()
        .context(|| "waiting for git commit".to_string())?;
    if !output.status.success() {
        fail!(
            "git commit failed: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "optimize_tests.rs"]
mod tests;
