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
        nextest: crate::nextest::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
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
        let (ranked, _) = rank_by_room(&hosts, 0, &starts.counts.recent, crate::host::Runs::Cargo);
        let run = running();
        starts
            .record(run.id(), &ranked[0].0.name, now + at)
            .unwrap();
        picks.push(ranked[0].0.name.clone());
        runs.push(run);
    }
    assert_eq!(picks, ["local", "m1", "local"]);
    let starts = read(&places, now + 3).unwrap();
    assert_eq!(starts.counts.recent.get("local"), Some(&2));
    assert_eq!(starts.counts.recent.get("m1"), Some(&1));
    let (_, why) = rank_by_room(&hosts, 0, &starts.counts.recent, crate::host::Runs::Cargo);
    assert!(
        why.contains("m1 has -3.0 cores of room after 1 build that started in the last minute"),
        "{why}"
    );
    drop(starts);
    for mut run in runs {
        run.kill().unwrap();
        run.wait().unwrap();
    }
}

/// A pick counts a run while its process runs: as recent while it is under
/// a minute old, and as live whatever its age. `read` deletes the file of a
/// run whose process ended.
#[test]
fn runs_count_while_they_run_and_recent_ones_under_a_minute() {
    let scratch = Scratch::new("cargo-starts-window");
    let places = scratch.places();
    let now = 1_800_000_000;
    let mut ended = running();
    ended.kill().unwrap();
    ended.wait().unwrap();
    let mut old = running();
    let mut new = running();
    let starts = read(&places, now).unwrap();
    starts.record(ended.id(), "m1", now).unwrap();
    let starts = read(&places, now).unwrap();
    starts.record(old.id(), "m1", now - WINDOW_SECONDS).unwrap();
    let starts = read(&places, now).unwrap();
    starts.record(new.id(), "m1", now).unwrap();
    let starts = read(&places, now).unwrap();
    assert_eq!(starts.counts.recent.get("m1"), Some(&1));
    assert_eq!(starts.counts.live.get("m1"), Some(&2));
    assert!(dir_of(&places).join(old.id().to_string()).exists());
    assert!(!dir_of(&places).join(ended.id().to_string()).exists());
    drop(starts);
    for run in [&mut old, &mut new] {
        run.kill().unwrap();
        run.wait().unwrap();
    }
    let starts = read(&places, now).unwrap();
    assert_eq!(starts.counts, Counts::default());
}
