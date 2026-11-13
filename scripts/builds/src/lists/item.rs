//! `builds item add | edit | note | wait | take | handoff | stop | drop |
//! defer | check-landing | landed | show | list`: the work queue. Each
//! command refuses a step that lacks what it needs, and every write is one
//! `Store::with_write`, so it lands whole or not at all. What reads the file
//! system or runs a program (a brief's file, `gh`, git) runs before the lock.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, read_input};
use crate::lists::check::faults;
use crate::lists::derive::{item_state, unmet_waits, waits_for_others};
use crate::lists::draft::{ItemDraft, ItemEdit, add_item, close, require_brief_from, require_draft_briefs, require_open_wait};
use crate::lists::store::Store;
use crate::lists::show::wait_words;
use crate::lists::{
    Audience, How, Id, Item, Kind, Level, Lists, Located, Note, Priority, Stamp, Standing, Wait, issue, item_show,
    landing, take,
};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds item add --title T --area A --details-file F [--for design-session] [--wait REF]... [--brief P] [--priority first|last --why WHY]\n       builds item edit N [--title T] [--area A] [--details-file F] [--brief P] [--for F]\n       builds item note N TEXT\n       builds item wait N (--on REF | --off REF)\n       builds item take N [M...] --worktree W --branch B --brief P [--as design-session]\n       builds item handoff N --brief P\n       builds item stop N --why WHY\n       builds item drop N --why WHY\n       builds item defer N --issue URL\n       builds item check-landing DIR [--done N]... [--part N]... SHA...\n       builds item landed DIR [--done N]... [--part N]... SHA...\n       builds item show N\n       builds item list [--running] [--ready] [--area A] [--for F]\n(REF is item:N, question:N, design:N or condition:TEXT)";

pub fn cmd_item(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = current_dir()?;
    let Some((command, rest)) = args.split_first() else { fail!("{USAGE}") };
    match command.as_str() {
        "add" => add(tool, rest, &cwd),
        "edit" => edit(tool, rest, &cwd),
        "note" => add_note(tool, rest, &cwd),
        "wait" => wait(tool, rest, &cwd),
        "take" => take::take(tool, rest, &cwd),
        "handoff" => handoff(tool, rest, &cwd),
        "stop" => stop(tool, rest, &cwd),
        "drop" => drop_item(tool, rest, &cwd),
        "defer" => defer(tool, rest, &cwd, &issue::real_gh),
        "check-landing" => landing::check_landing(tool, rest),
        "landed" => landing::landed(tool, rest, &cwd),
        "show" => item_show::show(tool, rest, &cwd),
        "list" => item_show::list(tool, rest, &cwd),
        other => fail!("unknown item command {other}\n{USAGE}"),
    }
}

/// `builds next`.
pub fn cmd_next(tool: &Tool, args: &[String]) -> Result<()> {
    item_show::next(tool, args, &current_dir()?)
}

pub fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().context(|| "reading the current directory".to_string())
}

/// `item:N`, `question:N`, `design:N` or `condition:TEXT`.
pub fn parse_wait(text: &str) -> Result<Wait> {
    let Some((kind, rest)) = text.split_once(':') else {
        fail!("'{text}' is not a wait: say item:N, question:N, design:N or condition:TEXT");
    };
    Ok(match kind {
        "item" => Wait::Item(Id::parse(rest)?),
        "question" => Wait::Question(Id::parse(rest)?),
        "design" => Wait::Design(Id::parse(rest)?),
        "condition" => Wait::Condition(rest.to_string()),
        _ => fail!("'{text}' is not a wait: the kind is item, question, design or condition"),
    })
}

/// Item <id>, open; names what it is otherwise.
pub fn open_item(lists: &Lists, id: Id) -> Result<&Item> {
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind: Kind::Item, standing: Standing::Open, .. }) => Ok(lists.item(id).expect("located open")),
        Some(Located { kind: Kind::Item, standing: Standing::Closed(closed), .. }) => {
            fail!("{id} is closed ({})", closed.how.label())
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not an item", kind.name()),
    }
}

fn one_id(args: &Args, usage: &str) -> Result<Id> {
    let [id] = args.exactly(1, usage)? else { unreachable!("exactly one") };
    Id::parse(id)
}

fn add(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &["title", "area", "details-file", "for", "wait", "brief", "priority", "why"],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let priority = match (args.choice::<Level>("priority")?, args.value("why")?) {
        (Some(level), Some(why)) => Some(Priority { level, why: why.to_string() }),
        (None, None) => None,
        (Some(_), None) => fail!("--priority needs --why: say why it goes {}", args.require("priority")?),
        (None, Some(_)) => fail!("--why belongs to --priority first|last"),
    };
    let draft = ItemDraft {
        title: args.require("title")?.to_string(),
        area: args.require("area")?.to_string(),
        details: read_input(args.require("details-file")?)?,
        r#for: args.choice("for")?.unwrap_or(Audience::Coordinator),
        waits: args.values("wait").into_iter().map(parse_wait).collect::<Result<_>>()?,
        brief: args.value("brief")?.map(str::to_string),
        priority,
    };
    require_draft_briefs(cwd, std::slice::from_ref(&draft))?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let (id, state) = store.with_write(&stamp, &format!("item add {}", draft.title), |lists| {
        let id = add_item(lists, &draft, None, &stamp)?;
        Ok((id, item_state(lists, lists.item(id).expect("just added"))))
    })?;
    note(format!("added {id} \"{}\" ({})", draft.title, state.label()));
    Ok(())
}

fn edit(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["title", "area", "details-file", "brief", "for"], &[])?;
    let id = one_id(&args, "builds item edit N [--title T] [--area A] [--details-file F] [--brief P] [--for F]")?;
    let edit = ItemEdit {
        title: args.value("title")?.map(str::to_string),
        area: args.value("area")?.map(str::to_string),
        details: args.value("details-file")?.map(read_input).transpose()?,
        brief: args.value("brief")?.map(str::to_string),
        r#for: args.choice("for")?,
    };
    if let Some(brief) = &edit.brief {
        require_brief_from(cwd, brief)?;
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item edit {id}"), |lists| {
        open_item(lists, id)?;
        edit.apply(lists, id)
    })?;
    note(format!("edited {id}"));
    Ok(())
}

fn add_note(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let Some((id, words)) = args.positional().split_first() else { fail!("usage: builds item note N TEXT") };
    let id = Id::parse(id)?;
    let text = words.join(" ");
    if text.trim().is_empty() {
        fail!("a note needs its text: builds item note {} TEXT", id.0);
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item note {id}"), |lists| {
        open_item(lists, id)?;
        lists.item_mut(id).expect("open").notes.push(Note { at: stamp.at, by: stamp.by.clone(), text });
        Ok(())
    })?;
    note(format!("noted on {id}"));
    Ok(())
}

/// Refuses a change that leaves the state failing its check, naming what
/// fails, instead of the check's own "nothing repairs them" wrapper (the
/// state on disk is untouched: the write has not happened).
fn require_state_holds(lists: &Lists, what: &str) -> Result<()> {
    let found = faults(lists);
    if found.is_empty() {
        return Ok(());
    }
    fail!("{what}: {}", found.join("; "));
}

fn wait(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["on", "off"], &[])?;
    let id = one_id(&args, "builds item wait N (--on REF | --off REF)")?;
    let (verb, reference, on) = match (args.value("on")?, args.value("off")?) {
        (Some(reference), None) => ("wait on", reference, true),
        (None, Some(reference)) => ("stop waiting on", reference, false),
        _ => fail!("give exactly one of --on REF and --off REF (REF is item:N, question:N, design:N or condition:TEXT)"),
    };
    let wait = parse_wait(reference)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let state = store.with_write(&stamp, &format!("item wait {id} {}", if on { "--on" } else { "--off" }), |lists| {
        let item = open_item(lists, id)?;
        if on {
            if wait == Wait::Item(id) {
                fail!("{id} cannot wait on itself");
            }
            require_open_wait(lists, &wait)?;
            if item.waits.contains(&wait) {
                fail!("{id} already waits on {reference}");
            }
            lists.item_mut(id).expect("open").waits.push(wait.clone());
        } else {
            let Some(at) = item.waits.iter().position(|w| *w == wait) else {
                let current: Vec<String> = item.waits.iter().map(|w| wait_words(lists, w)).collect();
                let current = if current.is_empty() { "nothing".to_string() } else { current.join("; ") };
                fail!("{id} does not wait on {reference}; it waits on {current}");
            };
            lists.item_mut(id).expect("open").waits.remove(at);
        }
        require_state_holds(lists, &format!("{id} cannot {verb} {reference}"))?;
        Ok(item_state(lists, lists.item(id).expect("open")))
    })?;
    note(format!("{id} {} {reference} ({})", if on { "now waits on" } else { "no longer waits on" }, state.label()));
    Ok(())
}

fn handoff(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["brief"], &[])?;
    let id = one_id(&args, "builds item handoff N --brief P")?;
    let brief = args.require("brief")?.to_string();
    require_brief_from(cwd, &brief)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item handoff {id}"), |lists| {
        open_item(lists, id)?;
        let Some(running) = lists.item_mut(id).expect("open").running.as_mut() else {
            fail!("{id} is not running; a handoff gives a running item's next agent its brief (`item take` starts one)");
        };
        running.brief = brief.clone();
        Ok(())
    })?;
    note(format!("{id} is handed to the brief {brief}"));
    Ok(())
}

fn stop(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let id = one_id(&args, "builds item stop N --why WHY")?;
    let why = args.require("why")?.to_string();
    if why.trim().is_empty() {
        fail!("--why is empty");
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item stop {id}"), |lists| {
        open_item(lists, id)?;
        let item = lists.item_mut(id).expect("open");
        if item.running.take().is_none() {
            fail!("{id} is not running");
        }
        item.notes.push(Note { at: stamp.at, by: stamp.by.clone(), text: format!("stopped: {why}") });
        Ok(())
    })?;
    note(format!("stopped {id}; it is back in the queue"));
    Ok(())
}

/// What stands in the way of item <id> leaving the queue without being done:
/// it runs, an open item waits on it, or it is the last item an open
/// question blocks. Names each and the command that frees it.
fn require_can_leave(lists: &Lists, id: Id, how: &str) -> Result<()> {
    let item = open_item(lists, id)?;
    if let Some(running) = &item.running {
        fail!("{id} runs in {}; `builds item stop {} --why WHY` first, then {how} it", running.worktree, id.0);
    }
    let on_it = Wait::Item(id);
    let waiting: Vec<&Item> = lists.open_items_waiting_on(&on_it).collect();
    if !waiting.is_empty() {
        let named: Vec<String> = waiting.iter().map(|i| format!("{} \"{}\"", i.id, i.title)).collect();
        let free: Vec<String> = waiting.iter().map(|i| format!("builds item wait {} --off item:{}", i.id.0, id.0)).collect();
        fail!("{} wait{} on {id}; free {} first ({}), then {how} it", named.join("; "), if waiting.len() == 1 { "s" } else { "" }, if waiting.len() == 1 { "it" } else { "each" }, free.join("; "));
    }
    for question in &lists.questions {
        let asked = Wait::Question(question.id);
        let blocked: Vec<Id> = lists.open_items_waiting_on(&asked).map(|i| i.id).collect();
        if blocked == [id] && question.blocks_nothing.is_none() {
            fail!(
                "{id} is the last item question {} \"{}\" blocks; {how} it would leave that question blocking nothing: \
                 answer or drop the question first, or say why it is asked anyway (`builds question edit {} --blocks-nothing WHY`)",
                question.id,
                question.title,
                question.id.0
            );
        }
    }
    Ok(())
}

fn drop_item(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let id = one_id(&args, "builds item drop N --why WHY")?;
    let why = args.require("why")?.to_string();
    if why.trim().is_empty() {
        fail!("--why is empty");
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item drop {id}"), |lists| {
        require_can_leave(lists, id, "drop")?;
        close(lists, id, How::Dropped(why.clone()), &stamp)
    })?;
    note(format!("dropped {id}"));
    Ok(())
}

fn defer(tool: &Tool, args: &[String], cwd: &Path, gh: issue::GhRunner<'_>) -> Result<()> {
    let args = Args::parse(args, &["issue"], &[])?;
    let id = one_id(&args, "builds item defer N --issue URL")?;
    let url = args.require("issue")?.to_string();
    issue::require_open_issue(gh, &url)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("item defer {id}"), |lists| {
        require_can_leave(lists, id, "defer")?;
        close(lists, id, How::Deferred(url.clone()), &stamp)
    })?;
    note(format!("deferred {id} to {url}"));
    Ok(())
}

/// The waits of <item> that keep it from starting, named; empty when none.
pub fn blocking_words(lists: &Lists, item: &Item) -> Vec<String> {
    let mut words: Vec<String> = unmet_waits(lists, item).iter().map(|w| wait_words(lists, w)).collect();
    if waits_for_others(lists, item) {
        words.push("every other open coordinator item (it is last)".to_string());
    }
    words
}

#[cfg(test)]
#[path = "item_tests.rs"]
mod tests;
