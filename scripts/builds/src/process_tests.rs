use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::Stdio;

use super::*;
use crate::testing::Scratch;

fn sh(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

/// A detached process leads a session of its own, and its record names it,
/// what it is to do and the process that started it, until it ends.
#[test]
fn a_detached_process_leads_a_session_and_is_recorded() {
    let scratch = Scratch::new("process-record");
    let volume = scratch.places().root;
    let mut command = sh("read line");
    command.stdin(Stdio::piped());
    let mut child = Detached::start(&volume, &mut command, "wait for a line").unwrap();
    let pid = child.id() as i32;
    // SAFETY: getsid(2) touches no memory.
    assert_eq!(unsafe { libc::getsid(pid) }, pid);
    let found = records(&volume).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, pid);
    assert_eq!(found[0].purpose, "wait for a line");
    assert_eq!(found[0].caller.pid, std::process::id() as i32);
    assert!(volume.join(FOLDER).join(found[0].file_name()).is_file());
    let processes = Processes::read().unwrap();
    assert_eq!(running(&volume, &processes).unwrap(), BTreeSet::from([pid]));
    drop(child.stdin.take());
    child.wait().unwrap();
    let processes = Processes::read().unwrap();
    assert_eq!(running(&volume, &processes).unwrap(), BTreeSet::new());
}

/// A process that took the pid once the recorded one had ended started after
/// the record's time, so the record does not name it.
#[test]
fn a_record_names_no_later_process_with_its_pid() {
    let record = Detached {
        pid: 40,
        started: 1_000,
        purpose: "wait".to_string(),
        caller: Caller {
            pid: 39,
            command: "builds cargo test".to_string(),
        },
    };
    assert!(record.names(40, 999));
    assert!(record.names(40, 1_001));
    assert!(!record.names(40, 1_002));
    assert!(!record.names(41, 999));
}

/// A start whose record cannot be written fails, and what it started is
/// killed.
#[test]
fn a_start_that_cannot_be_recorded_kills_what_it_started() {
    let scratch = Scratch::new("process-unrecorded");
    let volume = scratch.places().root;
    let folder = volume.join(FOLDER);
    std::fs::create_dir(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (mut reader, writer) = std::io::pipe().unwrap();
    let mut command = sh("sleep 600");
    command.stdout(writer);
    let started = Detached::start(&volume, &mut command, "sleep");
    drop(command);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(started.is_err());
    // The pipe ends once every process that holds its write end has.
    reader.read_to_end(&mut Vec::new()).unwrap();
}

/// A missing build volume fails the start, and builds makes no folder in its
/// place.
#[test]
fn a_start_under_a_missing_volume_fails() {
    let scratch = Scratch::new("process-no-volume");
    let volume = scratch.dir.join("not-mounted");
    assert!(Detached::start(&volume, &mut sh("true"), "end").is_err());
    assert!(!volume.exists());
}

/// A shell in a process group of its own that runs <script>, once it has
/// said `ready`.
fn ready_group(script: &str) -> (Child, BufReader<std::process::ChildStdout>) {
    let mut child = sh(script)
        .process_group(0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut said = String::new();
    out.read_line(&mut said).unwrap();
    assert_eq!(said, "ready\n");
    (child, out)
}

/// A stop sends the group SIGTERM, which what handles it ends on, saying what
/// it likes on the way.
#[test]
fn a_stopped_group_gets_sigterm_first() {
    // The child says `ready` once it runs a program of its own, which the
    // SIGTERM ends.
    let (child, mut out) =
        ready_group("trap 'echo stopped; exit 0' TERM; sh -c 'echo ready; exec sleep 600' & wait");
    let group = child.id() as i32;
    let reaper = std::thread::spawn(move || {
        let mut child = child;
        child.wait().unwrap()
    });
    stop_group(group, Duration::from_secs(60)).unwrap();
    assert!(reaper.join().unwrap().success());
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "stopped\n");
}

/// What ignores SIGTERM gets SIGKILL once the grace has passed.
#[test]
fn a_group_that_ignores_sigterm_is_killed_after_the_grace() {
    use std::os::unix::process::ExitStatusExt;
    let (mut child, mut out) = ready_group("trap '' TERM; echo ready; sleep 600");
    stop_group(child.id() as i32, Duration::from_millis(200)).unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    // Its `sleep` held the pipe too.
    out.read_to_end(&mut Vec::new()).unwrap();
}

/// A group with no process left is stopped, whether or not its last one has
/// been reaped.
#[cfg(target_os = "macos")]
#[test]
fn a_group_that_is_gone_is_stopped() {
    let mut child = sh("exit 0").process_group(0).spawn().unwrap();
    let group = child.id() as i32;
    // A zombie until the wait below.
    crate::procs::exit_after(group, || Ok(())).unwrap();
    stop_group(group, Duration::from_secs(60)).unwrap();
    child.wait().unwrap();
    stop_group(group, Duration::from_secs(60)).unwrap();
}
