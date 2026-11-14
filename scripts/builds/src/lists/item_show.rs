//! `builds item show`, `builds item list` and `builds next`: reading the
//! work queue. `next` is the coordinator's dispatch list; `list --running`
//! prints each running item's marker line, what to read after a compact.

use std::fmt::Write;
use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::args::Args;
use crate::lists::derive::{self, ItemState};
use crate::lists::item::open_item;
use crate::lists::running::marker;
use crate::lists::show::{item_line, wait_words};
use crate::lists::store::Store;
use crate::lists::{Audience, How, Id, Item, Kind, Level, Lists, Located, Standing, Wait};
use crate::text::local_minute;
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

/// Item <id>, open or closed, in full.
pub fn render_item(lists: &Lists, id: Id) -> Result<String> {
    let (item, state, open) = match lists.locate(id) {
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Open,
            ..
        }) => {
            let item = open_item(lists, id)?;
            (
                item,
                derive::item_state(lists, item).label().to_string(),
                true,
            )
        }
        Some(Located {
            kind: Kind::Item,
            standing: Standing::Closed(closed),
            ..
        }) => {
            let crate::lists::Record::Item(item) = &closed.record else {
                unreachable!("an item's closed record")
            };
            let text = format!(
                "closed, {} by {} {}",
                how_words(&closed.how),
                closed.closed.by,
                local_minute(closed.closed.at)
            );
            (item, text, false)
        }
        Some(Located { kind, .. }) => fail!("{id} is a {}, not an item", kind.name()),
        None => fail!("{id} is not a record"),
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
        let level = if priority.level == Level::First {
            "first"
        } else {
            "last"
        };
        let _ = writeln!(out, "priority {level}: {}", priority.why);
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
    for part in &item.landed {
        let _ = writeln!(
            out,
            "landed in {}: {} ({})",
            part.repo,
            part.commits.join(" "),
            local_minute(part.at)
        );
    }
    if let Some(from) = item.from {
        let _ = writeln!(out, "made by {from}");
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
    print!("{}", render_item(&store.load()?, Id::parse(id)?)?);
    Ok(())
}

pub fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["area", "for"], &["running", "ready"])?;
    args.exactly(
        0,
        "builds item list [--running] [--ready] [--area A] [--for F]",
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
    let chosen: Vec<&Item> = lists
        .items
        .iter()
        .filter(|item| !args.switch("running") || !item.running.is_empty())
        .filter(|item| {
            !args.switch("ready") || derive::item_state(&lists, item) == ItemState::Ready
        })
        .filter(|item| area.is_none_or(|a| item.area == a))
        .filter(|item| audience.is_none_or(|f| item.r#for == f))
        .collect();
    for item in chosen {
        if args.switch("running") && !item.running.is_empty() {
            println!("{} {}", item.id, item.title);
            for running in &item.running {
                println!("  {}", marker(running)?);
            }
        } else {
            println!("{}", item_line(&lists, item));
        }
    }
    Ok(())
}

/// `builds next`: the ready coordinator items in dispatch order.
pub fn next(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, "builds next")?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let lists = store.load()?;
    let ready = derive::dispatch_order(&lists);
    if ready.is_empty() {
        println!("nothing is ready to dispatch");
    }
    for (number, item) in ready.iter().enumerate() {
        let priority = match &item.priority {
            Some(p) if p.level == Level::First => " [first]",
            Some(_) => " [last]",
            None => "",
        };
        println!(
            "{:>3}. {} {}{priority} ({})",
            number + 1,
            item.id,
            item.title,
            item.area
        );
        if let Some(brief) = &item.brief {
            println!("       brief {brief}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "item_show_tests.rs"]
mod tests;
