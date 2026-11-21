//! The builds the dashboard starts, each a child run exactly as the person
//! would type it, so the lane's queue and app lock order it like any other
//! build: one `builds build-app <repo> <lane> [<ref>]` per click of a lane's
//! Build item, and one `builds app-build <repo> <commit> --shots <prefix>`
//! for each lane of a component whose shots of a commit the component page
//! shows and builds keeps none of (a shots run, component_shots.rs).
//!
//! Each repo's lane keeps its latest build-app run, in memory and in two
//! files under `<daemon dir>/runs/`: `<repo>.<lane>.json` (who started it,
//! which process, how it ended) and `<repo>.<lane>.log` (everything the build
//! printed). A shots run is kept the same way by its prefix, in
//! `<repo>.shots.<prefix>json` and `.log`, and a lane keeps the ended shots
//! runs of one prefix at a time: a new shots run in the lane deletes the
//! ended runs of its other prefixes, whose findings the shot store holds
//! when they took any. The
//! child writes the log itself, not through the daemon, so a daemon that goes
//! away neither breaks the build's output nor stops it: the build runs on and
//! installs its app.
//!
//! A daemon that starts finds the record of a run it did not see end. What
//! the record says then is read from the process, never written: the build is
//! still running (`Detached`: its output is still read, how it ends can't be
//! seen) or is gone without an exit status anyone saw (`Lost`). A lane with a
//! `Detached` run takes no second start, as with a run the daemon holds.
//!
//! A shots run's own process starts a process for its lane, which is the one
//! that waits in the lane's queue, so a shots run shows as running while it
//! waits, and its output says that it waits.

use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::fsevents::Stream;
use super::model::now_seconds;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::run_end::RunEnd;

/// The most of a run's output the page gets: its end.
const TAIL: u64 = 32 * 1024;

/// How a run ended: its exit code, or the signal that stopped it, and which
/// of passed, failed and stopped that makes it (run_end.rs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Ended {
    at: i64,
    code: Option<i32>,
    signal: Option<i32>,
    outcome: RunEnd,
}

/// What is written to `<repo>.<lane>.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Record {
    repo: String,
    lane: String,
    reference: Option<String>,
    pid: i32,
    started_at: i64,
    ended: Option<Ended>,
    /// The shot-name prefix of a shots run, whose reference is the commit it
    /// takes the shots of; none for a build-app run.
    shots: Option<String>,
}

/// What a run the page starts runs: `build-app <repo> <lane> [<reference>]`,
/// or `app-build <repo> <commit> --shots <prefix>` for a shots run.
#[derive(Clone, Copy)]
enum Job<'a> {
    BuildApp { reference: Option<&'a str> },
    Shots { commit: &'a str, prefix: &'a str },
}

/// Which run of a repo's lane a run is: its build-app run (no prefix) or
/// its shots run of a prefix.
type RunKey = (String, Lane, Option<String>);

struct Run {
    record: Record,
    /// This daemon started it and waits for its end.
    owned: bool,
}

/// Where a run is, as the page shows it.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// In the lane's queue, waiting for an app lock.
    Queued,
    Running,
    Succeeded,
    Failed,
    /// A stop signal ended it.
    Stopped,
    /// Started before the daemon did, and still running.
    Detached,
    /// Started before the daemon did, and gone with no exit status seen.
    Lost,
}

#[derive(Serialize, Debug)]
pub struct RunPage {
    pub lane: &'static str,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub phase: Phase,
    pub pid: i32,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// The end of what the build printed (stdout and stderr together).
    pub output: String,
    /// The output starts before this: the page shows only its end.
    pub output_cut: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// The request names something that isn't there or isn't allowed.
    Invalid(String),
    /// The lane already has a run queued or running.
    Busy(String),
    /// The build could not be started or recorded.
    Failed(String),
}

impl StartError {
    pub fn message(&self) -> &str {
        match self {
            StartError::Invalid(m) | StartError::Busy(m) | StartError::Failed(m) => m,
        }
    }
}

pub struct Runs {
    places: Places,
    dir: PathBuf,
    /// The program that runs `build-app`, and the arguments before it:
    /// this binary's path.
    program: Vec<String>,
    state: Mutex<HashMap<RunKey, Run>>,
    listener: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    stream: Mutex<Option<Stream>>,
}

/// Is <pid> a running `builds <command>`? A process alive under that pid
/// could be another that took the number.
fn runs_command(pid: i32, command: &str) -> bool {
    crate::procs::runs(pid)
        && crate::procs::args(pid).is_some_and(|args| args.iter().any(|a| a == command))
}

impl Record {
    /// The builds command the run's process runs.
    fn command(&self) -> &'static str {
        match self.shots {
            None => "build-app",
            Some(_) => "app-build",
        }
    }
}

impl Run {
    fn busy(&self) -> bool {
        self.record.ended.is_none()
            && (self.owned || runs_command(self.record.pid, self.record.command()))
    }

    fn phase(&self, queued: &[i32]) -> Phase {
        match (&self.record.ended, self.owned) {
            (Some(ended), _) => match ended.outcome {
                RunEnd::Passed => Phase::Succeeded,
                RunEnd::Failed => Phase::Failed,
                RunEnd::Stopped => Phase::Stopped,
            },
            (None, true) if queued.contains(&self.record.pid) => Phase::Queued,
            (None, true) => Phase::Running,
            (None, false) if runs_command(self.record.pid, self.record.command()) => {
                Phase::Detached
            }
            (None, false) => Phase::Lost,
        }
    }
}

/// The name of the files of the run of <repo>'s <lane> with <shots>, before
/// their extension: `<repo>.<lane>` for its build-app run, and
/// `<repo>.shots.<prefix>` without the prefix's last dot for a shots run.
fn file_stem(repo: &str, lane: Lane, shots: Option<&str>) -> String {
    match shots {
        None => format!("{repo}.{}", lane.word()),
        Some(prefix) => format!("{repo}.shots.{}", prefix.trim_end_matches('.')),
    }
}

fn record_file(dir: &Path, repo: &str, lane: Lane, shots: Option<&str>) -> PathBuf {
    dir.join(format!("{}.json", file_stem(repo, lane, shots)))
}

fn log_file(dir: &Path, repo: &str, lane: Lane, shots: Option<&str>) -> PathBuf {
    dir.join(format!("{}.log", file_stem(repo, lane, shots)))
}

/// Writes the record whole or not at all.
fn write_record(dir: &Path, record: &Record, lane: Lane) -> Result<()> {
    let stem = file_stem(&record.repo, lane, record.shots.as_deref());
    let file = dir.join(format!("{stem}.json"));
    let staged = dir.join(format!(".{stem}.tmp"));
    std::fs::write(
        &staged,
        serde_json::to_vec(record).expect("a record serializes"),
    )
    .context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, &file).context(|| format!("writing {}", file.display()))
}

fn plain_name(repo: &str) -> bool {
    !repo.is_empty() && repo != "." && repo != ".." && !repo.contains(['/', '\0'])
}

impl Runs {
    /// Reads the records left by earlier daemons and follows the folder the
    /// logs grow in. <program> is what runs `build-app` (this binary).
    pub fn start(places: Places, program: Vec<String>) -> Result<Arc<Runs>> {
        let dir = places.daemon_dir().join("runs");
        std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
        // Taken before the records are read: a write after it is reported.
        let since = Stream::current_event_id();
        let mut state = HashMap::new();
        for entry in std::fs::read_dir(&dir).context(|| format!("listing {}", dir.display()))? {
            let path = entry
                .context(|| format!("listing {}", dir.display()))?
                .path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let text = std::fs::read(&path).context(|| format!("reading {}", path.display()))?;
            let record: Record = serde_json::from_slice(&text)
                .context(|| format!("{} is not a run record", path.display()))?;
            let lane = Lane::parse_or_fail(&record.lane)?;
            state.insert(
                (record.repo.clone(), lane, record.shots.clone()),
                Run {
                    record,
                    owned: false,
                },
            );
        }
        let runs = Arc::new(Runs {
            places,
            dir: dir.clone(),
            program,
            state: Mutex::new(state),
            listener: Mutex::new(None),
            stream: Mutex::new(None),
        });
        let weak = Arc::downgrade(&runs);
        let started = Stream::start_since(
            since,
            &[dir.as_path()],
            &[],
            0.2,
            Box::new(move |_| {
                if let Some(runs) = weak.upgrade() {
                    runs.notify();
                }
            }),
        );
        *runs.stream.lock().expect("no panic holding it") = Some(started);
        Ok(runs)
    }

    /// Calls <listener> when a run starts, prints or ends.
    pub fn listen(&self, listener: Box<dyn Fn() + Send + Sync>) {
        *self.listener.lock().expect("no panic holding it") = Some(listener);
    }

    fn notify(&self) {
        if let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener();
        }
    }

    /// Starts `build-app <repo> <lane> [<reference>]`, unless the lane has a
    /// run queued or running. One call starts at most one build: the check
    /// and the start happen under one lock.
    pub fn start_build(
        self: &Arc<Self>,
        repo: &str,
        lane: &str,
        reference: Option<&str>,
    ) -> std::result::Result<(), StartError> {
        let Some(lane) = Lane::parse(lane) else {
            return Err(StartError::Invalid(format!(
                "{lane:?} is not a lane (mac | android)"
            )));
        };
        if !plain_name(repo) {
            return Err(StartError::Invalid(format!("{repo:?} is not a repo name")));
        }
        let reference = reference.map(str::trim).filter(|r| !r.is_empty());
        if let Some(reference) = reference
            && (reference.starts_with('-') || reference.contains('\0'))
        {
            return Err(StartError::Invalid(format!("{reference:?} is not a ref")));
        }
        match self.places.app_checkouts_of(repo, lane) {
            Ok(checkouts) if !checkouts.is_empty() => {}
            Ok(_) => {
                return Err(StartError::Invalid(format!(
                    "{repo} has no {} app checkout",
                    lane.word()
                )));
            }
            Err(error) => return Err(StartError::Failed(error.text())),
        }
        let mut state = self.state.lock().expect("no panic holding it");
        if let Some(run) = state.get(&(repo.to_string(), lane, None))
            && run.busy()
        {
            return Err(StartError::Busy(format!(
                "a {} build of {repo} is already queued or running",
                lane.word()
            )));
        }
        let run = self
            .spawn(repo, lane, Job::BuildApp { reference })
            .map_err(|error| StartError::Failed(error.text()))?;
        state.insert((repo.to_string(), lane, None), run);
        drop(state);
        self.notify();
        Ok(())
    }

    /// Starts `app-build <repo> <commit> --shots <prefix>`, which takes the
    /// shots of <commit> whose names start with <prefix> on <lane>, unless a
    /// shots run of that prefix is running; deletes the ended shots runs of
    /// the lane's other prefixes. One call starts at most one run. The
    /// component page is the one caller: its route checked that <commit> is a
    /// full commit id, and `ComponentKey::prefix` built <prefix> from a
    /// component's name (component_shots.rs).
    pub fn start_shots(
        self: &Arc<Self>,
        repo: &str,
        lane: Lane,
        commit: &str,
        prefix: &str,
    ) -> std::result::Result<(), StartError> {
        if !plain_name(repo) {
            return Err(StartError::Invalid(format!("{repo:?} is not a repo name")));
        }
        let key = (repo.to_string(), lane, Some(prefix.to_string()));
        let mut state = self.state.lock().expect("no panic holding it");
        if let Some(run) = state.get(&key)
            && run.busy()
        {
            return Err(StartError::Busy(format!(
                "builds is already taking the shots of {prefix} of {repo}"
            )));
        }
        let ended: Vec<RunKey> = state
            .iter()
            .filter(|((own_repo, own_lane, shots), run)| {
                own_repo == repo
                    && *own_lane == lane
                    && shots.as_deref().is_some_and(|other| other != prefix)
                    && !run.busy()
            })
            .map(|(key, _)| key.clone())
            .collect();
        for (_, _, shots) in &ended {
            for file in [
                record_file(&self.dir, repo, lane, shots.as_deref()),
                log_file(&self.dir, repo, lane, shots.as_deref()),
            ] {
                if let Err(error) = std::fs::remove_file(&file)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(StartError::Failed(format!(
                        "deleting {}: {error}",
                        file.display()
                    )));
                }
            }
        }
        for key in &ended {
            state.remove(key);
        }
        let run = self
            .spawn(repo, lane, Job::Shots { commit, prefix })
            .map_err(|error| StartError::Failed(error.text()))?;
        state.insert(key, run);
        drop(state);
        self.notify();
        Ok(())
    }

    /// Starts <job> in <repo>'s <lane>.
    fn spawn(self: &Arc<Self>, repo: &str, lane: Lane, job: Job) -> Result<Run> {
        let (reference, shots) = match job {
            Job::BuildApp { reference } => (reference, None),
            Job::Shots { commit, prefix } => (Some(commit), Some(prefix)),
        };
        let log_path = log_file(&self.dir, repo, lane, shots);
        let log = std::fs::File::create(&log_path)
            .context(|| format!("creating {}", log_path.display()))?;
        let errors = log
            .try_clone()
            .context(|| format!("opening {}", log_path.display()))?;
        let mut command = Command::new(&self.program[0]);
        command.args(&self.program[1..]);
        match job {
            Job::BuildApp { reference } => {
                command.arg("build-app").arg(repo).arg(lane.word());
                if let Some(reference) = reference {
                    command.arg(reference);
                }
            }
            Job::Shots { commit, prefix } => {
                command
                    .arg("app-build")
                    .arg(repo)
                    .arg(commit)
                    .arg("--shots")
                    .arg(prefix);
            }
        }
        command.stdin(Stdio::null()).stdout(log).stderr(errors);
        let mut child = command
            .spawn()
            .context(|| format!("starting {}", self.program[0]))?;
        let record = Record {
            repo: repo.to_string(),
            lane: lane.word().to_string(),
            reference: reference.map(str::to_string),
            pid: child.id() as i32,
            started_at: now_seconds(),
            ended: None,
            shots: shots.map(str::to_string),
        };
        if let Err(error) = write_record(&self.dir, &record, lane) {
            // A build nothing records can't be shown: stop it before it runs.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let runs = self.clone();
        let waiting = record.clone();
        std::thread::Builder::new()
            .name(format!("builds-run-{repo}-{}", lane.word()))
            .spawn(move || {
                let status = child.wait();
                runs.ended(&waiting, lane, status);
            })
            .context(|| "starting a thread".to_string())?;
        Ok(Run {
            record,
            owned: true,
        })
    }

    /// The child of <started> exited, or could not be waited for.
    fn ended(&self, started: &Record, lane: Lane, status: std::io::Result<ExitStatus>) {
        let ended = match status {
            Ok(status) => Ended {
                at: now_seconds(),
                code: status.code(),
                signal: status.signal(),
                outcome: RunEnd::of_code(crate::fail::exit_code(status)),
            },
            Err(error) => {
                eprintln!(
                    "builds: waiting for {} {}'s build: {error}",
                    started.repo, started.lane
                );
                Ended {
                    at: now_seconds(),
                    code: None,
                    signal: None,
                    outcome: RunEnd::Failed,
                }
            }
        };
        {
            let mut state = self.state.lock().expect("no panic holding it");
            if let Some(run) = state.get_mut(&(started.repo.clone(), lane, started.shots.clone()))
                && run.record.pid == started.pid
            {
                run.record.ended = Some(ended);
                // The run shows as ended here either way; a record that
                // can't be written is said, and a restart then shows this
                // run as one whose end wasn't seen.
                if let Err(error) = write_record(&self.dir, &run.record, lane) {
                    eprintln!("builds: {}", error.text());
                }
            }
        }
        self.notify();
    }

    /// The build-app runs of <repo>, one per lane that has one. <queued>
    /// gives the pids waiting in a lane's queue.
    pub fn pages(&self, repo: &str, queued: &dyn Fn(Lane) -> Vec<i32>) -> Vec<RunPage> {
        let state = self.state.lock().expect("no panic holding it");
        Lane::ALL
            .into_iter()
            .filter_map(|lane| {
                let run = state.get(&(repo.to_string(), lane, None))?;
                Some(self.page_of(run, lane, &queued(lane)))
            })
            .collect()
    }

    /// The shots run of <prefix> on <repo>'s <lane>, if the lane keeps one.
    pub fn shots_page(&self, repo: &str, lane: Lane, prefix: &str) -> Option<RunPage> {
        let state = self.state.lock().expect("no panic holding it");
        let run = state.get(&(repo.to_string(), lane, Some(prefix.to_string())))?;
        Some(self.page_of(run, lane, &[]))
    }

    /// <run> of <lane> as the page shows it; <queued> holds the pids waiting
    /// in the lane's queue.
    fn page_of(&self, run: &Run, lane: Lane, queued: &[i32]) -> RunPage {
        let record = &run.record;
        let (output, output_cut) = tail(&log_file(
            &self.dir,
            &record.repo,
            lane,
            record.shots.as_deref(),
        ));
        let ended = record.ended.as_ref();
        RunPage {
            lane: lane.word(),
            reference: record.reference.clone(),
            phase: run.phase(queued),
            pid: record.pid,
            started_at: record.started_at,
            ended_at: ended.map(|e| e.at),
            exit_code: ended.and_then(|e| e.code),
            signal: ended.and_then(|e| e.signal),
            output,
            output_cut,
        }
    }
}

impl Runs {
    /// Kills, by pid, each build this daemon started that has not ended. A
    /// test calls it as it ends, so a build waiting on a file in the test's
    /// scratch folder does not outlive the folder.
    #[cfg(test)]
    pub fn kill_started(&self) {
        let state = self.state.lock().expect("no panic holding it");
        for run in state.values() {
            if run.owned && run.record.ended.is_none() {
                unsafe { libc::kill(run.record.pid, libc::SIGKILL) };
            }
        }
    }
}

/// The end of the log at <path>, and whether more came before it. A log that
/// can't be read is said in its place.
fn tail(path: &Path) -> (String, bool) {
    crate::text::file_end(path, TAIL)
        .unwrap_or_else(|error| (format!("reading {}: {error}", path.display()), false))
}

#[cfg(test)]
#[path = "runs_tests.rs"]
mod tests;
