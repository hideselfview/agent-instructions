use super::*;
use crate::host::LOCAL;
use std::cell::{Cell, RefCell};

const GIB: u64 = 1024 * 1024;

fn host(name: &str, speed: Option<f64>) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: (name != LOCAL).then(|| format!("u@{name}")),
        cores: 10,
        speed,
    }
}

fn probe(free_gib: f64, git_fails: Option<&str>) -> Probed {
    Ok(Probe {
        platform: crate::host_platform::Platform::here(),
        memory: crate::host_memory::Memory::at_rest(),
        cores: 10,
        load: [1.0; 3],
        free_kib: (free_gib * GIB as f64) as u64,
        git_fails: git_fails.map(str::to_string),
        nextest: crate::cargo_program::Has::Nothing,
        mutants: crate::cargo_program::Has::Nothing,
        node: crate::cargo_program::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
    })
}

/// A host under the free-disk floor gets pruned and probed again, and the
/// line says what the prune freed.
#[test]
fn a_host_under_the_floor_is_pruned_and_probed_again() {
    let mut hosts = [host("m1", Some(0.8))];
    let mut probes = [probe(29.7, None)];
    let pruned = RefCell::new(Vec::new());
    let lines = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|host| {
                pruned.borrow_mut().push(host.name.clone());
                Ok(())
            },
            probe: &|_| probe(50.0, None),
            measure: &|_| panic!("a host with a speed is not measured"),
            stop_idle: &|_, _| panic!("no host is under memory pressure"),
        },
    );
    assert_eq!(*pruned.borrow(), ["m1"]);
    assert_eq!(
        lines,
        ["pruned host m1, which had 29.7G free, under the floor of 30.0G: it has 50.0G free now"]
    );
    assert_eq!(probes[0].as_ref().unwrap().free_kib, 50 * GIB);
}

/// A prune that finds another prune running says so and is no failure, and a
/// prune that fails says why; neither probes again.
#[test]
fn a_prune_that_is_running_or_fails_is_said() {
    let again = Cell::new(0);
    let fixes = |said: &'static str| {
        let again = &again;
        move |hosts: &mut [Host], probes: &mut [Probed]| {
            fix_hosts(
                hosts,
                probes,
                30 * GIB,
                &Fixes {
                    prune: &|_| Err(said.to_string()),
                    probe: &|_| {
                        again.set(again.get() + 1);
                        probe(50.0, None)
                    },
                    measure: &|_| panic!("not measured"),
                    stop_idle: &|_, _| panic!("no host is under memory pressure"),
                },
            )
        }
    };
    let mut hosts = [host("m1", Some(0.8))];
    let mut probes = [probe(29.0, None)];
    assert_eq!(
        fixes("builds: prune: another builds prune is running and holds /x")(
            &mut hosts,
            &mut probes
        ),
        ["host m1 has 29.0G free, under the floor of 30.0G: already pruning"]
    );
    assert_eq!(
        fixes("ssh to u@m1 failed")(&mut hosts, &mut probes),
        ["pruning host m1 failed: ssh to u@m1 failed"]
    );
    assert_eq!(again.get(), 0);
}

/// A host with no measured speed that nothing else holds back is measured. A
/// host under the floor is pruned first, and one with failing git or no
/// answer is left for the findings, with nothing fixed or said.
#[test]
fn a_host_with_no_speed_is_measured_and_a_host_needing_judgment_is_left() {
    let mut hosts = [
        host(LOCAL, Some(1.0)),
        host("new", None),
        host("broken", None),
        host("gone", None),
    ];
    let mut probes = [
        probe(100.0, None),
        probe(100.0, None),
        probe(100.0, Some("no license")),
        Err("ssh to u@gone failed".to_string()),
    ];
    let measured = RefCell::new(Vec::new());
    let lines = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|_| panic!("no host is under the floor"),
            probe: &|_| panic!("no host was pruned"),
            stop_idle: &|_, _| panic!("no host is under memory pressure"),
            measure: &|host| {
                measured.borrow_mut().push(host.name.clone());
                Ok(0.8)
            },
        },
    );
    assert_eq!(*measured.borrow(), ["new"]);
    assert_eq!(
        lines,
        ["measured the core speed of host new: 0.80 of this Mac's"]
    );
    assert_eq!(hosts[1].speed, Some(0.8));
    assert_eq!(hosts[2].speed, None);
    assert_eq!(hosts[3].speed, None);
}

/// A host that was under the floor and is pruned above it, with no speed, is
/// measured in the same pass.
#[test]
fn a_pruned_host_with_no_speed_is_measured_after_its_prune() {
    let mut hosts = [host("m1", None)];
    let mut probes = [probe(10.0, None)];
    let lines = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|_| Ok(()),
            probe: &|_| probe(60.0, None),
            measure: &|_| Ok(1.25),
            stop_idle: &|_, _| panic!("no host is under memory pressure"),
        },
    );
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(hosts[0].speed, Some(1.25));
}

fn pressed(free_gib: f64) -> Probed {
    let mut pressed = probe(free_gib, None).unwrap();
    pressed.memory.pressure = crate::host_memory::Pressure::Warning;
    Ok(pressed)
}

/// A host under memory pressure has its idle daemons stopped and says what it
/// stopped, then is probed again so the dispatch judgment sees the result. A
/// host at normal pressure is left alone.
#[test]
fn a_host_under_memory_pressure_has_its_idle_daemons_stopped_and_is_probed_again() {
    let mut hosts = [host("m1", Some(0.8)), host("m2", Some(0.8))];
    let mut probes = [pressed(100.0), probe(100.0, None)];
    let asked = RefCell::new(Vec::new());
    let lines = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|_| panic!("no host is under the floor"),
            probe: &|_| probe(100.0, None),
            measure: &|_| panic!("not measured"),
            stop_idle: &|host, pressure| {
                asked
                    .borrow_mut()
                    .push((host.name.clone(), pressure.to_string()));
                Ok(vec!["stopped the Gradle daemon".to_string()])
            },
        },
    );
    assert_eq!(
        *asked.borrow(),
        [(
            "m1".to_string(),
            "memory pressure warning, 80% free, swap 0M used".to_string()
        )]
    );
    assert_eq!(lines, ["stopped the Gradle daemon"]);
    assert!(!probes[0].as_ref().unwrap().memory.under_pressure());
}

/// A host with no idle daemon is not probed again, and a stop that fails is
/// said.
#[test]
fn a_host_with_nothing_to_stop_is_left_and_a_failed_stop_is_said() {
    let mut hosts = [host("m1", Some(0.8))];
    let mut probes = [pressed(100.0)];
    let none = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|_| panic!("no prune"),
            probe: &|_| panic!("nothing stopped, so no probe"),
            measure: &|_| panic!("not measured"),
            stop_idle: &|_, _| Ok(Vec::new()),
        },
    );
    assert!(none.is_empty());
    let failed = fix_hosts(
        &mut hosts,
        &mut probes,
        30 * GIB,
        &Fixes {
            prune: &|_| panic!("no prune"),
            probe: &|_| panic!("nothing stopped, so no probe"),
            measure: &|_| panic!("not measured"),
            stop_idle: &|_, _| Err("ssh to u@m1 failed".to_string()),
        },
    );
    assert_eq!(
        failed,
        ["stopping the idle build daemons of host m1 failed: ssh to u@m1 failed"]
    );
}

/// The fix stops each orphan whose only work is what it prints, and leaves
/// the rest to list: one health leaves to run, one with a pid that refused
/// the signal, and one whose script could not run.
#[test]
fn stops_the_orphans_health_stops_and_lists_the_rest() {
    use crate::orphans::{Kind, Orphan};
    let rows = crate::host_scan::parse_ps(
        "300 1 501 0.0 0:01.00 40:00 S cargo test\n301 1 501 0.0 0:01.00 40:00 S builds land side-3\n302 1 501 0.0 0:01.00 40:00 S rustc x\n303 1 501 0.0 0:01.00 40:00 S git status\n304 1 501 0.0 0:01.00 40:00 S cargo check\n",
    )
    .unwrap();
    let orphan = |at: usize, stops: bool| Orphan {
        host: Host::local(),
        kind: Kind::Process {
            row: rows[at].clone(),
            tree: vec![rows[at].pid],
            stops,
        },
    };
    let ran = std::cell::RefCell::new(Vec::new());
    let stop = |_: &Host, script: &str| {
        let pid = rows
            .iter()
            .find(|row| script.contains(&format!("kill -TERM {} ", row.pid)))
            .map(|row| row.pid)
            .expect("the script signals one of the rows");
        ran.borrow_mut().push(pid);
        match pid {
            303 => Ok("303 still runs because SIGTERM failed\n".to_string()),
            302 => Ok("302 still runs after SIGTERM\n".to_string()),
            304 => Err("ssh to the host failed".to_string()),
            _ => Ok(String::new()),
        }
    };
    let OrphanStops {
        lines,
        remaining,
        stopped,
    } = stop_orphans(
        vec![
            orphan(0, true),
            orphan(1, false),
            orphan(2, true),
            orphan(3, true),
            orphan(4, true),
        ],
        &stop,
    );
    assert_eq!(*ran.borrow(), [300, 302, 303, 304]);
    assert_eq!(
        lines,
        [
            "stopped this Mac pid 300 (cargo test), 40m old: its caller was gone",
            "stopping this Mac pid 302 (rustc x), 40m old: 302 still runs after SIGTERM",
            "stopping this Mac pid 303 (git status), 40m old: 303 still runs because SIGTERM failed",
            "stopping this Mac pid 304 (cargo check), 40m old failed: ssh to the host failed",
        ]
    );
    assert_eq!(
        remaining,
        [
            orphan(1, false),
            orphan(2, true),
            orphan(3, true),
            orphan(4, true)
        ]
    );
    assert_eq!(stopped, [orphan(0, true)]);
}

mod stalled_claims {
    use super::*;
    use crate::checkout::Checkout;
    use crate::lists::fixtures::NOON;
    use crate::lists::item_fixtures::{Demo, demo, words};

    /// Health's reading hours after the claim, with the demo's clock moved
    /// past the commit times git recorded.
    fn hours_later(demo: &mut Demo, hours: i64) {
        let real = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        demo.tool.now = real + hours * 3600;
    }

    fn item(demo: &Demo, args: &[&str]) {
        crate::lists::item_words(&demo.tool, &crate::testing::no_ssh(), &words(args)).unwrap();
    }

    fn take_in(demo: &Demo, side: &std::path::Path) {
        let details = demo.brief("details.txt");
        item(
            demo,
            &[
                "add",
                "--title",
                "Do the first thing now",
                "--area",
                "Alpha",
                "--details-file",
                &details,
            ],
        );
        let brief = demo.brief("b.md");
        item(
            demo,
            &[
                "take",
                "1",
                "--worktree",
                &side.display().to_string(),
                "--branch",
                "work",
                "--brief",
                &brief,
            ],
        );
    }

    fn side(demo: &Demo) -> (std::path::PathBuf, Checkout) {
        let side = demo.add_side(1, "agent A", "work");
        let checkout = Checkout::find(&side.display().to_string()).unwrap();
        crate::setup::setup(&demo.tool, &crate::testing::no_ssh(), &checkout).unwrap();
        crate::claim::write_claim(
            &checkout,
            "agent A",
            NOON,
            Some(crate::claim::Note::Starting),
        )
        .unwrap();
        (side, checkout)
    }

    fn fix_with_scan(
        demo: &Demo,
        scan: std::result::Result<crate::host_scan::Scan, String>,
    ) -> Vec<String> {
        let tool = &demo.tool;
        let health = crate::health::health(tool).unwrap();
        let scanned = crate::health::Scanned::judge(
            &tool.places,
            vec![(crate::host::Host::local(), scan)],
            &crate::job_claim::Holders::read().unwrap(),
            tool.now,
        );
        crate::health::finish_health(
            tool,
            &crate::testing::no_ssh(),
            health,
            &mut crate::lists::store::load_all(&tool.places),
            &scanned,
            &Ok(vec![demo.store.clone()]),
            &crate::claude_sessions::running_now(&tool.places.claude_sessions()),
        )
        .unwrap()
        .1
    }

    fn fix(demo: &Demo) -> Vec<String> {
        let scan = crate::host_scan::parse(
            &format!(
                "uid={}\nnow={}\ncpu_idle=90\nps_follows=1\n",
                demo.tool.places.uid, demo.tool.now
            ),
            &[],
        )
        .unwrap();
        fix_with_scan(demo, Ok(scan))
    }

    #[test]
    fn a_failed_local_scan_keeps_the_claim_of_an_idle_worktree() {
        let mut demo = demo("stalled-scan-failed");
        let (_, checkout) = side(&demo);
        hours_later(&mut demo, 3);
        assert!(fix_with_scan(&demo, Err("process scan failed".into())).is_empty());
        assert!(has_claim(&checkout));
    }

    fn has_claim(checkout: &Checkout) -> bool {
        Claim::read(&checkout.claim_file()).unwrap().is_some()
    }

    /// A claim that waits for an item that never started, with nothing in its
    /// worktree and no change for hours, is released.
    #[test]
    fn an_empty_claim_with_no_change_for_hours_is_released() {
        let mut demo = demo("stalled-empty");
        let (_, checkout) = side(&demo);
        hours_later(&mut demo, 3);
        let lines = fix(&demo);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("released ") && lines[0].contains("had no change for"),
            "{lines:?}"
        );
        assert!(!has_claim(&checkout));
    }

    /// An item marked running in such a worktree is stopped, back in the
    /// queue, and then the claim is released.
    #[test]
    fn an_item_running_in_an_empty_idle_worktree_is_stopped_and_the_claim_released() {
        let mut demo = demo("stalled-item");
        let (side, checkout) = side(&demo);
        take_in(&demo, &side);
        hours_later(&mut demo, 3);
        let lines = fix(&demo);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("stopped item 1, marked running in "),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("released "), "{lines:?}");
        assert!(!has_claim(&checkout));
        let lists = demo.store.load().unwrap();
        assert!(lists.items[0].running.is_empty());
        assert!(lists.items[0].kept.is_empty());
    }

    /// A worktree with a commit origin/main lacks keeps its claim and its
    /// running item.
    #[test]
    fn a_worktree_with_unlanded_work_keeps_its_claim_and_item() {
        let mut demo = demo("stalled-work");
        let (side, checkout) = side(&demo);
        take_in(&demo, &side);
        demo.commit(&side, "unlanded");
        hours_later(&mut demo, 3);
        assert!(fix(&demo).is_empty());
        assert!(has_claim(&checkout));
        assert_eq!(demo.store.load().unwrap().items[0].running.len(), 1);
    }

    #[test]
    fn a_recent_build_keeps_the_claim_of_an_unchanged_worktree() {
        use crate::app_timing::{RunRecord, record};
        let mut demo = demo("stalled-built");
        let (side, checkout) = side(&demo);
        take_in(&demo, &side);
        hours_later(&mut demo, 3);
        record(
            &demo.store.database(),
            &RunRecord {
                started_at: demo.tool.now - 120,
                repo: "demo".into(),
                lane: crate::lane::Lane::MAC,
                checkout: "mac-1".into(),
                built_ref: "work".into(),
                requested_commit: None,
                built_commit: None,
                who: Some("agent A".into()),
                wait_ms: 0,
                outcome: crate::run_end::RunEnd::Passed,
                load: None,
                steps: Vec::new(),
            },
        )
        .unwrap();
        assert!(fix(&demo).is_empty());
        assert!(has_claim(&checkout));
    }

    /// A claim younger than the setting's hours stays, whatever its worktree
    /// holds.
    #[test]
    fn a_claim_idle_for_less_than_the_hours_stays() {
        let mut demo = demo("stalled-young");
        let (side, checkout) = side(&demo);
        take_in(&demo, &side);
        hours_later(&mut demo, 1);
        crate::claim::write_claim(&checkout, "agent A", demo.tool.now - 600, None).unwrap();
        assert!(fix(&demo).is_empty());
        assert!(has_claim(&checkout));
        assert_eq!(demo.store.load().unwrap().items[0].running.len(), 1);
    }
}

/// Health releases a claim only while it is the claim it checked.
#[test]
fn a_claim_another_command_replaced_is_not_released() {
    let scratch = crate::testing::Scratch::new("health-replaced-claim");
    let top = scratch.dir.join("demo-worktrees/side-1");
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\n");
    std::fs::create_dir_all(top.parent().unwrap()).unwrap();
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "work",
            &top.display().to_string(),
            "main",
        ],
    );
    let checkout = crate::checkout::Checkout::find(&top.display().to_string()).unwrap();
    let checked = Claim {
        who: "agent".into(),
        since: 1_790_000_040,
        note: None,
    };
    let replaced = Claim {
        who: "another agent".into(),
        since: 1_790_000_100,
        note: None,
    };
    replaced.write(&checkout.claim_file()).unwrap();

    assert!(
        crate::claim::forget_claimed(&checkout, &checked)
            .unwrap()
            .is_none()
    );
    assert_eq!(Claim::read(&checkout.claim_file()).unwrap(), Some(replaced));
    assert!(
        crate::claim::forget_claimed(
            &checkout,
            &Claim::read(&checkout.claim_file()).unwrap().unwrap()
        )
        .unwrap()
        .is_some()
    );
}
