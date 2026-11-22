//! Install stops the old dashboard processes and starts the new ones with
//! their dashboard options. The install lock covers both steps and the launch
//! record, which lets an install retry a restart whose process is gone.

use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;
use crate::procs::{self, Process, Processes};
use crate::text::note;

const STOP_LIMIT: Duration = Duration::from_secs(10);
const START_LIMIT: Duration = Duration::from_secs(30);
const STARTING: &str = "BUILDS_DASHBOARD_RESTART";
pub(super) const READY: u8 = 1;

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
    let dashboards = running_dashboards(&processes.list, places.uid, procs::live_args)?;
    let program = crate::links::path(&places.home, "builds");
    let log = places.daemon_log();
    restart(
        &places.install_dir().join("dashboard-restart.json"),
        &dashboards,
        |pid| stop(pid).context(|| format!("Builds could not stop dashboard process {pid}")),
        |args| start(&program, args, &log, START_LIMIT).map(|child| child.id()),
    )
}

/// Saves the options before stopping any dashboard. An install that fails
/// keeps the launches that have yet to report readiness for the next attempt.
fn restart(
    record: &Path,
    dashboards: &[(i32, Vec<String>)],
    stop: impl Fn(i32) -> Result<()>,
    start: impl Fn(&[String]) -> Result<u32>,
) -> Result<()> {
    let mut pending: Vec<Vec<String>> = match fs::read(record) {
        Ok(bytes) => serde_json::from_slice(&bytes).context(|| {
            format!(
                "Builds could not read the dashboard launches in {}",
                record.display()
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => fail!("Builds could not read {}. {error}.", record.display()),
    };
    for (_, args) in dashboards {
        if !pending.contains(args) {
            pending.push(args.clone());
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    save(record, &pending)?;
    for (pid, _) in dashboards {
        stop(*pid)?;
    }
    for (index, args) in pending.iter().enumerate() {
        let pid = start(args)?;
        let remaining = &pending[index + 1..];
        if remaining.is_empty() {
            fs::remove_file(record)
                .context(|| format!("Builds could not remove {}", record.display()))?;
        } else {
            save(record, remaining)?;
        }
        note(format!("Builds restarted the dashboard as process {pid}."));
    }
    Ok(())
}

fn save(record: &Path, pending: &[Vec<String>]) -> Result<()> {
    let bytes = serde_json::to_vec(pending).context(|| {
        format!(
            "Builds could not record the dashboard launches in {}",
            record.display()
        )
    })?;
    crate::whole_file::write(record, bytes)
}

/// Registers for process exit before signaling, so a fast exit still wakes
/// the installer. The event also arrives for a dashboard we did not spawn.
fn stop(pid: i32) -> io::Result<()> {
    // SAFETY: kqueue returns a new descriptor on success.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return Err(io::Error::last_os_error());
    }
    let queue = unsafe { OwnedFd::from_raw_fd(queue) };
    let mut event = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: the call reads one change and writes no events.
    let registered = unsafe {
        libc::kevent(
            queue.as_raw_fd(),
            &event,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    if registered < 0 {
        return if_gone(io::Error::last_os_error());
    }
    // SAFETY: pid is the process the exit watch is registered for.
    if unsafe { libc::kill(pid, libc::SIGTERM) } < 0 {
        return if_gone(io::Error::last_os_error());
    }
    let deadline = Instant::now() + STOP_LIMIT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = libc::timespec {
            tv_sec: remaining.as_secs() as libc::time_t,
            tv_nsec: remaining.subsec_nanos().into(),
        };
        // SAFETY: the call reads no changes and writes at most one event.
        let count = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut event,
                1,
                &timeout,
            )
        };
        match count {
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The dashboard did not exit within 10 seconds.",
                ));
            }
            _ => return Ok(()),
        }
    }
}

fn if_gone(error: io::Error) -> io::Result<()> {
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn start(program: &Path, args: &[String], log: &Path, timeout: Duration) -> Result<Child> {
    let dir = log.parent().expect("the dashboard log has a folder");
    fs::create_dir_all(dir).context(|| format!("Builds could not create {}", dir.display()))?;
    let log_file = super::open_log(log)?;
    let (mut ready, report) = UnixStream::pair()
        .context(|| "Builds could not open a channel to the dashboard".to_string())?;
    ready
        .set_read_timeout(Some(timeout))
        .context(|| "Builds could not set a timeout for dashboard startup".to_string())?;
    let mut child = Command::new(program)
        .arg("dashboard")
        .args(args)
        .env(STARTING, "1")
        .stdin(Stdio::from(OwnedFd::from(report)))
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .process_group(0)
        .spawn()
        .context(|| {
            format!(
                "Builds could not start the dashboard with {}",
                program.display()
            )
        })?;
    let mut message = [0];
    let started = ready
        .read_exact(&mut message)
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
        fs::write(
            &program,
            "#!/bin/sh\necho 'Port already in use' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let mut previous = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let args = vec!["--port".to_string(), "7788".to_string()];
        let error = restart(
            &record,
            &[(previous.id() as i32, args.clone())],
            |pid| stop(pid).context(|| "stopping the old test process".to_string()),
            |args| start(&program, args, &log, START_LIMIT).map(|child| child.id()),
        )
        .unwrap_err()
        .text();
        assert_eq!(previous.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert!(
            error.contains("did not report that it was ready"),
            "{error}"
        );
        assert!(error.contains(log.to_str().unwrap()), "{error}");
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("Port already in use")
        );

        fs::write(
            &program,
            "#!/bin/sh\ntest \"$*\" = 'dashboard --port 7788' || exit 1\nprintf '\\001' >&0\n",
        )
        .unwrap();
        restart(
            &record,
            &[],
            |pid| panic!("unexpected dashboard process {pid}"),
            |used| {
                assert_eq!(used, args);
                let mut child = start(&program, used, &log, START_LIMIT)?;
                assert!(child.wait().unwrap().success());
                Ok(child.id())
            },
        )
        .unwrap();
        assert!(!record.exists());
    }
}
