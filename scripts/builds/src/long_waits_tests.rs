use super::*;
use crate::install_gate::Held;

const NOW: i64 = 1_795_069_061;

fn rows(text: &str) -> Vec<Row> {
    crate::stuck::parse_ps(text).unwrap()
}

fn scan(rows: Vec<Row>, jobs: Vec<JobRun>) -> std::result::Result<Scan, String> {
    Ok(Scan {
        uid: 501,
        at: 0,
        rows,
        cpu_idle: Ok(50.0),
        jobs,
        sccache: Ok(None),
        front_app: Ok(None),
        screen_locked: Ok(None),
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

/// This Mac and m1 while an install waits behind a hook run: the install
/// has run 20 minutes; the hook run's clippy runs as a job on m1 that has run
/// 36 minutes and waits on 3 compiles through sccache.
fn install_behind_a_hook_run() -> Vec<(Host, std::result::Result<Scan, String>)> {
    let here = rows(
        "100 1 501 0.0 0:01.00 20:00 S /Users/x/.local/bin/builds install\n\
         200 150 501 0.0 0:01.00 22:00 S builds checks run\n\
         210 200 501 0.0 0:01.00 21:30 S builds cargo clippy --all-targets\n\
         211 210 501 0.0 0:01.00 21:30 S ssh -o BatchMode=yes u@m1 sh -c x\n",
    );
    let there = rows(
        "5001 1 502 0.0 0:01.00 36:00 S builds cargo clippy --all-targets\n\
         5002 5001 502 0.0 0:01.00 36:00 S cargo clippy --all-targets\n\
         5003 5002 502 0.0 0:01.00 01:00 S sccache rustc --crate-name a\n\
         5004 5002 502 0.0 0:01.00 01:00 S sccache rustc --crate-name b\n\
         5005 5002 502 0.0 0:01.00 01:00 S sccache rustc --crate-name c\n",
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
    findings(scans, &install, &side_3, &|_| None, NOW)
        .iter()
        .map(Advice::line)
        .collect()
}

fn waiting_for_the_hook_run() -> Waiting {
    Waiting {
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
        lines(
            &install_behind_a_hook_run(),
            Ok(Some(waiting_for_the_hook_run()))
        ),
        [
            "install: builds install (pid 100) has run 20m and waits for process 200, builds checks run, since 01:00:00 (it waits on its job on host m1 in bae/remote-side-3, which has run 36m: 3 compiles through sccache); every hook run, review and landing that starts waits for it"
        ]
    );
    let mut stores = waiting_for_the_hook_run();
    stores.connections_closed = true;
    assert!(
        lines(&install_behind_a_hook_run(), Ok(Some(stores)))[0]
            .ends_with("; every builds command that opens a store waits for it")
    );
}

/// With no install waiting, the hook run is past its limit and names its
/// job; the job, which the hook run's line names, gets no line of its own.
#[test]
fn a_long_hook_run_names_the_job_it_waits_on() {
    assert_eq!(
        lines(&install_behind_a_hook_run(), Ok(None)),
        [
            "hook run: builds checks run (pid 200, in bae/side-3) has run 22m, longer than 10m; it waits on its job on host m1 in bae/remote-side-3, which has run 36m: 3 compiles through sccache"
        ]
    );
}

/// A hook run whose builds cargo runs on this Mac names that command.
#[test]
fn a_long_hook_run_names_the_builds_command_under_it() {
    let here = rows(
        "200 150 501 0.0 0:01.00 22:00 S builds checks run\n\
         210 200 501 0.0 0:01.00 05:00 S builds cargo clippy\n\
         212 210 501 0.0 0:01.00 05:00 S cargo clippy\n",
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
    let here = rows("210 150 501 0.0 0:01.00 01:00 S builds cargo test\n");
    let there = rows(
        "6000 1 502 0.0 0:01.00 36:00 S cargo test\n\
         6001 6000 502 0.0 0:01.00 31:00 S /Users/u/dev/bae-worktrees/remote-side-5/target/debug/deps/bae_views-0123456789abcdef\n\
         7000 1 502 0.0 0:01.00 50:00 S xcodebuild test\n",
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
    let here = rows("100 1 501 0.0 0:01.00 00:30 S builds install\n");
    let scans = [(Host::local(), scan(here, Vec::new()))];
    assert!(lines(&scans, Ok(Some(waiting_for_the_hook_run()))).is_empty());
    assert_eq!(
        lines(&scans, Err("opening install.lock: denied".to_string())),
        [
            "install: health could not read whether a builds install waits: opening install.lock: denied"
        ]
    );
}

/// A scan of this Mac that failed is a finding, and the install line stays,
/// with what it waits for as the gate's files say it; a job on a host whose
/// caller health cannot check here names the caller's pid.
#[test]
fn a_failed_scan_of_this_mac_is_a_finding_and_keeps_the_install_line() {
    let mut scans = install_behind_a_hook_run();
    scans[0].1 = Err("ps on this Mac failed (exit 1)".to_string());
    assert_eq!(
        lines(&scans, Ok(Some(waiting_for_the_hook_run()))),
        [
            "waits: health could not read this Mac's processes, so it cannot tell how long the hook runs, landings and cargo runs here have run, or what each waits on: ps on this Mac failed (exit 1)",
            "install: a builds install runs and waits for process 200, builds checks run, since 01:00:00 (health could not read what it waits on); every hook run, review and landing that starts waits for it",
            "job: host m1 has run a job in bae/remote-side-3 for 36m, longer than 30m, for process 210 on this Mac: 3 compiles through sccache",
        ]
    );
}

/// A job whose run id is not `<pid>-<nanoseconds>` is a finding that names
/// it, whatever its worktree.
#[test]
fn a_job_whose_run_id_does_not_read_is_a_finding() {
    let mut scans = install_behind_a_hook_run();
    let Ok(m1) = &mut scans[1].1 else {
        panic!("m1's scan reads");
    };
    m1.jobs = vec![JobRun {
        worktree: "/Users/u/dev/bae-worktrees/mac-1".to_string(),
        leader: 7000,
        run: "step-7".to_string(),
    }];
    assert_eq!(
        lines(&scans, Ok(None)),
        [
            "hook run: builds checks run (pid 200, in bae/side-3) has run 22m, longer than 10m; it waits on builds cargo clippy --all-targets (pid 210, in bae/side-3)",
            "job: host m1 runs a job in bae/mac-1 whose run id \"step-7\" is not <pid>-<nanoseconds>, so health cannot tell which builds command here runs it or how long it has run",
        ]
    );
}

/// A landing past its limit that waits for the apps turn names the landing
/// that holds it, with its worktree and how long it has run, instead of
/// saying that nothing runs under it.
#[test]
fn a_long_landing_names_the_landing_that_holds_its_turn() {
    let here = rows(
        "300 1 501 0.0 0:01.00 31:00 S builds --agent a1 land /w/side-8 --done 1322 abc\n\
         400 1 501 0.0 0:01.00 40:00 S builds land . --done 1253 def\n",
    );
    let checkout = |pid: i32| match pid {
        300 => Some("bae/side-8".to_string()),
        400 => Some("bae/side-13".to_string()),
        _ => None,
    };
    let waits = |pid: i32| {
        (pid == 300).then(|| {
            "the landing that changes the apps, process 400 landing --done 1253 def".to_string()
        })
    };
    let lines: Vec<String> = findings(
        &[(Host::local(), scan(here, Vec::new()))],
        &Ok(None),
        &checkout,
        &waits,
        NOW,
    )
    .iter()
    .map(Advice::line)
    .collect();
    assert_eq!(
        lines[1],
        "landing: builds land /w/side-8 --done 1322 abc (pid 300, in bae/side-8) has run 31m, longer than 30m; it waits for the landing that changes the apps: builds land . --done 1253 def (pid 400, in bae/side-13), which has run 40m"
    );
}
