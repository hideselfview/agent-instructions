//! builds: owns this machine's build volume, /Volumes/Builds, and everything
//! on it. `builds help` says what it does; src/help.txt is that text.

mod app_build;
mod app_config;
mod app_remote;
mod app_reset;
mod app_timing;
mod build_app;
mod cargo_place;
mod cargo_run;
mod cargo_runs;
mod checkout;
mod checks;
mod checks_run;
mod claim;
mod daemon;
mod db;
mod derive;
mod design_render;
mod evict;
mod fail;
mod floor;
mod free_worktrees;
mod health;
mod hook_impact;
mod hook_stats;
mod host;
mod install;
mod install_hook;
mod land;
mod lane;
mod lists;
mod lock;
mod manifest;
mod migrate;
mod places;
mod procs;
mod prune;
mod queue;
mod remote;
mod review;
mod sccache;
mod setup;
mod shells;
mod shot_findings;
mod side_git;
mod status;
mod stuck;
mod temp_builds;
#[cfg(test)]
mod testing;
mod text;
mod tool;
mod trash;
mod trees;
mod units;
mod usage;
mod warm;
mod worktree;

use std::process::ExitCode;

use fail::{Fail, Result};
use places::{Places, Rules};
use tool::Tool;

const HELP: &str = include_str!("help.txt");
const LISTS_HELP: &str = include_str!("lists/lists-help.txt");
const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect()
    {
        Ok(args) => args,
        Err(arg) => {
            text::note(format!(
                "an argument is not UTF-8: {}",
                arg.to_string_lossy()
            ));
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
        // These run in the caller's environment as it is, which cargo
        // inherits; `cargo` asks git about its checkout without changing it.
        "cargo" => return cargo_run::cmd_cargo(args),
        "checks" => return checks_run::cmd_checks(args),
        "install-hook" => return install_hook::cmd_install_hook(args),
        "lock" => return lock::cmd_lock(&Places::from_env()?, args),
        "record-run" => return app_timing::cmd_record_run(args),
        "app-build" => return app_build::cmd_app_build(&Places::from_env()?, args),
        "shots" => return shot_findings::cmd_shots(&Places::from_env()?, args),
        "queue" => return queue::cmd_queue(&Places::from_env()?, args),
        "takeover" => return queue::cmd_takeover(&Places::from_env()?, args),
        "-h" | "--help" | "help" => {
            match args {
                [topic] if topic == "lists" => print!("{LISTS_HELP}"),
                [topic] if topic == "workflow" => print!("{WORKFLOW_HELP}"),
                [topic] if topic == "briefs" => print!("{BRIEFS_HELP}"),
                _ => print!("{HELP}"),
            }
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
        "land" => land::cmd_land(args),
        "review" => review::cmd_review(&tool, args),
        "claim" => claim::cmd_claim(&tool.places, args, tool.now),
        "note" => claim::cmd_note(&tool.places, args),
        "release" => claim::cmd_release(&tool.places, args),
        "app-checkout" => worktree::cmd_app_checkout(&tool, args),
        "build-app" => build_app::cmd_build_app(&tool, args),
        "status" => status::cmd_status(&mut tool, args),
        "prune" => prune::cmd_prune(&tool, args),
        "install" => install::cmd_install(&tool, args),
        "units" => units::cmd_units(&tool, args),
        "du" => usage::cmd_du(args),
        "health" => health::cmd_health(&tool, args),
        "lists" => lists::cmd_lists(&tool, args),
        "decision" => lists::cmd_decision(&tool, args),
        "design" => lists::cmd_design(&tool, args),
        "question" => lists::question_command::cmd_question(&tool, args),
        "item" => lists::cmd_item(&tool, args),
        "next" => lists::cmd_next(&tool, args),
        "area" => lists::cmd_area(&tool, args),
        "dashboard" => daemon::cmd_dashboard(&tool, args),
        "host" => host::cmd_host(&tool, &host::Ssh::from_env(), args),
        "remote" => remote::cmd_remote(&tool, &host::Ssh::from_env(), args),
        other => fail!(
            "unknown command {other} (setup | new | land | review | claim | note | release | retire | app-checkout | status | health | lists | decision | design | question | item | next | area | dashboard | host | remote | prune | cargo | lock | app-build | shots | queue | takeover | build-app | install | checks | install-hook)"
        ),
    }
}
