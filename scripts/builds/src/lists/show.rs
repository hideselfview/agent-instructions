//! `lists show` as text: the stack in the order it is asked, then the queue.

use std::fmt::Write;

use crate::lists::derive::{self, ItemState};
use crate::lists::{Audience, Item, Lists, Level, Position, QuestionKind, Wait};
use crate::text::local_minute;

fn kind_words(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Question => "question",
        QuestionKind::YourStep => "for you to do",
        QuestionKind::Held => "held until you start it",
    }
}

fn wait_words(lists: &Lists, wait: &Wait) -> String {
    let named = |kind: &str, id: crate::lists::Id| {
        let title = lists.locate(id).map(|found| found.title).unwrap_or("?");
        format!("{kind} {id} \"{title}\"")
    };
    match wait {
        Wait::Item(id) => named("item", *id),
        Wait::Question(id) => named("question", *id),
        Wait::Design(id) => named("design change", *id),
        Wait::Condition(text) => format!("condition: {text}"),
    }
}

fn item_line(lists: &Lists, item: &Item) -> String {
    let state = derive::item_state(lists, item);
    let mut line = format!("  {} [{}] {}", item.id, state.label(), item.title);
    if item.r#for == Audience::DesignSession && state != ItemState::Running {
        line.push_str(" (design session's)");
    }
    let unmet: Vec<String> = derive::unmet_waits(lists, item).iter().map(|w| wait_words(lists, w)).collect();
    if !unmet.is_empty() {
        let _ = write!(line, "\n      waits on {}", unmet.join("; "));
    }
    if derive::waits_for_others(lists, item) {
        line.push_str("\n      last: waits until no other coordinator work is open");
    }
    if let Some(running) = &item.running {
        let _ = write!(line, "\n      running in {} on {}, brief {}", running.worktree, running.branch, running.brief);
    }
    line
}

pub fn render(lists: &Lists, mode: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} lists, version {}, {mode} mode", lists.project, lists.version);
    let stack = derive::stack_order(lists);
    let _ = writeln!(out, "\nWaiting on you ({})", stack.len());
    for (number, entry) in stack.iter().enumerate() {
        let q = entry.question;
        let _ = writeln!(
            out,
            "{:>3}. {} {} [{}] opened {} by {}",
            number + 1,
            q.id,
            q.title,
            kind_words(q.kind),
            local_minute(q.opened.at),
            q.opened.by
        );
        if let Some(placed) = &q.placed {
            let at = if placed.at == Position::First { "first" } else { "last" };
            let by = if placed.by_person { " by you" } else { "" };
            let _ = writeln!(out, "       placed {at}{by}: {}", placed.why);
        }
        match &q.blocks_nothing {
            Some(why) => {
                let _ = writeln!(out, "       blocks nothing: {why}");
            }
            None => {
                let named: Vec<String> = entry.blocks.iter().map(|i| format!("{} {}", i.id, i.title)).collect();
                let _ = writeln!(out, "       blocks {} ({} through chains): {}", entry.blocks.len(), entry.blocked_count, named.join("; "));
            }
        }
    }
    let state = |wanted: ItemState| lists.items.iter().filter(|i| derive::item_state(lists, i) == wanted).count();
    let _ = writeln!(out, "\nWork ({} ready, {} running)", state(ItemState::Ready), state(ItemState::Running));
    let running: Vec<&Item> = lists.items.iter().filter(|i| i.running.is_some()).collect();
    if !running.is_empty() {
        let _ = writeln!(out, "Running");
        for item in running {
            let _ = writeln!(out, "{}", item_line(lists, item));
        }
    }
    let first: Vec<&Item> = lists
        .items
        .iter()
        .filter(|i| i.running.is_none() && i.priority.as_ref().is_some_and(|p| p.level == Level::First))
        .collect();
    if !first.is_empty() {
        let _ = writeln!(out, "First");
        for item in first {
            let _ = writeln!(out, "{}", item_line(lists, item));
        }
    }
    for area in &lists.areas {
        let items: Vec<&Item> = lists
            .items
            .iter()
            .filter(|i| {
                i.area == *area
                    && i.running.is_none()
                    && !i.priority.as_ref().is_some_and(|p| p.level == Level::First)
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{area}");
        for item in items {
            let _ = writeln!(out, "{}", item_line(lists, item));
        }
    }
    let _ = writeln!(
        out,
        "\n{} designs open, {} decisions, {} closed",
        lists.designs.len(),
        lists.decisions.len(),
        lists.closed.len()
    );
    out
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
