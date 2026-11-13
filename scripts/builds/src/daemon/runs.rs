//! The runs of `builds build-app` the page starts: one `builds build-app
//! <repo> <lane> [<ref>]` child per click, run exactly as the person would type it, so the
//! lane's queue and app lock order it like any other build.
//!
//! Each repo's lane keeps its latest run, in memory and in two files under
//! `<daemon dir>/runs/`: `<repo>.<lane>.json` (who started it, which process,
//! how it ended) and `<repo>.<lane>.log` (everything the build printed). The
//! child writes the log itself, not through the daemon, so a daemon that goes
//! away neither breaks the build's output nor stops it: the build runs on and
//! installs its app.
//!
//! A daemon that starts finds the record of a run it did not see end. What
//! the record says then is read from the process, never written: the build is
//! still running (`Detached`: its output is still read, how it ends can't be
//! seen) or is gone without an exit status anyone saw (`Lost`). A lane with a
//! `Detached` run takes no second start, as with a run the daemon holds.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
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

/// The most of a run's output the page gets: its end.
const TAIL: u64 = 32 * 1024;

/// How a run ended: its exit code, or the signal that stopped it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Ended {
    at: i64,
    code: Option<i32>,
    signal: Option<i32>,
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
}

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
    /// Started before the daemon did, and still running.
    Detached,
    /// Started before the daemon did, and gone with no exit status seen.
    Lost,
}

#[derive(Serialize)]
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
    state: Mutex<HashMap<(String, Lane), Run>>,
    listener: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    stream: Mutex<Option<Stream>>,
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

/// Is <pid> a running `build-app`? A process alive under that pid could be
/// another that took the number.
fn is_build_app(pid: i32) -> bool {
    alive(pid) && crate::procs::args(pid).is_some_and(|args| args.iter().any(|a| a == "build-app"))
}

impl Run {
    fn busy(&self) -> bool {
        self.record.ended.is_none() && (self.owned || is_build_app(self.record.pid))
    }

    fn phase(&self, queued: &[i32]) -> Phase {
        match (&self.record.ended, self.owned) {
            (Some(ended), _) if ended.code == Some(0) => Phase::Succeeded,
            (Some(_), _) => Phase::Failed,
            (None, true) if queued.contains(&self.record.pid) => Phase::Queued,
            (None, true) => Phase::Running,
            (None, false) if is_build_app(self.record.pid) => Phase::Detached,
            (None, false) => Phase::Lost,
        }
    }
}

fn record_file(dir: &Path, repo: &str, lane: Lane) -> PathBuf {
    dir.join(format!("{repo}.{}.json", lane.word()))
}

fn log_file(dir: &Path, repo: &str, lane: Lane) -> PathBuf {
    dir.join(format!("{repo}.{}.log", lane.word()))
}

/// Writes the record whole or not at all.
fn write_record(dir: &Path, record: &Record, lane: Lane) -> Result<()> {
    let file = record_file(dir, &record.repo, lane);
    let staged = dir.join(format!(".{}.{}.tmp", record.repo, lane.word()));
    std::fs::write(&staged, serde_json::to_vec(record).expect("a record serializes"))
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
        let mut state = HashMap::new();
        for entry in std::fs::read_dir(&dir).context(|| format!("listing {}", dir.display()))? {
            let path = entry.context(|| format!("listing {}", dir.display()))?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let text = std::fs::read(&path).context(|| format!("reading {}", path.display()))?;
            let record: Record = serde_json::from_slice(&text).context(|| format!("{} is not a run record", path.display()))?;
            let lane = Lane::parse_or_fail(&record.lane)?;
            state.insert((record.repo.clone(), lane), Run { record, owned: false });
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
        let started = Stream::start(
            &[dir.as_path()],
            &[],
            0.2,
            Box::new(move |_| {
                if let Some(runs) = weak.upgrade() {
                    runs.notify();
                }
            }),
        );
        match started {
            Some(started) => *runs.stream.lock().expect("no panic holding it") = Some(started),
            None => eprintln!("builds: FSEvents would not follow {}", dir.display()),
        }
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
    pub fn start_build(self: &Arc<Self>, repo: &str, lane: &str, reference: Option<&str>) -> std::result::Result<(), StartError> {
        let Some(lane) = Lane::parse(lane) else {
            return Err(StartError::Invalid(format!("{lane:?} is not a lane (mac | android)")));
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
            Ok(_) => return Err(StartError::Invalid(format!("{repo} has no {} app checkout", lane.word()))),
            Err(error) => return Err(StartError::Failed(error.text())),
        }
        let mut state = self.state.lock().expect("no panic holding it");
        if let Some(run) = state.get(&(repo.to_string(), lane))
            && run.busy()
        {
            return Err(StartError::Busy(format!("a {} build of {repo} is already queued or running", lane.word())));
        }
        let run = self.spawn(repo, lane, reference).map_err(|error| StartError::Failed(error.text()))?;
        state.insert((repo.to_string(), lane), run);
        drop(state);
        self.notify();
        Ok(())
    }

    fn spawn(self: &Arc<Self>, repo: &str, lane: Lane, reference: Option<&str>) -> Result<Run> {
        let log_path = log_file(&self.dir, repo, lane);
        let log = std::fs::File::create(&log_path).context(|| format!("creating {}", log_path.display()))?;
        let errors = log.try_clone().context(|| format!("opening {}", log_path.display()))?;
        let mut command = Command::new(&self.program[0]);
        command.args(&self.program[1..]).arg("build-app").arg(repo).arg(lane.word());
        if let Some(reference) = reference {
            command.arg(reference);
        }
        command.stdin(Stdio::null()).stdout(log).stderr(errors);
        let mut child = command.spawn().context(|| format!("starting {}", self.program[0]))?;
        let record = Record {
            repo: repo.to_string(),
            lane: lane.word().to_string(),
            reference: reference.map(str::to_string),
            pid: child.id() as i32,
            started_at: now_seconds(),
            ended: None,
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
        Ok(Run { record, owned: true })
    }

    /// The child of <started> exited, or could not be waited for.
    fn ended(&self, started: &Record, lane: Lane, status: std::io::Result<ExitStatus>) {
        let ended = match status {
            Ok(status) => Ended { at: now_seconds(), code: status.code(), signal: status.signal() },
            Err(error) => {
                eprintln!("builds: waiting for {} {}'s build: {error}", started.repo, started.lane);
                Ended { at: now_seconds(), code: None, signal: None }
            }
        };
        {
            let mut state = self.state.lock().expect("no panic holding it");
            if let Some(run) = state.get_mut(&(started.repo.clone(), lane))
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

    /// The runs of <repo>, one per lane that has one. <queued> gives the pids
    /// waiting in a lane's queue.
    pub fn pages(&self, repo: &str, queued: &dyn Fn(Lane) -> Vec<i32>) -> Vec<RunPage> {
        let state = self.state.lock().expect("no panic holding it");
        let mut pages = Vec::new();
        for lane in Lane::ALL {
            let Some(run) = state.get(&(repo.to_string(), lane)) else { continue };
            let (output, output_cut) = tail(&log_file(&self.dir, repo, lane));
            let ended = run.record.ended.as_ref();
            pages.push(RunPage {
                lane: lane.word(),
                reference: run.record.reference.clone(),
                phase: run.phase(&queued(lane)),
                pid: run.record.pid,
                started_at: run.record.started_at,
                ended_at: ended.map(|e| e.at),
                exit_code: ended.and_then(|e| e.code),
                signal: ended.and_then(|e| e.signal),
                output,
                output_cut,
            });
        }
        pages
    }
}

/// The end of the log at <path>, and whether more came before it. A log that
/// can't be read is said in its place.
fn tail(path: &Path) -> (String, bool) {
    let read = (|| -> std::io::Result<(Vec<u8>, bool)> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        let cut = length > TAIL;
        if cut {
            file.seek(SeekFrom::Start(length - TAIL))?;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok((bytes, cut))
    })();
    match read {
        Ok((bytes, cut)) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            // A cut inside a line leaves its end: drop it.
            let text = match (cut, text.split_once('\n')) {
                (true, Some((_, rest))) => rest.to_string(),
                _ => text,
            };
            (text, cut)
        }
        Err(error) => (format!("reading {}: {error}", path.display()), false),
    }
}

#[cfg(test)]
#[path = "runs_tests.rs"]
mod tests;
