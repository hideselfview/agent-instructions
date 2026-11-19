use super::*;
use crate::host::{Host, Probe, rank_by_room};
use crate::testing::Scratch;

fn host(name: &str) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: (name != "local").then(|| format!("user@{name}")),
        cores: 0,
        speed: Some(1.0),
    }
}

fn probe(cores: u32, load: f64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        memory: crate::host_memory::Memory::at_rest(),
        cores,
        load: [load, load, load],
        free_kib: 1 << 30,
        git_fails: None,
        nextest: None,
    })
}

/// A process that runs until the test kills it, standing in for a
/// `builds cargo` that runs.
fn running() -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap()
}

/// Three runs that start within a minute, each picking after the one before
/// recorded its pick: the loads the hosts report stay the same, and still
/// the runs go to both hosts.
#[test]
fn a_burst_of_runs_picks_both_hosts() {
    let scratch = Scratch::new("cargo-starts-burst");
    let places = scratch.places();
    let now = 1_800_000_000;
    let hosts = vec![
        (host("local"), probe(14, 3.0)),
        (host("m1"), probe(10, 3.0)),
    ];
    let mut runs = Vec::new();
    let mut picks = Vec::new();
    for at in 0..3 {
        let starts = read(&places, now + at).unwrap();
        let (ranked, _) = rank_by_room(&hosts, 0, &starts.recent, crate::host::Runs::Cargo);
        let run = running();
        starts
            .record(run.id(), &ranked[0].0.name, now + at)
            .unwrap();
        picks.push(ranked[0].0.name.clone());
        runs.push(run);
    }
    assert_eq!(picks, ["local", "m1", "local"]);
    let starts = read(&places, now + 3).unwrap();
    assert_eq!(starts.recent.get("local"), Some(&2));
    assert_eq!(starts.recent.get("m1"), Some(&1));
    let (_, why) = rank_by_room(&hosts, 0, &starts.recent, crate::host::Runs::Cargo);
    assert!(
        why.contains("m1 has -3.0 cores of room after 1 cargo run that started in the last minute"),
        "{why}"
    );
    drop(starts);
    for mut run in runs {
        run.kill().unwrap();
        run.wait().unwrap();
    }
}

/// A pick does not count a run whose process ended, and `read` deletes the
/// file of any run that started a minute ago or earlier.
#[test]
fn only_runs_under_a_minute_old_that_still_run_count() {
    let scratch = Scratch::new("cargo-starts-window");
    let places = scratch.places();
    let now = 1_800_000_000;
    let mut ended = running();
    ended.kill().unwrap();
    ended.wait().unwrap();
    let mut old = running();
    let starts = read(&places, now).unwrap();
    starts.record(ended.id(), "m1", now).unwrap();
    let starts = read(&places, now).unwrap();
    starts.record(old.id(), "m1", now - WINDOW_SECONDS).unwrap();
    let starts = read(&places, now).unwrap();
    assert!(starts.recent.is_empty(), "{:?}", starts.recent);
    assert!(!dir_of(&places).join(old.id().to_string()).exists());
    assert!(dir_of(&places).join(ended.id().to_string()).exists());
    old.kill().unwrap();
    old.wait().unwrap();
}
