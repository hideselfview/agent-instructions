//! What one reviewer is told and how it must answer. The rules, the
//! project's instructions, the brief, the diff and the open confirmed
//! findings of the group's rules are all in its prompt.

use serde_json::{Value, json};

use crate::lists::spec_check::Brief;
use crate::review::groups::Group;
use crate::review::record::Finding;
use crate::review::rules::Rule;
use crate::text::one_line;

/// The name the group's project-facts verdict goes by.
pub const PROJECT_FACTS: &str = "project-facts";

pub const SYSTEM: &str = "You review one change against one group of rules. The message holds the change. When you have read-only tools, they read the repository as it is at the commit under review. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.";

pub struct Input<'a> {
    pub group: &'a Group,
    pub rules: &'a [&'a Rule],
    /// The project's always-loaded instructions; empty when it has none.
    pub project_text: &'a str,
    /// The landing's brief, when it has one.
    pub brief: Option<&'a crate::lists::spec_check::Brief>,
    pub diff: &'a str,
    /// The new text of the diff's file around each hunk, when the group
    /// reviews one file per call; empty otherwise.
    pub context: &'a str,
    /// The open confirmed findings of the group's rules, which the reviewer
    /// checks against the code as it is now.
    pub open: &'a [&'a Finding],
}

/// The verdict names a group answers for: its rules, then project-facts.
pub fn answer_names(group: &Group, rules: &[&Rule]) -> Vec<String> {
    let mut names: Vec<String> = rules.iter().map(|r| r.slug.clone()).collect();
    if group.project_facts {
        names.push(PROJECT_FACTS.to_string());
    }
    names
}

/// The JSON schema of every answer. It is the same for every group, so the
/// calls of one landing share their prefix in the prompt cache; builds checks
/// that the rules named are the group's own and that nothing is missing.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "summary": {"type": "string"},
            "follow_up": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "verdict"],
                    "properties": {
                        "id": {"type": "integer"},
                        "verdict": {
                            "anyOf": [
                                {"type": "string", "enum": ["fixed"]},
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["still_open"],
                                    "properties": {"still_open": {"type": "string"}}
                                }
                            ]
                        }
                    }
                }
            },
            "sites": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["file", "line", "kind", "code", "verdict"],
                    "additionalProperties": false,
                    "properties": {
                        "file": {"type": "string"},
                        "line": {"type": "integer"},
                        "kind": {"type": "string"},
                        "code": {"type": "string"},
                        "verdict": {"type": "string", "enum": ["finding", "none"]},
                        "rule": {"type": "string"},
                        "clause": {"type": "string"},
                        "problem": {"type": "string"}
                    }
                }
            },
            "rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["rule", "verdict", "findings"],
                    "additionalProperties": false,
                    "properties": {
                        "rule": {"type": "string"},
                        "verdict": {"type": "string", "enum": ["finding", "none"]},
                        "findings": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["clause", "file", "line", "code", "problem"],
                                "additionalProperties": false,
                                "properties": {
                                    "clause": {"type": "string"},
                                    "file": {"type": "string"},
                                    "line": {"type": "integer"},
                                    "code": {"type": "string"},
                                    "problem": {"type": "string"}
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// What the project and the landing give every group: the project's
/// instructions and the brief.
fn project_part(input: &Input) -> String {
    let mut out = String::new();
    if !input.project_text.is_empty() {
        out.push_str(&format!(
            "# Project instructions\n\n{}\n",
            input.project_text.trim_end()
        ));
    }
    if let Some(Brief { name: path, text }) = input.brief {
        out.push_str(&format!(
            "\n# The landing's brief ({path})\n\n{}\n",
            text.trim_end()
        ));
    }
    out
}

/// What the reviewer reads about the change: the diff, the new text of the
/// file around its hunks when the group reads one file at a time, and the
/// open confirmed findings.
fn change_part(input: &Input) -> String {
    let mut out = format!("{DIFF_START}{}\n```\n", input.diff.trim_end());
    if !input.context.is_empty() {
        out.push_str(&format!(
            "\n# The file's new text around each hunk\n\n```\n{}\n```\n",
            input.context.trim_end()
        ));
    }
    if !input.open.is_empty() {
        out.push_str("\n# Open findings\n\nEarlier reviews raised these findings and the coordinator confirmed them. Check each problem against the current branch and include its verdict in `follow_up`.\n");
        for finding in input.open {
            out.push_str(&format!(
                "\n- Finding {} ({}), {}:{}\n  code: {}\n  problem: {}\n",
                finding.id,
                finding.rule,
                finding.file,
                finding.line,
                one_line(&finding.code),
                one_line(&finding.problem)
            ));
        }
    }
    out
}

/// How the diff starts in a message `change_part` wrote.
const DIFF_START: &str = "# Diff\n\n```diff\n";

/// The diff a reviewer's message held, as `change_part` wrote it; None when
/// the message holds no diff. Every line of a diff starts with its mark or a
/// header word, so the first line of three backticks closes the diff.
pub fn diff_of_message(message: &str) -> Option<&str> {
    let rest = message.strip_prefix(DIFF_START)?;
    let end = rest.find("\n```\n")?;
    Some(&rest[..end])
}

/// How the group's part of a system prompt starts.
const GROUP_START: &str = "\n---\n\nGroup: ";

/// What is the group's own: its question, how to answer and its rules.
pub fn group_part(input: &Input) -> String {
    let group = input.group;
    let mut out = String::new();
    out.push_str(&format!(
        "{GROUP_START}{}\nQuestion: {}\n\n",
        group.name, group.question
    ));
    out.push_str("You answer for these rules:\n");
    for rule in input.rules {
        let kind = if rule.blocking {
            "blocking"
        } else {
            "reported only"
        };
        out.push_str(&format!("- {} ({kind})\n", rule.slug));
    }
    if group.project_facts {
        out.push_str(&format!(
            "- {PROJECT_FACTS} (reported only): the project facts in the project instructions above\n"
        ));
    }
    out.push_str(
        "\nFor open findings, give one entry in `follow_up` for every finding the message lists, even when you find no new issues. Use the finding number as `id`.\n\
         - Check whether the current branch still has the problem. Read the diff for fixes in other files, including tests, and open the relevant files to see what the code does. The original file and line identify where the earlier review found the problem. A fix can leave that line unchanged.\n\
         - Use the string `fixed` as the verdict if the problem is gone. If the problem remains, give an object with `still_open` set to a sentence that explains why. Omit `follow_up` when the message lists no open findings.\n",
    );
    if let Some(sites) = group.sites.as_deref().filter(|_| group.enumerate) {
        out.push_str(&format!(
            "\nFor new findings, use `summary` and `sites`. Omit `rules`.\n\
             - The message holds one changed file: its diff, then its new text around each hunk with line numbers. Read the new text to see what each value is, where it comes from and what the code around the site does with it.\n\
             - First list every site in the diff that {sites}. Give each site its file and kind, and copy its code exactly from the diff.\n\
             - Consider only sites on lines the diff adds or removes, the lines that start with `+` or `-`. The new text around each hunk, and the diff's lines that start with a space, are there to read, not to judge: the change does not touch them, or an earlier review read them. builds drops a finding on any of them. A diff with no such site has an empty `sites`, and `summary` says so; otherwise `summary` says in a sentence what the diff does.\n\
             - Then give each site a verdict against the rules listed above: `none` when it breaks none of them; otherwise `finding`, with `rule` set to the rule's name, `clause` copied exactly from that rule's text below, and one or two plain sentences on what goes wrong. A site that breaks two rules is listed twice, once per rule.\n",
        ));
    } else {
        out.push_str(
            "\nFor new findings, use `rules`. Omit `summary` and `sites`.\n\
             - Give one verdict for every rule listed: \"finding\" or \"none\". A rule the change does not touch gets \"none\" and an empty findings list.\n\
             - For each finding, copy the clause exactly from the rule below and the code exactly from the diff. Give the file's path. In one or two plain sentences, say what goes wrong.\n\
             - Raise findings only for lines the diff adds or removes, the lines that start with `+` or `-`. The diff's lines that start with a space are there to read, not to judge: the change does not touch them, or an earlier review read them. builds drops a finding on any of them.\n",
        );
    }
    out.push_str("- Give the line where the quote starts in the new file. Use the old file's line number when you quote removed code.\n");
    out.push_str("- When a site's verdict depends on a function or file outside the diff, open it before judging.\n");
    if group.commit_text {
        out.push_str(&format!(
            "- The diff holds the change's commit messages as one more file that the diff adds, {}. The repository holds no such file.\n",
            crate::review::files::COMMIT_MESSAGE
        ));
    }
    if let Some(extra) = &group.instructions {
        out.push_str(&format!("- {extra}\n"));
    }
    out.push_str("\n# Rules\n");
    for rule in input.rules {
        out.push_str(&format!("\n## {}\n\n{}\n", rule.slug, rule.text.trim_end()));
    }
    out
}

/// What one reviewer is sent: the system prompt holds everything the calls
/// of one group share (project instructions, brief, the group's question and
/// rules), so the prompt cache serves a group's later calls from it; the
/// message holds the change.
pub struct Parts {
    pub system: String,
    pub message: String,
}

pub fn build(input: &Input) -> Parts {
    Parts {
        system: system_text(input),
        message: change_part(input),
    }
}

fn system_text(input: &Input) -> String {
    format!("{SYSTEM}\n\n{}{}", project_part(input), group_part(input))
}

/// The prompt a group is given apart from what each review brings (the
/// change, the project's instructions and the brief): the system prompt's
/// opening, the group's part, its passes, whether it reads the commit
/// messages, and the answer schema. The store keeps it with every call, so
/// calls with the same model and the same prompt ran the same setup.
pub(super) fn setup(
    group: &Group,
    rules: &[&Rule],
    reviewer: &super::provider::Selected,
) -> String {
    let system = system_text(&Input {
        group,
        rules,
        project_text: "",
        brief: None,
        diff: "",
        context: "",
        open: &[],
    });
    let schema = schema();
    let prompt = setup_text(
        &reviewer.program().instructions(&system),
        group.passes,
        group.commit_text,
        &reviewer.program().schema(&schema),
    );
    reviewer.setup(prompt)
}

fn setup_text(system: &str, passes: usize, commit_text: bool, schema: &Value) -> String {
    format!(
        "{system}\n# Settings\n\npasses: {passes}\ncommit messages read: {}\n\n# Answer schema\n\n{schema}\n",
        if commit_text { "yes" } else { "no" },
    )
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
