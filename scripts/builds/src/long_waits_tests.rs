use super::*;
use crate::install_gate::Held;

const NOW: i64 = 1_795_069_061;

fn rows(text: &str) -> Vec<Row> {
    crate::stuck::parse_ps(text).unwrap()
}

fn scan(rows: Vec<Row>, jobs: Vec<JobRun>) -> std::result::Result<Scan, String> {
    Ok(Scan {
        uid: 501,
        rows,
        cpu_idle: Ok(50.0),
        jobs,
        sccache: Ok(None),
    })
}

fn m1() -> Host {
    Host {
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

/// A job in <worktree> on a host that the `builds` process <caller> on this
/// Mac started <minutes> ago, whose group leader there is <leader>.
fn job(worktree: &str, leader: i32, caller: i32, minutes: i64) -> JobRun {
    JobRun {
        worktree: format!("/Users/u/dev/bae-worktrees/{worktree}"),
        leader,
        run: format!("{caller}-{}", (NOW - minutes * 60) as u128 * 1_000_000_000),
    }
}

/// This Mac during the jam: an install that has waited 20 minutes for a hook
/// run, whose clippy runs as a job on m1 that has run 36 minutes and waits
/// on 3 compiles through sccache.
fn jam() -> Vec<(Host, std::result::Result<Scan, String>)> {
    let here = rows(
        "100 1 501 0.0 0:01.00 20:00 /Users/x/.local/bin/builds install\n\
         200 150 501 0.0 0:01.00 22:00 builds checks run\n\
         210 200 501 0.0 0:01.00 21:30 builds cargo clippy --all-targets\n\
         211 210 501 0.0 0:01.00 21:30 ssh -o BatchMode=yes u@m1 sh -c x\n",
    );
    let there = rows(
        "5001 1 502 0.0 0:01.00 36:00 builds cargo clippy --all-targets\n\
         5002 5001 502 0.0 0:01.00 36:00 cargo clippy --all-targets\n\
         5003 5002 502 0.0 0:01.00 01:00 sccache rustc --crate-name a\n\
         5004 5002 502 0.0 0:01.00 01:00 sccache rustc --crate-name b\n\
         5005 5002 502 0.0 0:01.00 01:00 sccache rustc --crate-name c\n",
    );
    vec![
        (Host::local(), scan(here, Vec::new())),
        (m1(), scan(there, vec![job("remote-side-3", 5001, 210, 36)])),
    ]
}

fn side_3(pid: i32) -> Option<String> {
    (pid == 200 || pid == 210).then(|| "bae/side-3".to_string())
}

fn lines(
    scans: &[(Host, std::result::Result<Scan, String>)],
    install: std::result::Result<Option<Waiting>, String>,
) -> Vec<String> {
    findings(scans, &install, &side_3, NOW)
        .iter()
        .map(Advice::line)
        .collect()
}

fn waiting_for_the_hook_run() -> Waiting {
    Waiting {
        commands_closed: true,
        connections_closed: false,
        held: vec![Held {
            pid: 200,
            text: "process 200, builds checks run, since 01:00:00".to_string(),
        }],
    }
}

/// An install that waits for a hook run names the hook run, what the hook run
/// waits on down to its job on a host, and which commands wait behind the
/// install; the hook run and its job get no lines of their own.
#[test]
fn an_install_names_what_it_waits_for_down_to_the_job_on_a_host() {
    assert_eq!(
        lines(&jam(), Ok(Some(waiting_for_the_hook_run()))),
        [
            "install: builds install (pid 100) has run 20m and waits for process 200, builds checks run, since 01:00:00 (it waits on its job on host m1 in bae/remote-side-3, which has run 36m: 3 compiles through sccache); every hook run, review and app build lane that starts waits for it"
        ]
    );
    let mut stores = waiting_for_the_hook_run();
    stores.connections_closed = true;
    assert!(
        lines(&jam(), Ok(Some(stores)))[0]
            .ends_with("; every builds command that opens a store waits for it")
    );
}

/// With no install waiting, the hook run is past its limit and names its
/// job; the job, which the hook run's line names, gets no line of its own.
#[test]
fn a_long_hook_run_names_the_job_it_waits_on() {
    assert_eq!(
        lines(&jam(), Ok(None)),
        [
            "hook run: builds checks run (pid 200, in bae/side-3) has run 22m, longer than 10m; it waits on its job on host m1 in bae/remote-side-3, which has run 36m: 3 compiles through sccache"
        ]
    );
}

/// A hook run whose builds cargo runs on this Mac names that command.
#[test]
fn a_long_hook_run_names_the_builds_command_under_it() {
    let here = rows(
        "200 150 501 0.0 0:01.00 22:00 builds checks run\n\
         210 200 501 0.0 0:01.00 05:00 builds cargo clippy\n\
         212 210 501 0.0 0:01.00 05:00 cargo clippy\n",
    );
    assert_eq!(
        lines(&[(Host::local(), scan(here, Vec::new()))], Ok(None)),
        [
            "hook run: builds checks run (pid 200, in bae/side-3) has run 22m, longer than 10m; it waits on builds cargo clippy (pid 210, in bae/side-3)"
        ]
    );
}

/// A cargo job on a host past its limit gets a line of its own with its
/// caller here and what runs under it. A job in an app checkout, a short
/// one, and one whose caller is gone (orphans.rs lists it) get none.
#[test]
fn a_long_cargo_job_on_a_host_names_its_caller_and_what_runs_under_it() {
    let here = rows("210 150 501 0.0 0:01.00 01:00 builds cargo test\n");
    let there = rows(
        "6000 1 502 0.0 0:01.00 36:00 cargo test\n\
         6001 6000 502 0.0 0:01.00 31:00 /Users/u/dev/bae-worktrees/remote-side-5/target/debug/deps/bae_views-0123456789abcdef\n\
         7000 1 502 0.0 0:01.00 50:00 xcodebuild test\n",
    );
    let jobs = vec![
        job("remote-side-5", 6000, 210, 36),
        job("remote-side-6", 6000, 210, 12),
        job("remote-side-7", 6000, 999, 50),
        job("mac-1", 7000, 210, 50),
    ];
    assert_eq!(
        lines(
            &[
                (Host::local(), scan(here, Vec::new())),
                (m1(), scan(there, jobs)),
            ],
            Ok(None)
        ),
        [
            "job: host m1 has run a job in bae/remote-side-5 for 36m, longer than 30m, for builds cargo test (pid 210, in bae/side-3) on this Mac: test binary bae_views-0123456789abcdef (pid 6001, 31m)"
        ]
    );
}

/// An install that has just closed its gates is no finding yet; one whose
/// gates health could not read is.
#[test]
fn a_new_install_is_no_finding_and_an_unread_one_is() {
    let here = rows("100 1 501 0.0 0:01.00 00:30 builds install\n");
    let scans = [(Host::local(), scan(here, Vec::new()))];
    assert!(lines(&scans, Ok(Some(waiting_for_the_hook_run()))).is_empty());
    assert_eq!(
        lines(&scans, Err("opening install.lock: denied".to_string())),
        [
            "install: health could not read whether a builds install waits: opening install.lock: denied"
        ]
    );
}
