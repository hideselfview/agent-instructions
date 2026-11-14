//! The app checkouts as build lanes, as the page draws them apart from the
//! worktree cards: nobody owns an app checkout and builds take turns in it,
//! so each shows who builds in it now, what runs, and who waits.

use serde::Serialize;

use super::model::{CheckoutPage, RepoPage};

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
        minutes: i64,
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppBuild {
    pub lane: String,
    /// The app checkout, `side-N`.
    pub checkout: String,
    /// A branch, or `detached at <commit>`.
    pub checked_out: String,
    pub disk_kib: Option<u64>,
    pub now: Now,
    /// What waits for a lock of the lane, next first: the branch of the
    /// worktree each build started from.
    pub waiting: Vec<String>,
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

/// What <repo>'s app build lanes do for <branch>: the lane, whether the
/// build runs, and in words how far it is: the checkout that builds it and
/// for how long, or its place in the lane's queue and how long it has
/// waited, then the steps it asked for. The running build first; None when
/// no build of the branch runs or waits.
pub fn build_for(repo: &RepoPage, branch: &str) -> Option<(String, bool, String)> {
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
            return Some((
                lane_name(&lock.lane),
                true,
                format!(
                    "builds in {} for {}, {}",
                    lock.checkout,
                    minutes_word(holder.minutes),
                    asked_words(&asked)
                ),
            ));
        }
    }
    for queue in &repo.queues {
        for (at, waiting) in queue.queue.iter().enumerate() {
            if let Some(asked) = named(&waiting.command) {
                return Some((
                    lane_name(&queue.lane),
                    false,
                    format!(
                        "place {} in the queue, waiting {}, {}",
                        at + 1,
                        minutes_word(waiting.minutes),
                        asked_words(&asked)
                    ),
                ));
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

/// One entry per live app checkout of <repo>, mac lane first.
pub fn build(repo: &RepoPage) -> Vec<AppBuild> {
    let mut found: Vec<AppBuild> =
        repo.checkouts
            .iter()
            .filter(|c| c.live && c.app_checkout)
            .filter_map(|c| {
                let lane = c.app_lane?;
                let holder = repo
                    .locks
                    .iter()
                    .find(|l| l.lane == lane && l.checkout == c.name)
                    .and_then(|l| l.holder.as_ref());
                let now = match holder {
                    None => Now::Idle,
                    Some(holder) if !holder.driver_running => Now::Stuck {
                        minutes: holder.minutes,
                        running: holder.running.iter().map(|r| plain_command(r)).collect(),
                    },
                    Some(holder) => {
                        let mut programs: Vec<Program> = Vec::new();
                        for process in &c.activity.processes {
                            match programs.iter_mut().find(|p| p.name == process.name) {
                                Some(program) => program.count += 1,
                                None => programs.push(Program {
                                    name: process.name.clone(),
                                    count: 1,
                                }),
                            }
                        }
                        Now::Building {
                            who: who(repo, holder.started_from.as_deref()),
                            branch: branch_of(&holder.command),
                            minutes: holder.minutes,
                            programs,
                            cpu: c.activity.cpu,
                            memory: c.activity.memory,
                        }
                    }
                };
                let waiting =
                    repo.queues
                        .iter()
                        .find(|q| q.lane == lane)
                        .map(|q| {
                            q.queue
                                .iter()
                                .map(|waiting| {
                                    started_from(repo, waiting.started_from.as_deref())
                                        .unwrap_or_else(|| {
                                            if waiting.command.contains("build-app") {
                                                "your test app".to_string()
                                            } else {
                                                "a build".to_string()
                                            }
                                        })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                Some(AppBuild {
                    lane: lane.to_string(),
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
                    now,
                    waiting,
                })
            })
            .collect();
    found.sort_by(|a, b| (b.lane.as_str(), &a.checkout).cmp(&(a.lane.as_str(), &b.checkout)));
    found
}

#[cfg(test)]
#[path = "app_builds_tests.rs"]
mod tests;
