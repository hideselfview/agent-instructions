//! `builds item show`, `builds item list`, `builds item times` and `builds
//! next`: reading the work queue. `next` is the coordinator's dispatch list;
//! `list --running` prints each running item's marker line, what to read
//! after a compact; `times` says how long items and branches ran.

use std::fmt::Write;
use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::args::Args;
use crate::lists::derive::{self, FLOW_DAYS, ItemState, Times, branch_times, item_times};
use crate::lists::item::open_item;
use crate::lists::running::marker;
use crate::lists::show::{item_line, parked_words, wait_words};
use crate::lists::store::Store;
use crate::lists::trail::{trail_line, trail_wanted};
use crate::lists::{Audience, Closed, How, Id, Item, Kind, Level, Lists, Located, Standing, Wait};
use crate::text::{counted, duration, local_minute};
use crate::tool::Tool;

fn how_words(how: &How) -> String {
    match how {
        How::Answered(decision) => format!("answered by decision {decision}"),
        How::Settled(rule) => format!("settled by {rule}"),
        How::Done => "done".to_string(),
        How::Landed(commits) => {
            let short: Vec<&str> = commits.iter().map(|c| &c[..c.len().min(9)]).collect();
            format!("landed ({})", short.join(" "))
        }
        How::Finished(made) => format!("finished: {made}"),
        How::Dropped(why) => format!("dropped: {why}"),
        How::Deferred(url) => format!("deferred to {url}"),
        How::MovedToStack(question) => format!("moved to the stack as {question}"),
    }
}

fn wait_line(lists: &Lists, wait: &Wait) -> String {
    let met = if derive::wait_met(lists, wait) {
        "met"
    } else {
        "not met"
    };
    format!("{} ({met})", wait_words(lists, wait))
}

/// <times> in words at <now>: when the first run started, when the work
/// landed or whether it runs now, how long it ran and in how many runs, the
/// time between runs when that is a minute or more, the time held and the
/// time it waited on the person.
pub fn times_words(times: &Times, now: i64) -> String {
    let ran = times.ran.at(now);
    let first = local_minute(times.first);
    let (mut words, end) = match (times.landed, times.ran.since) {
        (Some(landed), _) => (
            format!(
                "first started {first} and landed {}, {} later",
                local_minute(landed),
                duration(landed - times.first)
            ),
            Some(landed),
        ),
        (None, Some(_)) => (format!("first started {first} and runs now"), Some(now)),
        (None, None) => (format!("first started {first}"), None),
    };
    let _ = write!(
        words,
        "; ran {} in {}",
        duration(ran),
        counted(times.runs, "run", "runs")
    );
    if let Some(end) = end
        && end - times.first - ran >= 60
    {
        let _ = write!(
            words,
            " and {} between runs",
            duration(end - times.first - ran)
        );
    }
    let _ = write!(
        words,
        "; held the worktree {}; waited on you {}",
        duration(times.held.at(now)),
        duration(times.waiting.at(now))
    );
    words
}

/// The words that say when the store began to record held and waiting time,
/// for times whose first run started before <from>.
fn spans_from_words(from: i64) -> String {
    format!(
        "builds has recorded held and waiting time only since {}",
        local_minute(from)
    )
}

/// The item <id>, open or closed, with its closing (None while it is open).
fn item_and_closing(lists: &Lists, id: Id) -> Result<(&Item, Option<&Closed>)> {
    match lists.locate(id) {
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Open,
            ..
        }) => Ok((open_item(lists, id)?, None)),
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Closed(closed),
            ..
        }) => {
            let crate::lists::Record::Item(item) = &closed.record else {
                unreachable!("an item's closed record")
            };
            Ok((item, Some(closed)))
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not an item", kind.name()),
        None => fail!("{id} is not a record"),
    }
}

/// What `builds item times` prints at <now>: for each of <ids>, how long it
/// ran; with no ids, each running item, each item that landed in the last
/// `FLOW_DAYS` days, and each branch on which a run goes on or whose work
/// landed in those days. When a run it shows started before the store
/// began to record held and waiting time, a last line says from when the
/// store records that time.
pub fn times_text(lists: &Lists, ids: &[Id], now: i64) -> Result<String> {
    let mut lines: Vec<(String, Option<Times>)> = Vec::new();
    let item_line = |item: &Item, closed: Option<&How>, indent: &str| -> Result<_> {
        let times = item_times(lists, item, closed)?;
        let words = times
            .as_ref()
            .map_or("no run recorded".to_string(), |times| {
                times_words(times, now)
            });
        Ok((
            format!("{indent}{} {}: {words}", item.id, item.title),
            times,
        ))
    };
    if ids.is_empty() {
        let recent = now - FLOW_DAYS as i64 * 86400;
        lines.push(("running".to_string(), None));
        for item in lists.items.iter().filter(|item| !item.running.is_empty()) {
            lines.push(item_line(item, None, "  ")?);
        }
        lines.push((format!("landed in the last {FLOW_DAYS} days"), None));
        for closed in &lists.closed {
            if let crate::lists::Record::Item(item) = &closed.record
                && matches!(closed.how, How::Landed(_))
                && closed.closed.at >= recent
            {
                lines.push(item_line(item, Some(&closed.how), "  ")?);
            }
        }
        lines.push(("branches".to_string(), None));
        for ((repo, branch), times) in branch_times(lists)? {
            if times.ran.since.is_some() || times.landed.is_some_and(|landed| landed >= recent) {
                let line = format!("  {repo} {branch}: {}", times_words(&times, now));
                lines.push((line, Some(times)));
            }
        }
    }
    for id in ids {
        let (item, closed) = item_and_closing(lists, *id)?;
        lines.push(item_line(item, closed.map(|closed| &closed.how), "")?);
    }
    let mut out = String::new();
    for (line, _) in &lines {
        let _ = writeln!(out, "{line}");
    }
    let from = lines
        .iter()
        .find_map(|(_, times)| times.as_ref()?.spans_from);
    if let Some(from) = from {
        let _ = writeln!(out, "{}", spans_from_words(from));
    }
    Ok(out)
}

/// `builds item times [N...]`.
pub fn times(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let ids: Vec<Id> = args
        .positional()
        .iter()
        .map(|id| Id::parse(id))
        .collect::<Result<_>>()?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    print!("{}", times_text(&store.load()?, &ids, tool.now)?);
    Ok(())
}

/// Item <id>, open or closed, in full, its times as at <now>.
pub fn render_item(lists: &Lists, id: Id, now: i64) -> Result<String> {
    let (item, closed) = item_and_closing(lists, id)?;
    let (state, open) = match closed {
        None => (derive::item_state(lists, item).label().to_string(), true),
        Some(closed) => (
            format!(
                "closed, {} by {} {}",
                how_words(&closed.how),
                closed.closed.by,
                local_minute(closed.closed.at)
            ),
            false,
        ),
    };
    let mut out = String::new();
    let _ = writeln!(out, "{id} {} [{state}]", item.title);
    let _ = writeln!(
        out,
        "area: {}; for: {}",
        item.area,
        match item.r#for {
            Audience::Coordinator => "coordinator",
            Audience::DesignSession => "design session",
        }
    );
    if let Some(priority) = &item.priority {
        let _ = writeln!(
            out,
            "priority {}: {}",
            crate::lists::item::level_word(priority.level),
            priority.why
        );
    }
    if let Some(brief) = &item.brief {
        let _ = writeln!(out, "brief: {brief}");
    }
    for wait in &item.waits {
        let _ = writeln!(out, "waits on {}", wait_line(lists, wait));
    }
    // A closed item runs nowhere, whatever its record holds.
    let running = if open { item.running.as_slice() } else { &[] };
    for running in running {
        let _ = writeln!(out, "{}", marker(running)?);
        let _ = writeln!(
            out,
            "running since {} by {}",
            local_minute(running.since.at),
            running.since.by
        );
    }
    let parked = if open { item.parked.as_slice() } else { &[] };
    for parked in parked {
        let _ = writeln!(out, "{}", parked_words(parked));
    }
    for work in &item.dropped {
        let _ = writeln!(
            out,
            "dropped the work in {} on {}: {}",
            work.worktree,
            work.branch,
            crate::text::counted(work.commits.len(), "commit", "commits")
        );
        for commit in &work.commits {
            let _ = writeln!(out, "  {} {}", commit.sha, commit.subject);
        }
        if !work.files.is_empty() {
            let _ = writeln!(out, "  not committed: {}", work.files.join(", "));
        }
    }
    for check in &item.spec_checks {
        let _ = writeln!(out, "{}", crate::lists::spec_check::line(check));
    }
    if let Some(times) = item_times(lists, item, closed.map(|closed| &closed.how))? {
        let _ = match times.spans_from {
            Some(from) => writeln!(
                out,
                "times: {} ({})",
                times_words(&times, now),
                spans_from_words(from)
            ),
            None => writeln!(out, "times: {}", times_words(&times, now)),
        };
    }
    for part in &item.landed {
        let _ = writeln!(
            out,
            "landed in {}: {} ({})",
            part.repo,
            part.commits.join(" "),
            local_minute(part.at)
        );
    }
    if let Some(trail) = trail_line(lists, id)? {
        let _ = writeln!(out, "{trail}");
    }
    let _ = writeln!(
        out,
        "opened {} by {}",
        local_minute(item.opened.at),
        item.opened.by
    );
    for note in &item.notes {
        let _ = writeln!(
            out,
            "note {} {}: {}",
            local_minute(note.at),
            note.by,
            note.text
        );
    }
    let _ = write!(out, "\n{}\n", item.details.trim_end());
    Ok(out)
}

pub fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [id] = args.exactly(1, "builds item show N")? else {
        unreachable!("exactly one")
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    print!("{}", render_item(&store.load()?, Id::parse(id)?, tool.now)?);
    Ok(())
}

/// What `list` chooses the items it prints by.
struct Choice<'a> {
    running: bool,
    ready: bool,
    area: Option<&'a str>,
    audience: Option<Audience>,
    tag: Option<&'a str>,
}

/// The items `list` prints: those running when asked, those ready when asked
/// (in the queue's order, which the dashboard's Queue tab shows; the rest of
/// the list stays in id order), in the area, for the audience and whose trail
/// has the tag when given.
fn chosen<'a>(lists: &'a Lists, choice: &Choice) -> Result<Vec<&'a Item>> {
    let mut chosen: Vec<&Item> = Vec::new();
    for item in lists
        .items
        .iter()
        .filter(|item| !choice.running || !item.running.is_empty())
        .filter(|item| !choice.ready || derive::item_state(lists, item) == ItemState::Ready)
        .filter(|item| choice.area.is_none_or(|a| item.area == a))
        .filter(|item| choice.audience.is_none_or(|f| item.r#for == f))
    {
        if trail_wanted(lists, item.id, choice.tag)? {
            chosen.push(item);
        }
    }
    if choice.ready {
        derive::queue_order(lists, &mut chosen);
    }
    Ok(chosen)
}

pub fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["area", "for", "tag"], &["running", "ready"])?;
    args.exactly(
        0,
        "builds item list [--running] [--ready] [--area A] [--for F] [--tag T]",
    )?;
    let audience: Option<Audience> = args.choice("for")?;
    let area = args.value("area")?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    if let Some(area) = area
        && !lists.areas.iter().any(|known| known == area)
    {
        fail!(
            "area '{area}' does not exist (the areas are: {})",
            lists.areas.join("; ")
        );
    }
    let chosen = chosen(
        &lists,
        &Choice {
            running: args.switch("running"),
            ready: args.switch("ready"),
            area,
            audience,
            tag: args.value("tag")?,
        },
    )?;
    for item in chosen {
        if args.switch("running") && !item.running.is_empty() {
            println!("{} {}", item.id, item.title);
            for running in &item.running {
                println!("  {}", marker(running)?);
            }
        } else {
            println!("{}", item_line(&lists, item)?);
        }
    }
    Ok(())
}

/// `builds next`: the ready coordinator items in dispatch order.
pub fn next(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, "builds next")?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    print!("{}", next_text(&store.load()?));
    Ok(())
}

/// What `builds next` prints for <lists>: each ready coordinator item, its
/// brief, and the worktree it is parked in, which holds its work.
pub fn next_text(lists: &Lists) -> String {
    let ready = derive::dispatch_order(lists);
    let mut out = String::new();
    if ready.is_empty() {
        out.push_str("nothing is ready to dispatch\n");
    }
    for (number, item) in ready.iter().enumerate() {
        let priority = match &item.priority {
            Some(p) if p.level == Level::First => " [first]",
            Some(_) => " [last]",
            None => "",
        };
        let _ = writeln!(
            out,
            "{:>3}. {} {}{priority} ({})",
            number + 1,
            item.id,
            item.title,
            item.area
        );
        if let Some(brief) = &item.brief {
            let _ = writeln!(out, "       brief {brief}");
        }
        for parked in &item.parked {
            let _ = writeln!(out, "       {}", parked_words(parked));
        }
    }
    out
}

#[cfg(test)]
#[path = "item_show_tests.rs"]
mod tests;
