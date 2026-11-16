//! `builds item add | edit | note | wait | take | handoff | stop | drop |
//! defer | place | check-landing | landed | show | list`: the work queue's
//! commands. Each refuses a step that lacks what it needs, and every write is
//! one `Store::write` made of the owners' operations (`item`, `running`,
//! `landing`, `record`), so it lands whole or not at all. What reads the file
//! system or runs a program (a brief's file, `gh`, git) runs before the lock.

use std::path::Path;

use rusqlite::Connection;

use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir, parse_named, read_input};
use crate::lists::check::faults;
use crate::lists::derive::{item_state, unmet_waits, waits_for_others};
use crate::lists::draft::{require_brief_from, require_draft_briefs};
use crate::lists::item::{
    Audience, Item, ItemDraft, ItemEdit, Level, Note, Priority, RecordWait, Wait, add_item,
    add_note, add_wait, open_item, remove_wait, require_open_wait, set_priority,
};
use crate::lists::project;
use crate::lists::record::close;
use crate::lists::show::wait_words;
use crate::lists::store::{self, Store};
use crate::lists::{
    How, Id, Kind, Lists, Located, Stamp, dropped, issue, item_show, landing, parked, running, tag,
    take, trail,
};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds item add --title T --area A --details-file F [--for design-session] [--wait REF]... [--brief P] [--priority first|last --why WHY] [--origin N] [--tag T]...\n       builds item edit N [--title T] [--area A] [--details-file F] [--brief P] [--for F] [--origin N] [--tag T]... [--untag T]...\n       builds item note N TEXT\n       builds item wait N (--on REF | --off REF)\n       builds item take N [M...] --worktree W --branch B --brief P [--as design-session]\n       builds item park N --worktree W --branch B\n       builds item handoff N --brief P\n       builds item stop N --why WHY\n       builds item finished N --made WHAT\n       builds item drop N --why WHY\n       builds item defer N --issue URL\n       builds item place N first|last|none --why WHY\n       builds item check-landing DIR [--done N]... [--part N]... SHA...\n       builds item landed DIR [--done N]... [--part N]... SHA...\n       builds item show N\n       builds item list [--running] [--ready] [--area A] [--for F] [--tag T]\n(REF is item:N, question:N, design:N or condition:TEXT)";

pub fn cmd_item(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = current_dir()?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "add" => add(tool, rest, &cwd),
        "edit" => edit(tool, rest, &cwd),
        "note" => note_item(tool, rest, &cwd),
        "wait" => wait(tool, rest, &cwd),
        "take" => take::take(tool, rest, &cwd),
        "park" => take::park(tool, rest, &cwd),
        "handoff" => handoff(tool, rest, &cwd),
        "stop" => stop(tool, rest, &cwd),
        "finished" => finished(tool, rest, &cwd),
        "drop" => drop_item(tool, rest, &cwd),
        "defer" => defer(tool, rest, &cwd, &issue::real_gh),
        "place" => place(tool, rest, &cwd),
        "check-landing" => landing::check_landing(tool, rest),
        "landed" => landing::landed(tool, rest),
        "show" => item_show::show(tool, rest, &cwd),
        "list" => item_show::list(tool, rest, &cwd),
        other => fail!("unknown item command {other}\n{USAGE}"),
    }
}

/// `builds next`.
pub fn cmd_next(tool: &Tool, args: &[String]) -> Result<()> {
    item_show::next(tool, args, &current_dir()?)
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

fn one_id(args: &Args, usage: &str) -> Result<Id> {
    let [id] = args.exactly(1, usage)? else {
        unreachable!("exactly one")
    };
    Id::parse(id)
}

fn add(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "title",
            "area",
            "details-file",
            "for",
            "wait",
            "brief",
            "priority",
            "why",
            "origin",
            "tag",
        ],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let origin = args.id("origin")?;
    let priority = match (args.choice::<Level>("priority")?, args.value("why")?) {
        (Some(level), Some(why)) => Some(Priority {
            level,
            why: why.to_string(),
        }),
        (None, None) => None,
        (Some(_), None) => fail!(
            "--priority needs --why: say why it goes {}",
            args.require("priority")?
        ),
        (None, Some(_)) => fail!("--why belongs to --priority first|last"),
    };
    let draft = ItemDraft {
        title: args.require("title")?.to_string(),
        area: args.require("area")?.to_string(),
        details: read_input(args.require("details-file")?)?,
        r#for: args.choice("for")?.unwrap_or(Audience::Coordinator),
        waits: args
            .values("wait")
            .into_iter()
            .map(parse_wait)
            .collect::<Result<_>>()?,
        brief: args.value("brief")?.map(str::to_string),
        priority,
        tags: tag::parse_tags(args.values("tag"))?,
    };
    require_draft_briefs(cwd, std::slice::from_ref(&draft))?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let (id, state) = store.write(&stamp, &format!("item add {}", draft.title), |db| {
        let id = add_item(db, &draft, origin, &stamp)?;
        let lists = store::read(db)?;
        Ok((id, item_state(&lists, lists.item(id).expect("just added"))))
    })?;
    note(format!(
        "added {id} \"{}\" ({})",
        draft.title,
        state.label()
    ));
    Ok(())
}

/// `item edit N`: changes an open item's fields, and gives any item, open or
/// closed, the origin `--origin` names and the tags `--tag` names, and removes
/// the tags `--untag` names.
fn edit(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    const EDIT_USAGE: &str = "builds item edit N [--title T] [--area A] [--details-file F] [--brief P] [--for F] [--origin N] [--tag T]... [--untag T]...";
    let args = Args::parse(
        args,
        &[
            "title",
            "area",
            "details-file",
            "brief",
            "for",
            "origin",
            "tag",
            "untag",
        ],
        &[],
    )?;
    let id = one_id(&args, EDIT_USAGE)?;
    let origin = args.id("origin")?;
    let edit = ItemEdit {
        title: args.value("title")?.map(str::to_string),
        area: args.value("area")?.map(str::to_string),
        details: args.value("details-file")?.map(read_input).transpose()?,
        brief: args.value("brief")?.map(str::to_string),
        r#for: args.choice("for")?,
    };
    let tags = tag::parse_tags(args.values("tag"))?;
    let untags = tag::parse_tags(args.values("untag"))?;
    let fields = !edit.is_empty();
    if !fields && origin.is_none() && tags.is_empty() && untags.is_empty() {
        fail!(
            "the edit of {id} changes nothing: it names no field, no origin and no tag\n{EDIT_USAGE}"
        );
    }
    if let Some(brief) = &edit.brief {
        require_brief_from(cwd, brief)?;
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item edit {id}"), |db| {
        let lists = store::read(db)?;
        if fields {
            open_item(&lists, id)?;
            edit.apply(db, id)?;
        } else if !matches!(
            lists.locate(id),
            Some(Located {
                kind: Kind::Item,
                ..
            })
        ) {
            // The edit gives an open or a closed item its origin and tags; for
            // a record that is no item, open_item refuses the edit and says
            // what the record is.
            open_item(&lists, id)?;
        }
        if let Some(origin) = origin {
            trail::set_origin(db, trail::Entry::Item, id, origin)?;
        }
        tag::remove_tags(db, id, &untags)?;
        tag::add_tags(db, id, &tags)
    })?;
    note(format!("edited {id}"));
    Ok(())
}

fn note_item(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let Some((id, words)) = args.positional().split_first() else {
        fail!("usage: builds item note N TEXT")
    };
    let id = Id::parse(id)?;
    let text = words.join(" ");
    if text.trim().is_empty() {
        fail!("a note needs its text: builds item note {} TEXT", id.0);
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item note {id}"), |db| {
        open_item(&store::read(db)?, id)?;
        add_note(
            db,
            id,
            Note {
                at: stamp.at,
                by: stamp.by.clone(),
                text,
            },
        )
    })?;
    note(format!("noted on {id}"));
    Ok(())
}

/// Refuses a change that leaves the state failing its check, naming what
/// fails, instead of the check's own "nothing repairs them" wrapper (the
/// state on disk is untouched: the write has not happened).
fn require_state_holds(db: &Connection, what: &str) -> Result<()> {
    let found = faults(db, &project::name(db)?)?;
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
        _ => fail!(
            "give exactly one of --on REF and --off REF (REF is item:N, question:N, design:N or condition:TEXT)"
        ),
    };
    let wait = parse_wait(reference)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let state = store.write(
        &stamp,
        &format!("item wait {id} {}", if on { "--on" } else { "--off" }),
        |db| {
            let lists = store::read(db)?;
            let lists = &lists;
            let item = open_item(lists, id)?;
            if on {
                if wait == Wait::Item(id) {
                    fail!("{id} cannot wait on itself");
                }
                require_open_wait(db, &wait)?;
                if item.waits.contains(&wait) {
                    fail!("{id} already waits on {reference}");
                }
                add_wait(db, id, wait.clone())?;
            } else {
                if !item.waits.contains(&wait) {
                    let current: Vec<String> =
                        item.waits.iter().map(|w| wait_words(lists, w)).collect();
                    let current = if current.is_empty() {
                        "nothing".to_string()
                    } else {
                        current.join("; ")
                    };
                    fail!("{id} does not wait on {reference}; it waits on {current}");
                }
                remove_wait(db, id, &wait)?;
            }
            require_state_holds(db, &format!("{id} cannot {verb} {reference}"))?;
            let lists = store::read(db)?;
            Ok(item_state(&lists, lists.item(id).expect("open")))
        },
    )?;
    note(format!(
        "{id} {} {reference} ({})",
        if on {
            "now waits on"
        } else {
            "no longer waits on"
        },
        state.label()
    ));
    Ok(())
}

fn handoff(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["brief"], &[])?;
    let id = one_id(&args, "builds item handoff N --brief P")?;
    let brief = args.require("brief")?.to_string();
    require_brief_from(cwd, &brief)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item handoff {id}"), |db| {
        open_item(&store::read(db)?, id)?;
        running::handoff(db, id, &brief)
    })?;
    note(format!("{id} is handed to the brief {brief}"));
    Ok(())
}

fn stop(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let id = one_id(&args, "builds item stop N --why WHY")?;
    let why = args.why()?.to_string();
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item stop {id}"), |db| {
        open_item(&store::read(db)?, id)?;
        running::stop(db, id)?;
        add_note(
            db,
            id,
            Note {
                at: stamp.at,
                by: stamp.by.clone(),
                text: format!("stopped: {why}"),
            },
        )
    })?;
    note(format!("stopped {id}; it is back in the queue"));
    Ok(())
}

/// What stands in the way of item <id> leaving the queue without being done:
/// an open item waits on it, or it is the last item an open question blocks.
/// Names each and the command that frees it. Unless <work_goes> (a drop,
/// which throws the item's work away), it also must not run or be parked.
fn require_can_leave(lists: &Lists, id: Id, how: &str, work_goes: bool) -> Result<()> {
    let item = open_item(lists, id)?;
    if !work_goes && !item.running.is_empty() {
        let worktrees: Vec<&str> = item.running.iter().map(|r| r.worktree.as_str()).collect();
        fail!(
            "{id} runs in {}; `builds item stop {} --why WHY` first, then {how} it",
            worktrees.join(" and "),
            id.0
        );
    }
    if !work_goes && !item.parked.is_empty() {
        let worktrees: Vec<&str> = item.parked.iter().map(|p| p.worktree.as_str()).collect();
        fail!(
            "{id} is parked in {}, where its work waits; take it there and land the work before you {how} it",
            worktrees.join(" and ")
        );
    }
    lists.require_none_waits_on(RecordWait::Item(id), how)?;
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

/// `item finished N --made WHAT`: closes an open item whose work finished
/// with no commits (an audit, research), saying what it made, and removes it
/// from every worktree it runs in. Closing the item frees the items that wait
/// on it, as a landing does.
fn finished(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["made"], &[])?;
    let id = one_id(&args, "builds item finished N --made WHAT")?;
    let made = args.require("made")?.to_string();
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item finished {id}"), |db| {
        let lists = store::read(db)?;
        if let Some(item) = lists.item(id)
            && !item.parked.is_empty()
        {
            let worktrees: Vec<&str> = item.parked.iter().map(|p| p.worktree.as_str()).collect();
            fail!(
                "{id} is parked in {}, where its work waits; take it there and land the work",
                worktrees.join(" and ")
            );
        }
        running::end(db, id)?;
        close(db, id, How::Finished(made.clone()), &stamp)
    })?;
    note(format!("closed {id} as finished"));
    Ok(())
}

/// `item drop N --why WHY`: closes the item as dropped. When it runs or is
/// parked, the drop throws its work away: one store write closes the item
/// and records each worktree's branch, the commits origin/main lacks and the
/// files not committed, and then the drop detaches each worktree at
/// origin/main, discards its changes, deletes the branch and removes its
/// claim (lists/dropped.rs). The record comes first because a rollback
/// can't undo git: when git fails, the drop names the failure and the
/// command to run again, and a drop of an item closed as dropped throws away
/// what is left of its recorded work, since each git step does nothing to
/// work already thrown away.
fn drop_item(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let id = one_id(&args, "builds item drop N --why WHY")?;
    let why = args.why()?.to_string();
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    if let Some(crate::lists::Closed {
        record: crate::lists::Record::Item(item),
        how: How::Dropped(_),
        ..
    }) = store.load()?.closed_record(id)
        && !item.dropped.is_empty()
    {
        discard_all(id, &item.dropped)?;
        note(format!(
            "{id} is closed as dropped already; threw away the rest of the work it held"
        ));
        return Ok(());
    }
    let works = store.write(&stamp, &format!("item drop {id}"), |db| {
        let lists = store::read(db)?;
        require_can_leave(&lists, id, "drop", true)?;
        let item = open_item(&lists, id)?;
        let places = item
            .running
            .iter()
            .map(|r| (&r.worktree, &r.branch))
            .chain(item.parked.iter().map(|p| (&p.worktree, &p.branch)));
        let works = places
            .map(|(worktree, branch)| dropped::read(worktree, branch))
            .collect::<Result<Vec<_>>>()?;
        running::end(db, id)?;
        parked::end(db, id)?;
        close(db, id, How::Dropped(why.clone()), &stamp)?;
        dropped::record(db, id, &works)?;
        Ok(works)
    })?;
    discard_all(id, &works)?;
    for work in &works {
        note(format!(
            "threw away the work in {} on {}: {} and {}",
            work.worktree,
            work.branch,
            crate::text::counted(work.commits.len(), "commit", "commits"),
            crate::text::counted(
                work.files.len(),
                "file not committed",
                "files not committed"
            )
        ));
    }
    note(format!("dropped {id}"));
    Ok(())
}

/// Throws away <works>, the recorded work of dropped item <id>; a failure
/// names the drop to run again.
fn discard_all(id: Id, works: &[dropped::DroppedWork]) -> Result<()> {
    for work in works {
        if let Err(error) = dropped::discard(work) {
            fail!(
                "{id} is dropped and its work recorded, but throwing away the work in {} failed: {}; run `builds item drop {} --why WHY` again once that is fixed",
                work.worktree,
                error.text(),
                id.0
            );
        }
    }
    Ok(())
}

fn defer(tool: &Tool, args: &[String], cwd: &Path, gh: issue::GhRunner<'_>) -> Result<()> {
    let args = Args::parse(args, &["issue"], &[])?;
    let id = one_id(&args, "builds item defer N --issue URL")?;
    let url = args.require("issue")?.to_string();
    issue::require_open_issue(gh, &url)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item defer {id}"), |db| {
        require_can_leave(&store::read(db)?, id, "defer", false)?;
        close(db, id, How::Deferred(url.clone()), &stamp)
    })?;
    note(format!("deferred {id} to {url}"));
    Ok(())
}

fn place(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let [id, at] = args.exactly(2, "builds item place N first|last|none --why WHY")? else {
        unreachable!("exactly two")
    };
    let id = Id::parse(id)?;
    let why = args.why()?;
    let priority = match at.as_str() {
        "none" => None,
        other => Some(Priority {
            level: parse_named("the place", other, "first, last or none")?,
            why: why.to_string(),
        }),
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("item place {id} {at}"), |db| {
        open_item(&store::read(db)?, id)?;
        set_priority(db, id, priority)
    })?;
    note(format!("placed {id} {at}"));
    Ok(())
}

/// The waits of <item> that keep it from starting, named; empty when none.
pub fn blocking_words(lists: &Lists, item: &Item) -> Vec<String> {
    let mut words: Vec<String> = unmet_waits(lists, item)
        .iter()
        .map(|w| wait_words(lists, w))
        .collect();
    if waits_for_others(lists, item) {
        words.push("every other open coordinator item (it is last)".to_string());
    }
    words
}

#[cfg(test)]
#[path = "item_command_tests.rs"]
mod tests;
