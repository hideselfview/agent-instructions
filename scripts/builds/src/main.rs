//! builds: owns this machine's build volume, /Volumes/Builds, and everything
//! on it. `builds help` says what it does; src/help.txt is that text.

// Everything builds prints goes through output.rs, which caps what a command
// says about its work; clippy.toml lists the functions that reach the streams
// without a print macro. Tests print what they like.
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)
)]
#![cfg_attr(test, allow(clippy::disallowed_methods))]

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
mod args;
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
mod command;
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
mod discard;
mod download;
mod evict;
mod fail;
mod finder_dates;
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
mod host_display;
mod host_hold;
mod host_installed;
mod host_linux;
mod host_load;
mod host_memory;
mod host_platform;
mod host_sample;
mod host_scan;
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
mod output;
mod output_excerpt;
mod places;
mod proc_limit;
mod process;
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
mod roots;
mod rules_on_edit;
mod run_end;
mod run_tmp;
mod rust_stable;
mod rust_toolchain;
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
mod shots_compose;
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
mod system;
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
mod web_picture;
mod whole_file;
mod window_lock;
mod worktree;
mod worktree_removal;
mod xcode;
mod xcode_cache;

use std::process::ExitCode;

use command::{Command, Parsed};
use fail::{Fail, Result};
use install_gate::Stores;
use places::{Places, Rules};

fn main() -> ExitCode {
    let code = run_program();
    output::finish();
    code
}

fn run_program() -> ExitCode {
    let mut argv = std::env::args_os();
    let program = argv.next().unwrap_or_default();
    let args: Vec<std::ffi::OsString> = argv.collect();
    // builds installs each Rust toolchain itself, under a lock, so rustup
    // installs none for this command or for anything it starts
    // (rust_toolchain.rs).
    // SAFETY: no other thread runs yet, so none reads the environment while
    // set_var writes it.
    unsafe { std::env::set_var(rust_toolchain::AUTO_INSTALL_VARIABLE, "0") };
    let system = system::System::from_env();
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
        return match system.and_then(|system| hold_awake(&system, pid, what)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => exit_code_after(awake::HOLD_COMMAND, error),
        };
    }
    if let Some(fail) = links::run_as(&program, &args) {
        output::note(fail.text());
        return ExitCode::from(1);
    }
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect()
    {
        Ok(args) => args,
        Err(arg) => {
            output::note(format!(
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
    let system = match system {
        Ok(system) => system,
        Err(fail) => return exit_code_after(command_of(&args), fail),
    };
    run_command(&args, session, &system)
}

fn run_command(args: &[String], session: ack::agent::Session, system: &system::System) -> ExitCode {
    let Some(first) = args.first() else {
        output::answer(&*help::HELP);
        return ExitCode::from(2);
    };
    let parsed = match Command::parse(args) {
        Ok(parsed) => parsed,
        Err(fail) => return exit_code_after(first, fail),
    };
    let command = parsed.command;
    let words = match args::Args::of(command, parsed.rest) {
        Ok(words) => words,
        Err(fail) => return exit_code_after(first, fail),
    };
    let project = match project_flag::Flag::of(&words) {
        Ok(project) => project,
        Err(fail) => return exit_code_after(first, fail),
    };
    let nested = std::env::var_os(ack::PASSED).is_some();
    let _use = match enter_stores(command, args) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(first, fail),
    };
    if let Err(fail) = Places::from_env().and_then(|places| {
        ack::gate(&places, &session, &words, project.project())?;
        match &project {
            // Refuses a name that no project has, which the command itself
            // would never look at. A command that takes no use of the stores
            // opens none: `install` runs on stores of an older schema.
            project_flag::Flag::Unread(project) if command.stores() != Stores::Storeless => {
                lists::store::Store::named(&places, project)?
                    .repos()
                    .map(drop)
            }
            _ => Ok(()),
        }
    }) {
        return exit_code_after(first, fail);
    }
    if let Err(fail) = claimed_branch::gate(command, nested, &session) {
        return exit_code_after(first, fail);
    }
    // Held until the command ends, the measuring of what it wrote included.
    let _awake = match awake_for(system, &parsed) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(first, fail),
    };
    // Held until the command ends: dropping it deletes the run's temporary
    // folder.
    let _tmp = match Places::from_env().and_then(|places| run_tmp::enter(&places, command)) {
        Ok(held) => held,
        Err(fail) => return exit_code_after(first, fail),
    };
    let changed = roots::Changed::default();
    let result = run(&words, session, &changed, system);
    // The roots the command changed are measured whether it succeeded or
    // not: a failed build left what it wrote.
    if let Ok(places) = Places::from_env() {
        changed.record(&places);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(fail) => exit_code_after(first, fail),
    }
}

/// Keeps this Mac awake while <parsed> runs, when it runs work
/// (`Command::keeps_awake`).
fn awake_for(
    system: &system::System,
    parsed: &Parsed<'_, String>,
) -> Result<Option<awake::KeepAwake>> {
    if !parsed.command.keeps_awake() {
        return Ok(None);
    }
    let what = parsed
        .named
        .iter()
        .chain(parsed.rest)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    awake::KeepAwake::while_working(&system.power(), &what).map(Some)
}

/// `builds __hold-awake <pid> <what>` (`awake::hold_while`), which a job's
/// script runs on a host.
fn hold_awake(
    system: &system::System,
    pid: &std::ffi::OsStr,
    what: &std::ffi::OsStr,
) -> Result<()> {
    let Some(pid) = pid.to_str().and_then(|pid| pid.parse().ok()) else {
        crate::fail!("{} takes a process id", awake::HOLD_COMMAND);
    };
    awake::hold_while(&system.power(), pid, &what.to_string_lossy())
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
            output::note(failure.text());
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

/// Takes the use of the stores that <command> needs before it runs
/// (`Command::stores`). <args> is the command line after the program.
/// Returns the use the command holds for its life, or None when the command
/// takes a use per store connection, takes none, or runs under a holder's
/// use. An app build lane refuses to start if its parent picked its program
/// before an install moved the stores. Other commands that waited at a closed
/// gate run the program at the same path in their place, with the same
/// arguments.
///
/// Every command but those that hold a use for their life writes in short
/// transactions, or runs under a holder's use: `builds cargo` records a run
/// only for a hook check, under the hook run's use. An app build's lane
/// records its run after long work too, but holds no use, so that an install
/// never waits for its steps: it hands its run to the installed builds when
/// an install moved the stores meanwhile (handed_runs.rs). A landing records
/// its run after long work, and holds its use from its start until it has
/// written that record, then ends it before it fast-forwards the main
/// checkout, whose post-merge hook may run an install (land.rs).
fn enter_stores(command: Command, args: &[String]) -> Result<Option<install_gate::Use>> {
    let stores = command.stores();
    if stores == Stores::Storeless || std::env::var_os(install_gate::VARIABLE).is_some() {
        return Ok(None);
    }
    let places = Places::from_env()?;
    let pid = std::process::id();
    let (what, since) = (args.join(" "), text::clock());
    if stores == Stores::PerConnection {
        let parent_generation = install_gate::take_lane_generation()?;
        match install_gate::PerConnection::enter(
            places,
            pid,
            &what,
            &since,
            parent_generation,
            &mut |line| output::note(line),
        )? {
            install_gate::Entered::Start(uses) => install_gate::use_per_connection(uses),
            install_gate::Entered::RunAgain => return Err(install_gate::run_again()),
        }
        return Ok(None);
    }
    match install_gate::enter_for_life(&places, pid, &what, &since, &mut |line| output::note(line))?
    {
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

/// Runs the command of <args>.
fn run(
    args: &args::Args,
    session: ack::agent::Session,
    changed: &roots::Changed,
    system: &system::System,
) -> Result<()> {
    use Command::*;
    use command::*;
    let command = args.command();
    if command.runs_steps() {
        python::use_new_python(&Places::from_env()?)?;
    }
    // A tool in the caller's environment as it is.
    let as_called = |session| -> Result<tool::Tool> {
        tool::Tool::new(
            Places::from_env()?,
            Rules::from_env()?,
            this_program()?,
            session,
            changed.clone(),
            system.clone(),
        )
    };
    // A tool for a command that runs git in checkouts other than the one it
    // runs in, and sorts names as the caller does.
    let tool = |session| -> Result<tool::Tool> {
        checkout::drop_git_local_env()?;
        text::use_callers_collation();
        as_called(session)
    };
    let ssh = || system.ssh();
    let cwd = crate::args::current_dir;
    let out = &mut output::answer_handle();
    match command {
        // These run in the caller's environment as it is, which cargo
        // inherits; `cargo` asks git about its checkout without changing it.
        Cargo => cargo_run::cmd_cargo(system, args.positional(), changed),
        Lake => lake_run::cmd_lake(system, args.positional(), changed),
        Version => {
            output::answer_line(version());
            Ok(())
        }
        Programs => programs::cmd_programs(&Places::from_env()?, system, args),
        SccacheTmpdir => sccache::cmd_tmpdir(&Places::from_env()?.root, args),
        Checks(checks::Run) => checks_run::cmd_run(
            crate::checks::Machine {
                places: &Places::from_env()?,
                system,
            },
            args,
        ),
        Checks(checks::Whole) => checks_run::cmd_whole(
            crate::checks::Machine {
                places: &Places::from_env()?,
                system,
            },
            args,
        ),
        Checks(checks::Stats) => checks_run::stats(args),
        Checks(checks::List) => checks_run::cmd_list(
            crate::checks::Machine {
                places: &Places::from_env()?,
                system,
            },
            args,
        ),
        Checks(checks::Tests) => checks_run::cmd_tests(
            crate::checks::Machine {
                places: &Places::from_env()?,
                system,
            },
            args,
        ),
        Waits => waits::cmd_waits(args),
        InstallHook => install_hook::cmd_install_hook(args),
        Lock => lock::cmd_lock(&Places::from_env()?, system, args),
        AppBuild(app_build::Build) => crate::app_build::cmd_app_build(&as_called(session)?, args),
        AppBuild(app_build::LandingCoverage) => {
            crate::app_landing::cmd_landing_coverage(&Places::from_env()?, args)
        }
        AppBuild(app_build::DropShots) => {
            crate::app_shots_drop::cmd_drop_shots(&as_called(session)?, args)
        }
        AppBuild(app_build::RecordHanded) => {
            crate::handed_runs::cmd_record_handed(&Places::from_env()?, args)
        }
        Shots(shots::Picture) => shots_picture::cmd(&as_called(session)?, args),
        Shots(shots::Findings) => shot_findings::cmd_findings(&Places::from_env()?, args),
        Shots(shots::CheckProgram) => shots_check_program::cmd(&Places::from_env()?, args),
        Shots(shots::Checker) => shots_program::cmd_checker(&Places::from_env()?, args),
        Web(web::Picture) => web_picture::cmd(&Places::from_env()?, system, args),
        Brief => brief::cmd_brief(&crate::review::default_rules_root()?, args),
        Ack => ack::cmd_ack(&Places::from_env()?, &session, args),
        Agent(agent::Start) => ack::agent::cmd_start(&Places::from_env()?),
        RulesOnEdit => rules_on_edit::cmd_rules_on_edit(
            &Places::from_env()?,
            &crate::review::default_rules_root()?,
        ),
        Queue => queue::cmd_queue(&Places::from_env()?, args),
        Takeover => queue::cmd_takeover(&Places::from_env()?, args),
        Help => match help::lookup(args.positional()) {
            Ok(text) => {
                output::answer(text);
                Ok(())
            }
            Err(why) => fail!("{why}"),
        },
        Setup => setup::cmd_setup(&tool(session)?, &ssh(), args),
        New => worktree::cmd_new(&tool(session)?, &ssh(), args),
        Retire => worktree::cmd_retire(&tool(session)?, args),
        Discard => discard::cmd_discard(&tool(session)?, args, &cwd()?),
        Land => land::cmd_land(&tool(session)?, args),
        Review(review) => crate::review::cmd_review(&tool(session)?, review, args),
        Judge(judge::Listen) => crate::review::judge::cmd_listen(&tool(session)?, args),
        Claim => {
            let tool = tool(session)?;
            claim::cmd_claim(&tool.places, args, tool.now)
        }
        Note => {
            let tool = tool(session)?;
            claim::cmd_note(&tool.places, args, tool.now)
        }
        Release => claim::cmd_release(&tool(session)?.places, args),
        AppCheckout(app_checkout::Add) => worktree::cmd_app_checkout(&tool(session)?, &ssh(), args),
        AppCheckout(app_checkout::Remove) => {
            worktree::cmd_app_checkout_remove(&tool(session)?, &ssh(), args)
        }
        AppCheckout(app_checkout::Reset) => {
            worktree::cmd_app_checkout_reset(&tool(session)?, &ssh(), args)
        }
        AppClone => warm::cmd_app_clone(&tool(session)?, args),
        BuildApp => build_app::cmd_build_app(&tool(session)?, args),
        Status => status::cmd_status(&mut tool(session)?, &ssh(), args),
        Prune => prune::cmd_prune(&tool(session)?, args),
        Install => install::cmd_install(&tool(session)?),
        Units(units::Prune) => crate::units::cmd_prune(&tool(session)?, args),
        Units(units::GitCheckouts) => crate::units::cmd_git_checkouts(args),
        Units(units::AfterBuild) => crate::units::cmd_after_build(&tool(session)?, args),
        Du => usage::cmd_du(args),
        Health => health::cmd_health(&tool(session)?, args),
        Lists(command) => crate::lists::cmd_lists(&tool(session)?, command, args),
        Decision(command) => crate::lists::cmd_decision(&tool(session)?, command, args),
        Design(command) => crate::lists::cmd_design(&tool(session)?, command, args),
        Question(command) => {
            crate::lists::question_command::cmd_question(&tool(session)?, command, args)
        }
        Agent(agent::Run) => agent_command::run(&tool(session)?, args, &cwd()?, out),
        Agent(agent::Resume) => agent_command::resume(&tool(session)?, args, &cwd()?, out),
        Agent(agent::List) => agent_list::list(&tool(session)?, args, &cwd()?, out),
        Agent(agent::Show) => agent_list::show(&tool(session)?, args, &cwd()?, out),
        Item(command) => crate::lists::cmd_item(&tool(session)?, &ssh(), command, args),
        Next => crate::lists::cmd_next(&tool(session)?, args),
        Area(command) => crate::lists::cmd_area(&tool(session)?, command, args),
        Dashboard(dashboard::Serve) => {
            ack::agent::clear_for_daemon();
            daemon::cmd_dashboard(&tool(session)?, args)
        }
        Dashboard(dashboard::Stage) => {
            ack::agent::clear_for_daemon();
            daemon::stage::cmd_stage(args)
        }
        Host(host::Add) => crate::host::cmd_add(&tool(session)?, &ssh(), args),
        Host(host::Setup) => crate::host::cmd_setup(&tool(session)?, &ssh(), args),
        Host(host::SetupHere) => crate::host::cmd_setup_here(&tool(session)?, args.positional()),
        Host(host::SpeedHere) => crate::host::cmd_speed_here(),
        Host(host::Band) => host_band::cmd_band(&tool(session)?, args),
        Host(host::List) => crate::host::cmd_list(&tool(session)?, &ssh()),
        Host(host::Cache) => crate::host::cmd_cache(&tool(session)?),
        Host(host::Remove) => crate::host::cmd_remove(&tool(session)?, args),
        Host(host::ClearXcode) => crate::host::cmd_clear_xcode(&tool(session)?, args),
        Remote(remote::Run) => crate::remote::cmd_run(&tool(session)?, &ssh(), args),
        Remote(remote::Step) => crate::remote::cmd_step(&tool(session)?, &ssh(), args),
        Command::Tool => built_tools::cmd_tool(&tool(session)?.places, &ssh(), args),
        Toolchain => rust_toolchain::cmd_toolchain(&Places::from_env()?, system, args),
    }
}
