//! `builds cargo` and `builds record-use`: recording the build copies cargo
//! used, so setup and prune keep those while they delete the ones no build
//! used for days.

use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::checkout::{Checkout, is_app_checkout};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::places::Places;
use crate::units::record::record;

/// The cargo settings a build in <dir>'s checkout takes from the checkout's
/// role, as `--config` arguments.
///
/// The app checkout builds without incremental compilation. Agents take turns
/// in it, each moving it to their own branch, so a crate's incremental state
/// is rarely there for the next build to reuse, and sccache refuses every
/// incremental compile: with it off, sccache keeps each workspace crate a
/// branch built there, and the next build of that source comes from the
/// cache. Main and the other side worktrees keep incremental, which halves an
/// edit-and-recheck loop's CPU in a checkout that stays on one branch.
///
/// A `--config` argument, not a `CARGO_*` variable: cargo hands its
/// environment on to rustc and sccache keys every `CARGO_*` variable, so
/// with a variable every dependency built through this command would get
/// other cache entries than the same dependency built by cargo run by hand
/// in the same checkout. The argument changes only what it sets.
fn role_config(places: &Places, dir: &Path) -> Result<&'static [&'static str]> {
    let Some(checkout) = Checkout::at_keeping_env(dir)? else { return Ok(&[]) };
    if is_app_checkout(places, &checkout.build_root(places))? {
        Ok(&["--config", "build.incremental=false"])
    } else {
        Ok(&[])
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
    let config = role_config(&Places::from_env()?, &dir)?;
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

/// `builds record-use`: records the build copies a cargo report on stdin
/// names, passing every other line through.
pub fn cmd_record_use(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("usage: cargo ... --message-format=json-render-diagnostics | builds record-use");
    }
    match record(std::io::stdin().lock(), &mut std::io::stdout().lock())? {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, git, repo};

    #[test]
    fn only_the_app_checkout_builds_without_incremental() {
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
        let none: &[&str] = &[];
        assert_eq!(role_config(&places, &side_1).unwrap(), none, "no app checkout is recorded yet");

        std::fs::create_dir_all(places.root.join("demo")).unwrap();
        std::fs::write(places.app_checkout_file("demo"), "side-1\n").unwrap();
        let off: &[&str] = &["--config", "build.incremental=false"];
        assert_eq!(role_config(&places, &side_1).unwrap(), off);
        assert_eq!(role_config(&places, &side_1.join("crate/src")).unwrap(), off);
        assert_eq!(role_config(&places, &main).unwrap(), none);
        assert_eq!(role_config(&places, &scratch.dir.join("demo-worktrees/side-2")).unwrap(), none);
        assert_eq!(role_config(&places, &scratch.dir).unwrap(), none, "outside any checkout");
    }
}
