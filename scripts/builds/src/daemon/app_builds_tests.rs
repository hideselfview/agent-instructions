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
        errors: Vec::new(),
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
    let lane = &lanes[0].checkouts[0];
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
    let waiting: Vec<(&str, Option<&str>)> = lanes[0]
        .waiting
        .iter()
        .map(|w| (w.branch.as_str(), w.worktree.as_deref()))
        .collect();
    assert_eq!(
        waiting,
        vec![
            ("read-shape-apps-b2", Some("side-9")),
            ("bae-main", Some("bae-main"))
        ]
    );
    assert_eq!(lanes[0].waiting[0].minutes, 1);
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
        build(&stuck)[0].checkouts[0].now,
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
    assert!(matches!(
        build(&building)[0].checkouts[0].now,
        Now::Building { .. }
    ));
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
    assert_eq!(lanes[0].checkouts[0].now, Now::Idle);
    assert_eq!(lanes[1].checkouts[0].checked_out, "android-branch");
}

/// The wait of a build as `build_for` words it, ignoring a queued build's
/// place, which its own test checks.
fn app_build(lane: &str, running: bool, how: &str) -> (String, bool, String) {
    (lane.to_string(), running, how.to_string())
}

fn flat(wait: Option<ItemWait>) -> Option<(String, bool, String)> {
    match wait? {
        ItemWait::AppBuild {
            lane, running, how, ..
        } => Some((lane, running, how)),
        other => panic!("not an app build: {other:?}"),
    }
}

fn how_of(wait: Option<ItemWait>) -> String {
    flat(wait).unwrap().2
}

fn branch_built_by(command: &str) -> Option<String> {
    let mut app = checkout("side-1", Some("mac"), Some("mac-components-shots"));
    app.activity.processes = vec![process("xcodebuild")];
    let mut held = lock("side-1", 3, "/d/bae-worktrees/side-5", &[]);
    held.holder.as_mut().unwrap().command = command.into();
    match build(&repo(vec![app], vec![held], vec![]))
        .remove(0)
        .checkouts
        .remove(0)
        .now
    {
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

#[test]
fn a_branch_with_a_running_or_queued_app_build_is_found_in_its_lane() {
    let mut held = lock("side-1", 6, "/d/bae-worktrees/side-5", &[]);
    held.holder.as_mut().unwrap().command = "builds app-build bae mac-alerts macos".into();
    let page = repo(
        vec![checkout("side-1", Some("mac"), None)],
        vec![held],
        vec![
            queued(
                "/d/bae-worktrees/side-9",
                "builds app-build bae other macos",
            ),
            queued(
                "/d/bae-worktrees/side-4",
                "builds app-build bae reads-import-count",
            ),
        ],
    );
    assert_eq!(
        flat(build_for(&page, "mac-alerts")),
        Some(app_build(
            "Mac",
            true,
            "builds in side-1 for 6 minutes, asked for macos"
        ))
    );
    assert_eq!(
        flat(build_for(&page, "reads-import-count")),
        Some(app_build(
            "Mac",
            false,
            "place 2 in the queue, waiting 1 minute, asked for every step"
        ))
    );
    assert_eq!(build_for(&page, "no-build"), None);
}

/// The mac lane's queue as the live one holds it: a ticket file named
/// PID-N per waiter and an `order` file, two locks held, the lane's builds
/// running as `app-build <repo> --lane mac <branch> <steps>...`.
#[test]
fn the_lanes_real_queue_and_command_lines_name_each_waiting_branch() {
    use crate::lane::Lane;
    use crate::queue::Queue;
    let scratch = crate::testing::Scratch::new("app-builds-real-queue");
    let dir = scratch.dir.join(Lane::Mac.queue_dir_name());
    std::fs::create_dir_all(&dir).unwrap();
    for ticket in ["10239-1", "26684-1", "29988-1", "31898-1", "59768-1"] {
        std::fs::write(dir.join(ticket), "").unwrap();
    }
    std::fs::write(
        dir.join("order"),
        "31898-1\n59768-1\n26684-1\n29988-1\n10239-1\n",
    )
    .unwrap();
    std::fs::write(
        scratch.dir.join(Lane::Mac.checkouts_file_name()),
        "side-1\nside-14\n",
    )
    .unwrap();
    let line = |branch: &str, tail: &str| {
        format!("/Users/dima/.local/bin/builds app-build bae --lane mac {branch} {tail}")
    };
    let commands = [
        (
            26684,
            line(
                "mac-alerts",
                "macos macos-tests --shots mac mac.errors.alert",
            ),
        ),
        (
            29988,
            line(
                "reads-import-count",
                "macos macos-tests --test mac PaginatedListTests --test mac ImportSelectionTests",
            ),
        ),
        (
            10239,
            line(
                "reads-storage",
                "macos macos-tests --test mac StorageManagerStoreTests",
            ),
        ),
    ];
    let waiting = Queue::in_repo_dir(&scratch.dir, Lane::Mac).waiting();
    assert_eq!(waiting, vec![26684, 29988, 10239]);
    let queue: Vec<Queued> = waiting
        .iter()
        .map(|pid| Queued {
            pid: *pid,
            minutes: 20,
            command: commands.iter().find(|(p, _)| p == pid).unwrap().1.clone(),
            started_from: None,
        })
        .collect();
    let mut held = lock("side-1", 27, "/d/bae-worktrees/side-16", &[]);
    held.holder.as_mut().unwrap().command = line("reads-auto-picks", "macos macos-tests");
    let page = repo(
        vec![checkout("side-1", Some("mac"), None)],
        vec![held],
        queue,
    );
    assert_eq!(
        flat(build_for(&page, "mac-alerts")),
        Some(app_build(
            "Mac",
            false,
            "place 1 in the queue, waiting 20 minutes, asked for macos macos-tests"
        ))
    );
    assert_eq!(
        how_of(build_for(&page, "reads-import-count")),
        "place 2 in the queue, waiting 20 minutes, asked for macos macos-tests"
    );
    assert_eq!(
        how_of(build_for(&page, "reads-storage")),
        "place 3 in the queue, waiting 20 minutes, asked for macos macos-tests"
    );
    assert_eq!(
        flat(build_for(&page, "reads-auto-picks")),
        Some(app_build(
            "Mac",
            true,
            "builds in side-1 for 27 minutes, asked for macos macos-tests"
        ))
    );
    assert_eq!(
        branch_of(&line("mac-alerts", "macos")),
        Some("mac-alerts".into())
    );
}

#[test]
fn a_queued_build_knows_its_place_the_queues_length_and_what_it_asked() {
    let page = repo(
        vec![checkout("side-1", Some("mac"), None)],
        Vec::new(),
        vec![
            queued(
                "/d/bae-worktrees/side-9",
                "builds app-build bae other macos",
            ),
            Queued {
                minutes: 8,
                ..queued(
                    "/d/bae-worktrees/side-4",
                    "builds app-build bae --lane mac mine macos macos-tests",
                )
            },
            queued("/d/bae-worktrees/side-5", "builds app-build bae third"),
        ],
    );
    let Some(ItemWait::AppBuild {
        running: false,
        queued: Some(queued),
        ..
    }) = build_for(&page, "mine")
    else {
        panic!("not a queued build");
    };
    assert_eq!((queued.place, queued.of), (2, 3));
    assert_eq!(queued.line, "Asked 8m ago to run macos macos-tests.");
    assert_eq!(queued.steps.as_deref(), Some("macos macos-tests"));
    let Some(ItemWait::AppBuild {
        queued: Some(every),
        ..
    }) = build_for(&page, "third")
    else {
        panic!("not a queued build");
    };
    assert_eq!(every.line, "Asked 1m ago to run every step.");
    assert_eq!(every.steps, None);
}

#[test]
fn a_lane_has_a_row_for_each_checkout_here_and_for_each_on_another_mac() {
    let mut remote = lock(
        "m1:mac-1",
        4,
        "/d/bae-worktrees/side-5",
        &["/usr/bin/xcodebuild -x"],
    );
    remote.holder.as_mut().unwrap().app_head = Some("abcdef0123456789".into());
    let page = repo(
        vec![
            checkout("side-14", Some("mac"), None),
            checkout("side-1", Some("mac"), None),
        ],
        vec![remote, lock("side-1", 6, "/d/bae-worktrees/side-5", &[])],
        Vec::new(),
    );
    let lanes = build(&page);
    assert_eq!(lanes.len(), 1);
    let rows: Vec<(&str, bool)> = lanes[0]
        .checkouts
        .iter()
        .map(|c| (c.checkout.as_str(), c.remote))
        .collect();
    assert_eq!(
        rows,
        vec![("side-1", false), ("side-14", false), ("m1:mac-1", true)]
    );
    let m1 = &lanes[0].checkouts[2];
    assert_eq!(m1.disk_kib, None);
    assert_eq!(m1.checked_out, "detached at abcdef0");
    assert!(matches!(&m1.now, Now::Building { programs, .. } if programs[0].name == "xcodebuild"));
    assert_eq!(lanes[0].checkouts[1].now, Now::Idle);
}
