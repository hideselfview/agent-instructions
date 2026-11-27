//! The work health watches and the waits that link it. A wait does not
//! advance the waiting item's clock. A review advances the worktree it reads.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::{Command, Parsed, review};
use crate::db::Sql;
use crate::design_run::DesignCommand;
use crate::fail::{Fail, Result};
use crate::places::Places;

use crate::health::{Advice, Finding, FindingKind, Health, LandingWait, Scanned, WorktreeHealth};
use crate::host_scan::{Row, arguments, program};
use crate::install_gate::{InstallRead, Stores};
use crate::lists::derive::{ItemState, item_state, unmet_waits, wait_met};
use crate::lists::store::Loaded;
use crate::lists::{Id, Item, Lists, Wait};
use crate::text::{duration, minutes_since};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Record(String, Id),
    Review(String, i64),
    Worktree(String),
    Process(String, i32),
}

impl Key {
    fn local(pid: i32) -> Self {
        Self::Process(crate::host::LOCAL.to_string(), pid)
    }
}

#[derive(Clone)]
enum Blocker {
    Work(Key),
    /// A command that a branch's agent waits on
    /// (`WorktreeHealth::branch_runs`): a process that the trace of a record
    /// names, with what it waits on.
    Run(Key),
    Reason(String),
}

impl Blocker {
    fn reason(text: impl Into<String>) -> Self {
        Self::Reason(text.into())
    }

    /// The record or process waited on.
    fn key(&self) -> Option<&Key> {
        match self {
            Self::Work(key) | Self::Run(key) => Some(key),
            Self::Reason(_) => None,
        }
    }
}

struct Work {
    name: String,
    moved_at: Option<i64>,
    /// Processes or worktrees doing work on behalf of this record.
    doing: BTreeSet<Key>,
    waits: Vec<Blocker>,
    wait_join: &'static str,
    otherwise: Advice,
    report: bool,
    /// Whether the record is a work item, which a row counts among the items
    /// it blocks.
    item: bool,
}

impl Work {
    fn new(name: String, moved_at: Option<i64>, otherwise: impl Into<String>) -> Self {
        Self {
            name,
            moved_at,
            doing: BTreeSet::new(),
            waits: Vec::new(),
            wait_join: " and ",
            otherwise: Advice::said(otherwise.into()),
            report: true,
            item: false,
        }
    }
}

#[derive(Default)]
struct WorkGraph(BTreeMap<Key, Work>);

impl WorkGraph {
    fn get_mut(&mut self, key: &Key) -> Option<&mut Work> {
        self.0.get_mut(key)
    }

    fn moved_at(
        &self,
        key: &Key,
        known: &mut BTreeMap<Key, Option<i64>>,
        path: &mut BTreeSet<Key>,
    ) -> Option<i64> {
        if let Some(at) = known.get(key) {
            return *at;
        }
        let work = self.0.get(key)?;
        if !path.insert(key.clone()) {
            return work.moved_at;
        }
        let at = work
            .moved_at
            .into_iter()
            .chain(
                work.doing
                    .iter()
                    .filter_map(|child| self.moved_at(child, known, path)),
            )
            .max();
        path.remove(key);
        known.insert(key.clone(), at);
        at
    }

    fn waits(
        &self,
        work: &Work,
        now: i64,
        times: &BTreeMap<Key, Option<i64>>,
        seen: &mut BTreeSet<Key>,
        path: &mut BTreeSet<Key>,
        hide_processes: bool,
    ) -> Vec<String> {
        work.waits
            .iter()
            .filter_map(|blocker| {
                let (key, run) = match blocker {
                    Blocker::Reason(reason) => return Some(reason.clone()),
                    Blocker::Work(key) => (key, false),
                    Blocker::Run(key) => (key, true),
                };
                if hide_processes && !run && self.0.contains_key(key) && is_process(key) {
                    return None;
                }
                if path.contains(key) {
                    return Some(format!("{} again", self.0[key].name));
                }
                if !seen.insert(key.clone()) {
                    return None;
                }
                let Some(waited) = self.0.get(key) else {
                    return Some(match key {
                        Key::Process(host, pid) => {
                            format!("pid {pid} on {host} (health could not read it)")
                        }
                        Key::Worktree(path) => format!("{path} (health found no claim there)"),
                        Key::Review(project, id) => {
                            format!("review {id} of {project} (health could not read it)")
                        }
                        Key::Record(project, id) => {
                            format!("{id} of {project} (health could not read it)")
                        }
                    });
                };
                let age = match times[key] {
                    Some(at) => format!("last moved {} ago", elapsed(now, at)),
                    None => "health has no time for its last movement".to_string(),
                };
                path.insert(key.clone());
                let next = self.waits(waited, now, times, seen, path, hide_processes && !run);
                path.remove(key);
                if next.is_empty() {
                    Some(format!(
                        "{} ({age}. {})",
                        waited.name,
                        waited.otherwise.text.trim_end_matches('.')
                    ))
                } else {
                    Some(format!(
                        "{} ({age}), which waits on {}",
                        waited.name,
                        next.join(waited.wait_join)
                    ))
                }
            })
            .collect()
    }

    fn times(&self) -> BTreeMap<Key, Option<i64>> {
        let mut times = BTreeMap::new();
        for key in self.0.keys() {
            self.moved_at(key, &mut times, &mut BTreeSet::new());
        }
        times
    }

    /// The processes that a record, a worktree or an item names as doing its
    /// work or as a wait, with the processes those start or wait on.
    fn held_processes(&self) -> BTreeSet<&Key> {
        let named = |work: &'_ Work| -> Vec<&Key> {
            work.doing
                .iter()
                .chain(work.waits.iter().filter_map(Blocker::key))
                .filter_map(|key| self.0.get_key_value(key).map(|(key, _)| key))
                .filter(|key| is_process(key))
                .collect()
        };
        let mut held = BTreeSet::new();
        let mut next: Vec<&Key> = self
            .0
            .iter()
            .filter(|(key, _)| !is_process(key))
            .flat_map(|(_, work)| named(work))
            .collect();
        while let Some(key) = next.pop() {
            if held.insert(key) {
                next.extend(named(&self.0[key]));
            }
        }
        held
    }

    /// For each record, how many items wait on it, directly or through other
    /// records.
    fn items_blocked(&self) -> BTreeMap<&Key, usize> {
        let mut waiters: BTreeMap<&Key, Vec<&Key>> = BTreeMap::new();
        for (key, work) in &self.0 {
            for wait in &work.waits {
                if let Blocker::Work(blocker) = wait
                    && let Some((blocker, _)) = self.0.get_key_value(blocker)
                {
                    waiters.entry(blocker).or_default().push(key);
                }
            }
        }
        self.0
            .keys()
            .map(|key| {
                let mut seen = BTreeSet::from([key]);
                let mut next = vec![key];
                while let Some(at) = next.pop() {
                    for waiter in waiters.get(at).into_iter().flatten() {
                        if seen.insert(waiter) {
                            next.push(waiter);
                        }
                    }
                }
                let items = seen
                    .iter()
                    .filter(|waiter| **waiter != key && self.0[**waiter].item)
                    .count();
                (key, items)
            })
            .collect()
    }

    /// One row for each record that someone must act on: a record that has
    /// not moved for <limit> minutes and waits on no other record that health
    /// lists. A record that waits on another record gets no row: that record
    /// is the person's (a question), moves (so the wait is healthy), or has
    /// its own row, which counts the items it blocks. A process gets a row
    /// only when no record names it, so the rows name agents, worktrees and
    /// items, not the shells and cargo runs under them.
    fn findings(&self, times: &BTreeMap<Key, Option<i64>>, now: i64, limit: u64) -> Vec<Finding> {
        let held = self.held_processes();
        let blocked = self.items_blocked();
        self.0
            .iter()
            .filter_map(|(key, work)| {
                let at = times[key]?;
                if !work.report || (minutes_since(now, at) as u64) < limit {
                    return None;
                }
                if is_process(key) && held.contains(key) {
                    return None;
                }
                let waits_on_record = work.waits.iter().any(|wait| {
                    matches!(wait, Blocker::Work(blocker)
                        if self.0.contains_key(blocker) && !is_process(blocker))
                });
                if waits_on_record {
                    return None;
                }
                let subject = format!("{} last moved {} ago", work.name, elapsed(now, at));
                let trace = self.waits(
                    work,
                    now,
                    times,
                    &mut BTreeSet::new(),
                    &mut BTreeSet::from([key.clone()]),
                    !is_process(key),
                );
                let (mut text, command) = if trace.is_empty() {
                    (
                        format!("{subject}. {}", work.otherwise.text),
                        work.otherwise.command.clone(),
                    )
                } else {
                    (
                        format!("{subject} and waits on {}.", trace.join(work.wait_join)),
                        None,
                    )
                };
                match blocked[key] {
                    0 => {}
                    1 => text.push_str(" It blocks 1 item."),
                    items => text.push_str(&format!(" It blocks {items} items.")),
                }
                Some(Finding::new(FindingKind::Still, Advice { text, command }))
            })
            .collect()
    }
}

fn is_process(key: &Key) -> bool {
    matches!(key, Key::Process(..))
}

pub(super) fn elapsed(now: i64, at: i64) -> String {
    duration(now.saturating_sub(at).max(0))
}

fn record(project: &str, id: Id) -> Key {
    Key::Record(project.to_string(), id)
}

fn blockers(lists: &Lists, item: &Item) -> Vec<Blocker> {
    let mut waits: Vec<_> = unmet_waits(lists, item)
        .into_iter()
        .map(|wait| match wait {
            Wait::Item(id) | Wait::Question(id) | Wait::Design(id) => {
                Blocker::Work(record(&lists.project, *id))
            }
            Wait::Foreign { project, item } => Blocker::Work(record(project, *item)),
            Wait::Condition { text, until } => Blocker::reason(format!(
                "the {} to end the condition “{text}”",
                until.word()
            )),
        })
        .collect();
    waits.extend(
        crate::lists::derive::items_ahead_of_last(lists, item)
            .map(|ahead| Blocker::Work(record(&lists.project, ahead.id))),
    );
    waits
}

fn item_moved(lists: &Lists, item: &Item, event_at: Option<i64>) -> Result<i64> {
    let met = item
        .waits
        .iter()
        .filter(|wait| wait_met(lists, wait))
        .map(|wait| crate::lists::kept_work::met_at(lists, wait))
        .collect::<Result<Vec<_>>>()?;
    Ok(std::iter::once(item.opened.at)
        .chain(event_at)
        .chain(item.notes.iter().map(|note| note.at))
        .chain(item.running.iter().map(|run| run.since.at))
        .chain(item.runs.iter().map(|run| run.ended.at))
        .chain(
            item.spans
                .iter()
                .flat_map(|span| [Some(span.since), span.ended])
                .flatten(),
        )
        .chain(item.landed.iter().map(|part| part.at))
        .chain(met)
        .max()
        .expect("the item has an opening time"))
}

fn records(graph: &mut WorkGraph, loaded: &[Loaded], progress: Option<&Progress>) -> Result<()> {
    for project in loaded {
        let Ok(lists) = &project.lists else { continue };
        let name = &lists.project;
        let progress = progress.and_then(|progress| progress.project(name));
        for item in &lists.items {
            let state = item_state(lists, item);
            let otherwise = match state {
                // The item waits on the person's choice to start it, not on
                // someone who should act: health lists no row for it.
                ItemState::Backlog => continue,
                ItemState::Running => "The agent has no recorded wait.".to_string(),
                ItemState::Ready if !item.kept.is_empty() => {
                    let places = crate::lists::kept_work::places_words(
                        item.kept.iter().map(|kept| (&kept.worktree, &kept.branch)),
                    );
                    format!("The coordinator can resume the item or land its work in {places}.")
                }
                ItemState::Ready | ItemState::Waiting => {
                    "The item waits for an agent to start it.".to_string()
                }
                ItemState::DesignSession => "The item waits for the design session.".to_string(),
            };
            let mut work = Work::new(
                format!("item {}", item.named(Some(name))),
                Some(item_moved(
                    lists,
                    item,
                    progress.and_then(|progress| progress.items.get(&item.id).copied()),
                )?),
                otherwise,
            );
            work.item = true;
            work.waits = blockers(lists, item);
            work.doing.extend(
                item.running
                    .iter()
                    .map(|run| Key::Worktree(run.worktree.clone())),
            );
            graph.0.insert(record(name, item.id), work);
        }
        for question in &lists.questions {
            let mut work = Work::new(
                format!("question {} “{}” of {name}", question.id, question.title),
                Some(question.opened.at),
                "The question waits for the person.",
            );
            work.report = false;
            graph.0.insert(record(name, question.id), work);
        }
        for design in &lists.designs {
            let mut work = Work::new(
                format!("design {} “{}” of {name}", design.id, design.title),
                Some(
                    design
                        .approved
                        .as_ref()
                        .map_or(design.opened.at, |stamp| stamp.at),
                ),
                if design.awaits_approval() {
                    "The design change waits for approval."
                } else {
                    "The design change waits to land."
                },
            );
            work.report = false;
            graph.0.insert(record(name, design.id), work);
        }
        for closed in &lists.closed {
            let mut work = Work::new(
                format!(
                    "{} {} “{}” of {name}",
                    closed.record.kind().name(),
                    closed.record.id(),
                    closed.record.title()
                ),
                Some(closed.closed.at),
                format!("The record closed as {}.", closed.how.words()),
            );
            work.report = false;
            graph.0.insert(record(name, closed.record.id()), work);
        }
        for review in progress.into_iter().flat_map(|progress| &progress.reviews) {
            let landing = &review.landing;
            let numbers = |ids: &[i64]| {
                ids.iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let otherwise = if !review.unsettled.is_empty() {
                format!(
                    "The judge or coordinator must settle or answer findings {}.",
                    numbers(&review.unsettled)
                )
            } else if !review.confirmed.is_empty() {
                format!(
                    "The agent must fix confirmed findings {}.",
                    numbers(&review.confirmed)
                )
            } else {
                "The branch waits to land.".to_string()
            };
            graph.0.insert(
                Key::Review(name.clone(), landing.id),
                Work::new(
                    format!(
                        "review {} of {name} ({}, {})",
                        landing.id, landing.repo, landing.key
                    ),
                    Some(review.at),
                    otherwise,
                ),
            );
        }
    }
    Ok(())
}

fn worktree_pids(tree: &WorktreeHealth) -> impl Iterator<Item = i32> + '_ {
    tree.building
        .iter()
        .map(|run| run.pid)
        .chain(tree.remote_runs.iter().map(|run| run.pid))
        .chain(tree.branch_runs().map(|(_, pid)| pid))
        .chain(tree.app_build)
        .chain(tree.in_line.iter().map(|run| run.pid))
}

fn worktrees(
    graph: &mut WorkGraph,
    health: &Health,
    loaded: &[Loaded],
    owned: &BTreeMap<String, Vec<Key>>,
    progress: Option<&Progress>,
) {
    let mut held_reviews = BTreeSet::new();
    for tree in &health.worktrees {
        let mut work = Work::new(
            format!("worktree {} ({})", tree.checkout, tree.claim.who),
            Some(tree.moved_at),
            "The agent has no recorded wait.",
        );
        if let Some(note) = tree.claim.note_text() {
            work.otherwise = Advice::said(format!("The claim says “{note}”."));
        }
        if let Some(advice) = &tree.landing {
            work.otherwise = advice.clone();
        }

        let mut running_item = false;
        let runs: Vec<Key> = tree
            .branch_runs()
            .map(|(_, pid)| Key::local(pid))
            .filter(|key| graph.0.contains_key(key))
            .collect();
        let (repo, _) = tree
            .checkout
            .split_once('/')
            .expect("worktree labels include the repo");
        for project in loaded {
            let Ok(lists) = &project.lists else { continue };
            for item in crate::lists::running::running_in(lists, &tree.path) {
                running_item = true;
                work.name.push_str(&format!(
                    " running item {}",
                    item.named(Some(&lists.project))
                ));
                work.waits.extend(blockers(lists, item));
            }
            if let Some(branch) = tree.branch_at_work() {
                for review in progress
                    .and_then(|progress| progress.project(&lists.project))
                    .into_iter()
                    .flat_map(|progress| &progress.reviews)
                {
                    if review.landing.repo == repo
                        && review.landing.key.strip_prefix("branch:") == Some(branch)
                    {
                        work.moved_at = work.moved_at.max(Some(review.at));
                        let key = Key::Review(lists.project.clone(), review.landing.id);
                        let findings_wait =
                            !review.unsettled.is_empty() || !review.confirmed.is_empty();
                        // With no finding to answer, the branch waits on what
                        // its agent runs, and not yet to land.
                        if !findings_wait
                            && !runs.is_empty()
                            && let Some(ready) = graph.get_mut(&key)
                        {
                            ready.waits = runs.iter().cloned().map(Blocker::Run).collect();
                            ready.doing.extend(runs.iter().cloned());
                        }
                        if findings_wait || !runs.is_empty() {
                            work.waits.push(Blocker::Work(key.clone()));
                        }
                        held_reviews.insert((key, tree.checkout.clone()));
                    }
                }
            }
        }
        if let Some(approval) = &tree.design_approval {
            work.waits.extend(
                approval
                    .designs
                    .iter()
                    .map(|id| Blocker::Work(record(&approval.project, *id))),
            );
        }
        if let Some(progress) = progress {
            work.moved_at = work.moved_at.max(progress.for_branch(repo, tree));
        }
        work.doing.extend(
            worktree_pids(tree)
                .map(Key::local)
                .filter(|key| graph.0.contains_key(key)),
        );
        work.doing
            .extend(owned.get(&tree.path).into_iter().flatten().cloned());
        if tree.claim.is_held() && running_item && work.waits.is_empty() && work.doing.is_empty() {
            work.otherwise = Advice::said(
                "The item's waits have ended. The coordinator can resume the work.".to_string(),
            );
        }
        graph.0.insert(Key::Worktree(tree.path.clone()), work);
    }
    for (key, checkout) in &held_reviews {
        if let Some(review) = graph.0.get_mut(key) {
            review.name.push_str(&format!(" in worktree {checkout}"));
        }
    }
    for project in loaded {
        let Some(progress) = progress.and_then(|progress| progress.project(&project.project))
        else {
            continue;
        };
        for review in &progress.reviews {
            let key = Key::Review(project.project.clone(), review.landing.id);
            if review.landing.key.starts_with("detached:")
                && review.unsettled.is_empty()
                && review.confirmed.is_empty()
                && let Some(work) = graph.0.get_mut(&key)
            {
                work.otherwise = Advice::said(
                    "The reviewed commits are on no worktree's branch, and no item names them."
                        .to_string(),
                );
            }
        }
    }
}

fn command_kind(row: &Row) -> Option<&'static str> {
    use Command::*;
    if program(row) != "builds" {
        return None;
    }
    let words = arguments(row);
    let parsed = Command::of(&words)?;
    let command = parsed.command;
    match command {
        AppBuild | BuildApp | Lock => Some("app build"),
        Land => Some("landing"),
        Install => Some("install"),
        Review(review::Wait) => Some("review wait"),
        // The reviews that ask reviewers, which hold the stores for their
        // whole run.
        Review(_) if command.stores() == Stores::ForLife => Some("review"),
        Cargo | Lake | Remote(_) | Checks(_) => Some("process"),
        _ if command.starts_agent() => Some("agent"),
        _ => DesignCommand::of_words(&words).map(DesignCommand::word),
    }
}

/// When the newest write to an agent's log happened, for <pid> when it is a
/// `builds agent run` or `resume`, which holds the log open. A claude process
/// that builds started writes nothing there itself: its output goes to builds
/// through a pipe, so its movement is builds' write to the log.
fn agent_log_at(pid: i32) -> Option<i64> {
    crate::procs::open_files(pid)
        .iter()
        .filter(|path| crate::agent_launch::is_log(path))
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.mtime())
        .max()
}

fn output_at(pid: i32) -> std::result::Result<Option<i64>, String> {
    let mut newest = None;
    for path in crate::procs::output_paths(pid) {
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => newest = newest.max(Some(meta.mtime())),
            Ok(_) => {}
            // The process may close or replace a descriptor after the scan.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "health could not read output at {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(newest)
}

fn process_work(
    graph: &mut WorkGraph,
    health: &Health,
    scanned: &Scanned,
) -> BTreeMap<String, Vec<Key>> {
    let assigned: BTreeSet<_> = health
        .worktrees
        .iter()
        .flat_map(worktree_pids)
        .chain(
            health
                .locks
                .iter()
                .filter_map(|lock| lock.holder.as_ref().map(|h| h.pid)),
        )
        .chain(
            health
                .queues
                .iter()
                .flat_map(|q| q.queue.iter().map(|entry| entry.pid)),
        )
        .collect();
    let mut owned: BTreeMap<String, Vec<Key>> = BTreeMap::new();
    for (host, scan) in &scanned.scans {
        let Ok(scan) = scan else { continue };
        let local = host.target.is_none();
        let key = |pid| Key::Process(host.name.clone(), pid);
        let mut roots = BTreeSet::new();
        if local {
            roots.extend(&assigned);
            if let Ok(InstallRead::Active(wait)) = &scanned.install {
                roots.insert(wait.installer);
                roots.extend(wait.held.iter().chain(&wait.blocked).copied());
            }
            for row in scan
                .rows
                .iter()
                .filter(|row| row.uid == scan.uid && !row.zombie)
            {
                if program(row) == "builds"
                    && Command::of(&arguments(row)).is_some_and(|parsed| {
                        matches!(
                            parsed.command,
                            Command::Health | Command::Dashboard(_) | Command::Version
                        )
                    })
                {
                    continue;
                }
                let explicit =
                    command_kind(row).is_some() || crate::shells::is_agent_command(&row.command);
                let checkout = crate::procs::cwd(row.pid).and_then(|cwd| {
                    crate::procs::deepest_containing(
                        health.worktrees.iter().map(|tree| Path::new(&tree.path)),
                        &cwd,
                    )
                    .map(|at| &health.worktrees[at])
                });
                if let Some(tree) = checkout {
                    owned
                        .entry(tree.path.clone())
                        .or_default()
                        .push(key(row.pid));
                }
                if explicit
                    || checkout.is_some()
                    || crate::procs::environment(row.pid)
                        .and_then(|environment| crate::shells::agent_in_environment(&environment))
                        .is_some()
                {
                    roots.insert(row.pid);
                }
            }
        }
        roots.extend(scan.jobs.iter().map(|job| job.leader));
        let rows = crate::host_scan::trees(&scan.rows, &roots);
        let mut children: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        for row in rows.iter().filter(|row| !row.zombie) {
            children.entry(row.ppid).or_default().push(row.pid);
        }
        let by_pid: BTreeMap<i32, &Row> = rows.iter().map(|row| (row.pid, *row)).collect();
        for row in rows.into_iter().filter(|row| !row.zombie) {
            let pid = row.pid;
            let kind = command_kind(row).unwrap_or("process");
            let command = crate::shells::shown(&row.command);
            let place = crate::host::place(&host.name);
            let activity = scan
                .active
                .get(&pid)
                .map(|at| scanned.at - (scan.at - at).max(0));
            let name = format!("{kind} {pid} on {place} ({command})");
            // The builds command that starts an agent holds its log, and the
            // agent's own process writes only to builds through a pipe.
            let holder = match (command_kind(row), by_pid.get(&row.ppid)) {
                (Some("agent"), _) => Some(pid),
                (_, Some(parent)) if command_kind(parent) == Some("agent") => Some(parent.pid),
                _ => None,
            };
            let log = holder.filter(|_| local).and_then(agent_log_at);
            let (name, at) = match if local { output_at(pid) } else { Ok(None) } {
                Ok(output) => (name, activity.max(output).max(log)),
                Err(error) => (format!("{name} ({error})"), activity),
            };
            let mut work = Work::new(name, at, "Health found no recorded wait.");
            work.doing.extend(
                children
                    .get(&pid)
                    .into_iter()
                    .flatten()
                    .map(|pid| key(*pid)),
            );
            graph.0.insert(key(pid), work);
        }
    }
    for (host, scan) in &scanned.scans {
        let Ok(scan) = scan else { continue };
        for job in &scan.jobs {
            if scanned.holders.holder(&job.run) == Some(crate::job_claim::Holder::Runs)
                && let Some(caller) = job.caller()
                && let Some(work) = graph.get_mut(&Key::local(caller))
            {
                work.doing
                    .insert(Key::Process(host.name.clone(), job.leader));
            }
        }
    }
    owned
}

fn review_wait(places: &Places, pid: i32) -> std::result::Result<Option<Key>, String> {
    let Some(argv) = crate::procs::live_args(pid)? else {
        return Ok(None);
    };
    let words = crate::ack::agent::without_flag(&argv);
    let Some(Parsed {
        command: Command::Review(review::Wait),
        rest: args,
        ..
    }) = words.get(1..).and_then(Command::of)
    else {
        return Ok(None);
    };
    let cwd = crate::procs::cwd(pid)
        .ok_or_else(|| format!("process {pid} has no readable working directory"))?;
    let args: Vec<_> = args.iter().map(|word| (*word).to_string()).collect();
    let (store, id) = crate::review::wait_target(places, &args, &cwd).map_err(Fail::text)?;
    Ok(Some(Key::Review(store.project, id)))
}

fn process_waits(graph: &mut WorkGraph, health: &mut Health, scanned: &Scanned, places: &Places) {
    if let Some((_, Ok(local))) = scanned.scans.iter().find(|(host, _)| host.target.is_none()) {
        if let Ok(InstallRead::Active(gates)) = &scanned.install {
            let installer = Key::local(gates.installer);
            if let Some(work) = graph.get_mut(&installer) {
                work.name.push_str(" holding the install lock");
                work.waits
                    .extend(gates.held.iter().map(|pid| Blocker::Work(Key::local(*pid))));
            }
            for &pid in &gates.blocked {
                if let Some(work) = graph.get_mut(&Key::local(pid)) {
                    work.waits.push(Blocker::Work(installer.clone()));
                }
            }
        }
        if scanned.install == Ok(InstallRead::Handoff) {
            for row in local
                .rows
                .iter()
                .filter(|row| command_kind(row) == Some("install"))
            {
                if let Some(work) = graph.get_mut(&Key::local(row.pid)) {
                    work.otherwise = Advice::said(
                        "An installer held the lock when health read it. Health found no owner record.".to_string()
                    );
                }
            }
        }
        for row in &local.rows {
            if command_kind(row) == Some("review wait")
                && let Some(work) = graph.get_mut(&Key::local(row.pid))
            {
                match review_wait(places, row.pid) {
                    Ok(Some(key)) => work.waits.push(Blocker::Work(key)),
                    Ok(None) => {}
                    Err(error) => {
                        work.otherwise =
                            Advice::said(format!("Health could not read the review wait: {error}"))
                    }
                }
            }
        }
    }
    for tree in &health.worktrees {
        for landing in &tree.landings {
            if let LandingWait::For(wait) = &landing.waits
                && let Some(work) = graph.get_mut(&Key::local(landing.pid))
            {
                let blocker = crate::land::record_pid(&wait.holder)
                    .map(|pid| Blocker::Work(Key::local(pid)))
                    .unwrap_or_else(|| Blocker::reason(wait.to_string()));
                work.waits.push(blocker);
            }
        }
    }
    for lock in &health.locks {
        if let Some(holder) = &lock.holder
            && let Some(work) = graph.get_mut(&Key::local(holder.pid))
            && let Some(run) = &holder.run
        {
            work.moved_at = work.moved_at.max(Some(run.moved_at));
            if let Some(step) = &run.step {
                work.name.push_str(&format!(" running {}", step.name));
            }
        }
    }
    for lane in &health.queues {
        let queue = crate::queue::Queue::of(places, &lane.repo, lane.lane);
        let waiting = match queue.app_waits() {
            Ok(waiting) => waiting,
            Err(error) => {
                health.findings.push(Finding::new(
                    FindingKind::Processes,
                    Advice::said(format!(
                        "Health could not read waits in {}'s {} app queue: {}",
                        lane.repo,
                        lane.lane.word(),
                        error.text()
                    )),
                ));
                continue;
            }
        };
        for (pid, blockers) in waiting {
            if let Some(work) = graph.get_mut(&Key::local(pid)) {
                work.wait_join = " or ";
                work.waits
                    .extend(blockers.into_iter().map(|blocker| match blocker {
                        crate::queue::AppWait::Process(pid) => Blocker::Work(Key::local(pid)),
                        crate::queue::AppWait::HeldCheckout(entry) => Blocker::reason(format!(
                            "the build holding {entry}'s lock. Health could not read its process"
                        )),
                    }));
            }
        }
    }
}

/// Progress that health reads from the stores, apart from the work queue.
#[derive(Default, PartialEq, Eq)]
pub struct Progress(BTreeMap<String, std::result::Result<Arc<ProjectProgress>, String>>);

#[derive(PartialEq, Eq)]
struct ProjectProgress {
    items: BTreeMap<Id, i64>,
    reviews: Vec<crate::review::record::ReviewProgress>,
    ends: Vec<crate::branch_end::BranchEnd>,
}

impl ProjectProgress {
    fn read(store: &crate::lists::store::Store) -> Result<Self> {
        let db = store.open_existing()?;
        db.execute_batch("BEGIN").sql()?;
        let items = crate::lists::item_event::last_open(&db)?;
        let reviews = crate::review::record::open_progress(&db)?;
        let mut ends = crate::app_timing::branch_ends(&db)?;
        ends.extend(crate::land_run::branch_ends(&db)?);
        db.execute_batch("END").sql()?;
        Ok(Self {
            items,
            reviews,
            ends,
        })
    }
}

impl Progress {
    pub fn read(
        places: &Places,
        loaded: &std::result::Result<Vec<Loaded>, String>,
    ) -> std::result::Result<Self, String> {
        Self::read_after(places, loaded, &[], None)
    }

    pub fn complete(&self) -> bool {
        self.0.values().all(|project| project.is_ok())
    }

    pub fn read_after(
        places: &Places,
        loaded: &std::result::Result<Vec<Loaded>, String>,
        moved: &[PathBuf],
        previous: Option<&Self>,
    ) -> std::result::Result<Self, String> {
        let projects = loaded.as_ref().map_err(Clone::clone)?;
        let mut progress = BTreeMap::new();
        for project in projects {
            let name = &project.project;
            let store = crate::lists::store::Store::named(places, name).map_err(Fail::text)?;
            let kept = previous
                .and_then(|before| before.0.get(name))
                .filter(|read| read.is_ok() && !moved.contains(&store.database()));
            let read = match kept {
                Some(read) => read.clone(),
                None => ProjectProgress::read(&store)
                    .map(Arc::new)
                    .map_err(Fail::text),
            };
            progress.insert(name.clone(), read);
        }
        Ok(Self(progress))
    }

    fn project(&self, name: &str) -> Option<&ProjectProgress> {
        self.0.get(name)?.as_ref().ok().map(Arc::as_ref)
    }

    fn for_branch(&self, repo: &str, tree: &WorktreeHealth) -> Option<i64> {
        self.0
            .values()
            .filter_map(|project| project.as_ref().ok())
            .flat_map(|project| &project.ends)
            .filter(|end| {
                end.repo == repo
                    && (tree.branch_at_work() == Some(end.reference.as_str())
                        || tree.git.as_ref().is_ok_and(|git| git.owns(&end.reference)))
            })
            .map(|end| end.at)
            .max()
    }
}

pub fn append(
    health: &mut Health,
    loaded: &std::result::Result<Vec<Loaded>, String>,
    progress: &std::result::Result<Progress, String>,
    scanned: Option<&Scanned>,
    places: &Places,
    now: i64,
    limit: u64,
) {
    health.generated_at = now;
    let projects = loaded.as_deref().unwrap_or_default();
    health.schedule_needed |=
        loaded.is_err() || projects.iter().any(|project| project.lists.is_err());
    let mut graph = WorkGraph::default();
    let mut progress_fault = |project: Option<&str>, error: &str| {
        let place = project.map_or_else(String::new, |name| format!(" for {name}"));
        health.findings.push(Finding::new(
            FindingKind::Lists,
            Advice::said(format!(
                "Health could not read recorded progress{place}: {error}"
            )),
        ));
        health.schedule_needed = true;
    };
    let has_progress = match progress {
        Ok(progress) => {
            for (name, read) in &progress.0 {
                if let Err(error) = read
                    && !projects
                        .iter()
                        .any(|project| &project.project == name && project.lists.is_err())
                {
                    progress_fault(Some(name), error);
                }
            }
            progress.complete()
        }
        Err(error) => {
            if loaded.is_ok() {
                progress_fault(None, error);
            }
            false
        }
    };
    let loaded_progress = progress.as_ref().ok();
    if let Err(error) = records(&mut graph, projects, loaded_progress) {
        health.findings.push(Finding::new(
            FindingKind::Lists,
            Advice::said(format!(
                "Health could not trace an item wait: {}",
                error.text()
            )),
        ));
    }
    let owned = scanned
        .map(|scanned| {
            let owned = process_work(&mut graph, health, scanned);
            process_waits(&mut graph, health, scanned, places);
            owned
        })
        .unwrap_or_default();
    worktrees(&mut graph, health, projects, &owned, loaded_progress);
    for work in graph.0.values_mut() {
        if work.waits.is_empty() {
            work.waits
                .extend(work.doing.iter().cloned().map(Blocker::Work));
        }
    }
    let times = graph.times();
    let local_scan = scanned.is_some_and(|scanned| {
        scanned
            .scans
            .iter()
            .any(|(host, scan)| host.target.is_none() && scan.is_ok())
    });
    for tree in &mut health.worktrees {
        let key = Key::Worktree(tree.path.clone());
        tree.moved_at = times[&key].expect("the worktree has a claim time");
        tree.idle_for_release &= has_progress && local_scan;
    }
    health.schedule_needed |= projects.iter().any(|project| {
        project
            .lists
            .as_ref()
            .is_ok_and(|lists| !lists.items.is_empty())
    }) || graph
        .0
        .keys()
        .any(|key| matches!(key, Key::Process(..) | Key::Review(..)));
    health.findings.extend(graph.findings(&times, now, limit));
    health
        .shells
        .junk
        .retain(|shell| !matches!(shell.reason, crate::shells::Reason::Idle { .. }));
    if !health.shells.junk.is_empty() {
        health.findings.push(Finding::new(
            FindingKind::Shells,
            Advice::run(
                format!(
                    "{} in the list of shells. Stop them with",
                    crate::text::counted(
                        health.shells.junk.len(),
                        "junk shell runs",
                        "junk shells run"
                    )
                ),
                "builds health --stop-junk".to_string(),
            ),
        ));
    }
    if let Some(scanned) = scanned {
        if let Err(error) = &scanned.install {
            health.findings.push(Finding::new(
                FindingKind::Processes,
                Advice::said(format!("Health could not read the install gates: {error}")),
            ));
        }
        for (host, scan) in &scanned.scans {
            let Ok(scan) = scan else { continue };
            for job in &scan.jobs {
                if scanned.holders.holder(&job.run).is_none() {
                    health.findings.push(Finding::new(FindingKind::Processes, Advice::said(
                        format!("Health could not identify the caller of the job in {} on {} from its run ID {:?}.",
                            job.label(), crate::host::place(&host.name), job.run))));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;
    use crate::install_gate::Waiting;
    use crate::testing::Scratch;

    #[test]
    fn unread_lists_keep_the_health_schedule_needed() {
        let scratch = Scratch::new("health-unreadable-lists");
        let tool = scratch.tool(0);
        let mut health = crate::health::health(&tool).unwrap();
        let error = Err("the lists folder could not be read".to_string());
        append(
            &mut health,
            &error,
            &Err(error.as_ref().unwrap_err().clone()),
            None,
            &tool.places,
            tool.now,
            tool.rules.still_minutes,
        );
        assert!(health.schedule_needed);
    }

    fn graph_of(works: Vec<(Key, Work)>) -> WorkGraph {
        WorkGraph(works.into_iter().collect())
    }

    fn rows(graph: &WorkGraph, now: i64) -> Vec<String> {
        graph
            .findings(&graph.times(), now, 30)
            .into_iter()
            .map(|finding| finding.advice.text)
            .collect()
    }

    fn item_work(name: &str, at: i64, waits: Vec<Blocker>) -> Work {
        let mut work = Work::new(
            format!("item {name}"),
            Some(at),
            "The item waits for an agent.",
        );
        work.item = true;
        work.waits = waits;
        work
    }

    #[test]
    fn a_row_counts_the_items_that_wait_on_it_and_they_get_no_row() {
        let (now, old) = (100_000, 0);
        let root = Key::Record("p".to_string(), Id(1));
        let middle = Key::Record("p".to_string(), Id(2));
        let leaf = Key::Record("p".to_string(), Id(3));
        let graph = graph_of(vec![
            (
                root.clone(),
                item_work(
                    "#1",
                    old,
                    vec![Blocker::reason(
                        "the coordinator to end the condition “a host exists”",
                    )],
                ),
            ),
            (
                middle.clone(),
                item_work("#2", old, vec![Blocker::Work(root.clone())]),
            ),
            (leaf, item_work("#3", old, vec![Blocker::Work(middle)])),
        ]);
        let rows = rows(&graph, now);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].starts_with("item #1 last moved")
                && rows[0].contains("a host exists")
                && rows[0].ends_with("It blocks 2 items."),
            "{rows:?}"
        );
    }

    #[test]
    fn an_item_that_waits_on_the_person_or_on_work_that_moves_has_no_row() {
        let (now, old) = (100_000, 0);
        let question = Key::Record("p".to_string(), Id(1));
        let moving = Key::Record("p".to_string(), Id(2));
        let mut asked = Work::new(
            "question 1".to_string(),
            Some(old),
            "The question waits for the person.",
        );
        asked.report = false;
        let graph = graph_of(vec![
            (question.clone(), asked),
            (moving.clone(), item_work("#2", now - 60, Vec::new())),
            (
                Key::Record("p".to_string(), Id(3)),
                item_work("#3", old, vec![Blocker::Work(question)]),
            ),
            (
                Key::Record("p".to_string(), Id(4)),
                item_work("#4", old, vec![Blocker::Work(moving)]),
            ),
        ]);
        assert_eq!(rows(&graph, now), Vec::<String>::new());
    }

    #[test]
    fn a_stalled_worktree_has_a_row_that_names_the_worktree_and_not_its_processes() {
        let (now, old) = (100_000, 0);
        let tree = Key::Worktree("/w".to_string());
        let shell = Key::local(7);
        let mut work = Work::new(
            "worktree demo/side-1 (agent) running item #5".to_string(),
            Some(old),
            "The agent has no recorded wait.",
        );
        work.doing.insert(shell.clone());
        work.waits.push(Blocker::Work(shell.clone()));
        let graph = graph_of(vec![
            (tree, work),
            (
                shell,
                Work::new(
                    "process 7 on this Mac (zsh)".to_string(),
                    Some(old),
                    "Health found no recorded wait.",
                ),
            ),
        ]);
        let rows = rows(&graph, now);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].starts_with("worktree demo/side-1 (agent) running item #5")
                && !rows[0].contains("process 7"),
            "{rows:?}"
        );
    }

    #[test]
    fn an_install_wait_traces_the_command_to_the_idle_process() {
        let scratch = Scratch::new("health-install-wait");
        let mut tool = scratch.tool(0);
        tool.now = 10_000;
        let mut health = crate::health::health(&tool).unwrap();
        let scan = crate::host_scan::parse(
            "uid=501\nnow=10000\ncpu_idle=90\nps_follows=1\n\
             999905 0 501 0 0:00 50:00 S builds install\n\
             999901 0 501 0 0:00 35:00 S builds install\n\
             999902 0 501 0 0:00 45:00 S builds checks run\n\
             999903 999902 501 0 0:00 40:00 S cargo clippy\n\
             999904 0 501 0 0:00 32:00 S builds review .\n",
            &[],
        )
        .unwrap();
        let scanned = Scanned {
            scans: vec![(Host::local(), Ok(scan))],
            install: Ok(InstallRead::Active(Waiting {
                installer: 999901,
                held: vec![999902],
                blocked: vec![999904],
            })),
            holders: crate::job_claim::Holders::read().unwrap(),
            orphans: Vec::new(),
            caches: Ok(Vec::new()),
            at: tool.now,
        };
        append(
            &mut health,
            &Ok(Vec::new()),
            &Ok(Progress::default()),
            Some(&scanned),
            &tool.places,
            tool.now,
            tool.rules.still_minutes,
        );
        let found = health
            .findings
            .iter()
            .filter(|finding| finding.kind == FindingKind::Still)
            .map(|finding| &finding.advice.text)
            .collect::<Vec<_>>();
        let install = found
            .iter()
            .find(|line| line.starts_with("install 999901"))
            .unwrap();
        assert!(
            install.contains("35m")
                && install.contains("checks run")
                && install.contains("cargo clippy")
                && install.contains("40m"),
            "{install}"
        );
        let blocked = found
            .iter()
            .find(|line| line.starts_with("review 999904"))
            .unwrap();
        assert!(
            blocked.contains("install 999901") && blocked.contains("cargo clippy"),
            "{blocked}"
        );
        assert_eq!(found.len(), 5, "{found:?}");
    }

    /// A landing checks out other commits in its worktree. Health still reads
    /// the worktree as working on the landing's branch, so the branch's review
    /// waits on the landing: it has no row while the landing moves, and a
    /// landing that stops moving has one row, which names it.
    #[test]
    fn a_review_whose_branch_a_landing_lands_waits_on_the_landing() {
        let demo = crate::lists::item_fixtures::demo("health-landing-review");
        let side = demo.add_side(1, "agent", "work");
        demo.commit(&side, "a.rs");
        let checkout = crate::checkout::Checkout::find(&side.display().to_string()).unwrap();
        crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
        let conn = crate::db::open(&demo.store.database(), crate::db::Make::Never).unwrap();
        let change = crate::review::record::Change {
            repo: "demo",
            key: "branch:work",
        };
        let at = crate::lists::fixtures::NOON;
        crate::review::record::add_landing(&conn, change, at).unwrap();
        // The landing holds the worktree's turn and has HEAD elsewhere.
        crate::testing::git(&side, &["checkout", "-q", "--detach", "HEAD"]);
        let git_dir = crate::git::text(&side, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let path = Path::new(git_dir.trim()).join(crate::land::WORKTREE_TURN_FILE);
        let mut file = crate::flock::open_creating(&path).unwrap();
        crate::flock::lock(&file, libc::LOCK_EX, &path).unwrap();
        let record = crate::land::turn_record(999901, Some("work"), "--done 1 abc");
        std::io::Write::write_all(&mut file, record.as_bytes()).unwrap();
        let turn = crate::land_turns::read_turn(&side, 999901).unwrap();

        let rows_after = |minutes: i64| {
            let mut tool = demo.scratch.tool(0);
            tool.now = at + 90 * 60;
            let mut health = crate::health::health(&tool).unwrap();
            let tree = &mut health.worktrees[0];
            assert_eq!(tree.branch, None);
            tree.landings.push(crate::health::LandingRun {
                pid: 999901,
                place: 0,
                waits: LandingWait::Nothing,
                branch: turn.branch.clone(),
            });
            // The landing's process last used the CPU <minutes> ago.
            let scan = crate::host_scan::parse(
                &format!(
                    "uid=501\nnow={}\ncpu_idle=90\nps_follows=1\n\
                     999901 0 501 0 0:00 {minutes}:00 S builds land {} --done 1 abc\n",
                    tool.now,
                    side.display()
                ),
                &[],
            )
            .unwrap();
            let scanned = Scanned {
                scans: vec![(Host::local(), Ok(scan))],
                install: Ok(InstallRead::Idle),
                holders: crate::job_claim::Holders::read().unwrap(),
                orphans: Vec::new(),
                caches: Ok(Vec::new()),
                at: tool.now,
            };
            let loaded = crate::lists::store::load_all(&tool.places);
            append(
                &mut health,
                &loaded,
                &Progress::read(&tool.places, &loaded),
                Some(&scanned),
                &tool.places,
                tool.now,
                tool.rules.still_minutes,
            );
            health
                .findings
                .into_iter()
                .filter(|finding| finding.kind == FindingKind::Still)
                .map(|finding| finding.advice.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(rows_after(1), Vec::<String>::new());
        let rows = rows_after(45);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(
            rows[0].starts_with("review 1 of demo (demo, branch:work) in worktree demo/side-1")
                && rows[0].contains("and waits on landing 999901 on this Mac"),
            "{rows:?}"
        );
    }
}
