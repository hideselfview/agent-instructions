//! The kinds of findings the coordinator confirms most often on a branch's
//! first review. `builds help workflow` shows them to each implementing agent
//! before it writes its first draft. A kind is the paragraph or list item of a
//! rule's text that holds the clause a finding quotes (`check::clause_in`).
//! builds counts each kind over the first reviews of the last `BRANCHES`
//! branches and weighs each finding by how new its branch is: the weight
//! halves every `HALF_LIFE` branches back. A kind that no recent first review
//! has leaves the list, and comes back when agents make it again.

use rusqlite::Connection;

use crate::fail::Result;
use crate::review::check::{clause_in, squash};
use crate::review::record;
use crate::review::rules::Rule;

/// How many of the branches opened last the list counts.
pub const BRANCHES: usize = 30;
/// How many branches back a finding's weight halves.
pub const HALF_LIFE: f64 = 10.0;
/// The most kinds the list shows.
const SHOWN: usize = 6;
/// The least weight a kind needs for the list: one finding on the newest
/// branch weighs 1.
const LEAST_WEIGHT: f64 = 1.0;
/// How many characters of a kind's text the list shows, cut at the end of a
/// sentence.
const TEXT_LENGTH: usize = 200;

/// One kind of finding, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Frequent {
    pub rule: String,
    /// The opening sentences of the paragraph or list item, without markdown
    /// marks.
    pub text: String,
    /// How many confirmed findings of this kind the counted first reviews
    /// hold.
    pub findings: usize,
    /// The findings, each weighed by how new its branch is.
    pub weight: f64,
}

/// The kinds of the findings of <reviews> (each first review's rule and
/// clause pairs, newest branch first) with the most weight, heaviest first.
/// A finding whose clause no paragraph of its rule in <rules> holds counts
/// nowhere: its rule no longer has that text.
pub fn frequent(reviews: &[Vec<(String, String)>], rules: &[Rule]) -> Vec<Frequent> {
    let mut found: Vec<(String, Frequent)> = Vec::new();
    for (age, findings) in reviews.iter().enumerate() {
        let weight = 0.5_f64.powf(age as f64 / HALF_LIFE);
        for (slug, clause) in findings {
            let Some(rule) = rules.iter().find(|rule| &rule.slug == slug) else {
                continue;
            };
            let Some(piece) = pieces(rule.body())
                .into_iter()
                .find(|piece| clause_in(clause, piece))
            else {
                continue;
            };
            match found
                .iter_mut()
                .find(|(held, kind)| kind.rule == *slug && *held == piece)
            {
                Some((_, kind)) => {
                    kind.findings += 1;
                    kind.weight += weight;
                }
                None => found.push((
                    piece.clone(),
                    Frequent {
                        rule: slug.clone(),
                        text: opening(&piece),
                        findings: 1,
                        weight,
                    },
                )),
            }
        }
    }
    let mut kinds: Vec<Frequent> = found
        .into_iter()
        .map(|(_, kind)| kind)
        .filter(|kind| kind.weight >= LEAST_WEIGHT)
        .collect();
    kinds.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    kinds.truncate(SHOWN);
    kinds
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

/// The section of `builds help workflow` that lists the kinds of findings
/// the coordinator confirmed most often on the first reviews of <project>'s
/// recent branches.
pub fn section(db: &Connection, project: &str, rules: &[Rule]) -> Result<String> {
    let kinds = frequent(&record::confirmed_on_first_reviews(db, BRANCHES)?, rules);
    let mut out = String::from("\n\nWHAT REVIEWS CONFIRM MOST OFTEN\n\n");
    if kinds.is_empty() {
        out.push_str(&format!(
            "The first reviews of the last {BRANCHES} branches of project {project} hold too\nfew confirmed findings of any one kind to list here.\n"
        ));
        return Ok(out);
    }
    out.push_str(&format!(
        "The coordinator confirmed these kinds of findings most often on the first\nreview of the last {BRANCHES} branches of project {project}, and the list weighs newer\nbranches more. Each line names the rule, the number of such findings on\nthose branches, and the rule's text that the findings quote. Reread your\ndraft against each one before you commit.\n\n"
    ));
    for kind in kinds {
        out.push_str(&format!(
            "  {}, {}: {}\n",
            kind.rule,
            crate::text::counted(kind.findings, "finding", "findings"),
            kind.text
        ));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "frequent_tests.rs"]
mod tests;
