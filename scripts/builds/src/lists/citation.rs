//! What the citations in a sentence name, for the rulings check
//! (rulings.rs), `question show` and the dashboard. A sentence may cite each
//! place where a standing rule of the person's is written down:
//!
//! - a decision of the log: `decision #N`;
//! - a file of the rules checkout that states rules, by its path there: a
//!   rule (rules/<group>/<rule>.md, projects/<project>-rules/...), a project
//!   file (projects/<project>.md) or instructions-agent.md;
//! - the instructions every agent loads: `~/.claude/CLAUDE.md`, and a repo's
//!   CLAUDE.md, as `CLAUDE.md of repo <repo>` or the file's path in the repo's
//!   checkout ~/dev/<repo>;
//! - a section of a `builds help` page: `builds help workflow, DISPATCH`, or
//!   `builds help`, CARGO.
//!
//! builds checks that each one exists: the decision in the log, the file on
//! disk, the section among the page's headings.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::fail::{Context, Result};
use crate::lists::{Id, Kind, Lists, Question};

/// The file of the rules checkout that holds the instructions every agent
/// loads, which `install.sh` makes ~/.claude/CLAUDE.md from.
const INSTRUCTIONS: &str = "instructions-agent.md";

/// What a citation in a sentence names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cite {
    /// What a `decision #N` names.
    Decision(CitedDecision),
    /// A file of the rules checkout that states rules, as its path there.
    Rule(String),
    /// A CLAUDE.md every agent loads, as `builds ack` names it:
    /// "~/.claude/CLAUDE.md" or "the CLAUDE.md of repo <repo>".
    Instructions(String),
    /// A section of a `builds help` page: the page's topic, None for `builds
    /// help` itself, and the section's heading.
    Help {
        topic: Option<String>,
        section: String,
    },
    /// A path or section that names nothing builds finds, and why.
    Missing(String),
}

impl Cite {
    /// Why the citation names nothing builds keeps; None when it names a
    /// decision, a file or a section that exists.
    pub fn missing(&self) -> Option<String> {
        match self {
            Cite::Decision(CitedDecision::Missing { id, what }) => {
                Some(format!("it cites decision {id}, and {id} is {what}"))
            }
            Cite::Missing(why) => Some(why.clone()),
            Cite::Decision(CitedDecision::Decision { .. })
            | Cite::Rule(_)
            | Cite::Instructions(_)
            | Cite::Help { .. } => None,
        }
    }

    /// What the citation names, as the rulings check prints it; None for one
    /// that names nothing.
    pub fn named(&self) -> Option<String> {
        match self {
            Cite::Decision(CitedDecision::Decision { id, title }) => {
                Some(format!("decision {id} \"{title}\""))
            }
            Cite::Rule(path) => Some(format!("rule {path}")),
            Cite::Instructions(name) => Some(name.clone()),
            Cite::Help { topic, section } => Some(format!(
                "{}, {section}",
                crate::help::name(topic.as_deref())
            )),
            Cite::Decision(CitedDecision::Missing { .. }) | Cite::Missing(_) => None,
        }
    }
}

/// `builds help <topic>`, or `builds help` for None.
/// Where the citations of a sentence are looked up: the decision log, the
/// rules checkout and the home folder that holds ~/.claude and the repos'
/// checkouts in ~/dev.
pub struct Sources<'a> {
    lists: &'a Lists,
    rules_root: &'a Path,
    home: &'a Path,
    /// The files of the rules checkout that state rules, read when a
    /// sentence first names a Markdown path that no CLAUDE.md is.
    rules: Option<Vec<String>>,
}

impl<'a> Sources<'a> {
    pub fn new(lists: &'a Lists, rules_root: &'a Path, home: &'a Path) -> Sources<'a> {
        Sources {
            lists,
            rules_root,
            home,
            rules: None,
        }
    }

    /// What each citation in <sentence> names, in the order of the kinds
    /// the module lists.
    pub fn cites(&mut self, sentence: &str) -> Result<Vec<Cite>> {
        let mut cites: Vec<Cite> = decision_ids(sentence)
            .into_iter()
            .map(|id| Cite::Decision(cited_decision(self.lists, id)))
            .collect();
        let (repos, rest) = repo_instructions(sentence);
        for repo in repos {
            cites.push(self.repo_cite(&repo));
        }
        for path in md_paths(&rest) {
            let cite = if path == "CLAUDE.md" || path.ends_with("/CLAUDE.md") {
                self.instructions_cite(&path)
            } else {
                if self.rules.is_none() {
                    self.rules = Some(rule_paths(self.rules_root)?);
                }
                rule_cite(
                    self.rules.as_deref().expect("read above"),
                    &path,
                    self.rules_root,
                )
            };
            cites.push(cite);
        }
        cites.extend(help_cites(sentence));
        Ok(cites)
    }

    /// The CLAUDE.md of repo <repo>: the file in its checkout ~/dev/<repo>.
    fn repo_cite(&self, repo: &str) -> Cite {
        let file = self.repo_instructions_file(repo);
        if file.is_file() {
            Cite::Instructions(format!("the CLAUDE.md of repo {repo}"))
        } else {
            Cite::Missing(format!(
                "it cites the CLAUDE.md of repo {repo}, and {} does not exist",
                file.display()
            ))
        }
    }

    /// What <path>, a path to a CLAUDE.md that a sentence names, is to a
    /// citation: ~/.claude/CLAUDE.md, or the CLAUDE.md at the top of a
    /// repo's checkout ~/dev/<repo>, when the file exists.
    fn instructions_cite(&self, path: &str) -> Cite {
        let file = match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => PathBuf::from(path),
        };
        let neither = || {
            Cite::Missing(format!(
                "it cites {path}, which is neither ~/.claude/CLAUDE.md nor the CLAUDE.md of a repo's checkout in ~/dev"
            ))
        };
        if file == self.home.join(".claude/CLAUDE.md") {
            return if file.is_file() {
                Cite::Instructions("~/.claude/CLAUDE.md".to_string())
            } else {
                Cite::Missing(format!(
                    "it cites ~/.claude/CLAUDE.md, and {} does not exist",
                    file.display()
                ))
            };
        }
        let Some(repo) = file
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|repo| file == self.repo_instructions_file(repo))
        else {
            return neither();
        };
        self.repo_cite(&repo)
    }

    /// The CLAUDE.md at the top of <repo>'s checkout in the home folder.
    fn repo_instructions_file(&self, repo: &str) -> PathBuf {
        self.home
            .join(crate::host::clone_path(repo))
            .join("CLAUDE.md")
    }
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

/// The repos <text> names in `CLAUDE.md of repo <repo>`, and <text> with
/// those words taken out, so that no other citation reads them again.
fn repo_instructions(text: &str) -> (Vec<String>, String) {
    const WORDS: &str = "CLAUDE.md of repo ";
    let mut repos = Vec::new();
    let mut rest = String::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(WORDS) {
        let start = from + at;
        let name_at = start + WORDS.len();
        let name: String = text[name_at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        let name = name.trim_end_matches('.');
        rest.push_str(&text[from..start]);
        from = name_at + name.len();
        if !name.is_empty() {
            repos.push(name.to_string());
        }
    }
    rest.push_str(&text[from..]);
    (repos, rest)
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
        None => Cite::Missing(format!(
            "it cites {path}, which is no rule, project file or {INSTRUCTIONS} of the rules checkout {}",
            root.display()
        )),
    }
}

/// The sections of `builds help` pages that <text> cites: `builds help`, then
/// a page's topic when one follows, then the heading of one of that page's
/// sections, in capitals. A mention of `builds help` that no word in capitals
/// follows cites nothing.
fn help_cites(text: &str) -> Vec<Cite> {
    const WORDS: &str = "builds help";
    let mut cites = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(WORDS) {
        from += at + WORDS.len();
        let mut rest = text[from..].trim_start_matches(['`', ' ']);
        let word: String = rest.chars().take_while(char::is_ascii_lowercase).collect();
        let topic = if !word.is_empty() && crate::help::page(Some(&word)).is_some() {
            rest = &rest[word.len()..];
            Some(word)
        } else {
            None
        };
        let rest = rest.trim_start_matches(['`', ' ', ',', '(']);
        if !rest.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let words: String = rest
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || matches!(c, ' ' | ','))
            .collect();
        let page = crate::help::page(topic.as_deref()).expect("a known topic or none");
        let heading = crate::help::sections(page)
            .into_iter()
            .filter(|heading| {
                words.starts_with(heading)
                    && !words[heading.len()..].starts_with(|c: char| c.is_ascii_uppercase())
            })
            .max_by_key(|heading| heading.len());
        cites.push(match heading {
            Some(heading) => Cite::Help {
                topic,
                section: heading.to_string(),
            },
            None => Cite::Missing(format!(
                "it cites the section {} of {}, which has no such section",
                words.trim_end_matches([' ', ',']),
                crate::help::name(topic.as_deref())
            )),
        });
    }
    cites
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

#[cfg(test)]
#[path = "citation_tests.rs"]
mod tests;
