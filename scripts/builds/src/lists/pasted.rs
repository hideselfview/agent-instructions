//! The check that a brief carries no coding rule and no workflow text.
//! builds gives an agent each rule that covers a file when it first edits the
//! file (`rules_on_edit`), and the workflow when it runs `builds ack`, so a
//! brief that pastes either one gives the agent a second, older copy.
//!
//! A brief pastes a text when it holds the same run of `RUN` lines, one after
//! another, as the text does, ignoring each line's indent and the blank
//! lines. A run shorter than `MOST_CHARS` characters is no paste, since a
//! brief may quote a sentence of a rule.

use std::collections::HashMap;
use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::spec_check::Brief;

/// How many lines in a row make a paste.
const RUN: usize = 3;
/// The fewest characters a run of lines holds to count as a paste.
const MOST_CHARS: usize = 90;

/// A text that builds supplies to agents, and what to call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

/// What a brief pasted: the source and the first line of the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pasted {
    pub source: String,
    pub line: String,
}

/// The texts builds supplies to agents: each coding rule of the checkout at
/// <rules_root>, then the workflow page and the lists overview that `builds
/// ack` prints.
pub fn sources(rules_root: &Path) -> Result<Vec<Source>> {
    let mut sources: Vec<Source> = crate::review::rules::load(rules_root, None)?
        .into_iter()
        .map(|rule| Source {
            name: rule.path.clone(),
            text: rule.body().to_string(),
        })
        .collect();
    sources.push(Source {
        name: "builds help workflow".to_string(),
        text: crate::help::WORKFLOW_HELP.to_string(),
    });
    sources.push(Source {
        name: "builds help lists".to_string(),
        text: crate::help::LISTS_OVERVIEW.to_string(),
    });
    Ok(sources)
}

/// The non-blank lines of <text>, each without its indent.
fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// The first run of <brief>'s lines that one of <sources> holds in the same
/// order, None when it holds none.
pub fn pasted_from(brief: &str, sources: &[Source]) -> Option<Pasted> {
    let mut held: HashMap<String, (&str, &str)> = HashMap::new();
    for source in sources {
        let lines = lines(&source.text);
        for run in lines.windows(RUN) {
            held.entry(run.join("\n"))
                .or_insert((source.name.as_str(), run[0]));
        }
    }
    let lines = lines(brief);
    lines.windows(RUN).find_map(|run| {
        let joined = run.join("\n");
        if joined.chars().count() < MOST_CHARS {
            return None;
        }
        held.get(&joined).map(|(source, line)| Pasted {
            source: source.to_string(),
            line: line.to_string(),
        })
    })
}

/// Fails when <brief> pastes a coding rule of the checkout at <rules_root> or
/// the workflow text.
pub fn require_unpasted(brief: &Brief, rules_root: &Path) -> Result<()> {
    if let Some(pasted) = pasted_from(&brief.text, &sources(rules_root)?) {
        fail!(
            "the brief {} pastes text of {}, from the line {:?}. builds gives an agent each coding rule when it first edits a file the rule covers, and the workflow when it runs `builds ack`, so a brief carries neither: take the pasted text out, and write what the work should do",
            brief.name,
            pasted.source,
            pasted.line
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "pasted_tests.rs"]
mod tests;
