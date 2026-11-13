//! The whole-state check: everything that must hold of a store at all
//! times. It runs on every load and before every write. A store that fails it
//! makes every command fail naming each fault, and nothing repairs it.

use std::collections::{BTreeMap, BTreeSet};

use crate::fail;
use crate::fail::Result;
use crate::lists::{
    Closed, Decision, Id, Item, Kind, Lists, Located, Record, Resolution, Standing, Wait, day, side_worktree_parts,
};

/// The most words an item's title has, and the fewest.
pub const TITLE_WORDS: std::ops::RangeInclusive<usize> = 5..=7;

/// Is <name> usable as a project's folder name? Letters, digits, `-`, `_`
/// and inner dots; so no name leaves the store's parent folder.
pub fn project_name_fault(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok { None } else { Some(format!("'{name}' is not a project name: letters, digits, - _ and . (not first) only")) }
}

/// Why <title> is not a one-line title, or None.
pub fn title_fault(title: &str) -> Option<String> {
    if title.trim().is_empty() {
        Some("a title is empty".to_string())
    } else if title.contains(['\n', '\r', '\t']) {
        Some(format!("title '{}' is not one line", title.replace(['\n', '\r', '\t'], " ")))
    } else if title != title.trim() {
        Some(format!("title '{title}' has spaces at its ends"))
    } else {
        None
    }
}

/// Why <title> is not an item's title (one line, five to seven words).
pub fn item_title_fault(title: &str) -> Option<String> {
    if let Some(fault) = title_fault(title) {
        return Some(fault);
    }
    let words = title.split_whitespace().count();
    if TITLE_WORDS.contains(&words) {
        None
    } else {
        Some(format!(
            "item title '{title}' has {words} words; item titles are {} to {}",
            TITLE_WORDS.start(),
            TITLE_WORDS.end()
        ))
    }
}

/// Checks <lists>; the error names every fault, one per line.
pub fn check(lists: &Lists) -> Result<()> {
    let faults = faults(lists);
    if faults.is_empty() {
        return Ok(());
    }
    fail!(
        "the lists of {} fail their check, and nothing repairs them:\n  {}",
        lists.project,
        faults.join("\n  ")
    );
}

/// Every fault of <lists>.
pub fn faults(lists: &Lists) -> Vec<String> {
    let mut faults = Vec::new();
    header(lists, &mut faults);
    ids(lists, &mut faults);
    areas(lists, &mut faults);
    for question in &lists.questions {
        if let Some(fault) = title_fault(&question.title) {
            faults.push(format!("question {}: {fault}", question.id));
        }
        stamp(&question.opened.by, &format!("question {}", question.id), &mut faults);
        let waiting = lists.items.iter().any(|item| item.waits.contains(&Wait::Question(question.id)));
        match &question.blocks_nothing {
            None if !waiting => faults.push(format!(
                "question {} blocks no open item and says nothing about why it is asked",
                question.id
            )),
            Some(why) if why.trim().is_empty() => {
                faults.push(format!("question {} has an empty blocks-nothing reason", question.id))
            }
            _ => {}
        }
        if let Some(placed) = &question.placed
            && placed.why.trim().is_empty()
        {
            faults.push(format!("question {} is placed with no why", question.id));
        }
    }
    for item in &lists.items {
        open_item(lists, item, &mut faults);
    }
    for design in &lists.designs {
        design_change(lists, design, &mut faults);
    }
    for decision in &lists.decisions {
        decision_record(lists, decision, &mut faults);
    }
    for closed in &lists.closed {
        closed_record(lists, closed, &mut faults);
    }
    cycles(lists, &mut faults);
    replaces_cycles(lists, &mut faults);
    faults
}

fn stamp(by: &str, what: &str, faults: &mut Vec<String>) {
    if by.trim().is_empty() {
        faults.push(format!("{what} records no one who wrote it"));
    }
}

/// Why <repo> is no repo directory name, if it isn't.
pub fn repo_name_fault(repo: &str) -> Option<String> {
    (repo.is_empty() || repo.contains('/')).then(|| format!("repo '{repo}' is not a repo directory name"))
}

fn header(lists: &Lists, faults: &mut Vec<String>) {
    faults.extend(project_name_fault(&lists.project));
    if lists.repos.is_empty() {
        faults.push("the project has no repos".to_string());
    }
    let mut seen = BTreeSet::new();
    for repo in &lists.repos {
        faults.extend(repo_name_fault(repo));
        if !seen.insert(repo) {
            faults.push(format!("repo {repo} is listed twice"));
        }
    }
    for (what, map) in [
        ("design places", &lists.design_places),
        ("design sources", &lists.design_sources),
        ("spec files", &lists.spec_files),
    ] {
        for (repo, globs) in map {
            if !lists.repos.contains(repo) {
                faults.push(format!("{what} name repo {repo}, which is not one of the project's"));
            }
            if globs.iter().any(|glob| glob.trim().is_empty()) {
                faults.push(format!("repo {repo} has an empty glob among its {what}"));
            }
        }
    }
    if lists.version == 0 && !lists.log.is_empty() {
        faults.push("version 0 with a log".to_string());
    }
    let mut previous = 0;
    for line in &lists.log {
        if line.version <= previous {
            faults.push(format!("the log's version {} does not follow {previous}", line.version));
        }
        previous = line.version;
        stamp(&line.by, &format!("log line {}", line.version), faults);
    }
    if let Some(last) = lists.log.last()
        && last.version != lists.version
    {
        faults.push(format!("version is {} but the log ends at {}", lists.version, last.version));
    }
}

fn ids(lists: &Lists, faults: &mut Vec<String>) {
    let mut all: Vec<Id> = Vec::new();
    all.extend(lists.questions.iter().map(|r| r.id));
    all.extend(lists.items.iter().map(|r| r.id));
    all.extend(lists.designs.iter().map(|r| r.id));
    all.extend(lists.decisions.iter().map(|r| r.id));
    all.extend(lists.closed.iter().map(|c| c.record.id()));
    let mut seen = BTreeSet::new();
    for id in all {
        if !seen.insert(id) {
            faults.push(format!("{id} is the id of two records"));
        }
        if id.0 == 0 || id >= lists.next_id {
            faults.push(format!("{id} is not below next_id {}", lists.next_id));
        }
    }
}

fn areas(lists: &Lists, faults: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for area in &lists.areas {
        if let Some(fault) = title_fault(area) {
            faults.push(format!("area: {fault}"));
        }
        if !seen.insert(area) {
            faults.push(format!("area '{area}' is listed twice"));
        }
    }
}

/// Where a reference of <wanted> kind to <id> stands, or the fault.
fn reference(lists: &Lists, id: Id, wanted: &[Kind], from: &str) -> Option<String> {
    match lists.locate(id) {
        None => Some(format!("{from} names {id}, which no record has")),
        Some(Located { kind, .. }) if !wanted.contains(&kind) => {
            let names: Vec<&str> = wanted.iter().map(|k| k.name()).collect();
            Some(format!("{from} names {id}, a {}, where a {} belongs", kind.name(), names.join(" or ")))
        }
        Some(_) => None,
    }
}

fn wait_fault(lists: &Lists, wait: &Wait, from: &str) -> Option<String> {
    match wait {
        Wait::Item(id) => reference(lists, *id, &[Kind::Item], from),
        Wait::Question(id) => reference(lists, *id, &[Kind::Question], from),
        Wait::Design(id) => reference(lists, *id, &[Kind::Design], from),
        Wait::Condition(text) if text.trim().is_empty() => Some(format!("{from} has an empty condition")),
        Wait::Condition(_) => None,
    }
}

fn open_item(lists: &Lists, item: &Item, faults: &mut Vec<String>) {
    let from = format!("item {}", item.id);
    faults.extend(item_title_fault(&item.title).map(|f| format!("{from}: {f}")));
    stamp(&item.opened.by, &from, faults);
    if !lists.areas.contains(&item.area) {
        faults.push(format!("{from} is in area '{}', which does not exist", item.area));
    }
    for wait in &item.waits {
        faults.extend(wait_fault(lists, wait, &from));
        if let Some(target) = unmeetable(lists, wait) {
            faults.push(format!("{from} waits on {target}, which can no longer be met"));
        }
    }
    if let Some(origin) = item.from {
        faults.extend(reference(lists, origin, &[Kind::Decision, Kind::Design], &format!("{from}'s origin")));
    }
    if let Some(priority) = &item.priority
        && priority.why.trim().is_empty()
    {
        faults.push(format!("{from} has a priority with no why"));
    }
    if let Some(brief) = &item.brief
        && brief.trim().is_empty()
    {
        faults.push(format!("{from} has an empty brief path"));
    }
    if let Some(running) = &item.running {
        match side_worktree_parts(&running.worktree) {
            Some((repo, _)) if lists.repos.contains(&repo) => {}
            Some((repo, _)) => faults.push(format!(
                "{from} runs in {}, a worktree of repo {repo}, which is not one of the project's",
                running.worktree
            )),
            None => faults.push(format!(
                "{from} runs in {}, which is not a side worktree (<parent>/<repo>-worktrees/side-N)",
                running.worktree
            )),
        }
        if running.branch.trim().is_empty() || running.brief.trim().is_empty() {
            faults.push(format!("{from} runs with no branch or no brief"));
        }
        stamp(&running.since.by, &format!("{from}'s running marker"), faults);
    }
    for part in &item.landed {
        if !lists.repos.contains(&part.repo) || part.commits.is_empty() {
            faults.push(format!("{from} has a landed part of repo '{}' with {} commits", part.repo, part.commits.len()));
        }
    }
    notes(item, faults);
}

fn notes(item: &Item, faults: &mut Vec<String>) {
    for note in &item.notes {
        if note.text.trim().is_empty() {
            faults.push(format!("item {} has an empty note", item.id));
        }
        stamp(&note.by, &format!("a note of item {}", item.id), faults);
    }
}

/// The record a wait of an open item names when that record closed in a way
/// that never meets a wait (dropped, deferred, moved to the stack).
fn unmeetable(lists: &Lists, wait: &Wait) -> Option<String> {
    let id = match wait {
        Wait::Item(id) | Wait::Question(id) | Wait::Design(id) => *id,
        Wait::Condition(_) => return None,
    };
    let Some(Located { standing: Standing::Closed(closed), .. }) = lists.locate(id) else { return None };
    if crate::lists::derive::closed_meets_wait(closed) { None } else { Some(format!("{id} (closed as {})", closed.how.label())) }
}

fn design_change(lists: &Lists, design: &crate::lists::DesignChange, faults: &mut Vec<String>) {
    let from = format!("design change {}", design.id);
    faults.extend(title_fault(&design.title).map(|f| format!("{from}: {f}")));
    stamp(&design.opened.by, &from, faults);
    if !lists.repos.contains(&design.repo) {
        faults.push(format!("{from} is of repo {}, which is not one of the project's", design.repo));
    }
    for (path, resolution) in &design.places {
        match resolution {
            Resolution::Item(id) => faults.extend(reference(lists, *id, &[Kind::Item], &format!("{from}'s place {path}"))),
            Resolution::Matches(how) if how.trim().is_empty() => {
                faults.push(format!("{from}'s place {path} matches with no how it was checked"))
            }
            Resolution::Matches(_) => {}
        }
    }
    if design.no_app_place.as_deref().is_some_and(|why| why.trim().is_empty()) {
        faults.push(format!("{from} has an empty no-app-place reason"));
    }
    for (path, board) in &design.boards {
        if !design.places.contains_key(path) {
            faults.push(format!("{from}'s board {path} is no place of it"));
        }
        if board.link.trim().is_empty() {
            faults.push(format!("{from}'s board {path} has an empty canvas link"));
        }
    }
    if let Some(approved) = &design.approved {
        stamp(&approved.by, &format!("{from}'s approval"), faults);
    }
    for landing in &design.landed {
        if landing.commits.is_empty() {
            faults.push(format!("{from} has a landing with no commits"));
        }
    }
}

fn decision_record(lists: &Lists, decision: &Decision, faults: &mut Vec<String>) {
    let from = format!("decision {}", decision.id);
    faults.extend(title_fault(&decision.title).map(|f| format!("{from}: {f}")));
    stamp(&decision.recorded.by, &from, faults);
    if day::parse(&decision.date).is_none() {
        faults.push(format!("{from} is dated '{}', not YYYY-MM-DD", decision.date));
    }
    if decision.by.trim().is_empty() {
        faults.push(format!("{from} names no one who decided"));
    }
    if decision.body.trim().is_empty() {
        faults.push(format!("{from} has an empty body"));
    }
    if let Some(question) = decision.question {
        faults.extend(reference(lists, question, &[Kind::Question], &format!("{from}'s question")));
    }
    for work in &decision.work {
        faults.extend(reference(lists, *work, &[Kind::Item], &format!("{from}'s work")));
    }
    if let Some(replaces) = decision.replaces {
        if replaces == decision.id {
            faults.push(format!("{from} replaces itself"));
        } else {
            faults.extend(reference(lists, replaces, &[Kind::Decision], &format!("{from}'s replaces")));
        }
    }
}

fn closed_record(lists: &Lists, closed: &Closed, faults: &mut Vec<String>) {
    let from = format!("closed {} {}", closed.record.kind().name(), closed.record.id());
    stamp(&closed.closed.by, &from, faults);
    stamp(&closed.record.opened().by, &from, faults);
    faults.extend(title_fault(closed.record.title()).map(|f| format!("{from}: {f}")));
    if let Record::Item(item) = &closed.record {
        faults.extend(item_title_fault(&item.title).map(|f| format!("{from}: {f}")));
        for wait in &item.waits {
            faults.extend(wait_fault(lists, wait, &from));
        }
        if let Some(origin) = item.from {
            faults.extend(reference(lists, origin, &[Kind::Decision, Kind::Design], &format!("{from}'s origin")));
        }
    }
    if let Record::Design(design) = &closed.record {
        for (path, resolution) in &design.places {
            if let Resolution::Item(id) = resolution {
                faults.extend(reference(lists, *id, &[Kind::Item], &format!("{from}'s place {path}")));
            }
        }
    }
    if !closed.how.fits(closed.record.kind()) {
        faults.push(format!("{from} left as {}, which a {} cannot", closed.how.label(), closed.record.kind().name()));
    }
    match &closed.how {
        crate::lists::How::Answered(decision) => {
            faults.extend(reference(lists, *decision, &[Kind::Decision], &format!("{from}'s answer")))
        }
        crate::lists::How::MovedToStack(question) => {
            faults.extend(reference(lists, *question, &[Kind::Question], &format!("{from}'s move to the stack")))
        }
        crate::lists::How::Settled(text) | crate::lists::How::Dropped(text) | crate::lists::How::Deferred(text)
            if text.trim().is_empty() =>
        {
            faults.push(format!("{from} left as {} with nothing said", closed.how.label()))
        }
        crate::lists::How::Landed(commits) if commits.is_empty() => {
            faults.push(format!("{from} landed with no commits"))
        }
        _ => {}
    }
}

/// No open item waits on itself through a chain of open items.
fn cycles(lists: &Lists, faults: &mut Vec<String>) {
    let edges: BTreeMap<Id, Vec<Id>> = lists
        .items
        .iter()
        .map(|item| {
            let on = item
                .waits
                .iter()
                .filter_map(|wait| match wait {
                    Wait::Item(id) if lists.item(*id).is_some() => Some(*id),
                    _ => None,
                })
                .collect();
            (item.id, on)
        })
        .collect();
    let mut cleared: BTreeSet<Id> = BTreeSet::new();
    for start in edges.keys() {
        let mut path = Vec::new();
        if let Some(cycle) = find_cycle(*start, &edges, &mut path, &mut cleared) {
            let names: Vec<String> = cycle.iter().map(|id| id.to_string()).collect();
            faults.push(format!("items wait on each other in a circle: {}", names.join(" -> ")));
            return;
        }
    }
}

fn find_cycle(
    at: Id,
    edges: &BTreeMap<Id, Vec<Id>>,
    path: &mut Vec<Id>,
    cleared: &mut BTreeSet<Id>,
) -> Option<Vec<Id>> {
    if cleared.contains(&at) {
        return None;
    }
    if let Some(position) = path.iter().position(|id| *id == at) {
        let mut cycle = path[position..].to_vec();
        cycle.push(at);
        return Some(cycle);
    }
    path.push(at);
    for next in &edges[&at] {
        if let Some(cycle) = find_cycle(*next, edges, path, cleared) {
            return Some(cycle);
        }
    }
    path.pop();
    cleared.insert(at);
    None
}

/// No decision revises itself through a chain of revisions.
fn replaces_cycles(lists: &Lists, faults: &mut Vec<String>) {
    for decision in &lists.decisions {
        let mut at = decision.replaces;
        let mut steps = 0;
        while let Some(id) = at {
            if id == decision.id {
                if decision.replaces != Some(decision.id) {
                    faults.push(format!("decision {} revises itself through a chain of revisions", decision.id));
                }
                break;
            }
            steps += 1;
            if steps > lists.decisions.len() {
                break;
            }
            at = lists.decision(id).and_then(|d| d.replaces);
        }
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
