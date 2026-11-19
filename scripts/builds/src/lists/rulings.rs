//! The check that keeps a made-up ruling of the person's out of the lists and
//! the briefs: no stack entry, work item, closing reason or brief may tell an
//! agent or the person that the person decided something unless it names the
//! decision that records it or the rule that states it.
//!
//! A command that writes such text runs this check before it takes the
//! store's lock, as `builds item take` runs its spec check (spec_check.rs):
//! one reviewer with no tools (`review::call::invoke`) reads every text the
//! command is about to write and quotes each sentence that says the person
//! decided, ruled, chose, prefers, forbids or allowed something, and, in a
//! brief, each sentence that states a standing prohibition. builds finds each
//! quote in its text, sets aside a quote it does not find, and reads the
//! citations in each sentence it found: `decision #N`, which must name a
//! decision of the log, and a rule path, which must name a file of the rules
//! checkout that states rules. A sentence with no citation, or with one that
//! names nothing, refuses the command; builds prints each other sentence with
//! the title of each decision it cites. The check records nothing.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::item::{ItemDraft, ItemEdit};
use crate::lists::question::Blocked;
use crate::lists::{Id, Kind, Lists, Question};
use crate::review::Reviewer;
use crate::review::check::clause_in;

/// The first line of every message, which names the task.
const TASK: &str = "Task: rulings";

/// The model the reviewer runs: the check sorts sentences and reads no code.
const MODEL: &str = "sonnet";

/// The file of the rules checkout that holds the instructions every agent
/// loads, which `install.sh` makes ~/.claude/CLAUDE.md from.
const INSTRUCTIONS: &str = "instructions-agent.md";

/// What the texts a check reads are, which sets what the reviewer lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Stack entries, work items and the reasons that close questions: the
    /// reviewer lists each sentence that says the person decided something.
    Lists,
    /// A brief: the reviewer also lists each sentence that states a standing
    /// prohibition.
    Brief,
}

/// One text a command is about to write: what it is, such as "the
/// question's details", and its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub name: String,
    pub words: String,
}

impl Text {
    pub fn new(name: impl Into<String>, words: impl Into<String>) -> Text {
        Text {
            name: name.into(),
            words: words.into(),
        }
    }
}

/// The texts of <drafts>, the work items a request makes: each one's title
/// and details.
pub fn draft_texts(drafts: &[ItemDraft]) -> Vec<Text> {
    drafts
        .iter()
        .map(|draft| {
            Text::new(
                format!("the new work item \"{}\"", draft.title),
                format!("{}\n\n{}", draft.title, draft.details),
            )
        })
        .collect()
}

/// The texts <edit> gives item <id>: its new title and details, when it
/// gives them.
pub fn edit_texts(id: Id, edit: &ItemEdit) -> Vec<Text> {
    let mut texts = Vec::new();
    if let Some(title) = &edit.title {
        texts.push(Text::new(format!("the new title of item {id}"), title));
    }
    if let Some(details) = &edit.details {
        texts.push(Text::new(format!("the new details of item {id}"), details));
    }
    texts
}

/// The texts of the fates a closing gives the items that wait on the
/// question: each edit's title and details, and each drop's reason.
pub fn blocked_texts<'a>(blocked: impl IntoIterator<Item = (&'a Id, &'a Blocked)>) -> Vec<Text> {
    let mut texts = Vec::new();
    for (item, fate) in blocked {
        match fate {
            Blocked::Proceeds => {}
            Blocked::Edit(edit) => texts.extend(edit_texts(*item, edit)),
            Blocked::Drop(why) => {
                texts.push(Text::new(format!("the reason item {item} drops"), why))
            }
        }
    }
    texts
}

/// What a citation in a sentence names.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cite {
    /// What a `decision #N` names.
    Decision(CitedDecision),
    /// A file of the rules checkout that states rules, as its path there.
    Rule(String),
    /// A Markdown path that names no such file, and why.
    NoRule(String),
}

impl Cite {
    /// Why the citation names nothing builds keeps; None when it names a
    /// decision or a rule.
    fn missing(&self) -> Option<String> {
        match self {
            Cite::Decision(CitedDecision::Missing { id, what }) => {
                Some(format!("it cites decision {id}, and {id} is {what}"))
            }
            Cite::NoRule(why) => Some(why.clone()),
            Cite::Decision(CitedDecision::Decision { .. }) | Cite::Rule(_) => None,
        }
    }
}

/// A sentence the reviewer quoted, found in one of the texts, with what its
/// citations name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Claim {
    /// The name of the text builds found the sentence in.
    text: String,
    sentence: String,
    /// What the reviewer says the sentence claims the person decided.
    why: String,
    cites: Vec<Cite>,
}

impl Claim {
    /// Why the claim refuses the command: it cites nothing, or a citation
    /// names nothing. None when every citation names a decision or a rule.
    fn fault(&self) -> Option<String> {
        if self.cites.is_empty() {
            return Some("it names no decision (`decision #N`) and no rule path".to_string());
        }
        let missing: Vec<String> = self.cites.iter().filter_map(Cite::missing).collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing.join("; "))
        }
    }
}

/// What a check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Checked {
    claims: Vec<Claim>,
    /// Each quote builds found in no text.
    set_aside: Vec<String>,
}

impl Checked {
    /// The refusal, quoting each sentence that cites nothing or cites what
    /// builds does not keep; None when there is none.
    fn refusal(&self) -> Option<String> {
        let faults: Vec<String> = self
            .claims
            .iter()
            .filter_map(|claim| {
                claim.fault().map(|fault| {
                    format!(
                        "  in {}: \"{}\"\n    the check read it as: {}\n    {fault}",
                        claim.text,
                        claim.sentence.trim(),
                        claim.why.trim()
                    )
                })
            })
            .collect();
        if faults.is_empty() {
            return None;
        }
        Some(format!(
            "builds wrote nothing: each sentence below says the person decided something, or states a standing prohibition, and names no decision or rule that records it:\n{}\nIn the sentence itself, name the decision that records the person's choice (`decision #N`) or the path of the rule that states it (such as rules/<group>/<rule>.md), or take the sentence out when nothing records it. When the person has not decided, ask them: put the question on the stack with `builds question ask`, and say nothing about what they decided.",
            faults.join("\n")
        ))
    }

    /// What the command prints whatever it does: each sentence that cites
    /// what builds keeps, with the title of each decision it cites, and each
    /// quote builds set aside.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        for claim in &self.claims {
            if claim.fault().is_some() {
                continue;
            }
            let cites: Vec<String> = claim
                .cites
                .iter()
                .filter_map(|cite| match cite {
                    Cite::Decision(CitedDecision::Decision { id, title }) => {
                        Some(format!("decision {id} \"{title}\""))
                    }
                    Cite::Rule(path) => Some(format!("rule {path}")),
                    Cite::Decision(CitedDecision::Missing { .. }) | Cite::NoRule(_) => None,
                })
                .collect();
            notes.push(format!(
                "\"{}\" cites {}",
                claim.sentence.trim(),
                cites.join(" and ")
            ));
        }
        for sentence in &self.set_aside {
            notes.push(format!(
                "the rulings check set aside a quote it found in no text: \"{}\"",
                sentence.trim()
            ));
        }
        notes
    }
}

/// Runs the check of <texts>, which come from <source>, and fails with the
/// refusal when a sentence cites nothing or cites what builds does not keep.
/// Returns what the command prints (`Checked::notes`). Asks no reviewer when
/// every text is empty.
pub fn require_cited(
    reviewer: &Reviewer,
    lists: &Lists,
    source: Source,
    texts: &[Text],
) -> Result<Vec<String>> {
    let checked = check(reviewer, lists, source, texts)?;
    if let Some(refusal) = checked.refusal() {
        fail!("{refusal}");
    }
    Ok(checked.notes())
}

/// Runs the reviewer over <texts> and reads what each quoted sentence cites.
/// Fails when the reviewer gives no usable answer.
fn check(reviewer: &Reviewer, lists: &Lists, source: Source, texts: &[Text]) -> Result<Checked> {
    let texts: Vec<&Text> = texts
        .iter()
        .filter(|text| !text.words.trim().is_empty())
        .collect();
    if texts.is_empty() {
        return Ok(Checked::default());
    }
    let invoked = crate::review::call::invoke(
        &reviewer.program,
        MODEL,
        &schema(),
        &system(source),
        &message(&texts),
        None,
    );
    let quoted = match invoked.body.and_then(|body| quoted_of(&body)) {
        Ok(quoted) => quoted,
        Err(why) => fail!(
            "the rulings check gave no usable answer, so builds wrote nothing: {why}\nRun the command again."
        ),
    };
    // The rules checkout is read only when a sentence names a Markdown path.
    let mut rules: Option<Vec<String>> = None;
    let mut checked = Checked::default();
    for (sentence, why) in quoted {
        let Some(text) = texts.iter().find(|text| clause_in(&sentence, &text.words)) else {
            checked.set_aside.push(sentence);
            continue;
        };
        let mut cites: Vec<Cite> = decision_ids(&sentence)
            .into_iter()
            .map(|id| Cite::Decision(cited_decision(lists, id)))
            .collect();
        for path in md_paths(&sentence) {
            if rules.is_none() {
                rules = Some(rule_paths(&reviewer.rules_root)?);
            }
            let known = rules.as_deref().expect("read above");
            cites.push(rule_cite(known, &path, &reviewer.rules_root));
        }
        checked.claims.push(Claim {
            text: text.name.clone(),
            sentence,
            why,
            cites,
        });
    }
    Ok(checked)
}

/// The system prompt: what the reviewer lists for <source>.
fn system(source: Source) -> String {
    let (what, also) = match source {
        Source::Lists => (
            "text that an agent is about to give the person or another agent: a question for the person, a work item, or the reason that closes a question",
            "",
        ),
        Source::Brief => (
            "a brief: the instructions an agent follows for one task",
            "\n\nAlso list each sentence that states a standing prohibition or rule: one meant to hold beyond the task the brief describes, such as \"Agents never reword commit messages\" or \"Never edit the main checkout\".",
        ),
    };
    format!(
        "You read {what}. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.

List each sentence that says the person decided, ruled, chose, prefers, forbids or allowed something, or wants something done one way from now on. The person is the one the agents work for: the text may call them the person, the user, you, or by name.{also}

List each such sentence whether or not it names a decision or a rule: builds reads its citations itself. Skip a sentence that asks the person to decide, and one that says what an agent or the coordinator decided.

For each sentence you list, copy the whole sentence exactly into `sentence`, with any parentheses that follow it, and say in `why` what it says the person decided or what it forbids. builds looks for each sentence in the text and sets aside one it does not find."
    )
}

/// The message: the task and each text under its name.
fn message(texts: &[&Text]) -> String {
    let mut out = format!("{TASK}\n");
    for text in texts {
        out.push_str(&format!("\n# {}\n\n{}\n", text.name, text.words.trim_end()));
    }
    out
}

/// The JSON schema of the answer.
fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["claims"],
        "properties": {
            "claims": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sentence", "why"],
                    "properties": {
                        "sentence": {"type": "string"},
                        "why": {"type": "string"}
                    }
                }
            }
        }
    })
}

/// Each sentence and why of the answer <body>.
fn quoted_of(body: &Value) -> std::result::Result<Vec<(String, String)>, String> {
    let claims = body["claims"]
        .as_array()
        .ok_or("the answer has no claims list")?;
    claims
        .iter()
        .map(|claim| {
            let text = |key: &str| {
                claim[key]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("a claim of the answer has no {key}"))
            };
            Ok((text("sentence")?, text("why")?))
        })
        .collect()
}

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

/// Each word of <text> that is the path of a Markdown file, without the
/// marks and punctuation around it.
fn md_paths(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || "`()[]{}\"'<>,;".contains(c))
        .map(|word| word.trim_end_matches(['.', ':', '!', '?']))
        .filter(|word| word.len() > ".md".len() && word.ends_with(".md"))
        .map(str::to_string)
        .collect()
}

/// The files of the rules checkout at <root> that state rules, as paths
/// there: every rule a review reads, each project file
/// (projects/<project>.md) and the instructions every agent loads.
fn rule_paths(root: &Path) -> Result<Vec<String>> {
    let mut paths: Vec<String> = crate::review::rules::load(root, None)?
        .into_iter()
        .map(|rule| rule.path)
        .collect();
    let projects = root.join("projects");
    let listing = || format!("listing {}", projects.display());
    for entry in std::fs::read_dir(&projects).context(listing)? {
        let path = entry.context(listing)?.path();
        if path.is_file()
            && path.extension().is_some_and(|ext| ext == "md")
            && let Some(name) = path.file_name()
        {
            paths.push(format!("projects/{}", name.to_string_lossy()));
        }
    }
    if root.join(INSTRUCTIONS).is_file() {
        paths.push(INSTRUCTIONS.to_string());
    }
    Ok(paths)
}

/// What <path>, a Markdown path a sentence names, is to a citation: a rule
/// when it is one of <known> or ends with one after a folder.
fn rule_cite(known: &[String], path: &str, root: &Path) -> Cite {
    match known
        .iter()
        .find(|rule| path == rule.as_str() || path.ends_with(&format!("/{rule}")))
    {
        Some(rule) => Cite::Rule(rule.clone()),
        None => Cite::NoRule(format!(
            "it cites {path}, which is no rule, project file or {INSTRUCTIONS} of the rules checkout {}",
            root.display()
        )),
    }
}

/// What a `decision #N` in a text names: the rulings check, `question show`
/// and the dashboard all read it. The JSON names the variant in `found`.
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

/// <lines>, what the rulings check prints, one line each.
pub fn lines(lines: Vec<String>) -> String {
    lines.into_iter().map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
#[path = "rulings_tests.rs"]
mod tests;
