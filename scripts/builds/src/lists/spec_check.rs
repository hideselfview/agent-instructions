//! The check of a brief against the product spec that `builds item take`
//! runs before items start with the brief. For each worktree the take names
//! whose repo has spec files (the project's spec globs), one reviewer runs in
//! that worktree with read-only tools: `builds review`'s runner, with the
//! model of the review group that holds the rule
//! `every-change-agrees-with-its-product-spec`, whose text is the reviewer's
//! question. The reviewer quotes each sentence of the brief that states a
//! behavior the spec states otherwise (a conflict, with the spec's text) or
//! doesn't state at all (unstated). builds finds each quote in the brief and
//! in the spec file, reads the spec line from the file, and sets aside a
//! finding whose quote it does not find. take records every check in the
//! tables of spec_check.sql, which this module alone writes; a conflict
//! refuses the take unless the person decided it (`--spec-decided`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::design_paths::Globs;
use crate::lists::{Id, Kind, Lists, Stamp};
use crate::review::call::Usage;
use crate::review::check::reviewers_map;
use crate::review::check::{clause_in, squash};
use boards::{git, git_text};

/// The rule whose text the reviewer follows and whose group's model it runs.
pub const RULE: &str = "every-change-agrees-with-its-product-spec";

/// The first line of every message, which names the task.
const TASK: &str = "Task: spec-check";

/// One worktree of a take whose repo has spec files, and what the check reads
/// there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub repo: String,
    pub worktree: PathBuf,
    /// The commit the worktree's HEAD is at when the check reads its spec files.
    pub head: String,
    /// The repo's spec files that the worktree holds, as paths in the repo.
    pub spec_files: Vec<String>,
}

/// The targets among <worktrees> (repo, worktree): each one whose repo has
/// spec globs in <lists>. Fails when a repo's globs match no file its
/// worktree tracks, since the check would then read no spec.
pub fn targets(lists: &Lists, worktrees: &[(String, PathBuf)]) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    for (repo, worktree) in worktrees {
        let globs = Globs::of(lists, repo);
        if globs.spec.is_empty() {
            continue;
        }
        let tracked = git_text(
            git(worktree).args(["ls-files", "-z"]),
            "list the worktree's files",
        )?;
        let spec_files: Vec<String> = tracked
            .split('\0')
            .filter(|path| !path.is_empty() && globs.names_spec_file(path))
            .map(str::to_string)
            .collect();
        if spec_files.is_empty() {
            fail!(
                "project {} names the spec files {} for repo {repo}, and {} tracks none of them, so the spec check has no spec to read; fix the globs with `builds lists design-globs {} --spec-files {repo}:GLOB,...`",
                lists.project,
                globs.spec.join(", "),
                worktree.display(),
                lists.project
            );
        }
        let head = git_text(git(worktree).args(["rev-parse", "HEAD"]), "read HEAD")?
            .trim()
            .to_string();
        targets.push(Target {
            repo: repo.clone(),
            worktree: worktree.clone(),
            head,
            spec_files,
        });
    }
    Ok(targets)
}

/// The brief a take names: its name as given (`path` or `path#section`) and
/// its file's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brief {
    pub name: String,
    pub text: String,
}

impl Brief {
    /// The brief <name> from <file>; fails naming the file when it does not
    /// exist.
    pub fn read(name: &str, file: &Path) -> Result<Brief> {
        match std::fs::read_to_string(file) {
            Ok(text) => Ok(Brief {
                name: name.to_string(),
                text,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fail!("brief {name} does not exist: no file {}", file.display())
            }
            Err(error) => fail!("reading the brief {}: {error}", file.display()),
        }
    }

    /// The section the name gives after `#`, if any.
    fn section(&self) -> Option<&str> {
        self.name.split_once('#').map(|(_, section)| section)
    }
}

/// What every check of one take reads from the rules checkout.
pub struct Setup {
    /// The model of the review group that holds the rule.
    pub model: crate::review::Selection,
    /// The rule's whole file.
    pub rule_text: String,
    /// The rules checkout's commit, empty when it is no repo.
    pub rules_version: String,
}

impl Setup {
    /// The rule and its group's model, from the rules checkout at <root>.
    pub fn load(root: &Path) -> Result<Setup> {
        let rules = crate::review::rules::load(root, None)?;
        let Some(rule) = rules.iter().find(|rule| rule.slug == RULE) else {
            fail!(
                "the rules checkout {} has no rule {RULE}, whose text the spec check asks the reviewer to follow",
                root.display()
            );
        };
        let groups = crate::review::groups::Groups::load(root)?;
        let Some(group) = groups
            .groups
            .iter()
            .find(|group| group.rules.iter().any(|slug| slug == RULE))
        else {
            fail!(
                "the rule {RULE} ({}) is in no review group, whose model the spec check runs",
                rule.path
            );
        };
        Ok(Setup {
            model: group.model.clone(),
            rule_text: rule.text.clone(),
            rules_version: crate::review::run::rules_version(root),
        })
    }
}

/// What builds made of a finding's quotes: <T> when it found every quote,
/// or why it set the finding aside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Quotes<T> {
    Found(T),
    SetAside(String),
}

impl Quotes<()> {
    /// Whether <brief> holds <quote>.
    fn in_brief(brief: &Brief, quote: &str) -> Quotes<()> {
        if clause_in(quote, &brief.text) {
            Quotes::Found(())
        } else {
            Quotes::SetAside("the brief holds no such sentence".to_string())
        }
    }
}

/// A conflict, with the spec's text, or an unstated behavior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindingKind {
    Conflict(SpecQuote),
    /// What builds made of the brief's quote.
    Unstated(Quotes<()>),
}

impl FindingKind {
    fn word(&self) -> &'static str {
        match self {
            FindingKind::Conflict(_) => "conflict",
            FindingKind::Unstated(_) => "unstated",
        }
    }

    /// Why builds set the finding aside, if it did.
    fn set_aside(&self) -> Option<&str> {
        match self {
            FindingKind::Conflict(SpecQuote {
                line: Quotes::SetAside(why),
                ..
            })
            | FindingKind::Unstated(Quotes::SetAside(why)) => Some(why),
            _ => None,
        }
    }
}

/// What a check found, after builds looked for its quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    /// The brief's sentence.
    pub brief_quote: String,
    pub why: String,
}

/// The spec's text a conflict quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecQuote {
    pub file: String,
    pub quote: String,
    /// The line of the file where builds found the text.
    pub line: Quotes<i64>,
}

/// One reviewer's check of the brief in one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub target: Target,
    pub model: String,
    pub rules_version: String,
    pub system: String,
    pub message: String,
    /// The reviewer's JSON answer, or why it gave none.
    pub output: String,
    pub usage: Usage,
    /// Why the check gave no usable answer.
    pub failure: Option<String>,
    pub findings: Vec<Finding>,
}

impl Checked {
    /// Each conflict whose quotes builds found, with its spec quote and the
    /// line builds found the spec's text on.
    fn conflicts(&self) -> impl Iterator<Item = (&Finding, &SpecQuote, i64)> {
        self.findings
            .iter()
            .filter_map(|finding| match &finding.kind {
                FindingKind::Conflict(spec) => match spec.line {
                    Quotes::Found(line) => Some((finding, spec, line)),
                    Quotes::SetAside(_) => None,
                },
                FindingKind::Unstated(_) => None,
            })
    }

    /// Each unstated behavior whose quote builds found.
    fn unstated(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.kind == FindingKind::Unstated(Quotes::Found(())))
    }
}

/// The system prompt: what the check is and the rule it follows.
fn system(setup: &Setup) -> String {
    format!(
        "You check an implementation brief against its project's product spec before an agent builds it. Your read-only tools read the repository the agent will work in, and the message names its spec files. Answer with the JSON the schema asks for and nothing else. Write plain, literal, active sentences.

The check follows this rule:

{}

How to check the brief:

1. Read the brief in the message. Pick each sentence that states product behavior: what a person sees or does in the product, what the product does, or a schema or sync shape. Skip each sentence about code structure, tests, files, commits, checks or how the agent works.
2. For each sentence you picked, search the spec files for the behavior's words with Grep, and Read the whole section around each match.
3. When the spec states the behavior otherwise than the brief does, list a conflict: copy the brief's sentence exactly into `brief`, name the spec file in `spec_file` as the message lists it, copy the spec's text that states the behavior exactly into `spec` (whole lines, as the file holds them), and say in `why` how the two differ.
4. When the spec states nothing on the behavior, list it as unstated: copy the brief's sentence exactly into `brief`, and say in `why` which words you searched the spec for. Storage the brief adds (a table, a column, an index or a synced field) that the spec's schema text, such as a table's row or a sync shape, does not list is unstated, not a conflict: the commit adds it to the spec. Such storage is a conflict only when the spec's text says otherwise, such as a row that says the table does not store that value.
5. A sentence that agrees with the spec goes in neither list.

builds looks for each quote in the brief and in the spec file, and sets aside a finding whose quote it does not find.",
        setup.rule_text.trim_end()
    )
}

/// The message: the task, the spec files and the brief.
fn message(brief: &Brief, target: &Target) -> String {
    let mut out = format!(
        "{TASK}\n\nSpec files: {}\n\n# The brief ({})\n\n",
        target.spec_files.join(", "),
        brief.name
    );
    if let Some(section) = brief.section() {
        out.push_str(&format!(
            "The take names the section \"{section}\" of this brief: check that section.\n\n"
        ));
    }
    out.push_str(brief.text.trim_end());
    out.push('\n');
    out
}

/// The JSON schema of the answer.
fn schema() -> Value {
    let quoted = |required: &[&str]| {
        let properties: serde_json::Map<String, Value> = required
            .iter()
            .map(|key| (key.to_string(), json!({"type": "string"})))
            .collect();
        json!({
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": properties
            }
        })
    };
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["conflicts", "unstated"],
        "properties": {
            "conflicts": quoted(&["brief", "spec_file", "spec", "why"]),
            "unstated": quoted(&["brief", "why"])
        }
    })
}

/// Runs the reviewer <program> over <brief> in each of <targets>, at once.
pub fn check_all(
    program: &crate::review::Program,
    setup: &Setup,
    brief: &Brief,
    targets: &[Target],
) -> Vec<Checked> {
    reviewers_map(targets, |target| check(program, setup, brief, target))
}

/// Runs the reviewer <program> over <brief> in <target> and checks its quotes.
fn check(
    program: &crate::review::Program,
    setup: &Setup,
    brief: &Brief,
    target: &Target,
) -> Checked {
    let system = system(setup);
    let message = message(brief, target);
    let invoked = crate::review::call::invoke(
        program,
        &setup.model,
        &schema(),
        &system,
        &message,
        Some(&target.worktree),
    );
    let output = match &invoked.body {
        Ok(body) => body.to_string(),
        Err(why) => format!("no answer: {why}"),
    };
    let (failure, findings) = match invoked
        .body
        .and_then(|body| findings_of(&body, brief, target))
    {
        Ok(findings) => (None, findings),
        Err(why) => (Some(why), Vec::new()),
    };
    Checked {
        target: target.clone(),
        model: invoked.model,
        rules_version: setup.rules_version.clone(),
        system: program.instructions(&system).into_owned(),
        message,
        output,
        usage: invoked.usage,
        failure,
        findings,
    }
}

/// The findings of the answer <body>, each with builds' check of its quotes.
fn findings_of(
    body: &Value,
    brief: &Brief,
    target: &Target,
) -> std::result::Result<Vec<Finding>, String> {
    let list = |key: &str| -> std::result::Result<Vec<Value>, String> {
        body[key]
            .as_array()
            .cloned()
            .ok_or_else(|| format!("the answer has no {key} list"))
    };
    let text = |entry: &Value, key: &str| -> std::result::Result<String, String> {
        entry[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("an answer entry has no {key}"))
    };
    let mut findings = Vec::new();
    for entry in list("conflicts")? {
        let brief_quote = text(&entry, "brief")?;
        let file = text(&entry, "spec_file")?;
        let quote = text(&entry, "spec")?;
        let line = match Quotes::in_brief(brief, &brief_quote) {
            Quotes::SetAside(why) => Quotes::SetAside(why),
            Quotes::Found(()) => match spec_line(target, &file, &quote) {
                Ok(line) => Quotes::Found(line),
                Err(why) => Quotes::SetAside(why),
            },
        };
        findings.push(Finding {
            kind: FindingKind::Conflict(SpecQuote { file, quote, line }),
            brief_quote,
            why: text(&entry, "why")?,
        });
    }
    for entry in list("unstated")? {
        let brief_quote = text(&entry, "brief")?;
        findings.push(Finding {
            kind: FindingKind::Unstated(Quotes::in_brief(brief, &brief_quote)),
            brief_quote,
            why: text(&entry, "why")?,
        });
    }
    Ok(findings)
}

/// The line of spec file <file> of <target> where <quote> starts, or why
/// there is none.
fn spec_line(target: &Target, file: &str, quote: &str) -> std::result::Result<i64, String> {
    let file = file.strip_prefix("./").unwrap_or(file);
    if !target.spec_files.iter().any(|spec| spec == file) {
        return Err(format!(
            "{file} is not one of the spec files the check read ({})",
            target.spec_files.join(", ")
        ));
    }
    let path = target.worktree.join(file);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    line_of(&text, quote).ok_or_else(|| format!("{file} holds no such text"))
}

/// The line of <text> where <quote> starts. line_of matches the quote as
/// `review::check` matches a clause: it treats each run of whitespace as one
/// space and skips markdown marks.
pub fn line_of(text: &str, quote: &str) -> Option<i64> {
    let quote = squash(quote);
    if quote.is_empty() {
        return None;
    }
    let mut joined = String::new();
    let mut starts: Vec<(usize, usize)> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let squashed = squash(line);
        if squashed.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        starts.push((joined.len(), number + 1));
        joined.push_str(&squashed);
    }
    let at = joined.find(&quote)?;
    starts
        .iter()
        .rev()
        .find(|(offset, _)| *offset <= at)
        .map(|(_, line)| *line as i64)
}

/// The decision or question a take's `--spec-decided` names, checked against
/// <lists>.
pub fn decided_ref(lists: &Lists, text: &str) -> Result<(Id, Kind)> {
    let id = Id::parse(text)?;
    match lists.locate(id) {
        Some(found) if matches!(found.kind, Kind::Decision | Kind::Question) => {
            Ok((id, found.kind))
        }
        Some(found) => fail!(
            "--spec-decided {} names the {} {id}, and --spec-decided must name the decision or question that records the person's choice on the conflict",
            id.0,
            found.kind.name()
        ),
        None => fail!("--spec-decided {}: {id} is not a record", id.0),
    }
}

/// Records <checked>, the checks of <brief> for <items>, each with <decided>
/// when the take named it, and returns their ids.
pub fn record(
    db: &Connection,
    stamp: &Stamp,
    brief: &Brief,
    items: &[Id],
    checked: &[Checked],
    decided: Option<(Id, Kind)>,
) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for check in checked {
        let spec_files =
            serde_json::to_string(&check.target.spec_files).expect("a list of paths serializes");
        db.execute(
            "INSERT INTO spec_check (at, by, brief, brief_text, repo, worktree, head, spec_files, model, rules_version, system, message, output, tokens_in, tokens_out, tokens_cached, tokens_written, cost_microdollars, failure, decided_id, decided_kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                stamp.at,
                stamp.by,
                brief.name,
                brief.text,
                check.target.repo,
                check.target.worktree.display().to_string(),
                check.target.head,
                spec_files,
                check.model,
                check.rules_version,
                check.system,
                check.message,
                check.output,
                check.usage.tokens_in,
                check.usage.tokens_out,
                check.usage.tokens_cached,
                check.usage.tokens_written,
                // The spec check table uses zero when the provider reports no cost.
                check.usage.cost_microdollars,
                check.failure,
                decided.map(|(id, _)| id),
                decided.map(|(_, kind)| kind.word()),
            ],
        )
        .sql()?;
        let id = db.last_insert_rowid();
        for item in items {
            db.execute(
                "INSERT INTO spec_check_item (check_id, item_id) VALUES (?1, ?2)",
                params![id, item],
            )
            .sql()?;
        }
        for finding in &check.findings {
            let spec = match &finding.kind {
                FindingKind::Conflict(spec) => Some(spec),
                FindingKind::Unstated(_) => None,
            };
            let line = spec.and_then(|spec| match spec.line {
                Quotes::Found(line) => Some(line),
                Quotes::SetAside(_) => None,
            });
            db.execute(
                "INSERT INTO spec_check_finding (check_id, kind, brief_quote, spec_file, spec_line, spec_quote, why, check_failed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    finding.kind.word(),
                    finding.brief_quote,
                    spec.map(|spec| &spec.file),
                    line,
                    spec.map(|spec| &spec.quote),
                    finding.why,
                    finding.kind.set_aside(),
                ],
            )
            .sql()?;
        }
        ids.push(id);
    }
    Ok(ids)
}

/// A check as `builds item show` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecChecked {
    pub id: i64,
    pub at: i64,
    pub brief: String,
    pub repo: String,
    /// The conflicts whose quotes builds found.
    pub conflicts: usize,
    /// The unstated behaviors whose quotes builds found.
    pub unstated: usize,
    pub failure: Option<String>,
    /// The decision or question that records the person's choice on the
    /// conflicts.
    pub decided: Option<Id>,
}

/// Every check, by item, oldest first.
pub fn read_all(db: &Connection) -> Result<BTreeMap<Id, Vec<SpecChecked>>> {
    let mut statement = db
        .prepare(
            "SELECT i.item_id, c.id, c.at, c.brief, c.repo, c.failure, c.decided_id,
                (SELECT COUNT(*) FROM spec_check_finding f WHERE f.check_id = c.id AND f.kind = 'conflict' AND f.check_failed IS NULL),
                (SELECT COUNT(*) FROM spec_check_finding f WHERE f.check_id = c.id AND f.kind = 'unstated' AND f.check_failed IS NULL)
             FROM spec_check_item i JOIN spec_check c ON c.id = i.check_id
             ORDER BY i.item_id, c.id",
        )
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                SpecChecked {
                    id: row.get(1)?,
                    at: row.get(2)?,
                    brief: row.get(3)?,
                    repo: row.get(4)?,
                    failure: row.get(5)?,
                    decided: row.get(6)?,
                    conflicts: row.get::<_, i64>(7)? as usize,
                    unstated: row.get::<_, i64>(8)? as usize,
                },
            ))
        })
        .sql()?;
    let mut all: BTreeMap<Id, Vec<SpecChecked>> = BTreeMap::new();
    for row in rows {
        let (item, check) = row.sql()?;
        all.entry(item).or_default().push(check);
    }
    Ok(all)
}

/// <check> as one line of `builds item show`.
pub fn line(check: &SpecChecked) -> String {
    let found = match &check.failure {
        Some(why) => format!("failed: {why}"),
        None => format!(
            "{}, {}",
            crate::text::counted(check.conflicts, "conflict", "conflicts"),
            crate::text::counted(check.unstated, "unstated behavior", "unstated behaviors")
        ),
    };
    let decided = match check.decided {
        Some(id) => format!("; the person decided the conflicts in {id}"),
        None => String::new(),
    };
    format!(
        "spec check {} of {} against repo {}'s spec, {}: {found}{decided}",
        check.id,
        check.brief,
        check.repo,
        crate::text::local_minute(check.at)
    )
}

/// Why the take stops when a check failed: each failure, or None.
pub fn failures(checked: &[Checked]) -> Option<String> {
    let failed: Vec<String> = checked
        .iter()
        .filter_map(|check| {
            check.failure.as_ref().map(|why| {
                format!(
                    "  in {} (repo {}): {why}",
                    check.target.worktree.display(),
                    check.target.repo
                )
            })
        })
        .collect();
    if failed.is_empty() {
        return None;
    }
    Some(format!(
        "the spec check of the brief gave no usable answer, so builds took nothing:\n{}\nRun the take again.",
        failed.join("\n")
    ))
}

/// Each conflict whose quotes builds found, quoting the brief's sentence and
/// the spec's text with its file and line; None when there is none.
pub fn conflicts(brief: &Brief, checked: &[Checked]) -> Option<String> {
    let mut out = String::new();
    for check in checked {
        for (finding, spec, line) in check.conflicts() {
            out.push_str(&format!(
                "\n  the brief says: \"{}\"\n  {}:{line} (repo {}) says: \"{}\"\n  how they differ: {}\n",
                finding.brief_quote.trim(),
                spec.file,
                check.target.repo,
                spec.quote.trim(),
                finding.why.trim()
            ));
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(format!(
        "the brief {} states behavior that the product spec states otherwise:\n{out}",
        brief.name
    ))
}

/// The notes a check prints whatever the take does: each unstated behavior
/// for the agent to add to the spec, and each finding builds set aside.
pub fn notes(checked: &[Checked]) -> Vec<String> {
    let mut notes = Vec::new();
    for check in checked {
        for finding in check.unstated() {
            notes.push(format!(
                "the spec of repo {} ({}) states nothing on this behavior of the brief; tell the agent to add it to the spec in its commit: \"{}\"",
                check.target.repo,
                check.target.spec_files.join(", "),
                finding.brief_quote.trim()
            ));
        }
        for finding in &check.findings {
            if let Some(why) = finding.kind.set_aside() {
                notes.push(format!(
                    "the spec check set aside a {} in repo {} because {why}: \"{}\"",
                    finding.kind.word(),
                    check.target.repo,
                    finding.brief_quote.trim()
                ));
            }
        }
    }
    notes
}

/// The tables this module owns.
#[cfg(test)]
const TABLES: [&str; 3] = ["spec_check", "spec_check_item", "spec_check_finding"];

/// Writes a row of one of this module's tables as given, the constraints
/// still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "spec_check_tests.rs"]
pub mod tests;

/// Keeps each recorded check while allowing a reviewer to leave the cost unknown.
pub fn allow_unknown_cost(db: &rusqlite::Connection) -> Result<()> {
    use crate::db::{SPEC_CHECK_PART, remake_table, statement};
    remake_table(
        db,
        "spec_check",
        statement(SPEC_CHECK_PART, "CREATE TABLE spec_check (", ") STRICT;"),
        |db| {
            use crate::db::Sql;
            db.execute_batch("INSERT INTO spec_check SELECT * FROM spec_check_old")
                .sql()
        },
        &[],
    )
}
