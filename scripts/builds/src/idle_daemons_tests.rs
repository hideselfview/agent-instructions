use super::*;
use std::cell::RefCell;

const NOW: i64 = 1_000_000;
const HOUR: i64 = 3600;

/// What a host's listing printed: Gradle and Kotlin daemons idle for hours,
/// a Gradle daemon that works, another user's Gradle daemon, a Gradle client
/// that is no daemon, a booted simulator device and an unrelated process.
const LISTING: &str = "uid=501
 4101     1   501   0.0   0:41.20 06:30:12 Ss /Library/Java/jdk/bin/java -Xmx512m -cp gradle-launcher.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.10
 4102     1   501   0.0   2:10.00 15:02:44 Ss /Library/Java/jdk/bin/java -Xmx1g -cp kotlin-daemon.jar org.jetbrains.kotlin.daemon.KotlinCompileDaemon --daemon-runFilesPath /tmp
 4103     1   501  88.0  30:10.00 01:00:00 Ss /Library/Java/jdk/bin/java -cp gradle-launcher.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.10
 4104     1   502   0.0   0:10.00 09:00:00 Ss /Library/Java/jdk/bin/java -cp gradle-launcher.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.10
 4105  3000   501   0.0   0:03.00 00:30 Ss /Library/Java/jdk/bin/java -cp gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain test
 4106     1   501   0.0   0:09.00 03:00:00 Ss /Users/u/Library/Developer/CoreSimulator/Devices/ABC/data/launchd_sim
 4107     1   501   0.0   0:08.00 03:00:00 Ss /Users/u/Library/Developer/CoreSimulator/Devices/DEF/data/launchd_sim
 4108     1   501   0.0   0:01.00 03:00:00 Ss /usr/sbin/cfprefsd agent
";

fn found() -> Vec<Daemon> {
    let (uid, rows) = read_listing(LISTING).unwrap();
    assert_eq!(uid, 501);
    daemons(&rows, uid)
}

fn pids(idle: &[Idle]) -> Vec<i32> {
    idle.iter().map(|found| found.daemon.pid).collect()
}

/// Only this user's idle daemons are found: not a busy one, another user's,
/// a Gradle client or any other process.
#[test]
fn finds_this_users_idle_daemons_in_a_listing() {
    let kinds: Vec<_> = found().iter().map(|d| (d.pid, d.kind)).collect();
    assert_eq!(
        kinds,
        [
            (4101, Kind::Gradle),
            (4102, Kind::KotlinCompile),
            (4106, Kind::Simulator),
            (4107, Kind::Simulator)
        ]
    );
}

/// A daemon is idle when its lane's last build ended an hour or more ago,
/// whatever the daemon's age; the lane's builds judge each kind.
#[test]
fn a_daemon_is_idle_by_its_lanes_last_build() {
    let use_of = |lane| match lane {
        Lane::Android => LaneUse::LastBuild(Some(NOW - 7 * HOUR)),
        Lane::Mac => LaneUse::LastBuild(Some(NOW - 10 * 60)),
    };
    let idle = judge(found(), &use_of, NOW);
    assert_eq!(pids(&idle), [4101, 4102]);
    assert!(idle.iter().all(|found| found.recorded));
    // The Gradle daemon started 6.5 hours ago, after the build ended 7 hours
    // ago, so it has run unused for its whole life.
    assert_eq!(idle[0].seconds, 6 * HOUR + 30 * 60 + 12);
    assert_eq!(idle[1].seconds, 7 * HOUR);
}

/// A build that runs on the host keeps that lane's daemons, and one that
/// ended less than an hour ago does too.
#[test]
fn a_running_or_recent_build_keeps_its_lanes_daemons() {
    let building = |lane| match lane {
        Lane::Android => LaneUse::Building,
        Lane::Mac => LaneUse::LastBuild(Some(NOW - 2 * HOUR)),
    };
    assert_eq!(pids(&judge(found(), &building, NOW)), [4106, 4107]);
    let recent = |_| LaneUse::LastBuild(Some(NOW - 59 * 60));
    assert!(judge(found(), &recent, NOW).is_empty());
}

/// With no build on record, the daemon's own age decides.
#[test]
fn a_host_with_no_build_on_record_judges_by_the_daemons_age() {
    let none = |_| LaneUse::LastBuild(None);
    let idle = judge(found(), &none, NOW);
    assert_eq!(pids(&idle), [4101, 4102, 4106, 4107]);
    assert!(idle.iter().all(|found| !found.recorded));
    let young = vec![Daemon {
        pid: 9,
        kind: Kind::Gradle,
        age_seconds: 30 * 60,
    }];
    assert!(judge(young, &none, NOW).is_empty());
}

/// Each stop says what it stopped and why, a failed stop says why, and one
/// command stops all the simulators.
#[test]
fn stops_say_what_they_stopped_and_shut_down_simulators_once() {
    let use_of = |_| LaneUse::LastBuild(Some(NOW - 7 * HOUR));
    let idle = judge(found(), &use_of, NOW);
    let ran = RefCell::new(Vec::new());
    let lines = stop(
        &idle,
        "host m1",
        "memory pressure warning, 6% free, swap 7500M used",
        &|action| {
            ran.borrow_mut().push(action);
            if action == Stop::Kill(4102) {
                Err("no such process".to_string())
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(
        *ran.borrow(),
        [Stop::Kill(4101), Stop::Kill(4102), Stop::ShutdownSimulators]
    );
    assert_eq!(
        lines,
        [
            "host m1 is under memory pressure warning, 6% free, swap 7500M used: stopped the Gradle daemon (pid 4101) because no android lane build used host m1 for 6h30m",
            "host m1 is under memory pressure warning, 6% free, swap 7500M used: stopping the Kotlin compile daemon (pid 4102) failed: no such process",
            "host m1 is under memory pressure warning, 6% free, swap 7500M used: stopped the simulator (pid 4106) because no mac lane build used host m1 for 3h00m",
        ]
    );
}

#[test]
fn a_listing_without_a_user_line_is_an_error() {
    assert!(read_listing(" 1 0 0 0.0 0:00.00 00:01 /sbin/launchd\n").is_err());
}

/// The lane's use of a host reads the locks and the end records of the
/// checkouts on that host only, and a held lock means a build runs there.
#[test]
fn a_lane_is_in_use_on_the_host_whose_checkout_a_build_holds() {
    let scratch = crate::testing::Scratch::new("idle-daemons-lane-use");
    let places = scratch.places();
    std::fs::create_dir_all(places.app_locks_dir("demo", Lane::Android)).unwrap();
    std::fs::write(
        places.app_checkouts_file("demo", Lane::Android),
        "m1:builds-android-1\nside-1\n",
    )
    .unwrap();
    std::fs::write(
        places.app_lock_ended_file("demo", Lane::Android, "m1:builds-android-1"),
        "5000\n",
    )
    .unwrap();
    let ssh = Ssh::from_env();
    let host = |name: &str, remote: bool| Host {
        band: None,
        name: name.to_string(),
        target: remote.then(|| format!("u@{name}")),
        cores: 10,
        speed: None,
    };
    let m1 = host("m1", true);
    let here = host(crate::host::LOCAL, false);
    let other = host("m2", true);
    let android = |host: &Host| lane_use(&places, &ssh, host, Lane::Android).unwrap();
    assert_eq!(android(&m1), LaneUse::LastBuild(Some(5000)));
    assert_eq!(android(&here), LaneUse::LastBuild(None));
    assert_eq!(android(&other), LaneUse::LastBuild(None));
    assert_eq!(
        lane_use(&places, &ssh, &m1, Lane::Mac).unwrap(),
        LaneUse::LastBuild(None)
    );
    let held =
        crate::lane_size::lock_checkout(&places, "demo", Lane::Android, "m1:builds-android-1")
            .unwrap()
            .expect("nothing holds the lock");
    assert_eq!(android(&m1), LaneUse::Building);
    assert_eq!(android(&here), LaneUse::LastBuild(None));
    drop(held);
}

/// A repo that two projects hold (agent-instructions is in bae and builds)
/// has app build runs in each project's store. The lane's use reads both:
/// a held lock keeps the lane busy, a run in either project is the lane's
/// last build, and the lane is idle when only old runs exist.
#[test]
fn a_repo_two_projects_hold_is_read_in_both_projects() {
    use crate::app_timing::{DATABASE, RunRecord, record};
    use crate::run_end::RunEnd;
    let scratch = crate::testing::Scratch::new("idle-daemons-shared-repo");
    let places = scratch.places();
    let first = crate::lists::fixtures::make_store_named(&places, "first", &["shared"]);
    let second = crate::lists::fixtures::make_store_named(&places, "second", &["shared"]);
    let entry = "m1:builds-android-1";
    std::fs::create_dir_all(places.app_locks_dir("shared", Lane::Android)).unwrap();
    std::fs::write(
        places.app_checkouts_file("shared", Lane::Android),
        format!("{entry}\n"),
    )
    .unwrap();
    let ssh = Ssh::from_env();
    let m1 = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: None,
    };
    let ran = |at: i64| RunRecord {
        started_at: at,
        repo: "shared".to_string(),
        lane: Lane::Android,
        checkout: entry.to_string(),
        built_ref: "branch".to_string(),
        requested_commit: None,
        built_commit: None,
        who: None,
        wait_ms: 1000,
        outcome: RunEnd::Passed,
        load: None,
        steps: Vec::new(),
    };
    let android = || lane_use(&places, &ssh, &m1, Lane::Android).unwrap();

    record(&first.dir.join(DATABASE), &ran(1000)).unwrap();
    assert_eq!(android(), LaneUse::LastBuild(Some(1001)));
    record(&second.dir.join(DATABASE), &ran(2000)).unwrap();
    assert_eq!(android(), LaneUse::LastBuild(Some(2001)));
    record(&first.dir.join(DATABASE), &ran(3000)).unwrap();
    assert_eq!(android(), LaneUse::LastBuild(Some(3001)));

    let held = crate::lane_size::lock_checkout(&places, "shared", Lane::Android, entry)
        .unwrap()
        .expect("nothing holds the lock");
    assert_eq!(android(), LaneUse::Building);
    drop(held);
}
