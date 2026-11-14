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

/// The branch or ref a build's command line builds: `app-build <repo>
/// <branch> ...` names it as its second word; `build-app <repo> <lane>
/// [<ref>]` as its optional third, `origin/main` when it names none.
fn branch_of(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let at = words.iter().position(|word| {
        let name = word.rsplit('/').next().unwrap_or(word);
        name == "app-build" || name == "build-app"
    })?;
    let name = words[at].rsplit('/').next().unwrap_or(words[at]);
    let args: Vec<&str> = words[at + 1..]
        .iter()
        .copied()
        .filter(|word| !word.starts_with('-'))
        .collect();
    match name {
        "app-build" => args.get(1).map(|branch| branch.to_string()),
        _ => Some(args.get(2).copied().unwrap_or("origin/main").to_string()),
    }
}

/// What <repo>'s app build lanes do for <branch>: the lane and how far it
/// is (running for N minutes, or its place in the lane's queue), the
/// running build first. None when no build of the branch runs or waits.
pub fn build_for(repo: &RepoPage, branch: &str) -> Option<(String, String)> {
    let named = |command: &str| branch_of(command).as_deref() == Some(branch);
    let lane_name = |lane: &str| {
        let mut word = lane.to_string();
        if let Some(first) = word.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        word
    };
    for lock in &repo.locks {
        let Some(holder) = lock.holder.as_ref().filter(|h| h.driver_running) else {
            continue;
        };
        if named(&holder.command) {
            let minutes = holder.minutes;
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            return Some((
                lane_name(&lock.lane),
                format!("running for {minutes} {unit}"),
            ));
        }
    }
    for queue in &repo.queues {
        if let Some(at) = queue.queue.iter().position(|q| named(&q.command)) {
            return Some((
                lane_name(&queue.lane),
                format!("place {} in the queue", at + 1),
            ));
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
