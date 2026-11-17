//! The app checkouts as build lanes, as the page draws them apart from the
//! worktree cards: nobody owns an app checkout and builds take turns in it,
//! so each shows who builds in it now, what runs, and who waits.

use serde::Serialize;

use super::cards::{ItemWait, QueuedBuild};
use super::model::{CheckoutPage, RepoPage};
use crate::health::{Holder, LockHealth};
use crate::holder_run::RunTimes;
use crate::status::LaneChange;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Program {
    pub name: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Now {
    /// Nobody holds the lock.
    Idle,
    /// A build holds the lock and its programs run.
    Building {
        /// The claim of the worktree it was started from, else that
        /// worktree's name, else "A build".
        who: String,
        /// The branch (or ref) the running build is for, read from the
        /// build's own command line; None when it names none.
        branch: Option<String>,
        /// How long it waited for a checkout, how long it has built, and the
        /// step it runs; None when it keeps no record of its run.
        run: Option<RunTimes>,
        programs: Vec<Program>,
        cpu: f64,
        memory: u64,
    },
    /// The lock is held by a build that no longer runs.
    Stuck {
        minutes: i64,
        /// What still runs in the holder's tree, as plain commands.
        running: Vec<String>,
    },
}

/// One app checkout of a lane: a row of the lane's card.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppBuild {
    /// The app checkout, `side-N`, or `host:name` for one on another Mac.
    pub checkout: String,
    /// A branch, or `detached at <commit>`.
    pub checked_out: String,
    pub disk_kib: Option<u64>,
    /// Is it a checkout on another Mac? Its disk, CPU and memory are not
    /// read from here.
    pub remote: bool,
    /// When and why builds added it; None for one a person added.
    pub added_by_builds: Option<LaneChange>,
    pub now: Now,
}

/// One build waiting for a checkout of the lane.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Waiting {
    /// The branch the build is for, else what it is ("your test app").
    pub branch: String,
    /// The worktree it was started from; None when its folder is unknown.
    pub worktree: Option<String>,
    pub minutes: i64,
}

/// One app build lane of a repo: its checkouts, and the one queue they share.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppLane {
    pub lane: String,
    /// The checkouts that build for the lane, local ones first by name, then
    /// those on other Macs.
    pub checkouts: Vec<AppBuild>,
    /// What waits for a lock of the lane, next first.
    pub waiting: Vec<Waiting>,
    /// The checkouts builds removed from the lane lately, oldest first.
    pub removed_by_builds: Vec<LaneChange>,
}

/// What an `app-build` or `build-app` command line asks for.
struct Asked {
    /// The branch or ref it builds.
    branch: String,
    /// The steps it names; empty when it asks for every step or, for
    /// `build-app`, for the test app.
    steps: Vec<String>,
    /// Is it `build-app`, the person's test app?
    test_app: bool,
}

/// Reads <command>: `app-build <repo> [--lane <lane>] <branch> <step>...
/// [--test <lane> <class>]...` names its branch as the second plain word and
/// its steps as the plain words after it; `build-app <repo> <lane> [<ref>]`
/// names its optional third, `origin/main` when it names none. The build
/// that runs a lane's share of a request adds `--lane <lane>` after the repo.
fn asked(command: &str) -> Option<Asked> {
    let mut words = command.split_whitespace();
    let name = words
        .by_ref()
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .find(|name| *name == "app-build" || *name == "build-app")?;
    let mut plain: Vec<&str> = Vec::new();
    let mut flags = false;
    while let Some(word) = words.next() {
        if word == "--lane" {
            words.next();
        } else if word.starts_with('-') {
            flags = true;
        } else if !flags {
            plain.push(word);
        }
    }
    match name {
        "app-build" => Some(Asked {
            branch: plain.get(1)?.to_string(),
            steps: plain[2..].iter().map(|step| step.to_string()).collect(),
            test_app: false,
        }),
        _ => Some(Asked {
            branch: plain.get(2).copied().unwrap_or("origin/main").to_string(),
            steps: Vec::new(),
            test_app: true,
        }),
    }
}

fn branch_of(command: &str) -> Option<String> {
    asked(command).map(|asked| asked.branch)
}

fn minutes_word(minutes: i64) -> String {
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("{minutes} {unit}")
}

/// What a build asked for, in words: its steps, the test app, or every step.
fn asked_words(asked: &Asked) -> String {
    if asked.test_app {
        "asked for the test app".to_string()
    } else if asked.steps.is_empty() {
        "asked for every step".to_string()
    } else {
        format!("asked for {}", asked.steps.join(" "))
    }
}

/// "8m" or "3h": <minutes> as a card's line says how long ago.
fn short_age(minutes: i64) -> String {
    if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

/// How long a running build has built, waited and run its step, in words:
/// " for 7 minutes after waiting 25 minutes, running macos-tests for 4
/// minutes"; empty when it keeps no record of its run.
fn run_words(run: Option<&RunTimes>) -> String {
    let Some(run) = run else {
        return String::new();
    };
    let step = match &run.step {
        Some(step) => format!(", running {} for {}", step.name, minutes_word(step.minutes)),
        None => String::new(),
    };
    format!(
        " for {} after waiting {}{step}",
        minutes_word(run.building_minutes),
        minutes_word(run.waited_minutes)
    )
}

/// What <repo>'s app build lanes do for <branch>: the lane, whether the
/// build runs, and in words how far it is: the checkout that builds it, how
/// long it has built after how long a wait and the step it runs, or its
/// place in the lane's queue and how long it has waited, then the steps it
/// asked for. The running build first; None when no build of the branch runs
/// or waits.
pub fn build_for(repo: &RepoPage, branch: &str) -> Option<ItemWait> {
    let lane_name = |lane: &str| {
        let mut word = lane.to_string();
        if let Some(first) = word.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        word
    };
    let named = |command: &str| asked(command).filter(|asked| asked.branch == branch);
    for lock in &repo.locks {
        let Some(holder) = lock.holder.as_ref().filter(|h| h.driver_running) else {
            continue;
        };
        if let Some(asked) = named(&holder.command) {
            return Some(ItemWait::AppBuild {
                lane: lane_name(&lock.lane),
                running: true,
                how: format!(
                    "builds in {}{}, {}",
                    lock.checkout,
                    run_words(holder.run.as_ref()),
                    asked_words(&asked)
                ),
                queued: None,
            });
        }
    }
    for queue in &repo.queues {
        for (at, waiting) in queue.queue.iter().enumerate() {
            if let Some(asked) = named(&waiting.command) {
                let (steps, sentence) = if asked.test_app {
                    (None, "to build the test app".to_string())
                } else if asked.steps.is_empty() {
                    (None, "to run every step".to_string())
                } else {
                    (
                        Some(asked.steps.join(" ")),
                        format!("to run {}", asked.steps.join(" ")),
                    )
                };
                return Some(ItemWait::AppBuild {
                    lane: lane_name(&queue.lane),
                    running: false,
                    how: format!(
                        "place {} in the queue, waiting {}, {}",
                        at + 1,
                        minutes_word(waiting.minutes),
                        asked_words(&asked)
                    ),
                    queued: Some(QueuedBuild {
                        place: at + 1,
                        of: queue.queue.len(),
                        line: format!(
                            "Its agent asked {} ago {}.",
                            short_age(waiting.minutes),
                            sentence
                        ),
                        steps,
                    }),
                });
            }
        }
    }
    None
}

/// <command> without its program's folder and cut to a line.
fn plain_command(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or("");
    let program = program.rsplit('/').next().unwrap_or(program);
    let line: Vec<&str> = std::iter::once(program).chain(words).collect();
    let line = line.join(" ");
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(60).collect::<String>())
    } else {
        line
    }
}

fn find<'a>(repo: &'a RepoPage, checkout: &str) -> Option<&'a CheckoutPage> {
    repo.checkouts.iter().find(|c| c.live && c.name == checkout)
}

/// The app checkout, or its branch, that a build started from.
fn started_from(repo: &RepoPage, from: Option<&str>) -> Option<String> {
    let from = from?;
    let name = from.rsplit('/').next().unwrap_or(from);
    Some(match find(repo, name) {
        Some(c) => c.branch.clone().unwrap_or_else(|| name.to_string()),
        None => name.to_string(),
    })
}

fn who(repo: &RepoPage, from: Option<&str>) -> String {
    let Some(from) = from else {
        return "A build".into();
    };
    let name = from.rsplit('/').next().unwrap_or(from);
    match find(repo, name).and_then(|c| c.claim.as_ref()) {
        Some(claim) => claim.who.clone(),
        None => name.to_string(),
    }
}

/// What a holder of a lock builds now, as a row says it.
fn now_of(
    repo: &RepoPage,
    holder: Option<&Holder>,
    programs: Vec<Program>,
    cpu: f64,
    memory: u64,
) -> Now {
    match holder {
        None => Now::Idle,
        Some(holder) if !holder.driver_running => Now::Stuck {
            minutes: holder.minutes,
            running: holder.running.iter().map(|r| plain_command(r)).collect(),
        },
        Some(holder) => Now::Building {
            who: who(repo, holder.started_from.as_deref()),
            branch: branch_of(&holder.command),
            run: holder.run.clone(),
            programs,
            cpu,
            memory,
        },
    }
}

/// The programs of <processes>, each counted.
fn programs_of<'a>(names: impl Iterator<Item = &'a str>) -> Vec<Program> {
    let mut programs: Vec<Program> = Vec::new();
    for name in names {
        match programs.iter_mut().find(|p| p.name == name) {
            Some(program) => program.count += 1,
            None => programs.push(Program {
                name: name.to_string(),
                count: 1,
            }),
        }
    }
    programs
}

/// The row of the app checkout <c>.
fn local_row(repo: &RepoPage, c: &CheckoutPage, lane: &str) -> AppBuild {
    let holder = repo
        .locks
        .iter()
        .find(|l| l.lane == lane && l.checkout == c.name)
        .and_then(|l| l.holder.as_ref());
    let programs = programs_of(c.activity.processes.iter().map(|p| p.name.as_str()));
    AppBuild {
        checkout: c.name.clone(),
        checked_out: match &c.branch {
            Some(branch) => branch.clone(),
            None => match c.git.as_ref().and_then(|g| g.head.as_ref()) {
                Some(head) => format!(
                    "detached at {}",
                    head.sha.chars().take(7).collect::<String>()
                ),
                None => "detached".to_string(),
            },
        },
        disk_kib: c.total_kib,
        remote: false,
        added_by_builds: None,
        now: now_of(repo, holder, programs, c.activity.cpu, c.activity.memory),
    }
}

/// The row of the checkout <lock> is for on another Mac: only the lock and
/// its holder are known here.
fn remote_row(repo: &RepoPage, lock: &LockHealth) -> AppBuild {
    let programs = lock
        .holder
        .as_ref()
        .map(|h| {
            let names: Vec<String> = h
                .running
                .iter()
                .map(|r| plain_command(r).split(' ').next().unwrap_or("").to_string())
                .collect();
            programs_of(names.iter().map(String::as_str))
        })
        .unwrap_or_default();
    AppBuild {
        checkout: lock.checkout.clone(),
        checked_out: match lock.holder.as_ref().and_then(|h| h.app_head.as_ref()) {
            Some(head) => format!("detached at {}", head.chars().take(7).collect::<String>()),
            None => "detached".to_string(),
        },
        disk_kib: None,
        remote: true,
        added_by_builds: None,
        now: now_of(repo, lock.holder.as_ref(), programs, 0.0, 0),
    }
}

/// Who waits for the lane <lane> of <repo>, next first.
fn waiting_in(repo: &RepoPage, lane: &str) -> Vec<Waiting> {
    let Some(queue) = repo.queues.iter().find(|q| q.lane == lane) else {
        return Vec::new();
    };
    queue
        .queue
        .iter()
        .map(|waiting| Waiting {
            branch: started_from(repo, waiting.started_from.as_deref()).unwrap_or_else(|| {
                if waiting.command.contains("build-app") {
                    "your test app".to_string()
                } else {
                    "a build".to_string()
                }
            }),
            worktree: waiting
                .started_from
                .as_deref()
                .map(|from| from.rsplit('/').next().unwrap_or(from).to_string()),
            minutes: waiting.minutes,
        })
        .collect()
}

/// One lane per app build lane of <repo> that has a checkout, mac first: each
/// live app checkout here, then each checkout on another Mac (its lock is
/// the only thing read from here), each with when and why builds added it,
/// the lane's queue, and the checkouts builds removed from it lately.
pub fn build(repo: &RepoPage) -> Vec<AppLane> {
    let mut found: Vec<AppLane> = Vec::new();
    for lane in crate::lane::Lane::ALL {
        let word = lane.word();
        let mut checkouts: Vec<AppBuild> = repo
            .checkouts
            .iter()
            .filter(|c| c.live && c.app_checkout && c.app_lane == Some(word))
            .map(|c| local_row(repo, c, word))
            .collect();
        checkouts.sort_by(|a, b| a.checkout.cmp(&b.checkout));
        checkouts.extend(
            repo.locks
                .iter()
                .filter(|l| l.lane == word && crate::app_remote::split(&l.checkout).is_some())
                .map(|l| remote_row(repo, l)),
        );
        let recorded = repo.lanes.iter().find(|recorded| recorded.lane == word);
        for row in &mut checkouts {
            row.added_by_builds = recorded.and_then(|recorded| {
                recorded
                    .added_by_builds
                    .iter()
                    .find(|added| added.checkout == row.checkout)
                    .cloned()
            });
        }
        if !checkouts.is_empty() {
            found.push(AppLane {
                lane: word.to_string(),
                checkouts,
                waiting: waiting_in(repo, word),
                removed_by_builds: recorded
                    .map(|recorded| recorded.removed_by_builds.clone())
                    .unwrap_or_default(),
            });
        }
    }
    found
}

#[cfg(test)]
#[path = "app_builds_tests.rs"]
mod tests;
