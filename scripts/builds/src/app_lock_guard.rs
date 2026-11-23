//! Keeps a checkout reserved while a foreground command uses it, even if
//! the driver exits. The guard holds the locks and watches the command's exit
//! in the kernel. The command and its descendants never receive the locks.

#[cfg(target_os = "macos")]
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

#[cfg(target_os = "macos")]
use crate::fail::Context;
use crate::fail::Result;

#[cfg(target_os = "macos")]
pub const COMMAND: &str = "__app-lock-guard";
#[cfg(target_os = "macos")]
const CHECKOUT_FD: &str = "BUILDS_CHECKOUT_GUARD_FD";

#[derive(Clone, Copy, Debug)]
pub struct CheckoutLock<'a> {
    file: BorrowedFd<'a>,
    ticket: Option<BorrowedFd<'a>>,
}

impl<'a> CheckoutLock<'a> {
    pub fn new(file: BorrowedFd<'a>, ticket: Option<BorrowedFd<'a>>) -> Self {
        Self { file, ticket }
    }

    /// Passes the descriptors to an explicit recipient, either the guard or
    /// the command that `builds lock` runs. Changes only the child's flags.
    pub fn pass_to(self, command: &mut Command) {
        let fds = [Some(self.file), self.ticket].map(|fd| fd.map(|fd| fd.as_raw_fd()));
        unsafe {
            command.pre_exec(move || {
                for fd in fds.into_iter().flatten() {
                    crate::flock::inherit(fd)?;
                }
                Ok(())
            });
        }
    }

    #[cfg(target_os = "macos")]
    pub fn run<T>(self, mut command: Command, run: impl FnOnce(Command) -> Result<T>) -> Result<T> {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        use std::process::Stdio;

        let (runner, guard) = UnixStream::pair()
            .context(|| "Builds could not open a socket for the app checkout lock guard".into())?;
        let mut watcher = Command::new(crate::this_program()?);
        #[cfg(not(test))]
        watcher.arg(COMMAND);
        #[cfg(test)]
        watcher.args([
            "--exact",
            "app_lock_guard::watch_in_test",
            "--ignored",
            "--quiet",
        ]);
        watcher
            .process_group(0)
            .env(CHECKOUT_FD, self.file.as_raw_fd().to_string())
            .stdin(Stdio::from(OwnedFd::from(guard)))
            .stdout(Stdio::null());
        self.pass_to(&mut watcher);
        let mut child = watcher
            .spawn()
            .context(|| "Builds could not start the app checkout lock guard".into())?;
        drop(watcher);

        let socket = runner.as_raw_fd();
        // The command cannot exec until the guard registers its PID. If the
        // driver dies before starting the command, the guard reads EOF.
        unsafe { command.pre_exec(move || register(socket)) };
        let result = run(command);
        drop(runner);
        let guarded = child
            .wait()
            .context(|| "Builds could not wait for the app checkout lock guard".into())
            .and_then(|status| {
                if !status.success() {
                    crate::fail!("The app checkout lock guard exited with {status}");
                }
                Ok(())
            });
        crate::fail::with_cleanup(result, guarded)
    }

    pub fn run_in_group(
        self,
        command: Command,
        name: &str,
        limit: Option<std::time::Duration>,
        keep: crate::lock::Keep<'_>,
    ) -> Result<crate::lock::Within> {
        self.run(command, |command| {
            crate::lock::run_in_group_keeping(command, name, limit, keep)
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn run<T>(self, _command: Command, _run: impl FnOnce(Command) -> Result<T>) -> Result<T> {
        crate::fail!("Builds guards app checkout commands on macOS")
    }
}

#[cfg(target_os = "macos")]
fn register(socket: std::os::fd::RawFd) -> io::Result<()> {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;

    // The driver owns the socket. Its copy in the child closes on exec.
    let stream = std::mem::ManuallyDrop::new(unsafe { UnixStream::from_raw_fd(socket) });
    let mut stream = &*stream;
    stream.write_all(&unsafe { libc::getpid() }.to_ne_bytes())?;
    stream.read_exact(&mut [0])
}

#[cfg(target_os = "macos")]
pub fn watch() -> Result<()> {
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    use std::os::unix::net::UnixStream;

    let checkout = std::env::var(CHECKOUT_FD)
        .context(|| "The app checkout lock guard needs its descriptor".into())?
        .parse::<RawFd>()
        .context(|| "The app checkout lock guard needs a descriptor number".into())?;
    if checkout <= libc::STDERR_FILENO || unsafe { libc::fcntl(checkout, libc::F_GETFD) } < 0 {
        crate::fail!("The app checkout lock guard needs an open checkout descriptor");
    }
    // Close the checkout before exiting, which closes the ticket and wakes
    // the queued builds. A waiter that gets the ticket must find the checkout free.
    let _checkout = unsafe { OwnedFd::from_raw_fd(checkout) };
    let mut socket = unsafe { UnixStream::from_raw_fd(libc::STDIN_FILENO) };
    let mut pid = [0; size_of::<libc::pid_t>()];
    let peer = || "Builds lost the socket for the app checkout lock guard".into();
    if socket.read(&mut pid[..1]).context(peer)? == 0 {
        return Ok(());
    }
    socket.read_exact(&mut pid[1..]).context(peer)?;
    let pid = libc::pid_t::from_ne_bytes(pid);
    let queue = crate::kqueue::Queue::new()
        .context(|| "Builds could not open the app checkout lock guard's event queue".into())?;
    queue
        .watch_exit(pid)
        .context(|| format!("Builds could not watch process {pid} exit"))?;
    socket.write_all(&[0]).context(peer)?;
    queue
        .wait(&mut [crate::kqueue::event(0, 0, 0, 0)], None)
        .context(|| format!("Builds could not wait for process {pid} to exit"))?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
#[test]
#[ignore = "The app checkout lock guard starts this entry point in its own process"]
fn watch_in_test() {
    watch().unwrap();
}
