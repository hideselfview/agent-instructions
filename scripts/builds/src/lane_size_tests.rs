use super::*;
use crate::lock::{Where, take_where};
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::channel;
use std::time::Duration;

/// A host m1 whose answer to builds' question about its cores, load and disk
/// is the file `probe` in the scratch folder, and whose other commands run in
/// the folder `host-home` as its home, with no build volume.
struct World {
    scratch: Scratch,
    places: Places,
    ssh: Ssh,
}

impl World {
    fn new(name: &str, android: &str) -> World {
        let scratch = Scratch::new(name);
        let places = scratch.places();
        std::fs::create_dir_all(places.root.join("demo")).unwrap();
        std::fs::create_dir_all(&places.home).unwrap();
        std::fs::create_dir_all(scratch.dir.join("host-home/dev")).unwrap();
        std::fs::write(places.app_checkouts_file("demo", Lane::Android), android).unwrap();
        let program = scratch.dir.join("ssh");
        std::fs::write(
            &program,
            format!(
                r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2;;
    -*) shift;;
    *) break;;
  esac
done
shift
case "$*" in
  *hw.ncpu*) cat '{probe}'; exit 0;;
esac
cd '{home}' && HOME='{home}' BUILDS_VOLUME='{home}/no-volume' exec sh -c "$*"
"#,
                probe = scratch.dir.join("probe").display(),
                home = scratch.dir.join("host-home").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ssh = Ssh {
            program: program.display().to_string(),
        };
        let world = World {
            scratch,
            places,
            ssh,
        };
        world.host_has(10, 2.0, 900_000_000);
        crate::host::add(&world.places, &world.ssh, "m1", "user@m1").unwrap();
        world
    }

    /// Makes m1 say it has <cores>, a one-minute load of <load> and
    /// <free_kib> of disk free.
    fn host_has(&self, cores: u32, load: f64, free_kib: u64) {
        std::fs::write(
            self.scratch.dir.join("probe"),
            format!("os=Darwin\narch=arm64\ncores={cores}\nload={load} 0 0\nfree_kib={free_kib}\nmemory_free_percent=80\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=300\nmax_procs=8000\n"),
        )
        .unwrap();
    }

    fn at(&self, lane: Lane) -> RepoLane<'_> {
        RepoLane {
            places: &self.places,
            ssh: &self.ssh,
            repo: "demo",
            lane,
        }
    }

    fn set(&self) -> Vec<AppCheckout> {
        self.places
            .app_checkouts_recorded("demo", Lane::Android)
            .unwrap()
    }

    fn entries(&self) -> Vec<String> {
        self.set()
            .into_iter()
            .map(|checkout| checkout.entry)
            .collect()
    }

    /// Takes a lock of the android lane on a thread of its own, as a build
    /// that may take a checkout on a host; sends the checkout once it holds
    /// it, and frees it when told to.
    fn build_on_thread(
        &self,
        said: std::sync::mpsc::Sender<String>,
    ) -> std::sync::mpsc::Sender<()> {
        let (free, freed) = channel::<()>();
        let (places, ssh) = (self.places.clone(), self.ssh.clone());
        std::thread::spawn(move || {
            let held =
                take_where(&places, &ssh, "demo", Lane::Android, Where::Anywhere, 0).unwrap();
            said.send(held.checkout.clone()).unwrap();
            freed.recv().unwrap();
            held.release();
        });
        free
    }
}

/// The time the sizing tests pass as now, in seconds since 1970.
const NOW: i64 = 1_800_000_000;

fn hours_ago(hours: i64) -> i64 {
    NOW - hours * 3600
}

#[test]
fn a_build_that_waits_adds_a_checkout_on_a_host_with_room_and_builds_there() {
    let world = World::new("size-grow", "m1:app-1\n");
    let first = take_where(
        &world.places,
        &world.ssh,
        "demo",
        Lane::Android,
        Where::Anywhere,
        0,
    )
    .unwrap();
    assert_eq!(first.checkout, "m1:app-1");
    let (said, heard) = channel();
    let free = world.build_on_thread(said);
    let took = heard.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(took, "m1:builds-android-1");
    let set = world.set();
    assert_eq!(set[0], AppCheckout::by_person("m1:app-1"));
    let AddedBy::Builds { why, .. } = &set[1].added_by else {
        panic!("builds added it: {set:?}");
    };
    assert!(
        why.contains("1 build waited") && why.contains("host m1 had 8.0 cores"),
        "{why}"
    );
    free.send(()).unwrap();
    first.release();
}

#[test]
fn two_builds_that_wait_at_once_each_add_a_checkout_of_their_own() {
    let world = World::new("size-race", "m1:app-1\n");
    // One build holds m1:app-1, and two wait behind it.
    let dir = world.places.root.join("demo/app-android.queue");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("order"), "1-1\n2-1\n3-1\n").unwrap();
    let start = std::sync::Barrier::new(2);
    let (said, heard) = channel();
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let (start, said, at) = (&start, said.clone(), world.at(Lane::Android));
            scope.spawn(move || {
                start.wait();
                said.send(grow(at, 0, Need::Waits, NOW).unwrap()).unwrap();
            });
        }
    });
    let mut added = [heard.recv().unwrap(), heard.recv().unwrap()];
    added.sort();
    assert_eq!(
        added,
        [
            Some("m1:builds-android-1".to_string()),
            Some("m1:builds-android-2".to_string())
        ]
    );
    assert_eq!(world.entries().len(), 3);
    let queue = Queue::of(&world.places, "demo", Lane::Android);
    assert!(
        queue.waiting().is_empty(),
        "the lane has a checkout for each build in line"
    );
}

#[test]
fn a_lane_at_its_cap_or_a_host_with_no_room_gets_no_checkout() {
    let full: String = (1..=LANE_CAP).map(|n| format!("m1:app-{n}\n")).collect();
    let world = World::new("size-cap", &full);
    let queue = Queue::of(&world.places, "demo", Lane::Android);
    std::fs::create_dir_all(world.places.root.join("demo/app-android.queue")).unwrap();
    let order: String = (1..=LANE_CAP + 2).map(|n| format!("{n}-1\n")).collect();
    std::fs::write(
        world.places.root.join("demo/app-android.queue/order"),
        order,
    )
    .unwrap();
    assert_eq!(queue.waiting().len(), 2);
    let grown = grow(world.at(Lane::Android), 0, Need::Waits, NOW).unwrap();
    assert_eq!(grown, None, "the lane has its cap");
    assert_eq!(world.entries().len(), LANE_CAP);

    std::fs::write(
        world.places.app_checkouts_file("demo", Lane::Android),
        "m1:app-1\n",
    )
    .unwrap();
    world.host_has(10, 9.5, 900_000_000);
    let grown = grow(world.at(Lane::Android), 0, Need::Waits, NOW).unwrap();
    assert_eq!(grown, None, "the host has under a core free");
    world.host_has(10, 2.0, 35 * 1024 * 1024);
    let grown = grow(world.at(Lane::Android), 30 * 1024 * 1024, Need::Waits, NOW).unwrap();
    assert_eq!(
        grown, None,
        "a checkout would take the host under its floor"
    );
    assert_eq!(world.entries(), ["m1:app-1"]);
}

#[test]
fn two_lanes_that_add_at_once_never_pass_the_host_cap() {
    let world = World::new("size-host-cap", "");
    let places = &world.places;
    for round in 0..5 {
        // m1 holds one checkout under its cap, and builds wait in both lanes.
        std::fs::write(
            places.app_checkouts_file("demo", Lane::Android),
            "m1:app-1\nm1:app-2\n",
        )
        .unwrap();
        let mac: String = (1..HOST_CAP - 2).map(|n| format!("m1:mac-{n}\n")).collect();
        std::fs::write(places.app_checkouts_file("demo", Lane::Mac), mac).unwrap();
        for lane in Lane::ALL {
            let dir = places.root.join("demo").join(lane.queue_dir_name());
            std::fs::create_dir_all(&dir).unwrap();
            let order: String = (1..=HOST_CAP).map(|n| format!("{n}-{round}\n")).collect();
            std::fs::write(dir.join("order"), order).unwrap();
        }
        let start = std::sync::Barrier::new(2);
        let grown: Vec<Option<String>> = std::thread::scope(|scope| {
            let threads: Vec<_> = Lane::ALL
                .into_iter()
                .map(|lane| {
                    let start = &start;
                    let at = world.at(lane);
                    scope.spawn(move || {
                        start.wait();
                        grow(at, 0, Need::Waits, NOW).unwrap()
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert_eq!(
            grown.iter().filter(|added| added.is_some()).count(),
            1,
            "round {round}: {grown:?}"
        );
        assert_eq!(checkouts_on(places, "m1").unwrap(), HOST_CAP);
    }
}

#[test]
fn a_host_that_cannot_build_the_mac_lane_takes_no_mac_checkout() {
    let probes = vec![(
        Host {
            band: None,
            name: "m1".to_string(),
            target: Some("user@m1".to_string()),
            cores: 10,
            speed: None,
        },
        Ok(Probe {
            platform: crate::host_platform::Platform::here(),
            memory: crate::host_memory::Memory::at_rest(),
            cores: 10,
            load: [0.0; 3],
            free_kib: 900_000_000,
            git_fails: None,
            nextest: crate::nextest::Has::Nothing,
            processes: crate::proc_limit::ProcessUse {
                count: 100,
                limit: 10_000,
            },
        }),
    )];
    let (fits, _) = fitting_hosts(Lane::Mac, 0, &probes, &[]);
    assert_eq!(fits.len(), 1);
    let (fits, lacks) = fitting_hosts(Lane::Mac, 0, &probes, &["m1".to_string()]);
    assert!(fits.is_empty());
    assert!(lacks[0].contains("cannot build the mac lane"), "{lacks:?}");
    let mut linux = probes.clone();
    if let Ok(probe) = &mut linux[0].1 {
        probe.platform.os = crate::host_platform::Os::Linux;
    }
    for lane in Lane::ALL {
        let (fits, lacks) = fitting_hosts(lane, 0, &linux, &[]);
        assert!(fits.is_empty(), "a Linux host takes no {lane:?} checkout");
        assert!(lacks[0].contains("m1 runs Linux"), "{lacks:?}");
    }
}

#[test]
fn an_idle_checkout_builds_added_goes_and_a_persons_and_a_used_one_stay() {
    let idle_since = hours_ago(3);
    let world = World::new(
        "size-shrink",
        &format!(
            "m1:app-1\nm1:builds-android-1 builds {idle_since} 2 builds waited\nm1:builds-android-2 builds {idle_since} 3 builds waited\n"
        ),
    );
    let places = &world.places;
    // builds-android-2 had a build that ended a minute ago.
    std::fs::create_dir_all(places.app_locks_dir("demo", Lane::Android)).unwrap();
    std::fs::write(
        places.app_lock_ended_file("demo", Lane::Android, "m1:builds-android-2"),
        (NOW - 60).to_string(),
    )
    .unwrap();
    std::fs::write(
        places.app_lock_ended_file("demo", Lane::Android, "m1:builds-android-1"),
        hours_ago(2).to_string(),
    )
    .unwrap();
    let host_home = world.scratch.dir.join("host-home");
    let worktree = host_home.join("dev/demo-worktrees/builds-android-1");
    let built = host_home.join("builds-root.noindex/demo/builds-android-1/target");
    for dir in [&worktree, &built] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::create_dir_all(places.root.join("demo/m1:builds-android-1/out")).unwrap();

    shrink(world.at(Lane::Android), Run::Dry, NOW).unwrap();
    assert_eq!(world.entries().len(), 3, "a dry run removes nothing");

    shrink(world.at(Lane::Android), Run::Real, NOW).unwrap();
    assert_eq!(
        world.entries(),
        ["m1:app-1", "m1:builds-android-2"],
        "the person's checkout and the one used a minute ago stay"
    );
    assert!(!worktree.exists() && !built.parent().unwrap().exists());
    assert!(!places.root.join("demo/m1:builds-android-1").exists());
    assert!(
        !places
            .app_lock_file("demo", Lane::Android, "m1:builds-android-1")
            .exists()
    );
    let removed = places.app_checkouts_removed("demo", Lane::Android).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].entry, "m1:builds-android-1");
    assert_eq!(removed[0].why, "no build used it for 120 minutes");
}

#[test]
fn a_checkout_whose_host_removal_fails_stays_in_the_set() {
    let world = World::new(
        "size-kept",
        &format!(
            "m1:builds-android-1 builds {} 2 builds waited\n",
            hours_ago(3)
        ),
    );
    // The host has no clone of demo, so git cannot remove the worktree there.
    let host_home = world.scratch.dir.join("host-home");
    std::fs::create_dir_all(host_home.join("dev/demo-worktrees/builds-android-1/.git")).unwrap();
    shrink(world.at(Lane::Android), Run::Real, NOW).unwrap();
    assert_eq!(world.entries(), ["m1:builds-android-1"]);
    assert!(
        world
            .places
            .app_checkouts_removed("demo", Lane::Android)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_build_that_must_run_on_a_host_in_a_lane_with_none_there_adds_one() {
    let world = World::new("size-host", "side-1\n");
    let held = take_where(
        &world.places,
        &world.ssh,
        "demo",
        Lane::Android,
        Where::Hosts,
        0,
    )
    .unwrap();
    assert_eq!(held.checkout, "m1:builds-android-1");
    held.release();
    assert_eq!(world.entries(), ["side-1", "m1:builds-android-1"]);
}
