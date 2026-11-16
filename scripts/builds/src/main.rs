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
mod cargo_starts;
mod checkout;
mod checks;
mod checks_run;
mod claim;
mod core_speed;
mod daemon;
mod db;
mod derive;
mod design_render;
mod design_tokens;
mod evict;
mod fail;
mod flock;
mod floor;
mod free_worktrees;
mod health;
mod hook_check_starts;
mod hook_impact;
mod hook_stats;
mod host;
mod host_installed;
mod host_setup;
mod install;
mod install_gate;
mod install_hook;
mod land;
mod lane;
mod linker;
mod lists;
mod lock;
mod lock_diff;
mod lock_wait;
mod manifest;
mod migrate;
mod places;
mod procs;
mod prune;
mod queue;
mod remote;
mod remote_worktrees;
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
mod whole_file;
mod worktree;
mod xcode;

use std::process::ExitCode;

use fail::{Fail, Result};
use places::{Places, Rules};
use tool::Tool;

const HELP: &str = include_str!("help.txt");
const LISTS_HELP: &str = include_str!("lists/lists-help.txt");
const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

fn main() -> ExitCode {
    let mut argv = std::env::args_os();
    let program = argv.next().unwrap_or_default();
    if std::path::Path::new(&program).file_name() == Some(std::ffi::OsStr::new(linker::NAME)) {
        let args: Vec<std::ffi::OsString> = argv.collect();
        let Err(fail) = linker::run(&args);
        text::note(fail.text());
        return ExitCode::from(1);
    }
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
    let entered = enter_stores(command, &args);
    let _use = match entered {
        Ok(held) => held,
        Err(Fail::Message(message)) => {
            text::note(message);
            return ExitCode::from(1);
        }
        Err(Fail::Exit(code)) => return ExitCode::from(code.clamp(0, 255) as u8),
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

/// The commands that take no use of the stores (install_gate.rs): install,
/// which closes the gates, the dashboard, which install restarts, and the
/// help.
const UNGATED: [&str; 5] = ["install", "dashboard", "help", "-h", "--help"];

/// Whether `builds <command> <rest>` holds a use of the stores for its life
/// (install_gate.rs): it writes a store after long work, with what it measured
/// since it started. Every other command writes in short transactions, or
/// runs under a holder's use: `builds cargo` records a run only for a hook
/// check, under the hook run's use. An app build's lane records its findings
/// and its run after long work too, and takes its use once it holds the
/// lane's lock (app_build.rs).
fn holds_for_life(command: &str, rest: &[String]) -> bool {
    match command {
        // Records the hook run once its checks end (hook_stats.rs).
        "checks" => rest.first().is_some_and(|sub| sub == "run"),
        // A run, a replay and an audit record what the reviewers answered
        // once they answer (review/mod.rs); the other review commands read or
        // write in one transaction.
        "review" => !matches!(
            rest.first().map(String::as_str),
            Some(
                "groups"
                    | "settle"
                    | "fixed"
                    | "show"
                    | "check-landing"
                    | "landed"
                    | "misses"
                    | "rates"
                    | "scores"
                    | "case"
            )
        ),
        _ => false,
    }
}

/// Enters the stores for command <command>, run with <args> (the command
/// first): returns the use it holds for its life, or None when it takes a use
/// per store connection, takes none, or runs under a holder's use. A command
/// that waited at a closed gate while an install ran runs the program at this
/// program's path in its place, with the same arguments: install replaced the
/// program it started from.
fn enter_stores(command: &str, args: &[String]) -> Result<Option<install_gate::Use>> {
    if UNGATED.contains(&command) || std::env::var_os(install_gate::VARIABLE).is_some() {
        return Ok(None);
    }
    let places = Places::from_env()?;
    let pid = std::process::id();
    let (what, since) = (args.join(" "), text::clock());
    if !holds_for_life(command, &args[1..]) {
        if !install_gate::may_start(&places)? {
            return Err(install_gate::run_again());
        }
        install_gate::use_per_connection(install_gate::PerConnection::new(
            places, pid, &what, &since,
        )?);
        return Ok(None);
    }
    match install_gate::enter_for_life(&places, pid, &what, &since)? {
        install_gate::Entered::Holds(held) => {
            // SAFETY: no other thread runs yet, so none reads the environment
            // while this writes it. Every builds command this one starts runs
            // under its use.
            unsafe { std::env::set_var(install_gate::VARIABLE, pid.to_string()) };
            Ok(Some(held))
        }
        install_gate::Entered::RunAgain => Err(install_gate::run_again()),
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
