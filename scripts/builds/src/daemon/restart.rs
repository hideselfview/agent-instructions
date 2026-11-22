//! Install stops the old dashboard processes and starts the new ones with
//! their dashboard options. The install lock covers both steps and the launch
//! record, which lets an install retry a restart whose process is gone.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use serde::{Deserialize, Serialize};

use crate::places::{DEFAULT_ROOT, LISTS_VARIABLE, Places, ROOT_VARIABLE};
use crate::procs::{self, Process, Processes};
use crate::text::note;

const STOP_LIMIT: Duration = Duration::from_secs(10);
const START_LIMIT: Duration = Duration::from_secs(30);
const STARTING: &str = "BUILDS_DASHBOARD_RESTART";
pub(super) const READY: u8 = 1;

/// How to start a dashboard at the root that identifies it in the restart record.
#[derive(Deserialize, Serialize)]
struct Launch {
    cwd: PathBuf,
    lists: Option<String>,
    args: Vec<String>,
}

impl Launch {
    fn command(&self, program: &Path, root: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.cwd)
            .env(ROOT_VARIABLE, root)
            .arg("dashboard")
            .args(&self.args);
        if let Some(lists) = &self.lists {
            command.env(LISTS_VARIABLE, lists);
        } else {
            command.env_remove(LISTS_VARIABLE);
        }
        command
    }
}

fn read_launch(pid: i32, args: Vec<String>) -> Result<Option<(PathBuf, Launch)>> {
    let (Some(environment), Some(cwd)) = (procs::environment(pid), procs::cwd(pid)) else {
        if procs::ended(pid) {
            return Ok(None);
        }
        fail!("Builds could not read the launch settings of dashboard process {pid}.");
    };
    let variable = |name| {
        environment.iter().find_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            (key == name).then_some(value)
        })
    };
    let root = cwd.join(variable(ROOT_VARIABLE).unwrap_or(DEFAULT_ROOT));
    let root = fs::canonicalize(&root).context(|| {
        format!(
            "Builds could not resolve the dashboard root at {}",
            root.display()
        )
    })?;
    let launch = Launch {
        cwd,
        lists: variable(LISTS_VARIABLE).map(str::to_owned),
        args,
    };
    Ok(Some((root, launch)))
}

/// Pids and dashboard options for this user's dashboards, including one
/// that has begun startup and has yet to claim its socket.
fn running_dashboards(
    processes: &[Process],
    uid: u32,
    read_args: impl Fn(i32) -> std::result::Result<Option<Vec<String>>, String>,
) -> Result<Vec<(i32, Vec<String>)>> {
    let mut dashboards = Vec::new();
    for process in processes
        .iter()
        .filter(|p| p.uid == uid && p.name == "builds")
    {
        let Some(argv) = read_args(process.pid).map_err(Fail::Message)? else {
            continue;
        };
        if let Some(args) = super::dashboard_args(argv.iter().map(String::as_str)) {
            dashboards.push((process.pid, args.into_iter().map(str::to_owned).collect()));
        }
    }
    Ok(dashboards)
}

/// Stops each old dashboard before starting any replacements from the binary
/// install just put on PATH. Called while install holds its lock.
pub fn run(places: &Places) -> Result<()> {
    let processes = Processes::read()?;
    let mut running = BTreeMap::new();
    let mut stop_pids = Vec::new();
    let places_at = |root: &Path| {
        let mut at = places.clone();
        at.root = root.to_path_buf();
        at
    };
    for (pid, args) in running_dashboards(&processes.list, places.uid, procs::live_args)? {
        let Some((root, launch)) = read_launch(pid, args)? else {
            continue;
        };
        stop_pids.push(pid);
        if running.contains_key(&root) {
            // A competing startup does not replace the options of the dashboard
            // that already claimed this root.
            let lock = places_at(&root).daemon_lock();
            let owner = super::live::daemon_pid(&lock).context(|| {
                format!(
                    "Builds could not read the dashboard claim at {}",
                    lock.display()
                )
            })?;
            if owner != Some(pid) {
                continue;
            }
        }
        running.insert(root, launch);
    }
    let program = crate::links::path(&places.home, "builds");
    restart(
        &places.install_dir().join("dashboard-restart.json"),
        running,
        &stop_pids,
        |pid| {
            procs::terminate_and_wait(pid, Instant::now() + STOP_LIMIT)
                .context(|| format!("Builds could not stop dashboard process {pid}"))
        },
        |root, launch| {
            start(
                launch.command(&program, root),
                &places_at(root).daemon_log(),
            )
            .map(|child| child.id())
        },
    )
}

/// Saves the options before stopping any dashboard. An install that fails
/// keeps the launches that have yet to report readiness for the next attempt.
fn restart(
    record: &Path,
    running: BTreeMap<PathBuf, Launch>,
    stop_pids: &[i32],
    stop: impl Fn(i32) -> Result<()>,
    start: impl Fn(&Path, &Launch) -> Result<u32>,
) -> Result<()> {
    let mut pending: BTreeMap<PathBuf, Launch> = crate::readings_file::read_value(record)?;
    pending.extend(running);
    if pending.is_empty() {
        return Ok(());
    }
    save(record, &pending)?;
    for pid in stop_pids {
        stop(*pid)?;
    }
    let roots: Vec<_> = pending.keys().cloned().collect();
    let mut failed = Vec::new();
    for root in roots {
        let pid = match start(&root, &pending[&root]) {
            Ok(pid) => pid,
            Err(error) => {
                failed.push(format!("{}: {}", root.display(), error.text()));
                continue;
            }
        };
        pending.remove(&root);
        if pending.is_empty() {
            fs::remove_file(record)
                .context(|| format!("Builds could not remove {}", record.display()))?;
        } else {
            save(record, &pending)?;
        }
        note(format!("Builds restarted the dashboard as process {pid}."));
    }
    if !failed.is_empty() {
        fail!(
            "Builds could not restart these dashboards. {}",
            failed.join("\n")
        );
    }
    Ok(())
}

fn save(record: &Path, pending: &BTreeMap<PathBuf, Launch>) -> Result<()> {
    let bytes = serde_json::to_vec(pending).context(|| {
        format!(
            "Builds could not record the dashboard launches in {}",
            record.display()
        )
    })?;
    crate::whole_file::write(record, bytes)
}

fn start(mut command: Command, log: &Path) -> Result<Child> {
    let dir = log.parent().expect("the dashboard log has a folder");
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => fail!("Builds could not create {}. {error}.", dir.display()),
    }
    let log_file = super::open_log(log)?;
    let (mut ready, report) = UnixStream::pair()
        .context(|| "Builds could not open a channel to the dashboard".to_string())?;
    ready
        .set_read_timeout(Some(START_LIMIT))
        .context(|| "Builds could not set a timeout for dashboard startup".to_string())?;
    let mut child = command
        .env(STARTING, "1")
        .stdin(Stdio::from(OwnedFd::from(report)))
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .process_group(0)
        .spawn()
        .context(|| {
            format!(
                "Builds could not start the dashboard with {}",
                command.get_program().to_string_lossy()
            )
        })?;
    // Command keeps the supplied stdin descriptor for another spawn until it
    // drops. Only the child may keep that end while install waits for readiness.
    drop(command);
    let mut message = [0];
    let started = ready
        .read_exact(&mut message)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(error.kind(), "The dashboard closed its startup connection.")
            } else {
                error
            }
        })
        .context(|| {
            format!(
                "Dashboard process {} did not report that it was ready. See {}",
                child.id(),
                log.display()
            )
        })
        .and_then(|()| {
            if message != [READY] {
                fail!(
                    "Dashboard process {} sent an invalid startup message. See {}.",
                    child.id(),
                    log.display()
                );
            }
            Ok(())
        });
    match started {
        Ok(()) => Ok(child),
        Err(error) => {
            let stopped = child
                .kill()
                .and_then(|()| child.wait().map(|_| ()))
                .context(|| {
                    format!(
                        "Builds could not stop failed dashboard process {}",
                        child.id()
                    )
                });
            fail::with_cleanup(Err(error), stopped)
        }
    }
}

/// Takes the installer's startup connection from stdin and restores the
/// daemon's usual stdin before it can start any child processes.
pub(super) fn report_to_installer() -> Result<Option<File>> {
    if std::env::var_os(STARTING).is_none() {
        return Ok(None);
    }
    let stdin = io::stdin();
    let report = stdin
        .as_fd()
        .try_clone_to_owned()
        .context(|| "Builds could not take the dashboard startup connection".to_string())?;
    let null = File::open("/dev/null").context(|| "Builds could not open /dev/null".to_string())?;
    // SAFETY: both descriptors are live. dup2 gives stdin its own descriptor.
    if unsafe { libc::dup2(null.as_raw_fd(), stdin.as_raw_fd()) } < 0 {
        fail!(
            "Builds could not replace the dashboard's stdin. {}.",
            io::Error::last_os_error()
        );
    }
    Ok(Some(File::from(report)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn install_finds_a_dashboard_started_with_an_absolute_executable_path() {
        let process = |pid, uid, name: &str| Process {
            pid,
            ppid: 1,
            uid,
            name: name.to_string(),
            started: 100,
        };
        let processes = [
            process(11, 501, "builds"),
            process(12, 501, "builds"),
            process(13, 501, "builds"),
            process(14, 502, "builds"),
            process(15, 501, "bash"),
        ];
        let dashboards = running_dashboards(&processes, 501, |pid| {
            let args = match pid {
                11 => vec![
                    "/Users/test user/.local/bin/builds",
                    "dashboard",
                    "--listen",
                    "192.168.1.166",
                ],
                12 => vec![
                    "builds",
                    "--agent",
                    "local-agent",
                    "dashboard",
                    "--port",
                    "7788",
                ],
                13 => vec!["builds", "help", "dashboard"],
                _ => panic!("install should not read process {pid}"),
            };
            Ok(Some(args.into_iter().map(String::from).collect()))
        })
        .unwrap();
        assert_eq!(
            dashboards,
            [
                (11, vec!["--listen".into(), "192.168.1.166".into()]),
                (12, vec!["--port".into(), "7788".into()]),
            ]
        );
    }

    #[test]
    fn an_install_retries_a_dashboard_that_failed_before_readiness() {
        let scratch = crate::testing::Scratch::new("dashboard-restart");
        let record = scratch.dir.join("restart.json");
        let log = scratch.dir.join("log");
        let program = scratch.dir.join("builds");
        let root = scratch.dir.join("dashboard volume");
        fs::write(
            &program,
            "#!/bin/sh\necho 'Port already in use' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let mut previous = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let args = vec!["--port".to_string(), "7788".to_string()];
        let launch = Launch {
            cwd: scratch.dir.clone(),
            lists: Some("dashboard lists".to_string()),
            args: args.clone(),
        };
        let error = restart(
            &record,
            BTreeMap::from([(root.clone(), launch)]),
            &[previous.id() as i32],
            |pid| {
                procs::terminate_and_wait(pid, Instant::now() + STOP_LIMIT)
                    .context(|| "stopping the old test process".to_string())
            },
            |root, launch| start(launch.command(&program, root), &log).map(|child| child.id()),
        )
        .unwrap_err()
        .text();
        assert_eq!(previous.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert!(
            error.contains("did not report that it was ready"),
            "{error}"
        );
        assert!(error.contains("closed its startup connection"), "{error}");
        assert!(error.contains(log.to_str().unwrap()), "{error}");
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("Port already in use")
        );

        fs::write(
            &program,
            "#!/bin/sh\ntest \"$*\" = 'dashboard --port 7788' || exit 1\n\
             test \"$BUILDS_ROOT\" = \"$PWD/dashboard volume\" || exit 1\n\
             test \"$BUILDS_LISTS\" = 'dashboard lists' || exit 1\n\
             printf '\\001' >&0\n",
        )
        .unwrap();
        restart(
            &record,
            BTreeMap::new(),
            &[],
            |pid| panic!("unexpected dashboard process {pid}"),
            |location, launch| {
                assert_eq!(location, root);
                assert_eq!(launch.args, args);
                let mut child = start(launch.command(&program, location), &log)?;
                assert!(child.wait().unwrap().success());
                Ok(child.id())
            },
        )
        .unwrap();
        assert!(!record.exists());
    }

    #[test]
    fn a_running_dashboard_replaces_the_saved_launch_for_its_root() {
        let scratch = crate::testing::Scratch::new("dashboard-running-options");
        let record = scratch.dir.join("restart.json");
        let root = scratch.dir.join("volume");
        let launch = |port: &str| Launch {
            cwd: scratch.dir.clone(),
            lists: None,
            args: vec!["--port".to_string(), port.to_string()],
        };
        save(&record, &BTreeMap::from([(root.clone(), launch("7787"))])).unwrap();
        restart(
            &record,
            BTreeMap::from([(root.clone(), launch("7788"))]),
            &[11],
            |pid| {
                assert_eq!(pid, 11);
                Ok(())
            },
            |location, actual| {
                assert_eq!(location, root);
                assert_eq!(actual.args, ["--port", "7788"]);
                Ok(12)
            },
        )
        .unwrap();
        assert!(!record.exists());
    }
}
