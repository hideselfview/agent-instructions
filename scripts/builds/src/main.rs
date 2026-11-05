//! builds: owns this machine's build volume, /Volumes/Builds, and everything
//! on it. `builds help` says what it does; src/help.txt is that text.

mod cargo_run;
mod checkout;
mod claim;
mod evict;
mod fail;
mod floor;
mod install;
mod manifest;
mod places;
mod prune;
mod setup;
mod status;
mod temp_builds;
#[cfg(test)]
mod testing;
mod text;
mod tool;
mod units;
mod usage;
mod worktree;

use std::process::ExitCode;

use fail::{Fail, Result};
use places::{Places, Rules};
use tool::Tool;

const HELP: &str = include_str!("help.txt");

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os().skip(1).map(|arg| arg.into_string()).collect() {
        Ok(args) => args,
        Err(arg) => {
            text::note(format!("an argument is not UTF-8: {}", arg.to_string_lossy()));
            return ExitCode::from(1);
        }
    };
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::from(2);
    };
    match run(command, rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Message(message)) => {
            text::note(message);
            ExitCode::from(1)
        }
        Err(Fail::Exit(code)) => ExitCode::from(code.clamp(0, 255) as u8),
    }
}

fn run(command: &str, args: &[String]) -> Result<()> {
    match command {
        // These run in the caller's environment as it is and call no git.
        "cargo" => return cargo_run::cmd_cargo(args),
        "record-use" => return cargo_run::cmd_record_use(args),
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            return Ok(());
        }
        _ => {}
    }
    checkout::drop_git_local_env()?;
    text::use_callers_collation();
    let mut tool = Tool::new(Places::from_env()?, Rules::from_env()?);
    match command {
        "setup" => setup::cmd_setup(&tool, args),
        "new" => worktree::cmd_new(&tool, args),
        "retire" => worktree::cmd_retire(&tool, args),
        "claim" => claim::cmd_claim(args, tool.now),
        "note" => claim::cmd_note(args),
        "release" => claim::cmd_release(args),
        "app-checkout" => worktree::cmd_app_checkout(&tool, args),
        "status" => status::cmd_status(&mut tool, args),
        "prune" => prune::cmd_prune(&tool, args),
        "install" => install::cmd_install(&tool, args),
        "units" => units::cmd_units(args),
        "du" => usage::cmd_du(args),
        other => fail!(
            "unknown command {other} (setup | new | claim | note | release | retire | app-checkout | status | dashboard | prune | cargo | record-use | install)"
        ),
    }
}
