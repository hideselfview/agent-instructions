//! Install stops the old dashboard processes and starts the new ones with
//! their dashboard options. The install lock covers both steps and the launch
//! record, which lets an install retry a restart whose process is gone.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use serde::{Deserialize, Serialize};

use crate::places::{LISTS_VARIABLE, Places, ROOT_VARIABLE, build_root, lists_setting};
use crate::procs::{self, Process, Processes};
use crate::text::note;

const START_LIMIT: Duration = Duration::from_secs(30);
const STARTING: &str = "BUILDS_DASHBOARD_RESTART";
pub(super) const READY: u8 = 1;

/// A relative root keeps the directory it was relative to, so its socket
/// path retains the same length and spelling as before the restart.
#[derive(Deserialize, Serialize)]
enum LaunchRoot {
    Absolute(PathBuf),
    Relative { directory: PathBuf, path: PathBuf },
}

impl LaunchRoot {
    fn absolute_path(&self) -> PathBuf {
        match self {
            Self::Absolute(path) => path.clone(),
            Self::Relative { directory, path } => directory.join(path),
        }
    }

    fn apply(&self, command: &mut Command) {
        match self {
            Self::Absolute(path) => command.env(ROOT_VARIABLE, path),
            Self::Relative { directory, path } => {
                command.current_dir(directory).env(ROOT_VARIABLE, path)
            }
        };
    }
}

/// How to start a dashboard at the root that identifies it in the restart record.
#[derive(Deserialize, Serialize)]
struct Launch {
    root: LaunchRoot,
    lists: Option<PathBuf>,
    args: Vec<String>,
}

impl Launch {
    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        self.root.apply(&mut command);
        command.arg("dashboard").args(&self.args);
        if let Some(lists) = &self.lists {
            command.env(LISTS_VARIABLE, lists);
        } else {
            command.env_remove(LISTS_VARIABLE);
        }
        command
    }
}

fn read_launch(
    pid: i32,
    args: Vec<String>,
    read_environment: impl FnOnce(i32) -> Option<Vec<OsString>>,
    read_directory: impl FnOnce(i32) -> Option<PathBuf>,
) -> Result<Option<(PathBuf, Launch)>> {
    let Some(environment) = read_environment(pid) else {
        return unread_launch(pid);
    };
    let variable = |name| procs::environment_value(&environment, name);
    let root = build_root(variable(ROOT_VARIABLE))?;
    let lists = lists_setting(variable(LISTS_VARIABLE));
    let (root, lists) =
        if root.is_relative() || lists.as_ref().is_some_and(|path| path.is_relative()) {
            let Some(cwd) = read_directory(pid) else {
                return unread_launch(pid);
            };
            let lists = lists.map(|lists| cwd.join(lists));
            let root = if root.is_relative() {
                LaunchRoot::Relative {
                    directory: cwd,
                    path: root,
                }
            } else {
                LaunchRoot::Absolute(root)
            };
            (root, lists)
        } else {
            (LaunchRoot::Absolute(root), lists)
        };
    let path = root.absolute_path();
    let resolved = fs::canonicalize(&path).context(|| {
        format!(
            "Builds could not resolve the dashboard root at {}",
            path.display()
        )
    })?;
    Ok(Some((resolved, Launch { root, lists, args })))
}

fn unread_launch(pid: i32) -> Result<Option<(PathBuf, Launch)>> {
    if procs::ended(pid) {
        return Ok(None);
    }
    fail!("Builds could not read the launch settings of dashboard process {pid}.");
}

fn read_options(
    pid: i32,
    read_args: impl FnOnce(i32) -> std::result::Result<Option<Vec<String>>, String>,
) -> Result<Option<Vec<String>>> {
    let Some(argv) = read_args(pid).map_err(Fail::Message)? else {
        return Ok(None);
    };
    Ok(super::dashboard_args(argv.iter().map(String::as_str))
        .map(|args| args.into_iter().map(str::to_owned).collect()))
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
        if let Some(args) = read_options(process.pid, &read_args)? {
            dashboards.push((process.pid, args));
        }
    }
    Ok(dashboards)
}

/// Stops each old dashboard before starting any replacements from the binary
/// install just put on PATH. Called while install holds its lock.
pub fn run(places: &Places) -> Result<()> {
    let processes = Processes::read()?;
    let mut by_root = BTreeMap::<PathBuf, BTreeMap<i32, Launch>>::new();
    let places_at = |root: &Path| {
        let mut at = places.clone();
        at.root = root.to_path_buf();
        at
    };
    for (pid, args) in running_dashboards(&processes.list, places.uid, procs::live_args)? {
        let Some((root, launch)) = read_launch(pid, args, procs::environment, procs::cwd)? else {
            continue;
        };
        by_root.entry(root).or_default().insert(pid, launch);
    }
    let mut running = BTreeMap::new();
    let mut stop_pids = Vec::new();
    for (root, mut candidates) in by_root {
        let lock = places_at(&root).daemon_lock();
        let owner = super::live::daemon_pid(&lock).context(|| {
            format!(
                "Builds could not read the dashboard claim at {}",
                lock.display()
            )
        })?;
        let (pid, launch) =
            select_launch(&mut candidates, owner, |pid| read_owner_launch(pid, &root))?;
        stop_pids.push(pid);
        stop_pids.extend(candidates.into_keys());
        running.insert(root, launch);
    }
    let program = crate::links::path(&places.home, "builds");
    restart(
        &places.install_dir().join("dashboard-restart.json"),
        running,
        &stop_pids,
        |pid| {
            procs::terminate_and_wait(pid)
                .context(|| format!("Builds could not stop dashboard process {pid}"))
        },
        |root, launch| {
            start(launch.command(&program), &places_at(root).daemon_log()).map(|child| child.id())
        },
    )
}

/// Uses the claim holder's launch even when the process scan missed it.
fn select_launch(
    candidates: &mut BTreeMap<i32, Launch>,
    owner: Option<i32>,
    read_owner: impl FnOnce(i32) -> Result<Launch>,
) -> Result<(i32, Launch)> {
    let Some(pid) = owner else {
        return Ok(candidates
            .pop_first()
            .expect("the root has a starting dashboard"));
    };
    let launch = match candidates.remove(&pid) {
        Some(launch) => launch,
        None => read_owner(pid)?,
    };
    Ok((pid, launch))
}

/// The claim may belong to a dashboard that started after the process scan.
fn read_owner_launch(pid: i32, root: &Path) -> Result<Launch> {
    if let Some(args) = read_options(pid, procs::live_args)?
        && let Some((resolved, launch)) = read_launch(pid, args, procs::environment, procs::cwd)?
        && resolved == root
    {
        return Ok(launch);
    }
    fail!(
        "Builds could not read the launch of dashboard process {pid}, which claimed {}.",
        root.display()
    );
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
        let recorded = if pending.is_empty() {
            fs::remove_file(record)
                .context(|| format!("Builds could not remove {}", record.display()))
        } else {
            save(record, &pending)
        };
        if let Err(error) = recorded {
            failed.push(error.text());
            break;
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
    use std::ffi::OsStr;
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
    fn an_install_keeps_the_dashboard_launch_when_startup_fails() {
        let scratch = crate::testing::Scratch::new("dashboard-restart");
        let record = scratch.dir.join("restart.json");
        let log = scratch.dir.join("log");
        let program = scratch.dir.join("builds");
        let root = scratch.dir.join("dashboard volume");
        let alias = scratch.dir.join("volume-link");
        let lists = scratch.dir.join("dashboard lists");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        fs::write(
            &program,
            "#!/bin/sh\necho 'Port already in use' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let mut previous = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let args = vec!["--port".to_string(), "7788".to_string()];
        let (resolved, launch) = read_launch(
            previous.id() as i32,
            args.clone(),
            |_| {
                Some(vec![
                    format!("{ROOT_VARIABLE}={}", alias.display()).into(),
                    format!("{LISTS_VARIABLE}={}", lists.display()).into(),
                ])
            },
            |_| panic!("absolute paths do not need the dashboard's working directory"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved, root);
        let error = restart(
            &record,
            BTreeMap::from([(resolved, launch)]),
            &[previous.id() as i32],
            |pid| {
                procs::terminate_and_wait(pid)
                    .context(|| "stopping the old test process".to_string())
            },
            |_, launch| start(launch.command(&program), &log).map(|child| child.id()),
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

        restart(
            &record,
            BTreeMap::new(),
            &[],
            |pid| panic!("unexpected dashboard process {pid}"),
            |location, launch| {
                assert_eq!(location, root);
                assert_eq!(launch.args, args);
                let command = launch.command(&program);
                assert_eq!(command.get_current_dir(), None);
                let environment: BTreeMap<_, _> = command.get_envs().collect();
                assert_eq!(
                    environment[OsStr::new(ROOT_VARIABLE)],
                    Some(alias.as_os_str())
                );
                assert_eq!(
                    environment[OsStr::new(LISTS_VARIABLE)],
                    Some(lists.as_os_str())
                );
                Ok(12)
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
            root: LaunchRoot::Absolute(root.clone()),
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

    #[test]
    fn a_claim_holder_supplies_the_launch_when_the_scan_missed_it() {
        let launch = |port: &str| Launch {
            root: LaunchRoot::Absolute(PathBuf::from("/test/volume")),
            lists: None,
            args: vec!["--port".to_string(), port.to_string()],
        };
        let mut candidates = BTreeMap::from([(11, launch("7787"))]);
        let (pid, actual) = select_launch(&mut candidates, Some(12), |pid| {
            assert_eq!(pid, 12);
            Ok(launch("7788"))
        })
        .unwrap();
        assert_eq!(pid, 12);
        assert_eq!(actual.args, ["--port", "7788"]);
        assert_eq!(candidates.into_keys().collect::<Vec<_>>(), [11]);
    }
}
