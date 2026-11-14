//! `builds decision add | edit | find | show | list`: the decision log.
//! `add` records a decision with the work it makes, or the reason it makes
//! none; the log is never closed, and what replaces a decision is derived
//! from the decisions that name it, never stored.

use std::fmt::Write;

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, read_input, read_request};
use crate::lists::check::title_fault;
use crate::lists::decision::{self, NewDecision, add_decision};
use crate::lists::derive;
use crate::lists::draft::{require_draft_briefs, work_or_why};
use crate::lists::item::ItemDraft;
use crate::lists::store::{self, Store};
use crate::lists::{Decision, Id, Lists, PlanRef, Stamp, Standing, day};
use crate::text::{local_minute, note};
use crate::tool::Tool;

const USAGE: &str = "usage: builds decision add --request FILE [--by WHO]\n       builds decision edit N [--title T] [--body-file FILE] [--by WHO]\n       builds decision find WORD... \n       builds decision show N\n       builds decision list [--plan PATH] [--since YYYY-MM-DD]\n(every command takes --project P)";

/// A decision as `add --request` writes it. It carries no `question`: a
/// question's answer is recorded by `question answer`, which closes the
/// question in the same write.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub title: String,
    pub by: String,
    pub body: String,
    pub replaces: Option<Id>,
    pub plan: Option<PlanRef>,
    /// The items the decision makes, or ...
    pub work: Option<Vec<ItemDraft>>,
    /// ... why it makes none.
    pub no_work: Option<String>,
}

pub fn cmd_decision(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "add" => add(tool, rest, &cwd),
        "edit" => edit(tool, rest, &cwd),
        "find" => find(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "list" => list(tool, rest, &cwd),
        other => fail!("unknown decision command {other}\n{USAGE}"),
    }
}

fn add(tool: &Tool, args: &[String], cwd: &std::path::Path) -> Result<()> {
    let args = Args::parse(args, &["request"], &[])?;
    args.exactly(0, USAGE)?;
    let request: DecisionRequest = read_request(args.require("request")?)?;
    let work = work_or_why(request.work, request.no_work)?;
    require_draft_briefs(cwd, &work)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let new = NewDecision {
        title: request.title,
        by: request.by,
        body: request.body,
        question: None,
        replaces: request.replaces,
        plan: request.plan,
    };
    let words = format!("decision add --request {}", args.require("request")?);
    let added = store.write(&stamp, &words, |db| {
        let added = add_decision(db, &new, &work, &stamp)?;
        let lists = &store::read(db)?;
        let made: Vec<String> = added
            .items
            .iter()
            .map(|id| {
                format!(
                    "{id} {}",
                    lists.item(*id).map(|i| i.title.as_str()).unwrap_or("?")
                )
            })
            .collect();
        Ok((added.decision, made))
    })?;
    let (decision, made) = added;
    note(format!("recorded decision {decision}: {}", new.title));
    for item in made {
        note(format!("made item {item}"));
    }
    Ok(())
}

fn edit(tool: &Tool, args: &[String], cwd: &std::path::Path) -> Result<()> {
    let args = Args::parse(args, &["title", "body-file"], &[])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    let title = args.value("title")?.map(str::to_string);
    let body = args.value("body-file")?.map(read_input).transpose()?;
    if title.is_none() && body.is_none() {
        fail!("the edit of {id} changes nothing: give --title or --body-file");
    }
    if let Some(title) = &title
        && let Some(fault) = title_fault(title)
    {
        fail!("{fault}");
    }
    if body.as_deref().is_some_and(|b| b.trim().is_empty()) {
        fail!("the decision's body is empty; say what was decided and why");
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("decision edit {}", id.0), |db| {
        decision::edit(db, id, title, body)
    })?;
    note(format!("edited decision {id}"));
    Ok(())
}

/// A decision's one line: `#id date title (by)`.
fn line(decision: &Decision) -> String {
    format!(
        "{} {} {} ({})",
        decision.id, decision.date, decision.title, decision.by
    )
}

/// The decisions every word of <words> appears in, in title or body, ignoring
/// case; newest first.
pub fn matching<'a>(lists: &'a Lists, words: &[String]) -> Vec<&'a Decision> {
    let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    derive::decisions_newest_first(lists)
        .into_iter()
        .filter(|decision| {
            let text = format!("{}\n{}", decision.title, decision.body).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .collect()
}

fn find(tool: &Tool, args: &[String], cwd: &std::path::Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let words = args.positional();
    if words.is_empty() {
        fail!("usage: builds decision find WORD...: say what to look for");
    }
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    for decision in matching(&lists, words) {
        println!("{}", line(decision));
    }
    Ok(())
}

/// The decisions of the log, newest first, from <since> on and of <plan>.
pub fn listed<'a>(lists: &'a Lists, plan: Option<&str>, since: Option<&str>) -> Vec<&'a Decision> {
    derive::decisions_newest_first(lists)
        .into_iter()
        .filter(|d| plan.is_none_or(|path| d.plan.as_ref().is_some_and(|p| p.path == path)))
        .filter(|d| since.is_none_or(|from| d.date.as_str() >= from))
        .collect()
}

fn list(tool: &Tool, args: &[String], cwd: &std::path::Path) -> Result<()> {
    let args = Args::parse(args, &["plan", "since"], &[])?;
    args.exactly(0, USAGE)?;
    let since = args.value("since")?;
    if let Some(since) = since
        && day::parse(since).is_none()
    {
        fail!("--since '{since}' is not a date: use YYYY-MM-DD");
    }
    let plan = args.value("plan")?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    for decision in listed(&lists, plan, since) {
        let number = match (&decision.plan, plan) {
            (Some(p), Some(_)) => format!(" [{}]", p.number),
            _ => String::new(),
        };
        println!("{}{number}", line(decision));
    }
    Ok(())
}

fn named(lists: &Lists, id: Id) -> String {
    let title = lists.locate(id).map(|found| found.title).unwrap_or("?");
    format!("{id} \"{title}\"")
}

/// One decision in full: who decided and recorded it, what it answered,
/// made, replaces and is replaced by, then its body.
pub fn render(lists: &Lists, decision: &Decision) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {} {}", decision.id, decision.date, decision.title);
    let _ = writeln!(
        out,
        "decided by {}, recorded {} by {}",
        decision.by,
        local_minute(decision.recorded.at),
        decision.recorded.by
    );
    if let Some(question) = decision.question {
        let _ = writeln!(out, "answers question {}", named(lists, question));
    }
    if let Some(plan) = &decision.plan {
        let _ = writeln!(out, "plan {} {}", plan.path, plan.number);
    }
    if let Some(replaces) = decision.replaces {
        let _ = writeln!(out, "replaces {}", named(lists, replaces));
    }
    if let Some(by) = derive::replaced_by(lists, decision.id) {
        let _ = writeln!(out, "replaced by {}", named(lists, by));
    }
    for id in &decision.work {
        let state = match lists.locate(*id).map(|found| found.standing) {
            Some(Standing::Open) => "open".to_string(),
            Some(Standing::Closed(closed)) => closed.how.label().to_string(),
            None => "missing".to_string(),
        };
        let _ = writeln!(out, "made item {} [{state}]", named(lists, *id));
    }
    let _ = write!(out, "\n{}\n", decision.body.trim_end());
    out
}

fn show(tool: &Tool, args: &[String], cwd: &std::path::Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    let Some(decision) = lists.decision(id) else {
        fail!("{id} is not a decision")
    };
    print!("{}", render(&lists, decision));
    Ok(())
}

#[cfg(test)]
#[path = "decision_command_tests.rs"]
mod tests;
