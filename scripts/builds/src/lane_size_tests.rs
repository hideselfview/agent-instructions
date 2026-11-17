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
            format!("cores={cores}\nload={load} 0 0\nfree_kib={free_kib}\n"),
        )
        .unwrap();
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

fn hours_ago(hours: i64) -> i64 {
    crate::app_timing::now() - hours * 3600
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
fn two_builds_that_wait_at_once_each_get_a_new_checkout() {
    let world = World::new("size-race", "m1:app-1\n");
    let first = take_where(
        &world.places,
        &world.ssh,
        "demo",
        Lane::Android,
        Where::Anywhere,
        0,
    )
    .unwrap();
    let (said, heard) = channel();
    let free_a = world.build_on_thread(said.clone());
    let free_b = world.build_on_thread(said);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while world.entries().len() < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "{:?}",
            world.entries()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // A build tells the other builds in line that the set changed with
    // SIGUSR1, and never signals its own process, in which both of these
    // builds run; the test sends it in place of a build in another process.
    unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
    let mut took = [
        heard.recv_timeout(Duration::from_secs(20)).unwrap(),
        heard.recv_timeout(Duration::from_secs(20)).unwrap(),
    ];
    took.sort();
    assert_eq!(took, ["m1:builds-android-1", "m1:builds-android-2"]);
    assert_eq!(world.entries().len(), 3);
    free_a.send(()).unwrap();
    free_b.send(()).unwrap();
    first.release();
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
    let grown = grow(
        &world.places,
        &world.ssh,
        0,
        "demo",
        Lane::Android,
        Need::Waits,
    )
    .unwrap();
    assert_eq!(grown, None, "the lane has its cap");
    assert_eq!(world.entries().len(), LANE_CAP);

    std::fs::write(
        world.places.app_checkouts_file("demo", Lane::Android),
        "m1:app-1\n",
    )
    .unwrap();
    world.host_has(10, 9.5, 900_000_000);
    let grown = grow(
        &world.places,
        &world.ssh,
        0,
        "demo",
        Lane::Android,
        Need::Waits,
    )
    .unwrap();
    assert_eq!(grown, None, "the host has under a core free");
    world.host_has(10, 2.0, 35 * 1024 * 1024);
    let grown = grow(
        &world.places,
        &world.ssh,
        30 * 1024 * 1024,
        "demo",
        Lane::Android,
        Need::Waits,
    )
    .unwrap();
    assert_eq!(
        grown, None,
        "a checkout would take the host under its floor"
    );
    assert_eq!(world.entries(), ["m1:app-1"]);
}

#[test]
fn a_host_that_holds_its_cap_of_checkouts_takes_no_more() {
    let probes = vec![(
        Host {
            name: "m1".to_string(),
            target: Some("user@m1".to_string()),
            cores: 10,
            speed: None,
        },
        Ok(Probe {
            cores: 10,
            load: [0.0; 3],
            free_kib: 900_000_000,
            git_fails: None,
        }),
    )];
    let held = |count| HashMap::from([("m1".to_string(), count)]);
    assert!(choose(Lane::Mac, 0, &probes, &held(HOST_CAP - 1), &[]).is_ok());
    let Err(why) = choose(Lane::Mac, 0, &probes, &held(HOST_CAP), &[]) else {
        panic!("m1 holds its cap");
    };
    assert!(why.contains("the most a host holds"), "{why}");
    let Err(why) = choose(Lane::Mac, 0, &probes, &held(0), &["m1".to_string()]) else {
        panic!("m1 cannot build the mac lane");
    };
    assert!(why.contains("cannot build the mac lane"), "{why}");
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
        (crate::app_timing::now() - 60).to_string(),
    )
    .unwrap();
    std::fs::write(
        places.app_lock_ended_file("demo", Lane::Android, "m1:builds-android-1"),
        hours_ago(2).to_string(),
    )
    .unwrap();
    let host_home = world.scratch.dir.join("host-home");
    let worktree = host_home.join("dev/demo-worktrees/builds-android-1");
    let built = host_home.join("builds-root/demo/builds-android-1/target");
    for dir in [&worktree, &built] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::create_dir_all(places.root.join("demo/m1:builds-android-1/out")).unwrap();

    shrink(places, &world.ssh, "demo", Lane::Android, Run::Dry).unwrap();
    assert_eq!(world.entries().len(), 3, "a dry run removes nothing");

    shrink(places, &world.ssh, "demo", Lane::Android, Run::Real).unwrap();
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
    shrink(&world.places, &world.ssh, "demo", Lane::Android, Run::Real).unwrap();
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
