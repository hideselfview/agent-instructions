//! The rules the coordinator confirms findings of most often on a branch's
//! first review. `builds rules-on-edit` shows each one beside the rule when
//! an agent first edits a file the rule covers. builds counts the confirmed
//! findings of each rule over the first reviews of the last `BRANCHES`
//! branches of every project's store. A rule with `LEAST_FINDINGS` findings
//! in that window is listed, with the paragraphs or list items of its
//! text that its findings quote most (`check::clause_in`). A rule that no
//! recent first review confirms leaves the list, and comes back when agents
//! break it again.

use rusqlite::Connection;

use crate::fail::Result;
use crate::review::check::{clause_in, squash};
use crate::review::record::{self, FirstReview};
use crate::review::rules::Rule;
use crate::text::note;

/// How many of the branches opened last the list counts.
pub const BRANCHES: usize = 30;
/// The fewest findings a rule needs on the counted branches to be listed.
const LEAST_FINDINGS: usize = 2;
/// The most quoted paragraphs the list shows under a rule.
const QUOTED_SHOWN: usize = 3;
/// How many characters of a paragraph the list shows, cut at the end of a
/// sentence.
const TEXT_LENGTH: usize = 200;

/// One rule, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Frequent {
    pub rule: String,
    /// How many confirmed findings of this rule the counted first reviews
    /// hold.
    pub findings: usize,
    /// The paragraphs or list items of the rule that its findings quote, the
    /// most quoted first. A finding whose clause no paragraph holds any more
    /// counts in `findings` and appears here under no paragraph (`frequent`
    /// reports it in `Counted::left_out`).
    pub quoted: Vec<Quoted>,
}

/// A paragraph or list item of a rule, as the list shows it, and how many of
/// the rule's findings quote it.
#[derive(Clone, Debug, PartialEq)]
pub struct Quoted {
    /// The opening sentences of the paragraph or list item, without markdown
    /// marks.
    pub text: String,
    pub findings: usize,
}

/// What `frequent` counted: the rules with at least `LEAST_FINDINGS`
/// findings, and a line for each finding it left out of the
/// counts or of the paragraphs, saying why.
#[derive(Clone, Debug, PartialEq)]
pub struct Counted {
    pub rules: Vec<Frequent>,
    pub left_out: Vec<String>,
}

/// Counts the confirmed findings of <reviews>, the first reviews of the
/// branches opened last, newest first, by rule. `frequent` leaves out a
/// finding whose rule is not among <rules>, since the checkout no longer has
/// that rule. A finding whose clause no paragraph of its rule holds any more
/// still counts under its rule.
pub fn frequent(reviews: &[&FirstReview], rules: &[Rule]) -> Counted {
    let mut found: Vec<Frequent> = Vec::new();
    let mut left_out = Vec::new();
    for review in reviews {
        for finding in &review.confirmed {
            let Some(rule) = rules.iter().find(|rule| rule.slug == finding.rule) else {
                left_out.push(format!(
                    "the confirmed findings of the rules leave out finding {}, whose rule {} the rules checkout no longer holds",
                    finding.id, finding.rule
                ));
                continue;
            };
            let at = match found.iter().position(|held| held.rule == finding.rule) {
                Some(at) => at,
                None => {
                    found.push(Frequent {
                        rule: finding.rule.clone(),
                        findings: 0,
                        quoted: Vec::new(),
                    });
                    found.len() - 1
                }
            };
            let frequent = &mut found[at];
            frequent.findings += 1;
            let Some(piece) = pieces(rule.body())
                .into_iter()
                .find(|piece| clause_in(&finding.clause, piece))
            else {
                left_out.push(format!(
                    "the confirmed findings of the rules count finding {} under rule {} but lists no paragraph for it, since no paragraph of the rule holds its clause",
                    finding.id, finding.rule
                ));
                continue;
            };
            let text = opening(&piece);
            match frequent.quoted.iter_mut().find(|held| held.text == text) {
                Some(held) => held.findings += 1,
                None => frequent.quoted.push(Quoted { text, findings: 1 }),
            }
        }
    }
    let mut rules: Vec<Frequent> = found
        .into_iter()
        .filter(|rule| rule.findings >= LEAST_FINDINGS)
        .collect();
    for rule in &mut rules {
        rule.quoted
            .sort_by_key(|quoted| std::cmp::Reverse(quoted.findings));
    }
    Counted { rules, left_out }
}

/// The paragraphs and top-level list items of a rule's <body>, without its
/// headings.
fn pieces(body: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for line in body.lines() {
        if line.trim().is_empty() || line.starts_with("- ") || line.starts_with('#') {
            if !piece.trim().is_empty() {
                pieces.push(std::mem::take(&mut piece));
            }
            piece.clear();
        }
        if !line.starts_with('#') {
            piece.push_str(line);
            piece.push('\n');
        }
    }
    if !piece.trim().is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// The whole sentences that open <piece>, before any list nested in it, up to
/// `TEXT_LENGTH` characters, or its first sentence when that one is longer.
fn opening(piece: &str) -> String {
    let lead: Vec<&str> = piece
        .lines()
        .enumerate()
        .take_while(|(at, line)| *at == 0 || !line.trim_start().starts_with("- "))
        .map(|(_, line)| line)
        .collect();
    let text = squash(&lead.join("\n"));
    let text = text.strip_prefix("- ").unwrap_or(&text);
    if text.len() <= TEXT_LENGTH {
        return text.to_string();
    }
    let ends: Vec<usize> = text.match_indices(". ").map(|(at, _)| at + 1).collect();
    let end = ends
        .iter()
        .rev()
        .find(|end| **end <= TEXT_LENGTH)
        .or(ends.first())
        .copied()
        .unwrap_or(text.len());
    text[..end].to_string()
}

/// The first reviews the list counts: those of <own>, the store of
/// <project>, and, from each store of <others>, only the confirmed findings
/// of the rules that <rules> holds under `rules/`, since those rules apply to
/// every project. A project's own rules count only in its own store. The
/// result is oldest first.
fn counted_reviews(
    own: Vec<FirstReview>,
    others: Vec<Vec<FirstReview>>,
    rules: &[Rule],
) -> Vec<FirstReview> {
    let shared = |slug: &str| {
        rules
            .iter()
            .any(|rule| rule.slug == slug && rule.path.starts_with("rules/"))
    };
    let mut all = own;
    for mut reviews in others {
        for review in &mut reviews {
            review.confirmed.retain(|finding| shared(&finding.rule));
        }
        all.extend(reviews);
    }
    all.sort_by_key(|review| review.at);
    all
}

/// The rules the coordinator confirmed findings of most often on the first
/// reviews of the recent branches of <project> and of the other projects,
/// whose findings of shared rules count too. <db> is the store of
/// <project>, <others> the stores of the other projects. Prints on stderr
/// each finding the count left out (`Counted::left_out`).
pub fn confirmed_often(
    db: &Connection,
    others: &[&Connection],
    rules: &[Rule],
) -> Result<Vec<Frequent>> {
    let own = record::first_reviews(db)?;
    let others = others
        .iter()
        .map(|db| record::first_reviews(db))
        .collect::<Result<Vec<_>>>()?;
    let reviews = counted_reviews(own, others, rules);
    let newest: Vec<&FirstReview> = reviews.iter().rev().take(BRANCHES).collect();
    let counted = frequent(&newest, rules);
    for why in &counted.left_out {
        note(why);
    }
    Ok(counted.rules)
}

impl Frequent {
    /// What an agent that edits a file the rule covers reads beside the rule:
    /// how many findings the recent first reviews confirmed of it, and the
    /// paragraphs of its text that they quote most.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "Recent first reviews confirmed {} of this rule. The paragraphs they quote most:\n",
            crate::text::counted(self.findings, "finding", "findings"),
        );
        for quoted in self.quoted.iter().take(QUOTED_SHOWN) {
            out.push_str(&format!("- {}x {}\n", quoted.findings, quoted.text));
        }
        out
    }
}

#[cfg(test)]
#[path = "frequent_tests.rs"]
mod tests;
