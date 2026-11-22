use super::*;
use crate::host::Ssh;
use crate::lane::Lane;
use crate::places::Places;
use crate::testing::Scratch;

fn used(entries: &[(&str, Option<i64>)]) -> Vec<(String, Option<i64>)> {
    entries
        .iter()
        .map(|(entry, last)| (entry.to_string(), *last))
        .collect()
}

#[test]
fn the_warmest_source_is_the_one_a_build_used_last() {
    let ranked = rank_sources(&used(&[
        ("side-3", Some(100)),
        ("m1:app-2", Some(300)),
        ("m1:app-1", Some(300)),
        ("m1:builds-mac-1", None),
        ("side-4", Some(200)),
    ]));
    assert_eq!(ranked, ["m1:app-1", "m1:app-2", "side-4", "side-3"]);
}

#[test]
fn a_checkout_no_build_used_is_no_source() {
    assert!(rank_sources(&used(&[("m1:app-1", None)])).is_empty());
    assert!(rank_sources(&[]).is_empty());
}

/// Places whose repo "demo" has the mac lane's set <set>, with each entry of
/// <ended> let go of by a build at its time.
fn lane(scratch: &Scratch, set: &str, ended: &[(&str, i64)]) -> Places {
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.app_checkouts_file("demo", Lane::Mac), set).unwrap();
    std::fs::create_dir_all(places.app_locks_dir("demo", Lane::Mac)).unwrap();
    for (entry, at) in ended {
        std::fs::write(
            places.app_lock_ended_file("demo", Lane::Mac, entry),
            format!("{at}\n"),
        )
        .unwrap();
    }
    places
}

fn at<'a>(places: &'a Places, ssh: &'a Ssh) -> RepoLane<'a> {
    RepoLane {
        places,
        ssh,
        repo: "demo",
        lane: Lane::Mac,
    }
}

fn unused_ssh() -> Ssh {
    Ssh {
        program: "ssh-is-not-run".to_string(),
    }
}

#[test]
fn a_source_is_on_the_new_checkouts_host_and_is_not_the_new_checkout() {
    let scratch = Scratch::new("clone-source-host");
    let places = lane(
        &scratch,
        "side-3\nm1:app-1\nm1:builds-mac-1\nm2:app-1\n",
        &[
            ("side-3", 900),
            ("m1:app-1", 100),
            ("m1:builds-mac-1", 500),
            ("m2:app-1", 800),
        ],
    );
    let ssh = unused_ssh();
    let at = at(&places, &ssh);
    assert_eq!(
        ranked(at, "m1", "m1:builds-mac-1").unwrap(),
        ["m1:app-1"],
        "the new checkout is not its own source, and no other host's checkout is one"
    );
    assert_eq!(
        ranked(at, crate::host::LOCAL, "side-9").unwrap(),
        ["side-3"]
    );
    assert!(has_source(at, "m1").unwrap() && !has_source(at, "m3").unwrap());
    let source = take_source(at, "m1", "m1:builds-mac-1", Duration::ZERO)
        .unwrap()
        .expect("m1:app-1 is free");
    assert_eq!(source.entry, "m1:app-1");
}

#[test]
fn a_source_a_build_holds_is_waited_for_and_then_given_up() {
    let scratch = Scratch::new("clone-source-wait");
    let places = lane(
        &scratch,
        "m1:app-1\nm1:app-2\n",
        &[("m1:app-1", 100), ("m1:app-2", 200)],
    );
    let ssh = unused_ssh();
    let at = at(&places, &ssh);
    let both: Vec<_> = ["m1:app-2", "m1:app-1"]
        .into_iter()
        .map(|entry| {
            lock_checkout(&places, "demo", Lane::Mac, entry)
                .unwrap()
                .unwrap()
        })
        .collect();
    assert!(
        take_source(at, "m1", "m1:builds-mac-1", Duration::ZERO)
            .unwrap()
            .is_none(),
        "every source is building and the wait is over"
    );
    let mut both = both.into_iter();
    drop(both.next());
    let source = take_source(at, "m1", "m1:builds-mac-1", Duration::from_secs(30))
        .unwrap()
        .expect("the warmest one came free");
    assert_eq!(source.entry, "m1:app-2");
    drop(both);
}

#[test]
fn a_host_with_no_checkout_of_the_lane_gives_no_source() {
    let scratch = Scratch::new("clone-source-none");
    let places = lane(&scratch, "m1:app-1\n", &[]);
    let ssh = unused_ssh();
    assert!(
        take_source(at(&places, &ssh), "m1", "m1:builds-mac-1", Duration::ZERO)
            .unwrap()
            .is_none()
    );
}
