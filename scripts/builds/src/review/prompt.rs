//! What one reviewer is told and how it must answer. A reviewer has no tools:
//! the rules, the project's instructions, the principles, the brief, the
//! settled findings and the diff are all in its prompt.

use serde_json::{Value, json};

use crate::review::groups::Group;
use crate::review::rules::Rule;

/// The name the group's project-facts verdict goes by.
pub const PROJECT_FACTS: &str = "project-facts";

pub const SYSTEM: &str = "You review one change against one group of rules. You have no tools: everything you need is in the message. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.";

/// A finding the coordinator already settled, passed to a rerun so the
/// reviewer does not raise it again, reworded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub rule: String,
    pub file: String,
    pub code: String,
    /// `confirmed` or `dropped`.
    pub how: String,
    pub why: String,
}

pub struct Input<'a> {
    pub group: &'a Group,
    pub rules: &'a [&'a Rule],
    /// The project's always-loaded instructions; empty when it has none.
    pub project_text: &'a str,
    /// (name, text) of each principle the group names.
    pub principles: &'a [(String, String)],
    /// (path, text) of the landing's brief, when it has one.
    pub brief: Option<&'a (String, String)>,
    pub settled: &'a [Settled],
    pub diff: &'a str,
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

/// What every group of one landing is given first, identical for all of them:
/// the project's instructions, the brief and the diff. The prompt cache serves
/// the later calls of a landing from this prefix.
pub fn shared(input: &Input) -> String {
    let mut out = String::new();
    if !input.project_text.is_empty() {
        out.push_str(&format!(
            "# Project instructions\n\n{}\n",
            input.project_text.trim_end()
        ));
    }
    if let Some((path, text)) = input.brief {
        out.push_str(&format!(
            "\n# The landing's brief ({path})\n\n{}\n",
            text.trim_end()
        ));
    }
    out.push_str(&format!(
        "\n# Diff\n\n```diff\n{}\n```\n",
        input.diff.trim_end()
    ));
    out
}

/// What is the group's own: its question, how to answer, its rules, its
/// principles and the findings already settled.
pub fn group_part(input: &Input) -> String {
    let group = input.group;
    let mut out = String::new();
    out.push_str(&format!(
        "\n---\n\nGroup: {}\nQuestion: {}\n\n",
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
    if group.enumerate {
        out.push_str(
            "\nHow to answer. Put your answer in `summary` and `sites`, and leave `rules` out.\n\
             - First list every site in the diff that handles rows a query returned, in Rust, Swift or Kotlin: a filter, a pick (first, find, first(where:)), a membership test (contains, isSubset, a Set of ids), a count, a sort, a group, a join, a map that drops rows, or a subscription taken once. Give each site its file, the line number in the new file, its kind, and its code copied exactly from the diff.\n\
             - Then give each site a verdict against each \"Never\" clause of the rule: `finding`, with the clause copied exactly from the rule's text below and one or two plain sentences on what goes wrong, or `none`.\n\
             - Consider only sites on lines the diff adds or changes. A diff with no such site has an empty `sites`, and `summary` says so; otherwise `summary` says in a sentence what the diff does with returned rows.\n\
             - Do not raise again a finding listed under \"Findings already settled\".\n",
        );
    } else {
        out.push_str(
            "\nHow to answer. Put your answer in `rules`, and leave `summary` and `sites` out.\n\
             - Give one verdict for every rule listed: \"finding\" or \"none\". A rule the change does not touch gets \"none\" and an empty findings list.\n\
             - A finding has three quotes and a sentence. The clause is copied exactly from that rule's text below. The code is copied exactly from the diff, with the file's path and the line number in the new file. The problem says in one or two plain sentences what goes wrong.\n\
             - Raise findings only for lines the diff adds or changes.\n\
             - Do not raise again a finding listed under \"Findings already settled\".\n",
        );
    }
    if let Some(extra) = &group.instructions {
        out.push_str(&format!("- {extra}\n"));
    }
    for (name, text) in input.principles {
        out.push_str(&format!("\n# Principle: {name}\n\n{}\n", text.trim_end()));
    }
    out.push_str("\n# Rules\n");
    for rule in input.rules {
        out.push_str(&format!("\n## {}\n\n{}\n", rule.slug, rule.text.trim_end()));
    }
    if !input.settled.is_empty() {
        out.push_str("\n# Findings already settled\n\n");
        for s in input.settled {
            out.push_str(&format!(
                "- {} in {}, code `{}`: settled {}: {}\n",
                s.rule,
                s.file,
                s.code.replace('\n', " "),
                s.how,
                s.why
            ));
        }
    }
    out
}

/// The whole prompt: the shared part first, then the group's own.
pub fn build(input: &Input) -> String {
    format!("{}{}", shared(input), group_part(input))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
