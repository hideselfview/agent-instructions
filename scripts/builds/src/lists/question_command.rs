//! `builds question ask | answer | settle | done | drop | defer | place |
//! edit | show | list`: the decision stack. Each write is one `Store::write`,
//! so it lands whole or not at all; what a step needs is checked inside it,
//! and outside work (a brief file, GitHub, the rulings check of the text the
//! write keeps) before the lock.

use std::path::Path;

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, parse_named, read_input, read_request};
use crate::lists::check::title_fault;
use crate::lists::citation;
use crate::lists::decision::{NewDecision, add_decision};
use crate::lists::derive::{self, ItemState};
use crate::lists::draft::{require_draft_briefs, work_or_why};
use crate::lists::dropped;
use crate::lists::issue::{GhRunner, real_gh, require_open_issue};
use crate::lists::item::{ItemDraft, RecordWait, add_item, add_wait, move_waits_to_question};
use crate::lists::question::{self, Blocked, BlockedMap, NewQuestion, Spot, apply_blocked};
use crate::lists::record::close;
use crate::lists::rulings::{self, Source, Text};
use crate::lists::show::{blocks_lines, kind_words, stack_entry, stack_line};
use crate::lists::store::{self, Store};
use crate::lists::trail::{self, trail_line, trail_wanted};
use crate::lists::{
    Effect, How, Id, Kind, Lists, Located, PlanRef, Question, QuestionKind, Stamp, Standing, Wait,
    tag,
};
use crate::tool::Tool;

const USAGE: &str = "usage: builds question ask --title T --kind question|your-step|held --effect E --details-file F (--blocks N[,N] | --blocks-nothing WHY) [--from-item N | --origin N] [--first WHY] [--plan PATH --plan-number Qn] [--tag T]...
       builds question answer N --request F
       builds question settle N --request F
       builds question done N --request F
       builds question drop N --why WHY
       builds question defer N --issue URL
       builds question place N (first|last|auto | --above M | --below M) --why WHY [--by-person]
       builds question edit N [--title T] [--details-file F] [--effect E] [--blocks-nothing WHY] [--origin N] [--tag T]... [--untag T]...
       builds question show N
       builds question list [--tag T]
  every command also takes --project P and --by WHO";

pub fn cmd_question(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    print!("{}", run(tool, args, &cwd, &real_gh)?);
    Ok(())
}

/// Runs one command and returns what it prints.
pub fn run(tool: &Tool, args: &[String], cwd: &Path, gh: GhRunner<'_>) -> Result<String> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    let words = format!("question {}", args.join(" "));
    match command.as_str() {
        "ask" => ask(tool, rest, cwd, &words),
        "answer" => resolve(tool, rest, cwd, &words, Verb::Answer),
        "settle" => resolve(tool, rest, cwd, &words, Verb::Settle),
        "done" => resolve(tool, rest, cwd, &words, Verb::Done),
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
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    Id::parse(id)
}

/// What follows from a write: the stack's top and what it blocks.
fn stack_top(lists: &Lists) -> Result<String> {
    Ok(match derive::stack_order(lists).first() {
        None => "the stack is empty".to_string(),
        Some(entry) => format!(
            "the stack's top is {} {} [{}, {}]: {}",
            entry.question.id,
            entry.question.title,
            kind_words(entry.question.kind),
            entry.question.effect.words(),
            blocks_lines(entry)?.join("\n  ")
        ),
    })
}

fn open_question(lists: &Lists, id: Id) -> Result<&Question> {
    if let Some(question) = lists.question(id) {
        return Ok(question);
    }
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located {
            kind: Kind::Question,
            standing: Standing::Closed(closed),
            ..
        }) => {
            fail!("question {id} is closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not a question", kind.name()),
    }
}

fn ask(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(
        args,
        &[
            "title",
            "kind",
            "effect",
            "details-file",
            "blocks",
            "blocks-nothing",
            "first",
            "plan",
            "plan-number",
            "from-item",
            "origin",
            "tag",
        ],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let tags = tag::parse_tags(args.values("tag"))?;
    let title = args.require("title")?.to_string();
    if let Some(fault) = title_fault(&title) {
        fail!("{fault}");
    }
    let kind: QuestionKind = parse_named(
        "--kind",
        args.require("kind")?,
        "question, your-step or held",
    )?;
    let effect: Effect = parse_named("--effect", args.require("effect")?, Effect::NAMES)?;
    let details = read_input(args.require("details-file")?)?;
    if details.trim().is_empty() {
        fail!("details are empty; say what is asked");
    }
    let blocks = args.ids("blocks")?.unwrap_or_default();
    let blocks_nothing = args.value("blocks-nothing")?.map(str::to_string);
    if !blocks.is_empty() && blocks_nothing.is_some() {
        fail!(
            "--blocks and --blocks-nothing contradict each other: it blocks items or it blocks nothing"
        );
    }
    if blocks_nothing
        .as_deref()
        .is_some_and(|why| why.trim().is_empty())
    {
        fail!("--blocks-nothing needs its why");
    }
    let from_item = args.id("from-item")?;
    // The question that takes an item's place comes from it.
    let origin = match (from_item, args.id("origin")?) {
        (Some(_), Some(_)) => fail!(
            "--from-item and --origin contradict each other: the question that takes item N's place comes from N"
        ),
        (from_item, origin) => from_item.or(origin),
    };
    let first = match args.value("first")? {
        Some(why) if why.trim().is_empty() => fail!("--first needs its why"),
        first => first.map(str::to_string),
    };
    let plan = match (args.value("plan")?, args.value("plan-number")?) {
        (None, None) => None,
        (Some(path), Some(number)) if !path.trim().is_empty() && !number.trim().is_empty() => {
            Some(PlanRef {
                path: path.to_string(),
                number: number.to_string(),
            })
        }
        _ => fail!("--plan and --plan-number go together, each with a value"),
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let mut texts = vec![
        Text::new("the question's title", &title),
        Text::new("the question's details", &details),
    ];
    if let Some(why) = &blocks_nothing {
        texts.push(Text::new("why the question blocks nothing", why));
    }
    if let Some(why) = &first {
        texts.push(Text::new("why the question goes first", why));
    }
    let cited = rulings::require_cited(
        &tool.reviewer,
        &tool.places.home,
        &store.load()?,
        Source::Lists,
        &texts,
    )?;
    let id = store.write(&stamp, words, |db| {
        let lists = &store::read(db)?;
        if let Some(from) = from_item {
            let Some(item) = lists.item(from) else { fail!("--from-item names {from}, which is not an open item") };
            if !item.running.is_empty() {
                fail!("item {from} is running; run `builds item stop {} --why …` first, then move it to the stack", from.0);
            }
            if !item.kept.is_empty() {
                fail!("item {from} is kept, and its work waits in its worktree; take it there and land the work first");
            }
        }
        let new = NewQuestion {
            title: title.clone(),
            details: details.clone(),
            kind,
            effect,
            blocks_nothing: blocks_nothing.clone(),
            first: first.clone(),
            plan: plan.clone(),
            origin,
        };
        let id = question::ask(db, new, &stamp)?;
        tag::add_tags(db, id, &tags)?;
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
            // What waited on the item waits on the question that takes its place.
            for moved in move_waits_to_question(db, from, id)? {
                if !waiting.contains(&moved) {
                    waiting.push(moved);
                }
            }
        }
        if waiting.is_empty() && blocks_nothing.is_none() {
            fail!(
                "say what the question blocks: --blocks N[,N] (open items, which gain a wait on it), or --blocks-nothing WHY (why it is asked though nothing waits)"
            );
        }
        if let Some(from) = from_item && blocks_nothing.is_some() && !waiting.is_empty() {
            let named: Vec<String> = waiting.iter().map(Id::to_string).collect();
            fail!(
                "--blocks-nothing contradicts the items that waited on item {from}: {} wait{} on the new question instead, so the new question blocks {}",
                named.join(", "),
                if named.len() == 1 { "s" } else { "" },
                if named.len() == 1 { "that item" } else { "those items" }
            );
        }
        for blocked in &waiting {
            add_wait(db, *blocked, Wait::Question(id))?;
        }
        if let Some(from) = from_item {
            close(db, from, How::MovedToStack(id), &stamp)?;
        }
        Ok(id)
    })?;
    let lists = store.load()?;
    let mut out = format!("asked question {id}\n");
    out.push_str(&rulings::lines(cited));
    if let Some(from) = from_item {
        out.push_str(&format!(
            "item {from} moved to the stack; what waited on it now waits on {id}\n"
        ));
    }
    out.push_str(&format!("{}\n", stack_top(&lists)?));
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Answer,
    Settle,
    Done,
}

impl Verb {
    fn word(self) -> &'static str {
        match self {
            Verb::Answer => "answer",
            Verb::Settle => "settle",
            Verb::Done => "done",
        }
    }
}

/// How a question closes, with what the request gives for it: the person's
/// decision, the rule that settles it, or what shows its step done.
enum Closing {
    Answer(AnswerDecision),
    Settle(String),
    Done(String),
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
/// (answer), the rule that settles it (settle), what shows the step done
/// (done), and for each the fate of every item that waits and the work that
/// follows.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    decision: Option<AnswerDecision>,
    by_rule: Option<String>,
    shown_by: Option<String>,
    #[serde(default)]
    blocked: BlockedMap,
    work: Option<Vec<ItemDraft>>,
    no_work: Option<String>,
}

/// The request's field <name> for <verb>, which the verb needs: refused when
/// missing (<needs> says what it holds) or blank.
fn field_needed<T>(
    verb: Verb,
    name: &str,
    value: Option<T>,
    needs: &str,
    blank: fn(&T) -> bool,
) -> Result<T> {
    match value {
        None => fail!("`question {}` needs `{name}`: {needs}", verb.word()),
        Some(value) if blank(&value) => fail!("`{name}` is empty"),
        Some(value) => Ok(value),
    }
}

/// The request's field <name> for <verb>, which the verb does not take:
/// refused when present (<elsewhere> says what to use instead).
fn field_refused<T>(verb: Verb, name: &str, value: &Option<T>, elsewhere: &str) -> Result<()> {
    if value.is_some() {
        fail!(
            "`{name}` is not part of `question {}`; {elsewhere}",
            verb.word()
        );
    }
    Ok(())
}

/// What <request> holds for <verb> and nothing of the other verbs'.
fn closing_of(verb: Verb, request: &mut Request) -> Result<Closing> {
    const TAKES_SHOWN_BY: &str = "`question done` takes it";
    const DONE_TAKES: &str = "done takes `shown_by`, `blocked` and `work` or `no_work`";
    let (decision, by_rule, shown_by) = (
        request.decision.take(),
        request.by_rule.take(),
        request.shown_by.take(),
    );
    Ok(match verb {
        Verb::Answer => {
            field_refused(
                verb,
                "by_rule",
                &by_rule,
                "a decision of the person is `decision`, a rule is `question settle`",
            )?;
            field_refused(verb, "shown_by", &shown_by, TAKES_SHOWN_BY)?;
            Closing::Answer(field_needed(
                verb,
                "decision",
                decision,
                "{title, body, by} in the request",
                |_| false,
            )?)
        }
        Verb::Settle => {
            field_refused(
                verb,
                "decision",
                &decision,
                "settling records none: a question settled from rules is not the person's",
            )?;
            field_refused(verb, "shown_by", &shown_by, TAKES_SHOWN_BY)?;
            Closing::Settle(field_needed(
                verb,
                "by_rule",
                by_rule,
                "the rule or code that settles it",
                |rule| rule.trim().is_empty(),
            )?)
        }
        Verb::Done => {
            field_refused(verb, "decision", &decision, DONE_TAKES)?;
            field_refused(verb, "by_rule", &by_rule, DONE_TAKES)?;
            Closing::Done(field_needed(
                verb,
                "shown_by",
                shown_by,
                "what shows the step is done (the person's words, a decision, a commit or the code)",
                |shown| shown.trim().is_empty(),
            )?)
        }
    })
}

/// Adds each of <drafts> as a work item, in order.
fn add_drafts(db: &rusqlite::Connection, drafts: &[ItemDraft], stamp: &Stamp) -> Result<Vec<Id>> {
    drafts
        .iter()
        .map(|draft| add_item(db, draft, None, stamp))
        .collect()
}

fn resolve(tool: &Tool, args: &[String], cwd: &Path, words: &str, verb: Verb) -> Result<String> {
    let args = Args::parse(args, &["request"], &[])?;
    let id = question_id(&args)?;
    let mut request: Request = read_request(args.require("request")?)?;
    let closing = closing_of(verb, &mut request)?;
    let drafts = work_or_why(request.work, request.no_work)?;
    let blocked = request.blocked;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    require_draft_briefs(&tool.places, &store.repos()?, &drafts)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    // The decision of an answer is the record a citation names, so the check
    // reads the rest of the request.
    let mut texts = Vec::new();
    if let Closing::Settle(rule) = &closing {
        texts.push(Text::new("the reason that settles the question", rule));
    }
    texts.extend(rulings::draft_texts(&drafts));
    texts.extend(rulings::blocked_texts(&blocked));
    let cited = rulings::require_cited(
        &tool.reviewer,
        &tool.places.home,
        &store.load()?,
        Source::Lists,
        &texts,
    )?;
    // In a repo two projects hold, an item of either project may run in a
    // worktree an answer's dropped item keeps, so the drop reads the other
    // projects' items too.
    let other_projects = dropped::other_projects(&tool.places, &store.project)?;
    let resolved = store.write(&stamp, words, |db| {
        let lists = &store::read(db)?;
        let question = open_question(lists, id)?.clone();
        match (verb, question.kind) {
            (Verb::Done, QuestionKind::Question) => {
                fail!("question {id} is a question: `builds question answer {} --request F` records the decision", id.0)
            }
            (Verb::Answer, QuestionKind::YourStep | QuestionKind::Held) => fail!(
                "question {id} is {}: `builds question done {} --request F` closes it",
                kind_words(question.kind),
                id.0
            ),
            _ => {}
        }
        if drafts.iter().any(|draft| draft.waits.contains(&Wait::Question(id))) {
            fail!("a work item waits on {id}, which this closes; the work that follows has nothing to wait for");
        }
        let dropped = apply_blocked(db, id, &blocked, &other_projects, &stamp)?;
        let (how, made) = match closing {
            Closing::Answer(decision) => {
                let new = NewDecision {
                    title: decision.title,
                    by: decision.by,
                    body: decision.body,
                    question: Some(id),
                    replaces: decision.replaces,
                    plan: question.plan.clone(),
                };
                let added = add_decision(db, &new, &drafts, &stamp)?;
                (How::Answered(added.decision), (Some(added.decision), added.items))
            }
            Closing::Settle(rule) => {
                let items = add_drafts(db, &drafts, &stamp)?;
                (How::Settled(rule), (None, items))
            }
            Closing::Done(shown) => {
                let items = add_drafts(db, &drafts, &stamp)?;
                (How::Done(shown), (None, items))
            }
        };
        close(db, id, how, &stamp)?;
        let lists = &store::read(db)?;
        let ready: Vec<Id> = blocked
            .iter()
            .filter(|(item, fate)| !matches!(fate, Blocked::Drop(_)) && lists.item(**item).is_some())
            .map(|(item, _)| *item)
            .filter(|item| lists.item(*item).is_some_and(|i| derive::item_state(lists, i) == ItemState::Ready))
            .collect();
        Ok((made, ready, dropped))
    })?;
    let ((decision, items), ready, dropped) = resolved;
    dropped::discard_each(&dropped)?;
    let lists = store.load()?;
    let mut out = match (verb, decision) {
        (Verb::Answer, Some(decision)) => {
            format!("answered question {id} with decision {decision}\n")
        }
        (Verb::Settle, _) => format!("settled question {id} by rule (no decision recorded)\n"),
        _ => format!("question {id} done\n"),
    };
    out.push_str(&rulings::lines(cited));
    if !items.is_empty() {
        let named: Vec<String> = items.iter().map(Id::to_string).collect();
        out.push_str(&format!("made items {}\n", named.join(", ")));
    }
    for (item, fate) in &blocked {
        if let Blocked::Drop(why) = fate {
            for (_, works) in dropped.iter().filter(|(dropped, _)| dropped == item) {
                for work in works {
                    out.push_str(&format!("{}\n", dropped::thrown_away_line(work)));
                }
            }
            out.push_str(&format!("dropped item {item}: {why}\n"));
        }
    }
    for item in ready {
        let title = lists.item(item).map(|i| i.title.as_str()).unwrap_or("?");
        out.push_str(&format!("ready now: {item} {title}\n"));
    }
    out.push_str(&format!("{}\n", stack_top(&lists)?));
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaving {
    Drop,
    Defer,
}

/// `drop` and `defer`: the question leaves with nothing waiting on it.
fn leave(
    tool: &Tool,
    args: &[String],
    cwd: &Path,
    words: &str,
    gh: GhRunner<'_>,
    leaving: Leaving,
) -> Result<String> {
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
    store.write(&stamp, words, |db| {
        let lists = &store::read(db)?;
        open_question(lists, id)?;
        let verb = if leaving == Leaving::Drop {
            "drop"
        } else {
            "defer"
        };
        lists.require_none_waits_on(RecordWait::Question(id), verb)?;
        close(db, id, how, &stamp)
    })?;
    let lists = store.load()?;
    let verb = if leaving == Leaving::Drop {
        "dropped"
    } else {
        "deferred"
    };
    Ok(format!("{verb} question {id}\n{}\n", stack_top(&lists)?))
}

fn place(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(args, &["why", "above", "below"], &["by-person"])?;
    let (id, spot) = match (args.positional(), args.id("above")?, args.id("below")?) {
        ([_], Some(_), Some(_)) => fail!(
            "--above and --below contradict each other: name one question to place it next to"
        ),
        ([id], Some(next), None) => (id, Some(Spot::Above(next))),
        ([id], None, Some(next)) => (id, Some(Spot::Below(next))),
        ([id, at], None, None) => (
            id,
            match at.as_str() {
                "first" => Some(Spot::First),
                "last" => Some(Spot::Last),
                "auto" => None,
                other => fail!("the place is first, last or auto, not '{other}'"),
            },
        ),
        ([_, _], _, _) => {
            fail!("--above and --below take the place of first, last or auto: give one of them")
        }
        _ => fail!("{USAGE}"),
    };
    let id = Id::parse(id)?;
    let why = args.why()?;
    let by_person = args.switch("by-person");
    if spot.is_none() && by_person {
        fail!(
            "--by-person goes with first, last, --above or --below; with auto, the stack orders the question by what it unblocks and its effect"
        );
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, words, |db| {
        let lists = &store::read(db)?;
        open_question(lists, id)?;
        match spot {
            Some(spot) => question::place(db, id, spot, why, by_person),
            None => question::unplace(db, id),
        }
    })?;
    let lists = store.load()?;
    let to = match spot {
        Some(Spot::First) => "first".to_string(),
        Some(Spot::Last) => "last".to_string(),
        Some(Spot::Above(next)) => format!("above {next}"),
        Some(Spot::Below(next)) => format!("below {next}"),
        None => "by what it unblocks and its effect".to_string(),
    };
    Ok(format!(
        "placed question {id} {to}\n{}\n",
        stack_top(&lists)?
    ))
}

/// `question edit N`: changes an open question's fields, and gives any
/// question, open or closed, the origin `--origin` names and the tags `--tag`
/// names, and removes the tags `--untag` names.
fn edit(tool: &Tool, args: &[String], cwd: &Path, words: &str) -> Result<String> {
    let args = Args::parse(
        args,
        &[
            "title",
            "details-file",
            "effect",
            "blocks-nothing",
            "origin",
            "tag",
            "untag",
        ],
        &[],
    )?;
    let id = question_id(&args)?;
    let origin = args.id("origin")?;
    let tags = tag::parse_tags(args.values("tag"))?;
    let untags = tag::parse_tags(args.values("untag"))?;
    let title = args.value("title")?.map(str::to_string);
    if let Some(fault) = title.as_deref().and_then(title_fault) {
        fail!("{fault}");
    }
    let details = args.value("details-file")?.map(read_input).transpose()?;
    if details.as_deref().is_some_and(|d| d.trim().is_empty()) {
        fail!("details are empty; say what is asked");
    }
    let effect = args
        .value("effect")?
        .map(|word| parse_named::<Effect>("--effect", word, Effect::NAMES))
        .transpose()?;
    let blocks_nothing = args.value("blocks-nothing")?.map(str::to_string);
    if blocks_nothing
        .as_deref()
        .is_some_and(|why| why.trim().is_empty())
    {
        fail!("--blocks-nothing needs its why");
    }
    let fields =
        title.is_some() || details.is_some() || effect.is_some() || blocks_nothing.is_some();
    if !fields && origin.is_none() && tags.is_empty() && untags.is_empty() {
        fail!(
            "the edit changes nothing: name --title, --details-file, --effect, --blocks-nothing, --origin, --tag or --untag"
        );
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let mut texts = Vec::new();
    if let Some(title) = &title {
        texts.push(Text::new("the question's new title", title));
    }
    if let Some(details) = &details {
        texts.push(Text::new("the question's new details", details));
    }
    if let Some(why) = &blocks_nothing {
        texts.push(Text::new("why the question blocks nothing", why));
    }
    let cited = rulings::require_cited(
        &tool.reviewer,
        &tool.places.home,
        &store.load()?,
        Source::Lists,
        &texts,
    )?;
    store.write(&stamp, words, |db| {
        let lists = &store::read(db)?;
        if fields {
            open_question(lists, id)?;
            question::edit(db, id, title, details, effect, blocks_nothing)?;
        } else if !matches!(
            lists.locate(id),
            Some(Located {
                kind: Kind::Question,
                ..
            })
        ) {
            // The edit gives an open or a closed question its origin and
            // tags; for a record that is no question, open_question refuses
            // the edit and says what the record is.
            open_question(lists, id)?;
        }
        if let Some(origin) = origin {
            trail::set_origin(db, trail::Entry::Question, id, origin)?;
        }
        tag::remove_tags(db, id, &untags)?;
        tag::add_tags(db, id, &tags)
    })?;
    let lists = store.load()?;
    let mut out = format!("edited question {id}\n");
    out.push_str(&rulings::lines(cited));
    out.push_str(&format!("{}\n", stack_top(&lists)?));
    Ok(out)
}

/// Gives open question <id> of <store> the effect <effect>, as `question
/// edit N --effect E` does. The dashboard's effect control writes through
/// this function.
pub fn set_effect(store: &Store, id: Id, effect: Effect, stamp: &Stamp) -> Result<()> {
    let words = format!("question edit {} --effect {}", id.0, effect.word());
    store.write(stamp, &words, |db| {
        question::edit(db, id, None, None, Some(effect), None)
    })
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let id = question_id(&args)?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    let stack = derive::stack_order(&lists);
    if let Some(at) = stack.iter().position(|entry| entry.question.id == id) {
        let entry = &stack[at];
        let mut out = stack_entry(&lists, at + 1, entry)?;
        if let Some(plan) = &entry.question.plan {
            out.push_str(&format!("       plan {} {}\n", plan.path, plan.number));
        }
        for cited in citation::question_cites(&lists, entry.question) {
            out.push_str(&format!("       {}\n", citation::cited_line(&cited)));
        }
        out.push_str(&format!("\n{}\n", entry.question.details.trim_end()));
        return Ok(out);
    }
    match lists.locate(id) {
        Some(Located {
            kind: Kind::Question,
            standing: Standing::Closed(closed),
            title,
        }) => {
            let trail = trail_line(&lists, id)?
                .map(|line| format!("{line}\n"))
                .unwrap_or_default();
            Ok(format!(
                "{id} {title}\nclosed {} ({}) by {}\n{}\n{trail}",
                closed.how.label(),
                crate::text::local_minute(closed.closed.at),
                closed.closed.by,
                closed.how.words()
            ))
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not a question", kind.name()),
        None => fail!("{id} is not a record"),
    }
}

/// The stack in the order it is asked, one line per question; with `--tag
/// T`, only the questions whose trail has T, each keeping its place in the
/// stack.
fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &["tag"], &[])?;
    args.exactly(0, USAGE)?;
    let wanted = args.value("tag")?;
    let lists = Store::resolve(&tool.places, &args, cwd)?.load()?;
    let stack = derive::stack_order(&lists);
    if stack.is_empty() {
        return Ok("the stack is empty\n".to_string());
    }
    let mut shown = String::new();
    for (n, entry) in stack.iter().enumerate() {
        if trail_wanted(&lists, entry.question.id, wanted)? {
            shown.push_str(&stack_line(&lists, n + 1, entry)?);
        }
    }
    if shown.is_empty()
        && let Some(wanted) = wanted
    {
        return Ok(format!(
            "no question on the stack has the tag {wanted} on its trail\n"
        ));
    }
    Ok(shown)
}

#[cfg(test)]
#[path = "question_command_tests.rs"]
mod tests;
