//! builds: owns this machine's build volume, /Volumes/Builds, and everything
//! on it. `builds help` says what it does; src/help.txt is that text.

mod ack;
mod app_build;
mod app_config;
mod app_remote;
mod app_reset;
mod app_site;
mod app_timing;
mod build_app;
mod built_tools;
mod card_profile;
mod cargo_place;
mod cargo_read;
mod cargo_run;
mod cargo_run_crate;
mod cargo_starts;
mod cargo_update;
mod checkout;
mod checks;
mod checks_run;
mod claim;
mod claude_sessions;
mod core_speed;
mod daemon;
mod db;
mod derive;
mod design_artifacts;
mod design_canvas;
mod design_publish;
mod design_render;
mod design_system_artifact;
mod design_system_assets;
mod evict;
mod fail;
mod flock;
mod floor;
mod free_worktrees;
mod glob;
mod health;
mod health_fix;
mod holder_run;
mod hook_check_starts;
mod hook_impact;
mod hook_stats;
mod host;
mod host_band;
mod host_installed;
mod host_load;
mod host_memory;
mod host_sample;
mod host_setup;
mod idle_daemons;
mod install;
mod install_gate;
mod install_hook;
mod job_claim;
mod kept;
mod land;
mod land_run;
mod lane;
mod lane_size;
mod linker;
mod links;
mod lists;
mod lock;
mod lock_diff;
mod lock_wait;
mod main_checkout;
mod manifest;
mod migrate;
mod nextest;
mod places;
mod proc_limit;
mod procs;
mod prune;
mod prune_schedule;
mod queue;
mod remote;
mod remote_worktrees;
mod review;
mod rust_stable;
mod rustc_wrapper;
mod sccache;
mod setup;
mod shells;
mod shot_findings;
mod shots_program;
mod side_git;
mod spotlight;
mod sql_shape;
mod status;
mod stuck;
mod temp_builds;
#[cfg(test)]
mod testing;
mod text;
mod tool;
mod trash;
mod trees;
mod ui_test_settings;
mod units;
mod unlanded;
mod usage;
mod waits;
mod warm;
mod whole_file;
mod worktree;
mod xcode;

use std::process::ExitCode;

use fail::{Fail, Result};
use places::{Places, Rules};
use tool::Tool;

const HELP: &str = include_str!("help.txt");
/// What the lists are and who writes them, which every agent reads.
const LISTS_OVERVIEW: &str = include_str!("lists/lists-overview.txt");
/// `builds help lists`: the overview, then each command.
const LISTS_HELP: &str = concat!(
    include_str!("lists/lists-overview.txt"),
    "\n\n",
    include_str!("lists/lists-help.txt")
);
const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

fn main() -> ExitCode {
    let mut argv = std::env::args_os();
    let program = argv.next().unwrap_or_default();
    let args: Vec<std::ffi::OsString> = argv.collect();
    if let Some(fail) = links::run_as(&program, &args) {
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
    let args = match ack::agent::take_flag(&args) {
        Ok(args) => args,
        Err(fail) => return exit_code(fail),
    };
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::from(2);
    };
    let _use = match enter_stores(command, &args) {
        Ok(held) => held,
        Err(fail) => return exit_code(fail),
    };
    if let Err(fail) = Places::from_env().and_then(|places| ack::gate(&places, command, rest)) {
        return exit_code(fail);
    }
    match run(command, rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => exit_code(fail),
    }
}

/// The exit code of a command that failed with <fail>, after saying its
/// message on stderr.
fn exit_code(fail: Fail) -> ExitCode {
    match fail {
        Fail::Message(message) => {
            text::note(message);
            ExitCode::from(1)
        }
        Fail::Exit(code) => ExitCode::from(code.clamp(0, 255) as u8),
    }
}

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
        // A run, a copy review, a replay and an audit record what the
        // reviewers answered once they answer (review/mod.rs); the other
        // review commands read or write in one transaction.
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

/// Takes the use of the stores that <command> needs before it runs. <args>
/// starts with the command. Returns the use the command holds for its life,
/// or None when the command takes a use per store connection, takes none, or
/// runs under a holder's use. A command
/// that waited at a closed gate while an install ran runs the program at this
/// program's path in its place, with the same arguments: install replaced the
/// program it started from.
fn enter_stores(command: &str, args: &[String]) -> Result<Option<install_gate::Use>> {
    if install_gate::STORELESS.contains(&command)
        || std::env::var_os(install_gate::VARIABLE).is_some()
    {
        return Ok(None);
    }
    let places = Places::from_env()?;
    let pid = std::process::id();
    let (what, since) = (args.join(" "), text::clock());
    if !holds_for_life(command, &args[1..]) {
        if !install_gate::may_start(&places, &mut |line| text::note(line))? {
            return Err(install_gate::run_again());
        }
        install_gate::use_per_connection(install_gate::PerConnection::new(
            places, pid, &what, &since,
        )?);
        return Ok(None);
    }
    match install_gate::enter_for_life(&places, pid, &what, &since, &mut |line| text::note(line))? {
        install_gate::Entered::Holds(held) => {
            // SAFETY: no other thread runs yet, so none reads the environment
            // while set_var writes it. Every builds command this one starts runs
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
        "waits" => return waits::cmd_waits(args),
        "install-hook" => return install_hook::cmd_install_hook(args),
        "lock" => return lock::cmd_lock(&Places::from_env()?, args),
        "app-build" => {
            return app_build::cmd_app_build(
                &Places::from_env()?,
                Rules::from_env()?.floor_kib,
                args,
            );
        }
        "shots" => return shots_program::cmd_shots(&Places::from_env()?, args),
        "ack" => return ack::cmd_ack(&Places::from_env()?, args),
        "agent" => return ack::agent::cmd_agent(&Places::from_env()?, args),
        "queue" => return queue::cmd_queue(&Places::from_env()?, args),
        "takeover" => return queue::cmd_takeover(&Places::from_env()?, args),
        "-h" | "--help" | "help" => {
            match args {
                [topic] if topic == "lists" => print!("{LISTS_HELP}"),
                [topic] if topic == "workflow" => {
                    print!("{WORKFLOW_HELP}");
                    print!(
                        "{}",
                        review::workflow_section(
                            &Places::from_env()?,
                            &lists::args::current_dir()?
                        )?
                    );
                }
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
        "setup" => setup::cmd_setup(&tool, &host::Ssh::from_env(), args),
        "new" => worktree::cmd_new(&tool, &host::Ssh::from_env(), args),
        "retire" => worktree::cmd_retire(&tool, args),
        "land" => land::cmd_land(&tool.places, args),
        "review" => review::cmd_review(&tool, args),
        "claim" => claim::cmd_claim(&tool.places, args, tool.now),
        "note" => claim::cmd_note(&tool.places, args, tool.now),
        "release" => claim::cmd_release(&tool.places, args),
        "app-checkout" => worktree::cmd_app_checkout(&tool, &host::Ssh::from_env(), args),
        "build-app" => build_app::cmd_build_app(&tool, args),
        "status" => status::cmd_status(&mut tool, &host::Ssh::from_env(), args),
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
        "tool" => built_tools::cmd_tool(&tool.places, &host::Ssh::from_env(), args),
        other => fail!(
            "builds has no command {other}. Its commands are ack, agent, setup, new, land, review, claim, note, release, retire, app-checkout, status, health, lists, decision, design, question, item, next, area, dashboard, host, remote, prune, tool, cargo, lock, app-build, shots, queue, takeover, build-app, install, checks and install-hook."
        ),
    }
}
