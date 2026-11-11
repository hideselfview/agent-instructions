//! `builds lock` on a terminal: the build changes terminal modes, which stops
//! a background process group, so the build's group has to hold the terminal
//! while it runs. A pseudo-terminal is the build's stdin and controlling
//! terminal, with `builds` as its foreground process, as in a shell.

use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");
const PATIENCE: Duration = Duration::from_secs(20);

#[test]
fn a_build_that_sets_terminal_modes_finishes_when_builds_runs_on_a_terminal() {
    let dir = PathBuf::from(format!("/private/tmp/bt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume/demo")).unwrap();
    std::fs::write(dir.join("volume/demo/app-checkout"), "side-1\n").unwrap();

    let (mut master, mut slave) = (0, 0);
    assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) }, 0);

    let mut command = Command::new(BUILDS);
    command
        .env("BUILDS_ROOT", dir.join("volume"))
        .env("HOME", dir.join("home"))
        .args(["lock", "demo", "sh", "-c", "echo $$ > \"$0/pid\"; stty -echo; stty echo", dir.to_str().unwrap()])
        .stdin(unsafe { Stdio::from(std::os::fd::OwnedFd::from_raw_fd(libc::dup(slave))) })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            // Its own session, the terminal its controlling one: it is then
            // the terminal's foreground process, as a shell's job is.
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let finished = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() > deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if finished.is_none() {
        // Stopped processes ignore SIGTERM.
        unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
        let _ = child.wait();
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap_or_default();
        if let Ok(pid) = pid.trim().parse::<i32>() {
            unsafe { libc::killpg(pid, libc::SIGKILL) };
        }
    }
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let status = finished.expect("the build never finished: its group was stopped on the terminal");
    assert!(status.success(), "{status}");
}
