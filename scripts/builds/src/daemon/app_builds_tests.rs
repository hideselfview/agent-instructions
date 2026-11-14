use super::*;
use crate::claim::Claim;
use crate::daemon::model::{Activity, ProcessPage};
use crate::health::{Holder, LaneQueue, LockHealth, Queued};
use crate::side_git::{Commit, SideGit};

fn checkout(name: &str, lane: Option<&'static str>, branch: Option<&str>) -> CheckoutPage {
    CheckoutPage {
        name: name.into(),
        path: format!("/d/bae-worktrees/{name}"),
        root: format!("/v/bae/{name}"),
        live: true,
        main: false,
        branch: branch.map(str::to_string),
        app_checkout: lane.is_some(),
        app_lane: lane,
        claim: None,
        git: Some(SideGit {
            head: Some(Commit {
                sha: "9e8d3d5".into(),
                full: "9e8d3d5".into(),
                time: 0,
                authored: 0,
                subject: String::new(),
            }),
            ahead: Vec::new(),
            uncommitted: Some(0),
            last_write: None,
            error: None,
        }),
        state: Some("free"),
        folders: Some(Vec::new()),
        total_kib: Some(18_000_000),
        activity: Activity::default(),
        words: Default::default(),
    }
}

fn claimed(who: &str) -> Claim {
    Claim {
        who: who.into(),
        since: "2026-11-14 07:00".into(),
        note: None,
        note_state: None,
    }
}

fn lock(checkout: &str, minutes: i64, from: &str, running: &[&str]) -> LockHealth {
    LockHealth {
        repo: "bae".into(),
        lane: "mac".into(),
        checkout: checkout.into(),
        path: "/v/bae/app.locks/x".into(),
        holder: Some(Holder {
            pid: 1,
            minutes,
            command: "builds lock".into(),
            running: running.iter().map(|r| r.to_string()).collect(),
            started_from: Some(from.into()),
            app_head: None,
            driver_running: true,
        }),
    }
}

fn process(name: &str) -> ProcessPage {
    ProcessPage {
        pid: 1,
        started: 0,
        name: name.into(),
        cpu: Some(1.0),
        memory: 100,
    }
}

fn repo(checkouts: Vec<CheckoutPage>, locks: Vec<LockHealth>, queue: Vec<Queued>) -> RepoPage {
    RepoPage {
        name: "bae".into(),
        lanes: Vec::new(),
        locks,
        queues: vec![LaneQueue {
            repo: "bae".into(),
            lane: "mac".into(),
            queue,
        }],
        runs: Vec::new(),
        app_builds: Vec::new(),
        checkouts,
    }
}

fn queued(from: &str, command: &str) -> Queued {
    Queued {
        pid: 2,
        minutes: 1,
        command: command.into(),
        started_from: Some(from.into()),
    }
}

#[test]
fn a_lane_with_a_running_build_says_whose_it_is_and_what_runs() {
    let mut app = checkout("side-1", Some("mac"), None);
    app.activity = Activity {
        processes: vec![
            process("xcodebuild"),
            process("swift-frontend"),
            process("swift-frontend"),
        ],
        cpu: 210.0,
        memory: 6_000,
        started: Some(0),
    };
    let mut agent = checkout("side-5", None, Some("mac-alerts-branch"));
    agent.claim = Some(claimed("mac-alerts"));
    let mut waiting = checkout("side-9", None, Some("read-shape-apps-b2"));
    waiting.claim = Some(claimed("read-shape"));
    let page = repo(
        vec![app, agent, waiting],
        vec![lock("side-1", 6, "/d/bae-worktrees/side-5", &[])],
        vec![
            queued("/d/bae-worktrees/side-9", "builds app-build"),
            queued("/d/bae-main", "builds build-app bae mac"),
        ],
    );
    let lanes = build(&page);
    assert_eq!(lanes.len(), 1);
    let lane = &lanes[0];
    assert_eq!(lane.checked_out, "detached at 9e8d3d5");
    assert_eq!(lane.disk_kib, Some(18_000_000));
    assert_eq!(
        lane.now,
        Now::Building {
            who: "mac-alerts".into(),
            branch: None,
            minutes: 6,
            programs: vec![
                Program {
                    name: "xcodebuild".into(),
                    count: 1
                },
                Program {
                    name: "swift-frontend".into(),
                    count: 2
                },
            ],
            cpu: 210.0,
            memory: 6_000,
        }
    );
    assert_eq!(
        lane.waiting,
        vec!["read-shape-apps-b2".to_string(), "bae-main".to_string()]
    );
}

#[test]
fn a_lock_only_leftover_processes_hold_is_stuck_and_a_running_build_is_not() {
    let mut leftover = lock(
        "side-13",
        37 * 60,
        "/d/bae-worktrees/side-2",
        &["/usr/bin/adb -L tcp:5037"],
    );
    leftover.holder.as_mut().unwrap().driver_running = false;
    let stuck = repo(
        vec![checkout("side-13", Some("mac"), None)],
        vec![leftover],
        Vec::new(),
    );
    assert_eq!(
        build(&stuck)[0].now,
        Now::Stuck {
            minutes: 37 * 60,
            running: vec!["adb -L tcp:5037".into()]
        }
    );
    // A build that holds the lock with its driver running is building, even
    // when no build program has started yet and however long it has held it.
    let building = repo(
        vec![checkout("side-13", Some("mac"), None)],
        vec![lock("side-13", 37 * 60, "/d/bae-worktrees/side-2", &[])],
        Vec::new(),
    );
    assert!(matches!(build(&building)[0].now, Now::Building { .. }));
}

#[test]
fn a_free_lock_is_idle_and_the_mac_lane_comes_first() {
    let mut android = checkout("side-13", Some("android"), Some("android-branch"));
    android.git = None;
    let mac = checkout("side-1", Some("mac"), None);
    let page = repo(vec![android, mac], Vec::new(), Vec::new());
    let lanes = build(&page);
    assert_eq!(
        lanes.iter().map(|l| l.lane.as_str()).collect::<Vec<_>>(),
        vec!["mac", "android"]
    );
    assert_eq!(lanes[0].now, Now::Idle);
    assert_eq!(lanes[1].checked_out, "android-branch");
}

fn branch_built_by(command: &str) -> Option<String> {
    let mut app = checkout("side-1", Some("mac"), Some("mac-components-shots"));
    app.activity.processes = vec![process("xcodebuild")];
    let mut held = lock("side-1", 3, "/d/bae-worktrees/side-5", &[]);
    held.holder.as_mut().unwrap().command = command.into();
    match build(&repo(vec![app], vec![held], vec![])).remove(0).now {
        Now::Building { branch, .. } => branch,
        other => panic!("not building: {other:?}"),
    }
}

#[test]
fn a_lane_names_the_branch_of_the_build_that_runs_not_the_one_checked_out_before() {
    assert_eq!(
        branch_built_by("/Users/dima/.local/bin/builds app-build bae reads-import-count macos"),
        Some("reads-import-count".into())
    );
    assert_eq!(
        branch_built_by("builds app-build bae reads-import-count --test mac Foo"),
        Some("reads-import-count".into())
    );
    assert_eq!(
        branch_built_by("builds build-app bae mac v1.2"),
        Some("v1.2".into())
    );
    assert_eq!(
        branch_built_by("builds build-app bae mac"),
        Some("origin/main".into())
    );
    assert_eq!(branch_built_by("builds lock bae"), None);
}
