//! Runs a reviewer and checks its structured answer against the rules it read.

use std::path::Path;

use serde_json::{Value, json};

use super::provider::Selected;
use crate::review::prompt;

/// One finding as the reviewer wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw {
    pub rule: String,
    pub clause: String,
    pub file: String,
    pub line: i64,
    pub code: String,
    pub problem: String,
}

/// What a reviewer answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// (rule, `finding` or `none`).
    pub verdicts: Vec<(String, String)>,
    pub findings: Vec<Raw>,
    /// For a group that lists the sites it judges: its summary and sites, as JSON.
    pub sites: Option<String>,
    /// The open findings the prompt listed whose problem the code no longer
    /// has.
    pub closed: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub model: String,
    /// The reviewer's JSON answer as printed, or why there was none.
    pub output: String,
    pub usage: Usage,
    /// The answer, or why there is none.
    pub answer: Result<Answer, String>,
    pub diagnostics: Vec<String>,
}

/// Reported and derived token counts. An absent count has no known value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Tokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cached: Option<i64>,
    pub written: Option<i64>,
    pub uncached: Option<i64>,
    pub reasoning: Option<i64>,
}

impl Tokens {
    pub const ZERO: Self = Self {
        input: Some(0),
        output: Some(0),
        cached: Some(0),
        written: Some(0),
        uncached: Some(0),
        reasoning: Some(0),
    };

    fn add(self, other: Self) -> (Self, bool) {
        let mut overflowed = false;
        let mut add = |left: Option<i64>, right: Option<i64>| {
            let (Some(left), Some(right)) = (left, right) else {
                return None;
            };
            let sum = left.checked_add(right);
            overflowed |= sum.is_none();
            sum
        };
        let tokens = Self {
            input: add(self.input, other.input),
            output: add(self.output, other.output),
            cached: add(self.cached, other.cached),
            written: add(self.written, other.written),
            uncached: add(self.uncached, other.uncached),
            reasoning: add(self.reasoning, other.reasoning),
        };
        (tokens, overflowed)
    }
}

const TOKEN_OVERFLOW: &str = "the token total exceeds the range builds can record";

pub type TokenTotal = Result<Tokens, &'static str>;

pub fn sum_tokens(tokens: impl IntoIterator<Item = Tokens>) -> TokenTotal {
    tokens.into_iter().try_fold(Tokens::ZERO, |total, next| {
        let (total, overflowed) = total.add(next);
        if overflowed {
            Err(TOKEN_OVERFLOW)
        } else {
            Ok(total)
        }
    })
}

pub(super) fn tokens_text(tokens: TokenTotal, detailed: bool) -> String {
    let tokens = match tokens {
        Ok(tokens) => tokens,
        Err(why) => return why.to_string(),
    };
    let count = |n: Option<i64>| n.map_or_else(|| "unknown".to_string(), |n| n.to_string());
    let input = count(tokens.input);
    let output = count(tokens.output);
    if detailed {
        let out = tokens.reasoning.map_or_else(
            || format!("{output} out"),
            |reasoning| format!("{output} out ({reasoning} reasoning)"),
        );
        let uncached = count(tokens.uncached);
        let cached = count(tokens.cached);
        let written = count(tokens.written);
        format!(
            "{input} tokens in ({uncached} uncached, {cached} read from the cache, {written} written to it), {out}"
        )
    } else {
        format!("{input} tokens in, {output} out")
    }
}

/// The tokens a reviewer used and the cost the provider reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub tokens: Tokens,
    /// Cost in millionths of a US dollar, when the provider reports one.
    pub cost_microdollars: Option<i64>,
}

impl Usage {
    pub const ZERO: Self = Self {
        tokens: Tokens::ZERO,
        cost_microdollars: Some(0),
    };

    pub(super) fn merge(&mut self, other: Self) -> Result<(), String> {
        let (tokens, token_overflowed) = self.tokens.add(other.tokens);
        let cost = add_cost(
            self.cost_microdollars.map(i128::from),
            other.cost_microdollars,
        );
        let cost = cost
            .map(i64::try_from)
            .transpose()
            .map_err(|_| "the review cost exceeds the range builds can record");
        let mut errors = Vec::new();
        self.tokens = tokens;
        if token_overflowed {
            errors.push(TOKEN_OVERFLOW);
        }
        self.cost_microdollars = cost.unwrap_or_else(|why| {
            errors.push(why);
            None
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(". "))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageTotal {
    pub tokens: TokenTotal,
    pub cost_microdollars: Option<i128>,
}

pub(super) fn sum_usage(calls: impl IntoIterator<Item = Usage>) -> UsageTotal {
    calls.into_iter().fold(
        UsageTotal {
            tokens: Ok(Tokens::ZERO),
            cost_microdollars: Some(0),
        },
        |total, call| UsageTotal {
            tokens: total
                .tokens
                .and_then(|tokens| sum_tokens([tokens, call.tokens])),
            cost_microdollars: add_cost(total.cost_microdollars, call.cost_microdollars),
        },
    )
}

pub(super) fn add_cost(total: Option<i128>, cost: Option<i64>) -> Option<i128> {
    Some(total? + i128::from(cost?))
}

pub(super) fn known_cost_text(cost: i128) -> String {
    format!("${:.2}", cost as f64 / 1_000_000.0)
}

pub(super) fn cost_text(cost: Option<i128>) -> String {
    cost.map_or_else(|| "cost not recorded".to_string(), known_cost_text)
}

/// A reviewer's model, token usage and answer in the schema.
#[derive(Clone, Debug, PartialEq)]
pub struct Invoked {
    pub model: String,
    pub usage: Usage,
    pub body: Result<Value, String>,
    pub diagnostics: Vec<String>,
}

impl Invoked {
    pub fn answer_text(&self) -> String {
        match &self.body {
            Ok(body) => body.to_string(),
            Err(why) => failed_answer_text(why),
        }
    }

    pub fn failed(model: String, why: String) -> Self {
        Self {
            model,
            usage: Usage::default(),
            body: Err(why),
            diagnostics: Vec::new(),
        }
    }
}

pub use super::provider::invoke;

pub fn note_diagnostics(subject: impl std::fmt::Display, diagnostics: &[String]) {
    for diagnostic in diagnostics {
        crate::text::note(format!("{subject}: {diagnostic}"));
    }
}

pub fn failed_answer_text(why: &str) -> String {
    format!("no answer: {why}")
}

/// Runs a group's reviewer and reads its answer: verdicts per rule, or, for a
/// group that lists sites, a summary and a verdict per site, and the open
/// findings it closes, each one of <given>.
pub(super) fn run(
    reviewer: &Selected,
    names: &[String],
    given: &[i64],
    enumerate: bool,
    parts: &prompt::Parts,
    checkout: Option<&Path>,
) -> Output {
    let invoked = reviewer.invoke(&prompt::schema(), &parts.system, &parts.message, checkout);
    output_of(invoked, names, given, enumerate)
}

fn output_of(invoked: Invoked, names: &[String], given: &[i64], enumerate: bool) -> Output {
    let output = invoked.answer_text();
    Output {
        model: invoked.model,
        output,
        usage: invoked.usage,
        diagnostics: invoked.diagnostics,
        answer: invoked.body.and_then(|body| {
            let mut answer = if enumerate {
                sites_of(&body, names)?
            } else {
                answer_of(&body, names)?
            };
            answer.closed = closed_of(&body, given)?;
            Ok(answer)
        }),
    }
}

/// Reads the answer of <text>, the JSON `claude -p` printed.
#[cfg(test)]
pub fn parse(text: &str, names: &[String]) -> Output {
    output_of(
        super::provider::claude_envelope(text.as_bytes(), "opus"),
        names,
        &[],
        false,
    )
}

/// The open findings the answer closes: none when it names none, and each
/// one of <given>, the findings the prompt listed.
fn closed_of(body: &Value, given: &[i64]) -> Result<Vec<i64>, String> {
    let Some(listed) = body.get("closed") else {
        return Ok(Vec::new());
    };
    let listed = listed
        .as_array()
        .ok_or("the answer's closed is not a list")?;
    let mut closed = Vec::new();
    for id in listed {
        let id = id
            .as_i64()
            .ok_or("the answer's closed holds something other than a finding number")?;
        if !given.contains(&id) {
            return Err(format!(
                "the answer closes finding {id}, which the prompt did not list"
            ));
        }
        if !closed.contains(&id) {
            closed.push(id);
        }
    }
    Ok(closed)
}

fn answer_of(body: &Value, names: &[String]) -> Result<Answer, String> {
    let rules = body["rules"]
        .as_array()
        .ok_or("the answer has no rules list")?;
    let mut verdicts: Vec<(String, String)> = Vec::new();
    let mut findings = Vec::new();
    for entry in rules {
        let rule = entry["rule"].as_str().ok_or("a verdict names no rule")?;
        let verdict = entry["verdict"].as_str().ok_or("a verdict says nothing")?;
        if !names.iter().any(|n| n == rule) {
            return Err(format!(
                "the answer has a verdict for {rule}, which is not in the group"
            ));
        }
        if verdict != "finding" && verdict != "none" {
            return Err(format!(
                "the verdict for {rule} is {verdict}, not finding or none"
            ));
        }
        if verdicts.iter().any(|(r, _)| r == rule) {
            return Err(format!("the answer has two verdicts for {rule}"));
        }
        let listed = entry["findings"].as_array().cloned().unwrap_or_default();
        if (verdict == "finding") == listed.is_empty() {
            return Err(format!(
                "the verdict for {rule} is {verdict} with {} findings",
                listed.len()
            ));
        }
        for f in listed {
            let text = |key: &str| f[key].as_str().map(str::to_string);
            findings.push(Raw {
                rule: rule.to_string(),
                clause: text("clause").ok_or("a finding has no clause")?,
                file: text("file").ok_or("a finding has no file")?,
                line: f["line"].as_i64().ok_or("a finding has no line")?,
                code: text("code").ok_or("a finding has no code")?,
                problem: text("problem").ok_or("a finding has no problem")?,
            });
        }
        verdicts.push((rule.to_string(), verdict.to_string()));
    }
    let missing: Vec<&str> = names
        .iter()
        .filter(|n| !verdicts.iter().any(|(r, _)| r == *n))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the answer has no verdict for {}",
            missing.join(", ")
        ));
    }
    Ok(Answer {
        verdicts,
        findings,
        sites: None,
        closed: Vec::new(),
    })
}

/// The answer of a group that lists the sites it judges: a summary, then
/// every site with its verdict. A finding site names its rule (a group of one
/// rule may leave it out). A rule is `finding` when any site is a finding for
/// it, else `none`.
fn sites_of(body: &Value, names: &[String]) -> Result<Answer, String> {
    let summary = body["summary"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or("the answer has no summary of the sites")?;
    let sites = body["sites"]
        .as_array()
        .ok_or("the answer has no sites list")?;
    let mut findings = Vec::new();
    for site in sites {
        let text = |key: &str| site[key].as_str().map(str::to_string);
        let file = text("file").ok_or("a site has no file")?;
        let code = text("code").ok_or("a site has no code")?;
        let line = site["line"].as_i64().ok_or("a site has no line")?;
        match site["verdict"].as_str() {
            Some("none") => {}
            Some("finding") => {
                let rule = match (text("rule"), names) {
                    (Some(rule), _) if names.contains(&rule) => rule,
                    (Some(rule), _) => {
                        return Err(format!(
                            "a finding site names {rule}, which is not in the group"
                        ));
                    }
                    (None, [only]) => only.clone(),
                    (None, _) => return Err("a finding site names no rule".to_string()),
                };
                findings.push(Raw {
                    rule,
                    clause: text("clause").ok_or("a finding site has no clause")?,
                    file,
                    line,
                    code,
                    problem: text("problem").ok_or("a finding site has no problem")?,
                });
            }
            _ => return Err("a site's verdict is not finding or none".to_string()),
        }
    }
    let verdicts = names
        .iter()
        .map(|name| {
            let verdict = if findings.iter().any(|f| &f.rule == name) {
                "finding"
            } else {
                "none"
            };
            (name.clone(), verdict.to_string())
        })
        .collect();
    Ok(Answer {
        verdicts,
        findings,
        sites: Some(json!({"summary": summary, "sites": sites}).to_string()),
        closed: Vec::new(),
    })
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
