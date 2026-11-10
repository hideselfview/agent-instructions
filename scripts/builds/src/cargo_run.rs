//! `builds cargo`: recording the build copies cargo used, so setup and prune
//! keep those while they delete the ones no build used for days, holding the
//! checkout's guard while the build runs, and pruning the copies it replaced
//! once it ends.

use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;
use crate::units::GUARD;
use crate::units::record::record;

/// Does <checkout> build with incremental compilation? Only main;
/// `role_config` says why.
pub fn builds_incrementally(checkout: &Checkout) -> bool {
    checkout.is_main()
}

/// The cargo settings a build in <checkout> takes from the checkout's role,
/// as `--config` arguments.
///
/// Every side worktree builds without incremental compilation. sccache
/// refuses every incremental compile, so with it off sccache keeps each
/// workspace crate any worktree built, and a build of the same sources in
/// another worktree, or in the app checkout, where agents take turns on
/// their own branches, comes from the cache; and no incremental folders
/// pile up (on 2026-11-10, deleting those nothing had written for ten
/// minutes in five busy bae worktrees freed 38 GB).
/// Main, the person's checkout, keeps incremental, which halves an
/// edit-and-recheck loop's CPU in a checkout that stays on one branch.
///
/// A `--config` argument, not a `CARGO_*` variable: cargo hands its
/// environment on to rustc and sccache keys every `CARGO_*` variable, so
/// with a variable every dependency built through this command would get
/// other cache entries than the same dependency built by cargo run by hand
/// in the same checkout. The argument changes only what it sets.
fn role_config(checkout: Option<&Checkout>) -> &'static [&'static str] {
    match checkout {
        Some(checkout) if !builds_incrementally(checkout) => &["--config", "build.incremental=false"],
        _ => &[],
    }
}

/// Starts `builds units after-build <checkout>` in a session of its own, so
/// it outlives this process: it deletes the build copies this build
/// replaced, once nothing holds the checkout's guard or the profile's lock.
/// What it says goes to <root>/.after-build.log, which it holds an exclusive
/// flock on for its whole run (taken here, on the file description it
/// inherits as its stderr), so whoever needs it finished waits on that lock;
/// one still running from an earlier build is waited for first. Space it
/// doesn't free stays for the next prune, so a failure to start it is said
/// and the build's own result stands.
fn start_after_build(checkout: &Checkout, root: &Path) {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;
    let started = (|| -> std::io::Result<()> {
        let log = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".after-build.log"))?;
        while unsafe { libc::flock(log.as_raw_fd(), libc::LOCK_EX) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        log.set_len(0)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["units", "after-build"])
            .arg(&checkout.top)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        command.spawn().map(|_| ())
    })();
    if let Err(error) = started {
        crate::text::note(format!("starting the prune of the build copies this build replaced: {error}"));
    }
}

/// Runs cargo with these arguments plus the flag that has it report every
/// unit it uses on stdout, which the recorder reads, and the settings of the
/// checkout's role (`role_config`). Exits with cargo's status, or the
/// recorder's when cargo succeeded and a reported unit could not be recorded.
pub fn cmd_cargo(args: &[String]) -> Result<()> {
    let (subcommand, format): (&[String], &str) = match args.first().map(String::as_str) {
        Some("build" | "check" | "clippy" | "doc" | "rustc" | "rustdoc" | "test" | "bench" | "run" | "install") => {
            (&args[..1], "--message-format=json-render-diagnostics")
        }
        Some("nextest") => match args.get(1).map(String::as_str) {
            Some("run" | "list" | "archive") => (&args[..2], "--cargo-message-format=json-render-diagnostics"),
            other => fail!("cargo: cargo nextest {} builds nothing; run it without builds", other.unwrap_or("")),
        },
        None => fail!("usage: builds cargo <subcommand> [<args>...]"),
        Some(other) => fail!("cargo: cargo {other} builds nothing; run it without builds"),
    };
    let dir = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    let places = Places::from_env()?;
    let checkout = Checkout::at_keeping_env(&dir)?;
    let config = role_config(checkout.as_ref());
    // Held until cargo has exited and the recorder has read its last line.
    let root = checkout.as_ref().map(|checkout| checkout.build_root(&places)).filter(|root| root.is_dir());
    let guard = match &root {
        Some(root) => Some(crate::evict::lock_shared_creating(&root.join(GUARD))?),
        None => None,
    };
    let mut child = Command::new("cargo")
        .args(subcommand)
        .arg(format)
        .args(config)
        .args(&args[subcommand.len()..])
        .stdout(Stdio::piped())
        .spawn()
        .context(|| "running cargo".to_string())?;
    let stdout = child.stdout.take().expect("piped above");
    let recorded = record(BufReader::new(stdout), &mut std::io::stdout().lock());
    let status = child.wait().context(|| "waiting for cargo".to_string())?;
    drop(guard);
    if let (Some(checkout), Some(root)) = (&checkout, &root) {
        start_after_build(checkout, root);
    }
    // cargo and the recorder print their own failures.
    let recorded = match recorded {
        Ok(code) => code,
        Err(Fail::Message(message)) => {
            crate::text::note(message);
            1
        }
        Err(Fail::Exit(code)) => code,
    };
    if !status.success() {
        use std::os::unix::process::ExitStatusExt;
        return Err(Fail::Exit(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))));
    }
    match recorded {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, git, repo};

    fn role(dir: &Path) -> &'static [&'static str] {
        role_config(Checkout::at(dir).unwrap().as_ref())
    }

    #[test]
    fn only_main_builds_incrementally() {
        let scratch = Scratch::new("cargo-role");
        let places = scratch.places();
        let main = scratch.dir.join("demo");
        repo(&main, "target 1G\n");
        for name in ["side-1", "side-2"] {
            let side = scratch.dir.join("demo-worktrees").join(name);
            git(&main, &["worktree", "add", "-q", "--detach", &side.display().to_string()]);
        }
        let side_1 = scratch.dir.join("demo-worktrees/side-1");
        std::fs::create_dir_all(side_1.join("crate/src")).unwrap();
        std::fs::create_dir_all(places.root.join("demo")).unwrap();
        std::fs::write(places.app_checkout_file("demo"), "side-1\n").unwrap();
        let none: &[&str] = &[];
        let off: &[&str] = &["--config", "build.incremental=false"];
        assert_eq!(role(&side_1), off, "the app checkout");
        assert_eq!(role(&side_1.join("crate/src")), off);
        assert_eq!(role(&scratch.dir.join("demo-worktrees/side-2")), off, "another side worktree");
        assert_eq!(role(&main), none);
        assert_eq!(role(&main.join("crate")), none);
        assert_eq!(role(&scratch.dir), none, "outside any checkout");
    }
}
