//! The check of one finding by a second model against the text of the rule the
//! finding names. `review case propose` checks each finding it proposes for a
//! case before the case joins the test set, and an audit checks each candidate
//! miss it raises and settles it with the model's answer. The model reads the
//! rule's text, the finding, and the diff of the finding's file with the new
//! text around each hunk, and answers whether the code breaks the rule as the
//! finding says.

use std::path::Path;

use serde_json::{Value, json};

use crate::review::call::{self, Usage};
use crate::review::check::names_file;
use crate::review::files;
use crate::review::prompt::PROJECT_FACTS;
use crate::review::record::Finding;
use crate::review::run::{Diff, Material};

/// The model that checks a finding unless a command names another.
pub const MODEL: &str = "high";

/// The first line of every message, before the rule's slug.
const TASK: &str = "Task: model-check";

/// What the model answered for one finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub model: String,
    /// Whether the model found that the code breaks the rule as the finding
    /// says, with its reason; or why the check gave no usable answer.
    pub verdict: Result<(bool, String), String>,
    pub usage: Usage,
}

impl Checked {
    /// The reason a settle records for this answer: the model's own words,
    /// after what it decided.
    pub fn settle_why(&self) -> Option<(&'static str, String)> {
        let (confirmed, why) = self.verdict.as_ref().ok()?;
        let (how, word) = if *confirmed {
            ("confirmed", "confirmed")
        } else {
            ("dropped", "rejected")
        };
        Some((how, format!("the {} check {word} it: {why}", self.model)))
    }
}

/// The system prompt: what the check is, and the rule's text.
fn system(rule: &str, rule_text: &str) -> String {
    format!(
        "You check one finding of a code review against the rule it names, before builds counts the finding as true. The message holds the finding and the diff of its file, with the new text of the file around each hunk. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.

Confirm the finding when the quoted code is in the diff's added lines and breaks the quoted clause of the rule as the finding says. Reject it when the clause does not cover the code, when the code follows the rule, or when the problem the finding names is not there. In `why`, say in one or two sentences what the code does against the clause.

The rule {rule}:

{}",
        rule_text.trim_end()
    )
}

/// The message: the task, the finding and its file's diff.
fn message(finding: &Finding, file_diff: &str, context: &str) -> String {
    let mut out = format!(
        "{TASK} {}\n\nThe finding:\nclause: {}\nfile: {}\nline: {}\ncode: {}\nproblem: {}\n\nThe diff of {}:\n{}",
        finding.rule,
        finding.clause,
        finding.file,
        finding.line,
        finding.code,
        finding.problem,
        finding.file,
        file_diff
    );
    if !context.is_empty() {
        out.push_str(&format!("\nThe new text of the file:\n{context}"));
    }
    out
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "why"],
        "properties": {
            "verdict": {"type": "string", "enum": ["confirmed", "rejected"]},
            "why": {"type": "string"}
        }
    })
}

/// The diff of the file <finding> names in <diff>, the commit messages as one
/// more file, and the new text around its hunks; None when the diff has no
/// such file.
fn file_of(diff: &Diff, finding: &Finding) -> Option<(String, String)> {
    if finding.file == files::COMMIT_MESSAGE {
        let (file, _) = files::commit_message_file(&diff.messages)?;
        return Some((file.text, String::new()));
    }
    let file = files::split(&diff.text)
        .into_iter()
        .find(|f| names_file(&f.path, &finding.file))?;
    let context = files::context(diff.source.as_ref(), &file.path, &file.text);
    Some((file.text, context))
}

/// Asks <program> in <model> whether <finding>, over <diff>, breaks its rule
/// in <material>.
pub fn check(
    program: &Path,
    model: &str,
    material: &Material,
    finding: &Finding,
    diff: &Diff,
) -> Checked {
    let failed = |why: String| Checked {
        model: super::provider::model_name(program, model),
        verdict: Err(why),
        usage: Usage::default(),
    };
    let rule_text = if finding.rule == PROJECT_FACTS {
        Some(material.project_text.as_str())
    } else {
        material
            .rules
            .iter()
            .find(|r| r.slug == finding.rule)
            .map(|r| r.text.as_str())
    };
    let Some(rule_text) = rule_text else {
        return failed(format!("the rules checkout has no rule {}", finding.rule));
    };
    let Some((file_diff, context)) = file_of(diff, finding) else {
        return failed(format!("the diff has no file {}", finding.file));
    };
    let invoked = call::invoke(
        program,
        model,
        &schema(),
        &system(&finding.rule, rule_text),
        &message(finding, &file_diff, &context),
        None,
    );
    let verdict = invoked.body.and_then(|body| {
        let why = body["why"]
            .as_str()
            .filter(|why| !why.trim().is_empty())
            .ok_or("the answer gives no reason")?
            .trim()
            .to_string();
        match body["verdict"].as_str() {
            Some("confirmed") => Ok((true, why)),
            Some("rejected") => Ok((false, why)),
            _ => Err("the verdict is not confirmed or rejected".to_string()),
        }
    });
    Checked {
        model: invoked.model,
        verdict,
        usage: invoked.usage,
    }
}

#[cfg(test)]
#[path = "model_check_tests.rs"]
mod tests;
