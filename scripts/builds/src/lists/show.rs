//! `lists show` as text: the stack in the order it is asked, then the queue.

use std::fmt::Write;

use crate::fail::Result;
use crate::lists::derive::{self, ItemState, StackEntry};
use crate::lists::trail::{trail_line, trail_wanted};
use crate::lists::{Audience, Item, Level, Lists, Question, QuestionKind, Wait};
use crate::text::local_minute;

pub fn kind_words(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Question => "question",
        QuestionKind::YourStep => "for you to do",
        QuestionKind::Held => "held until you start it",
    }
}

pub fn wait_words(lists: &Lists, wait: &Wait) -> String {
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

/// Where an item is parked, as the queue's lines say it.
pub fn parked_words(parked: &crate::lists::parked::Parked) -> String {
    format!(
        "parked in {} on {} since {} by {}",
        parked.worktree,
        parked.branch,
        local_minute(parked.since.at),
        parked.since.by
    )
}

/// An item as lines. Fails when `trail::trail` fails on the item's trail.
pub fn item_line(lists: &Lists, item: &Item) -> Result<String> {
    let state = derive::item_state(lists, item);
    let mut line = format!("  {} [{}] {}", item.id, state.label(), item.title);
    if item.r#for == Audience::DesignSession && state != ItemState::Running {
        line.push_str(" (design session's)");
    }
    if let Some(trail) = trail_line(lists, item.id)? {
        let _ = write!(line, "\n      {trail}");
    }
    let unmet: Vec<String> = derive::unmet_waits(lists, item)
        .iter()
        .map(|w| wait_words(lists, w))
        .collect();
    if !unmet.is_empty() {
        let _ = write!(line, "\n      waits on {}", unmet.join("; "));
    }
    if derive::waits_for_others(lists, item) {
        line.push_str("\n      last: waits until no other coordinator work is open");
    }
    for running in &item.running {
        let _ = write!(
            line,
            "\n      running in {} on {}, brief {}",
            running.worktree, running.branch, running.brief
        );
    }
    for parked in &item.parked {
        let _ = write!(line, "\n      {}", parked_words(parked));
    }
    Ok(line)
}

/// What an entry of the stack blocks, worked out from the open items that
/// wait on it, as lines: the count, then each item it blocks directly with
/// its priority (first, last or none), how many items wait behind it and when
/// it opened. When no
/// open item waits on the entry, one line gives the reason the entry stays
/// open. Fails for an entry with neither, which the check refuses.
pub fn blocks_lines(entry: &StackEntry<'_>) -> Result<Vec<String>> {
    if entry.blocks.is_empty() {
        let Some(why) = &entry.question.blocks_nothing else {
            crate::fail!(
                "question {} blocks no open item and gives no reason it stays on the stack, which the check refuses",
                entry.question.id
            );
        };
        return Ok(vec![format!("blocks nothing: {why}")]);
    }
    let mut lines = vec![format!(
        "blocks {} ({} through chains):",
        entry.blocks.len(),
        entry.blocked_count
    )];
    for blocked in &entry.blocks {
        let item = blocked.item;
        let mut facts = vec![match &item.priority {
            Some(priority) => format!(
                "priority {}: {}",
                crate::lists::item::level_word(priority.level),
                priority.why
            ),
            None => "priority none".to_string(),
        }];
        facts.push(match blocked.behind {
            0 => "no item waits behind it".to_string(),
            1 => "1 item waits behind it".to_string(),
            n => format!("{n} items wait behind it"),
        });
        facts.push(format!("opened {}", local_minute(item.opened.at)));
        lines.push(format!(
            "  {} {} ({})",
            item.id,
            item.title,
            facts.join("; ")
        ));
    }
    Ok(lines)
}

/// Who placed a question where on the stack, and why; None for a question
/// that nobody placed, which the stack orders by what it unblocks and its
/// effect.
pub fn place_words(question: &Question) -> Option<String> {
    question.placed.as_ref().map(|placed| {
        let who = if placed.by_person { "you" } else { "an agent" };
        format!("{who} placed it {}: {}", placed.at.word(), placed.why)
    })
}

/// An entry of the stack as one line, as `question list` prints it: its
/// place in the order, its id and title, then its effect, how many open
/// items it blocks when it blocks any, who asked it and when, and its trail.
pub fn stack_line(lists: &Lists, number: usize, entry: &StackEntry<'_>) -> Result<String> {
    let q = entry.question;
    let mut facts = vec![q.effect.words().to_string()];
    if entry.blocked_count > 0 {
        facts.push(format!("blocks {}", entry.blocked_count));
    }
    facts.push(format!("{}, {}", q.opened.by, local_minute(q.opened.at)));
    if let Some(trail) = trail_line(lists, q.id)? {
        facts.push(trail);
    }
    Ok(format!(
        "{number:>3}. {} {}  {}\n",
        q.id,
        q.title,
        facts.join(" · ")
    ))
}

/// An entry of the stack as lines: its place in the order and its title,
/// then who asked it and when, its effect, who placed it and why when
/// someone did, what it blocks, and its trail.
pub fn stack_entry(lists: &Lists, number: usize, entry: &StackEntry<'_>) -> Result<String> {
    let q = entry.question;
    let mut out = format!(
        "{number:>3}. {} {} [{}]\n",
        q.id,
        q.title,
        kind_words(q.kind)
    );
    let _ = writeln!(
        out,
        "       asked by {}, {}",
        q.opened.by,
        local_minute(q.opened.at)
    );
    let _ = writeln!(out, "       effect: {}", q.effect.words());
    if let Some(placed) = place_words(q) {
        let _ = writeln!(out, "       {placed}");
    }
    for line in blocks_lines(entry)? {
        let _ = writeln!(out, "       {line}");
    }
    if let Some(trail) = trail_line(lists, q.id)? {
        let _ = writeln!(out, "       {trail}");
    }
    Ok(out)
}

/// The stack and the queue as text; with <wanted>, only the questions and
/// items whose trail has that tag, each question keeping its place in the
/// stack.
pub fn render(lists: &Lists, wanted: Option<&str>) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "{} lists, version {}", lists.project, lists.version);
    if let Some(wanted) = wanted {
        let _ = writeln!(out, "tagged {wanted}");
    }
    let mut stack: Vec<(usize, StackEntry<'_>)> = Vec::new();
    for (at, entry) in derive::stack_order(lists).into_iter().enumerate() {
        if trail_wanted(lists, entry.question.id, wanted)? {
            stack.push((at, entry));
        }
    }
    let _ = writeln!(out, "\nWaiting on you ({})", stack.len());
    for (at, entry) in &stack {
        out.push_str(&stack_entry(lists, at + 1, entry)?);
    }
    let mut items: Vec<&Item> = Vec::new();
    for item in &lists.items {
        if trail_wanted(lists, item.id, wanted)? {
            items.push(item);
        }
    }
    let state = |wanted: ItemState| {
        items
            .iter()
            .filter(|i| derive::item_state(lists, i) == wanted)
            .count()
    };
    let _ = writeln!(
        out,
        "\nWork ({} ready, {} running)",
        state(ItemState::Ready),
        state(ItemState::Running)
    );
    let running: Vec<&Item> = items
        .iter()
        .copied()
        .filter(|i| !i.running.is_empty())
        .collect();
    if !running.is_empty() {
        let _ = writeln!(out, "Running");
        for item in running {
            let _ = writeln!(out, "{}", item_line(lists, item)?);
        }
    }
    let first: Vec<&Item> = items
        .iter()
        .copied()
        .filter(|i| {
            i.running.is_empty() && i.priority.as_ref().is_some_and(|p| p.level == Level::First)
        })
        .collect();
    if !first.is_empty() {
        let _ = writeln!(out, "First");
        for item in first {
            let _ = writeln!(out, "{}", item_line(lists, item)?);
        }
    }
    for area in &lists.areas {
        let in_area: Vec<&Item> = items
            .iter()
            .copied()
            .filter(|i| {
                i.area == *area
                    && i.running.is_empty()
                    && !i.priority.as_ref().is_some_and(|p| p.level == Level::First)
            })
            .collect();
        if in_area.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{area}");
        for item in in_area {
            let _ = writeln!(out, "{}", item_line(lists, item)?);
        }
    }
    let _ = writeln!(
        out,
        "\n{} designs open, {} decisions, {} closed",
        lists.designs.len(),
        lists.decisions.len(),
        lists.closed.len()
    );
    Ok(out)
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
