//! `builds cargo` and `builds record-use`: recording the build copies cargo
//! used, so setup and prune keep those while they delete the ones no build
//! used for days.

use std::io::BufReader;
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::units::record::record;

/// Runs cargo with these arguments plus the flag that has it report every
/// unit it uses on stdout, which the recorder reads. Exits with cargo's
/// status, or the recorder's when cargo succeeded and a reported unit could
/// not be recorded.
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
    let mut child = Command::new("cargo")
        .args(subcommand)
        .arg(format)
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
