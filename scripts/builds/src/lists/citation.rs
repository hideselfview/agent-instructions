//! What the `decision #N` citations in a question name, for `question show`
//! and the dashboard: builds checks that each cited decision exists in the
//! log.

use serde::Serialize;

use crate::lists::{Id, Kind, Lists, Question};

/// The ids each `decision #N` in <text> names, in order, each once. The word
/// may be in any case.
fn decision_ids(text: &str) -> Vec<Id> {
    // ASCII lowercasing keeps every byte where it was.
    let lower = text.to_ascii_lowercase();
    let mut ids: Vec<Id> = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("decision") {
        from += at + "decision".len();
        let Some(digits) = lower[from..].trim_start().strip_prefix('#') else {
            continue;
        };
        let number: String = digits.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = Id::parse(&number)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// What `decision <id>` names in <lists>.
fn cited_decision(lists: &Lists, id: Id) -> CitedDecision {
    match lists.locate(id) {
        Some(found) if found.kind == Kind::Decision => CitedDecision::Decision {
            id,
            title: found.title.to_string(),
        },
        Some(found) => CitedDecision::Missing {
            id,
            what: format!(
                "the {} \"{}\", not a decision",
                found.kind.name(),
                found.title
            ),
        },
        None => CitedDecision::Missing {
            id,
            what: "no record".to_string(),
        },
    }
}

/// What a `decision #N` in a text names: `question show` and the
/// dashboard read it. The JSON names the variant in `found`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "found", rename_all = "lowercase")]
pub enum CitedDecision {
    /// A decision of the log, with its title.
    Decision { id: Id, title: String },
    /// A number that names no decision, and what it names instead, such as
    /// "no record".
    Missing { id: Id, what: String },
}

/// What each `decision #N` in the title and details of <question> names.
pub fn question_cites(lists: &Lists, question: &Question) -> Vec<CitedDecision> {
    decision_ids(&format!("{}\n{}", question.title, question.details))
        .into_iter()
        .map(|id| cited_decision(lists, id))
        .collect()
}

/// <cited> as one line of `question show`.
pub fn cited_line(cited: &CitedDecision) -> String {
    match cited {
        CitedDecision::Decision { id, title } => format!("cites decision {id}: {title}"),
        CitedDecision::Missing { id, what } => format!("cites decision {id}, and {id} is {what}"),
    }
}

#[cfg(test)]
#[path = "citation_tests.rs"]
mod tests;
