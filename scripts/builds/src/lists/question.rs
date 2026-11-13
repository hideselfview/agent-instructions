//! `builds question ask | answer | settle | done | drop | defer | place |
//! edit | show | list`: the decision stack. Each write is one `with_write`,
//! so it lands whole or not at all; what a step needs is checked inside it,
//! and outside work (a brief file, GitHub) before the lock.

use std::path::Path;

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, read_input, read_request};
use crate::lists::check::title_fault;
use crate::lists::derive::{self, ItemState};
use crate::lists::draft::{BlockedMap, ItemDraft, NewDecision, add_decision, add_item, apply_blocked, close, require_draft_briefs, work_or_why};
use crate::lists::issue::{GhRunner, real_gh, require_open_issue};
use crate::lists::show::{blocks_text, kind_words, stack_entry};
use crate::lists::store::Store;
use crate::lists::{
    How, Id, Kind, Lists, Located, PlanRef, Placement, Position, QuestionKind, Stamp, Standing, Wait,
};
use crate::tool::Tool;

const USAGE: &str = "usage: builds question ask --title T --kind question|your-step|held --details-file F (--blocks N[,N] | --blocks-nothing WHY) [--from-item N] [--first WHY] [--plan PATH --plan-number Qn]
       builds question answer N --request F
       builds question settle N --request F
       builds question done N --request F
       builds question drop N --why WHY
       builds question defer N --issue URL
       builds question place N first|last|auto --why WHY [--by-person]
       builds question edit N [--title T] [--details-file F] [--blocks-nothing WHY | --blocks-something]
       builds question show N
       builds question list
  every command also takes --project P and --by WHO";

pub fn cmd_question(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    print!("{}", run(tool, args, &cwd, &real_gh)?);
    Ok(())
}

/// Runs one command and returns what it prints.
pub fn run(tool: &Tool, args: &[String], cwd: &Path, gh: GhRunner<'_>) -> Result<String> {
    let Some((command, rest)) = args.split_first() else { fail!("{USAGE}") };
    let words = format!("question {}", args.join(" "));
    match command.as_str() {
        "ask" => ask(tool, rest, cwd, &words),
        "answer" => resolve(tool, rest, cwd, &words, Closing::Answer),
        "settle" => resolve(tool, rest, cwd, &words, Closing::Settle),
        "done" => resolve(tool, rest, cwd, &words, Closing::Done),
        "drop" => leave(tool, rest, cwd, &words, gh, Leaving::Drop),
        "defer" => leave(tool, rest, cwd, &words, gh, Leaving::Defer),
        "place" => place(tool, rest, cwd, &words),
        "edit" => edit(tool, rest, cwd, &words),
        "show" => show(tool, rest, cwd),
        "list" => list(tool, rest, cwd),
        other => fail!("unknown question command {other}\n{USAGE}"),
    }
}

fn question_id(args: &Args) -> Result<Id> {
    let [id] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    Id::parse(id)
}

/// What follows from a write: the stack's top and what it blocks.
fn stack_top(lists: &Lists) -> String {
    match derive::stack_order(lists).first() {
        None => "the stack is empty".to_string(),
        Some(entry) => format!(
            "the stack's top is {} {} [{}]: {}",
            entry.question.id,
            entry.question.title,
            kind_words(entry.question.kind),
            blocks_text(entry)
        ),
    }
}

fn open_question(lists: &Lists, id: Id) -> Result<&crate::lists::Question> {
    if let Some(question) = lists.question(id) {
        return Ok(question);
    }
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind: Kind::Question, standing: Standing::Closed(closed), .. }) => {
            fail!("question {id} is closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not a question", kind.name()),
    }
}

/// A variant's name as the request files spell it: through the type's own
/// serde names, so the flag and the JSON never disagree.
fn parse_named<T: serde::de::DeserializeOwned>(flag: &str, text: &str, names: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| crate::fail::Fail::Message(format!("{flag} is {names}, not '{text}'")))
}

fn ask(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(
        args,
        &["title", "kind", "details-file", "blocks", "blocks-nothing", "first", "plan", "plan-number", "from-item"],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let title = args.require("title")?.to_string();
    if let Some(fault) = title_fault(&title) {
        fail!("{fault}");
    }
    let kind: QuestionKind = parse_named("--kind", args.require("kind")?, "question, your-step or held")?;
    let details = read_input(args.require("details-file")?)?;
    if details.trim().is_empty() {
        fail!("details are empty; say what is asked");
    }
    let blocks = args.ids("blocks")?.unwrap_or_default();
    let blocks_nothing = args.value("blocks-nothing")?.map(str::to_string);
    if !blocks.is_empty() && blocks_nothing.is_some() {
        fail!("--blocks and --blocks-nothing contradict each other: it blocks items or it blocks nothing");
    }
    if blocks_nothing.as_deref().is_some_and(|why| why.trim().is_empty()) {
        fail!("--blocks-nothing needs its why");
    }
    let from_item = args.id("from-item")?;
    let placed = match args.value("first")? {
        None => None,
        Some(why) if why.trim().is_empty() => fail!("--first needs its why"),
        Some(why) => Some(Placement { at: Position::First, why: why.to_string(), by_person: false }),
    };
    let plan = match (args.value("plan")?, args.value("plan-number")?) {
        (None, None) => None,
        (Some(path), Some(number)) if !path.trim().is_empty() && !number.trim().is_empty() => {
            Some(PlanRef { path: path.to_string(), number: number.to_string() })
        }
        _ => fail!("--plan and --plan-number go together, each with a value"),
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, words, |lists| {
        let id = lists.alloc_id();
        for blocked in &blocks {
            if Some(*blocked) == from_item {
                fail!("--blocks names {blocked}, the item --from-item closes");
            }
            match lists.locate(*blocked) {
                None => fail!("--blocks names {blocked}, which no record has"),
                Some(Located { kind: Kind::Item, standing: Standing::Open, .. }) => {}
                Some(Located { kind: Kind::Item, standing: Standing::Closed(closed), .. }) => {
                    fail!("--blocks names {blocked}, an item that is closed ({})", closed.how.label())
                }
                Some(Located { kind, .. }) => fail!("--blocks names {blocked}, a {}, not an item", kind.name()),
            }
        }
        // The items that wait on the question once it exists.
        let mut waiting: Vec<Id> = blocks.clone();
        if let Some(from) = from_item {
            let Some(item) = lists.item(from) else { fail!("--from-item names {from}, which is not an open item") };
            if item.running.is_some() {
                fail!("item {from} is running; `builds item stop {} --why …` first, then move it to the stack", from.0);
            }
            // What waited on the item waits on the question that takes its place.
            for other in lists.items.iter_mut() {
                for wait in other.waits.iter_mut().filter(|w| **w == Wait::Item(from)) {
                    *wait = Wait::Question(id);
                    if !waiting.contains(&other.id) {
                        waiting.push(other.id);
                    }
                }
            }
        }
        if waiting.is_empty() && blocks_nothing.is_none() {
            fail!(
                "say what the question blocks: --blocks N[,N] (open items, which gain a wait on it), or --blocks-nothing WHY (why it is asked though nothing waits)"
            );
        }
        for item in lists.items.iter_mut().filter(|i| waiting.contains(&i.id)) {
            if !item.waits.contains(&Wait::Question(id)) {
                item.waits.push(Wait::Question(id));
            }
        }
        lists.questions.push(crate::lists::Question {
            id,
            title,
            details,
            kind,
            blocks_nothing,
            placed,
            plan,
            opened: stamp.clone(),
        });
        if let Some(from) = from_item {
            close(lists, from, How::MovedToStack(id), &stamp)?;
        }
        Ok(())
    })?;
    let lists = store.load()?;
    let id = Id(lists.next_id.0 - 1);
    let mut out = format!("asked question {id}\n");
    if let Some(from) = from_item {
        out.push_str(&format!("item {from} moved to the stack; what waited on it now waits on {id}\n"));
    }
    out.push_str(&format!("{}\n", stack_top(&lists)));
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Answer,
    Settle,
    Done,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerDecision {
    title: String,
    body: String,
    by: String,
    replaces: Option<Id>,
}

/// What `answer`, `settle` and `done` read from `--request`: the decision
/// (answer), the rule that settles it (settle), and for each the fate of every
/// item that waits and the work that follows.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    decision: Option<AnswerDecision>,
    by_rule: Option<String>,
    #[serde(default)]
    blocked: BlockedMap,
    work: Option<Vec<ItemDraft>>,
    no_work: Option<String>,
}

fn resolve(tool: &Tool, args: &[String], cwd: &Path, words: &str, closing: Closing) -> Result<String> {
    let verb = match closing {
        Closing::Answer => "answer",
        Closing::Settle => "settle",
        Closing::Done => "done",
    };
    let args = Args::parse(args, &["request"], &[])?;
    let id = question_id(&args)?;
    let request: Request = read_request(args.require("request")?)?;
    // What the request must hold for this verb, and nothing of the others'.
    let stray = |field: &str, others: &str| -> Result<()> {
        fail!("`{field}` is not part of `question {verb}`; {others}")
    };
    match closing {
        Closing::Answer => {
            if request.by_rule.is_some() {
                stray("by_rule", "a decision of the person is `decision`, a rule is `question settle`")?;
            }
        }
        Closing::Settle => {
            if request.decision.is_some() {
                stray("decision", "settling records none: a question settled from rules is not the person's")?;
            }
        }
        Closing::Done => {
            if request.decision.is_some() || request.by_rule.is_some() {
                stray("decision` or `by_rule", "done takes `blocked` and `work` or `no_work`")?;
            }
        }
    }
    let decision = match (closing, request.decision) {
        (Closing::Answer, None) => fail!("`question answer` needs `decision` {{title, body, by}} in the request"),
        (_, decision) => decision,
    };
    let by_rule = match (closing, request.by_rule) {
        (Closing::Settle, None) => fail!("`question settle` needs `by_rule`: the rule or code that settles it"),
        (Closing::Settle, Some(rule)) if rule.trim().is_empty() => fail!("`by_rule` is empty"),
        (_, rule) => rule,
    };
    let drafts = work_or_why(request.work, request.no_work)?;
    require_draft_briefs(cwd, &drafts)?;
    let blocked = request.blocked;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let resolved = store.with_write(&stamp, words, |lists| {
        let question = open_question(lists, id)?.clone();
        match (closing, question.kind) {
            (Closing::Done, QuestionKind::Question) => {
                fail!("question {id} is a question: `builds question answer {} --request F` records the decision", id.0)
            }
            (Closing::Answer, QuestionKind::YourStep | QuestionKind::Held) => fail!(
                "question {id} is {}: `builds question done {} --request F` closes it",
                kind_words(question.kind),
                id.0
            ),
            _ => {}
        }
        if drafts.iter().any(|draft| draft.waits.contains(&Wait::Question(id))) {
            fail!("a work item waits on {id}, which this closes; the work that follows has nothing to wait for");
        }
        apply_blocked(lists, id, &blocked, &stamp)?;
        let (how, made) = match decision {
            Some(decision) => {
                let new = NewDecision {
                    title: decision.title,
                    by: decision.by,
                    body: decision.body,
                    question: Some(id),
                    replaces: decision.replaces,
                    plan: question.plan.clone(),
                };
                let added = add_decision(lists, &new, &drafts, &stamp)?;
                (How::Answered(added.decision), (Some(added.decision), added.items))
            }
            None => {
                let mut items = Vec::new();
                for draft in &drafts {
                    items.push(add_item(lists, draft, None, &stamp)?);
                }
                let how = match by_rule {
                    Some(rule) => How::Settled(rule),
                    None => How::Done,
                };
                (how, (None, items))
            }
        };
        close(lists, id, how, &stamp)?;
        let ready: Vec<Id> = blocked
            .iter()
            .filter(|(item, fate)| !matches!(fate, crate::lists::draft::Blocked::Drop(_)) && lists.item(**item).is_some())
            .map(|(item, _)| *item)
            .filter(|item| lists.item(*item).is_some_and(|i| derive::item_state(lists, i) == ItemState::Ready))
            .collect();
        Ok((made, ready))
    })?;
    let ((decision, items), ready) = resolved;
    let lists = store.load()?;
    let mut out = match (closing, decision) {
        (Closing::Answer, Some(decision)) => format!("answered question {id} with decision {decision}\n"),
        (Closing::Settle, _) => format!("settled question {id} by rule (no decision recorded)\n"),
        _ => format!("question {id} done\n"),
    };
    if !items.is_empty() {
        let named: Vec<String> = items.iter().map(Id::to_string).collect();
        out.push_str(&format!("made items {}\n", named.join(", ")));
    }
    for (item, fate) in &blocked {
        if let crate::lists::draft::Blocked::Drop(why) = fate {
            out.push_str(&format!("dropped item {item}: {why}\n"));
        }
    }
    for item in ready {
        let title = lists.item(item).map(|i| i.title.as_str()).unwrap_or("?");
        out.push_str(&format!("ready now: {item} {title}\n"));
    }
    out.push_str(&format!("{}\n", stack_top(&lists)));
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaving {
    Drop,
    Defer,
}

/// `drop` and `defer`: the question leaves with nothing waiting on it.
fn leave(tool: &Tool, args: &[String], cwd: &Path, words: &str, gh: GhRunner<'_>, leaving: Leaving) -> Result<String> {
    let args = match leaving {
        Leaving::Drop => Args::parse(args, &["why"], &[])?,
        Leaving::Defer => Args::parse(args, &["issue"], &[])?,
    };
    let id = question_id(&args)?;
    let how = match leaving {
        Leaving::Drop => {
            let why = args.require("why")?;
            if why.trim().is_empty() {
                fail!("--why is empty");
            }
            How::Dropped(why.to_string())
        }
        Leaving::Defer => {
            let url = args.require("issue")?;
            require_open_issue(gh, url)?;
            How::Deferred(url.to_string())
        }
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, words, |lists| {
        open_question(lists, id)?;
        let waiting: Vec<String> = lists
            .items
            .iter()
            .filter(|item| item.waits.contains(&Wait::Question(id)))
            .map(|item| format!("{} {}: `builds item wait {} --off question:{}`", item.id, item.title, item.id.0, id.0))
            .collect();
        if !waiting.is_empty() {
            fail!("items still wait on question {id}; free each first:\n  {}", waiting.join("\n  "));
        }
        close(lists, id, how, &stamp)
    })?;
    let lists = store.load()?;
    let verb = if leaving == Leaving::Drop { "dropped" } else { "deferred" };
    Ok(format!("{verb} question {id}\n{}\n", stack_top(&lists)))
}

fn place(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(args, &["why"], &["by-person"])?;
    let [id, at] = args.exactly(2, USAGE)? else { unreachable!("exactly two") };
    let id = Id::parse(id)?;
    let why = args.require("why")?;
    if why.trim().is_empty() {
        fail!("--why is empty");
    }
    let placed = match at.as_str() {
        "auto" => {
            if args.switch("by-person") {
                fail!("--by-person goes with first or last; auto gives the place back to the order of what it unblocks");
            }
            None
        }
        other => {
            let at: Position = parse_named("the place", other, "first, last or auto")?;
            Some(Placement { at, why: why.to_string(), by_person: args.switch("by-person") })
        }
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, words, |lists| {
        open_question(lists, id)?;
        lists.questions.iter_mut().find(|q| q.id == id).expect("open").placed = placed;
        Ok(())
    })?;
    let lists = store.load()?;
    Ok(format!("placed question {id} {at}\n{}\n", stack_top(&lists)))
}

fn edit(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(args, &["title", "details-file", "blocks-nothing"], &["blocks-something"])?;
    let id = question_id(&args)?;
    let title = args.value("title")?.map(str::to_string);
    if let Some(fault) = title.as_deref().and_then(title_fault) {
        fail!("{fault}");
    }
    let details = args.value("details-file")?.map(read_input).transpose()?;
    if details.as_deref().is_some_and(|d| d.trim().is_empty()) {
        fail!("details are empty; say what is asked");
    }
    let blocks_nothing = args.value("blocks-nothing")?.map(str::to_string);
    if blocks_nothing.as_deref().is_some_and(|why| why.trim().is_empty()) {
        fail!("--blocks-nothing needs its why");
    }
    let blocks_something = args.switch("blocks-something");
    if blocks_nothing.is_some() && blocks_something {
        fail!("--blocks-nothing and --blocks-something contradict each other");
    }
    if title.is_none() && details.is_none() && blocks_nothing.is_none() && !blocks_something {
        fail!("the edit changes nothing: name --title, --details-file, --blocks-nothing or --blocks-something");
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, words, |lists| {
        open_question(lists, id)?;
        let waited_on = lists.items.iter().any(|item| item.waits.contains(&Wait::Question(id)));
        if blocks_something && !waited_on {
            fail!(
                "no open item waits on question {id}, so it would block nothing with no why: `builds item wait ITEM --on question:{}` makes an item wait, or give --blocks-nothing WHY",
                id.0
            );
        }
        let question = lists.questions.iter_mut().find(|q| q.id == id).expect("open");
        if let Some(title) = title {
            question.title = title;
        }
        if let Some(details) = details {
            question.details = details;
        }
        if let Some(why) = blocks_nothing {
            question.blocks_nothing = Some(why);
        }
        if blocks_something {
            question.blocks_nothing = None;
        }
        Ok(())
    })?;
    let lists = store.load()?;
    Ok(format!("edited question {id}\n{}\n", stack_top(&lists)))
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let id = question_id(&args)?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    let stack = derive::stack_order(&lists);
    if let Some(at) = stack.iter().position(|entry| entry.question.id == id) {
        let entry = &stack[at];
        let mut out = stack_entry(at + 1, entry);
        if let Some(plan) = &entry.question.plan {
            out.push_str(&format!("       plan {} {}\n", plan.path, plan.number));
        }
        out.push_str(&format!("\n{}\n", entry.question.details.trim_end()));
        return Ok(out);
    }
    match lists.locate(id) {
        Some(Located { kind: Kind::Question, standing: Standing::Closed(closed), title }) => Ok(format!(
            "{id} {title}\nclosed {} ({}) by {}\n",
            closed.how.label(),
            crate::text::local_minute(closed.closed.at),
            closed.closed.by
        )),
        Some(Located { kind, .. }) => fail!("{id} is a {}, not a question", kind.name()),
        None => fail!("{id} is not a record"),
    }
}

fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    let stack = derive::stack_order(&lists);
    if stack.is_empty() {
        return Ok("the stack is empty\n".to_string());
    }
    Ok(stack.iter().enumerate().map(|(n, entry)| stack_entry(n + 1, entry)).collect())
}

#[cfg(test)]
#[path = "question_tests.rs"]
mod tests;
