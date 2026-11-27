//! builds: owns this machine's build volume, /Volumes/Builds, and everything
//! on it. `builds help` says what it does; src/help.txt is that text.

mod ack;
mod agent_command;
mod agent_launch;
mod agent_list;
mod agent_prompt;
mod agent_prune;
mod agent_record;
mod agent_stream;
mod app_build;
mod app_build_base_misses;
mod app_build_commit;
mod app_build_requested_commit;
mod app_build_reused_shots;
mod app_build_step_failure;
mod app_build_step_shots;
mod app_clone;
mod app_config;
mod app_design_shots;
mod app_landing;
mod app_lock_guard;
mod app_platform;
mod app_remote;
mod app_reset;
mod app_shots_drop;
mod app_site;
mod app_timing;
mod app_tree;
mod app_waiting_shots;
mod awake;
mod branch_end;
mod brief;
mod build_app;
#[cfg(test)]
#[path = "../build_source.rs"]
mod build_source;
#[cfg(test)]
#[path = "build_source_tests.rs"]
mod build_source_tests;
mod built_tools;
mod capped_store;
mod card_profile;
mod card_state;
mod cargo_mutants;
mod cargo_needs;
mod cargo_place;
mod cargo_program;
mod cargo_read;
mod cargo_run;
mod cargo_run_commit;
mod cargo_run_crate;
mod cargo_run_outcome;
mod cargo_starts;
mod cargo_update;
mod checkout;
mod checks;
mod checks_run;
mod child_input;
mod claim;
mod claimed_branch;
mod claude_sessions;
mod clock;
#[cfg(target_os = "macos")]
mod core_foundation;
mod core_speed;
mod daemon;
mod db;
mod derive;
mod design_artifacts;
mod design_canvas;
mod design_publish;
mod design_render;
mod design_run;
mod design_system_artifact;
mod design_system_assets;
mod download;
mod evict;
mod fail;
mod flock;
mod floor;
mod free_worktrees;
mod fsevents_log;
mod git;
mod glob;
mod handed_runs;
mod health;
mod health_fix;
mod health_new;
mod health_stillness;
mod help;
mod holder_run;
mod hook_check_failure;
mod hook_check_starts;
mod hook_impact;
mod hook_stats;
mod host;
mod host_band;
mod host_chrome;
mod host_front_app;
mod host_hold;
mod host_installed;
mod host_linux;
mod host_load;
mod host_memory;
mod host_platform;
mod host_sample;
mod host_scan;
mod host_screen_lock;
mod host_setup;
mod idle_daemons;
mod inbox;
mod install;
mod install_gate;
mod install_hook;
mod job_claim;
mod kept;
#[cfg(target_os = "macos")]
mod kqueue;
mod lake_run;
mod land;
mod land_run;
mod land_squash;
mod land_turns;
mod lane;
mod lane_size;
mod lean;
mod linker;
mod links;
mod lists;
mod lock;
mod lock_diff;
mod lock_wait;
mod lock_wake;
mod main_checkout;
mod manifest;
mod migrate;
mod model_tier;
mod orphans;
mod output_excerpt;
mod places;
mod proc_limit;
mod process_readings;
mod procs;
mod programs;
mod project_flag;
mod prune;
mod prune_schedule;
mod python;
mod queue;
mod readings_file;
mod remote;
mod remote_worktrees;
mod repo_lanes;
mod review;
mod rules_on_edit;
mod run_end;
mod run_tmp;
mod rust_stable;
mod rustc_wrapper;
mod sccache;
mod sccache_history;
mod sccache_queue;
mod sccache_readings;
mod sccache_service;
mod setup;
mod shells;
mod shot_causes;
mod shot_findings;
mod shot_unconfirmed;
mod shots_cache;
mod shots_check_program;
mod shots_compare_again;
mod shots_declared;
mod shots_hits;
mod shots_in_use;
mod shots_picture;
mod shots_program;
mod side_git;
mod sizes;
mod sleeps;
mod spotlight;
mod sql_shape;
mod status;
mod step_failure;
mod step_outcomes;
mod stuck;
mod temp_builds;
#[cfg(test)]
mod testing;
mod text;
mod tool;
mod trash;
mod tree_state;
mod trees;
mod ui_test_settings;
mod units;
mod unlanded;
mod usage;
mod waits;
mod warm;
mod whole_file;
mod window_lock;
mod worktree;
mod xcode;
mod xcode_cache;

use std::process::ExitCode;

use fail::{Fail, Result};
use places::{Places, Rules};
use tool::Tool;

fn main() -> ExitCode {
    let mut argv = std::env::args_os();
    let program = argv.next().unwrap_or_default();
    let args: Vec<std::ffi::OsString> = argv.collect();
    #[cfg(target_os = "macos")]
    if args.len() == 1 && args[0] == app_lock_guard::COMMAND {
        return match app_lock_guard::watch() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => exit_code_after(app_lock_guard::COMMAND, error),
        };
    }
    if let [word, pid, what] = args.as_slice()
        && word == awake::HOLD_COMMAND
    {
        return match hold_awake(pid, what) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => exit_code_after(awake::HOLD_COMMAND, error),
        };
    }
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
    let (args, agent) = match ack::agent::take_flag(&args) {
        Ok(args) => args,
        Err(fail) => {
            return exit_code_after(command_of(&args), fail);
        }
    };
    let session = ack::agent::Session::from_env(agent);
    run_command(&args, session)
}

fn run_command(args: &[String], session: ack::agent::Session) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        print!("{}", help::HELP);
        return ExitCode::from(2);
    };
    let split = match project_flag::split(command, rest) {
        Ok(split) => split,
        Err(fail) => return exit_code_after(command, fail),
    };
    let rest = split.args.as_slice();
    let nested = std::env::var_os(ack::PASSED).is_some();
    let _use = match enter_stores(command, args) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(command, fail),
    };
    if let Err(fail) = Places::from_env().and_then(|places| {
        ack::gate(&places, &session, command, rest, split.flag.project())?;
        match &split.flag {
            // Refuses a name that no project has, which the command itself
            // would never look at. A command that takes no use of the stores
            // opens none: `install` runs on stores of an older schema.
            project_flag::Flag::Unread(project) if !install_gate::storeless(command, rest) => {
                lists::store::Store::named(&places, project)?
                    .repos()
                    .map(drop)
            }
            _ => Ok(()),
        }
    }) {
        return exit_code_after(command, fail);
    }
    if let Err(fail) = claimed_branch::gate(command, nested, &session) {
        return exit_code_after(command, fail);
    }
    // Held until the command ends, the measuring of what it wrote included.
    let _awake = match awake_for(command, rest) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(command, fail),
    };
    // Held until the command ends: dropping it deletes the run's temporary
    // folder.
    let _tmp = match Places::from_env().and_then(|places| run_tmp::enter(&places, command, rest)) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(command, fail),
    };
    let result = run(command, rest, session);
    // The command's folders are measured whether it succeeded or not: a
    // failed build left what it wrote.
    if let Ok(places) = Places::from_env() {
        sizes::record_after(&places, command, rest);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => exit_code_after(command, fail),
    }
}

/// Keeps this Mac awake while `builds <command> <args>` runs, when it runs
/// work (`awake::does_work`).
fn awake_for(command: &str, args: &[String]) -> Result<Option<awake::KeepAwake>> {
    if !awake::does_work(command, args) {
        return Ok(None);
    }
    let what = std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    awake::KeepAwake::while_working(&Places::from_env()?.power, &what).map(Some)
}

/// `builds __hold-awake <pid> <what>` (`awake::hold_while`), which a job's
/// script runs on a host.
fn hold_awake(pid: &std::ffi::OsStr, what: &std::ffi::OsStr) -> Result<()> {
    let Some(pid) = pid.to_str().and_then(|pid| pid.parse().ok()) else {
        crate::fail!("{} takes a process id", awake::HOLD_COMMAND);
    };
    awake::hold_while(&Places::from_env()?.power, pid, &what.to_string_lossy())
}

/// The command that <args> name: the first word that is not `--agent ID`, ""
/// when there is none.
fn command_of(args: &[String]) -> &str {
    ack::agent::without_flag(args)
        .first()
        .copied()
        .unwrap_or("")
}

/// The exit code of `builds <command>` after it failed with <fail>: the
/// code the failure names, else `failure_code` of the command.
fn exit_code_after(command: &str, fail: Fail) -> ExitCode {
    match fail {
        Fail::Exit(code) => ExitCode::from(code.clamp(0, 255) as u8),
        failure => {
            text::note(failure.text());
            ExitCode::from(failure_code(command))
        }
    }
}

/// The exit code of a command that fails with a message. The hook that gives
/// an agent its rules exits with 2, which Claude Code answers by stopping the
/// edit and showing the agent the message; any other code lets the edit go
/// ahead, and the agent never learns that its rules are missing.
fn failure_code(command: &str) -> u8 {
    if command == "rules-on-edit" { 2 } else { 1 }
}

/// Whether `builds <command> <rest>` holds a use of the stores for its life
/// (install_gate.rs): it writes a store after long work, with what it measured
/// since it started. Every other command writes in short transactions, or
/// runs under a holder's use: `builds cargo` records a run only for a hook
/// check, under the hook run's use. An app build's lane records its run
/// after long work too, but holds no use, so that an install never waits for
/// its steps: it hands its run to the installed builds when an install moved
/// the stores meanwhile (handed_runs.rs). A landing records its run after
/// long work, and
/// holds its use from its start until it has written that record, then ends
/// it before it fast-forwards the main checkout, whose post-merge hook may run
/// an install (land.rs).
fn holds_for_life(command: &str, rest: &[String]) -> bool {
    match command {
        // Records the hook run once its checks end (hook_stats.rs).
        "checks" => rest.first().is_some_and(|sub| sub == "run"),
        // A run, a copy review, a replay and an audit record what the
        // reviewers answered once they answer (review/mod.rs). A wait reads
        // the store each time a settle tells it to, and holds no use while
        // it waits (review/judge.rs); the other review commands read or
        // write in one transaction.
        "review" => !matches!(
            rest.first().map(String::as_str),
            Some(
                "groups"
                    | "wait"
                    | "settle"
                    | "dispute"
                    | "keep"
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
/// or None when the command takes a use per store connection, takes none (a
/// command `install_gate::storeless` names, or the shots checker), or runs
/// under a holder's use. An app build lane refuses to start if its parent
/// picked its program before an install moved the stores. Other commands
/// that waited at a closed gate run the program at the same path in their
/// place, with the same arguments.
fn enter_stores(command: &str, args: &[String]) -> Result<Option<install_gate::Use>> {
    if install_gate::storeless(command, &args[1..])
        || (command == "shots" && shots_program::runs_checker(&args[1..]))
        || std::env::var_os(install_gate::VARIABLE).is_some()
    {
        return Ok(None);
    }
    let places = Places::from_env()?;
    let pid = std::process::id();
    let (what, since) = (args.join(" "), text::clock());
    if !holds_for_life(command, &args[1..]) {
        let parent_generation = install_gate::take_lane_generation()?;
        match install_gate::PerConnection::enter(
            places,
            pid,
            &what,
            &since,
            parent_generation,
            &mut |line| text::note(line),
        )? {
            install_gate::Entered::Start(uses) => install_gate::use_per_connection(uses),
            install_gate::Entered::RunAgain => return Err(install_gate::run_again()),
        }
        return Ok(None);
    }
    match install_gate::enter_for_life(&places, pid, &what, &since, &mut |line| text::note(line))? {
        install_gate::Entered::Start(held) => {
            // SAFETY: no other thread runs yet, so none reads the environment
            // while set_var writes it. Every builds command this one starts runs
            // under its use.
            unsafe { std::env::set_var(install_gate::VARIABLE, pid.to_string()) };
            Ok(Some(held))
        }
        install_gate::Entered::RunAgain => Err(install_gate::run_again()),
    }
}

/// The sources this program is built from, as a tar archive (build.rs):
/// `builds host setup` builds them on a Linux host.
pub const SOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/source.tar"));

/// The path of this program.
pub fn this_program() -> Result<std::path::PathBuf> {
    use fail::Context;
    std::env::current_exe().context(|| "finding this program".to_string())
}

/// `builds <hash>`: the hash of the sources this program is built from
/// (build.rs), the same for every OS's build of them.
pub fn version() -> String {
    format!("builds {}", env!("BUILDS_SOURCE"))
}

fn run(command: &str, args: &[String], session: ack::agent::Session) -> Result<()> {
    if python::STEP_COMMANDS.contains(&command) {
        python::use_new_python(&Places::from_env()?)?;
    }
    match command {
        // These run in the caller's environment as it is, which cargo
        // inherits; `cargo` asks git about its checkout without changing it.
        "cargo" => return cargo_run::cmd_cargo(args),
        "lake" => return lake_run::cmd_lake(args),
        "version" => {
            println!("{}", version());
            return Ok(());
        }
        "programs" => return programs::cmd_programs(&Places::from_env()?, args),
        "sccache-tmpdir" => return sccache::cmd_tmpdir(&Places::from_env()?.root, args),
        "checks" => return checks_run::cmd_checks(args),
        "waits" => return waits::cmd_waits(args),
        "install-hook" => return install_hook::cmd_install_hook(args),
        "lock" => return lock::cmd_lock(&Places::from_env()?, args),
        "app-build" => {
            return app_build::cmd_app_build(
                &Tool::new(
                    Places::from_env()?,
                    Rules::from_env()?,
                    this_program()?,
                    session,
                )?,
                args,
            );
        }
        "shots" if args.first().is_some_and(|sub| sub == "picture") => {
            return shots_picture::cmd(
                &Tool::new(
                    Places::from_env()?,
                    Rules::from_env()?,
                    this_program()?,
                    session,
                )?,
                &args[1..],
            );
        }
        "shots" => return shots_program::cmd_shots(&Places::from_env()?, args),
        "brief" => {
            let root = review::default_rules_root()?;
            return brief::cmd_brief(&root, args);
        }
        "ack" => return ack::cmd_ack(&Places::from_env()?, &session, args),
        "agent" if args.first().is_some_and(|sub| sub == "start") => {
            return ack::agent::cmd_agent(&Places::from_env()?, args);
        }
        "rules-on-edit" => {
            let root = review::default_rules_root()?;
            return rules_on_edit::cmd_rules_on_edit(&Places::from_env()?, &root, args);
        }
        "queue" => return queue::cmd_queue(&Places::from_env()?, args),
        "takeover" => return queue::cmd_takeover(&Places::from_env()?, args),
        "-h" | "--help" | "help" => match help::lookup(args) {
            Ok(text) => {
                print!("{text}");
                return Ok(());
            }
            Err(why) => fail!("{why}"),
        },
        _ => {}
    }
    if command == "dashboard" {
        ack::agent::clear_for_daemon();
    }
    checkout::drop_git_local_env()?;
    text::use_callers_collation();
    let mut tool = Tool::new(
        Places::from_env()?,
        Rules::from_env()?,
        this_program()?,
        session,
    )?;
    match command {
        "setup" => setup::cmd_setup(&tool, &host::Ssh::from_env(), args),
        "new" => worktree::cmd_new(&tool, &host::Ssh::from_env(), args),
        "retire" => worktree::cmd_retire(&tool, args),
        "land" => land::cmd_land(&tool, args),
        "review" => review::cmd_review(&tool, args),
        "judge" => review::judge::cmd_judge(&tool, args),
        "claim" => claim::cmd_claim(&tool.places, args, tool.now),
        "note" => claim::cmd_note(&tool.places, args, tool.now),
        "release" => claim::cmd_release(&tool.places, args),
        "app-checkout" => worktree::cmd_app_checkout(&tool, &host::Ssh::from_env(), args),
        "app-clone" => warm::cmd_app_clone(&tool, args),
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
        "agent" => agent_command::cmd_agent(&tool, args),
        "item" => lists::cmd_item(&tool, &host::Ssh::from_env(), args),
        "next" => lists::cmd_next(&tool, args),
        "area" => lists::cmd_area(&tool, args),
        "dashboard" => daemon::cmd_dashboard(&tool, args),
        "host" => host::cmd_host(&tool, &host::Ssh::from_env(), args),
        "remote" => remote::cmd_remote(&tool, &host::Ssh::from_env(), args),
        "tool" => built_tools::cmd_tool(&tool.places, &host::Ssh::from_env(), args),
        other => fail!(
            "builds has no command {other}. Its commands are ack, agent, rules-on-edit, setup, new, land, review, judge, claim, note, release, retire, app-checkout, app-clone, status, health, lists, brief, decision, design, question, item, next, area, dashboard, host, remote, prune, tool, cargo, lake, lock, app-build, shots, queue, takeover, build-app, install, checks, install-hook and version."
        ),
    }
}
