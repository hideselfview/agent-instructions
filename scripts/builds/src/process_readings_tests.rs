use super::*;

const NOW: i64 = 1_795_069_000;

fn row(pid: i32, cpu: f64, age: i64) -> Row {
    crate::stuck::parse_ps(&format!(
        "{pid} 1 0 0.0 {}:{:05.2} {age} S /usr/libexec/d{pid}\n",
        (cpu / 60.0) as i64,
        cpu % 60.0
    ))
    .unwrap()
    .remove(0)
}

/// What a host listed `seconds_ago` before NOW: a process of pid <pid> that
/// then had <cpu> seconds of CPU time and had run <age> seconds.
fn reading(seconds_ago: i64, procs: &[(i32, f64, i64)]) -> Reading {
    let at = NOW - seconds_ago;
    Reading {
        at,
        procs: procs
            .iter()
            .map(|&(pid, cpu, age)| Proc {
                pid,
                start: at - age,
                cpu,
            })
            .collect(),
    }
}

fn share(rows: Vec<Row>, readings: &[Reading]) -> Vec<Option<f64>> {
    measure(rows, NOW, readings)
        .iter()
        .map(|row| row.cpu_percent)
        .collect()
}

/// A daemon with hours of CPU that used 12 of the last 60 seconds reads 20%,
/// and one that used none reads 0%.
#[test]
fn a_busy_daemon_and_an_idle_daemon_read_their_recent_share() {
    let before = reading(60, &[(617, 21_000.0, 400_000), (88, 7_000.0, 400_000)]);
    let rows = vec![row(617, 21_012.0, 400_060), row(88, 7_000.0, 400_060)];
    let shares = share(rows, &[before]);
    assert!((shares[0].unwrap() - 20.0).abs() < 1e-9, "{shares:?}");
    assert_eq!(shares[1], Some(0.0));
}

#[test]
fn a_process_the_reading_lacks_has_no_share() {
    let before = reading(60, &[(617, 21_000.0, 400_000)]);
    assert_eq!(share(vec![row(900, 3.0, 20)], &[before]), [None]);
}

#[test]
fn a_new_process_on_a_reused_pid_has_no_share() {
    // Pid 617 then ran 400000 seconds; the one now started 30 seconds ago.
    let before = reading(60, &[(617, 21_000.0, 400_000)]);
    assert_eq!(share(vec![row(617, 25.0, 30)], &[before]), [None]);
}

#[test]
fn a_scan_with_no_usable_reading_gives_no_share() {
    let rows = || vec![row(617, 21_012.0, 400_060)];
    assert_eq!(share(rows(), &[]), [None]);
    // Too young to say anything, and too old to say anything about now.
    let young = reading(MIN_SECONDS - 1, &[(617, 21_000.0, 400_000)]);
    let old = reading(MAX_SECONDS + 1, &[(617, 21_000.0, 400_000)]);
    assert_eq!(share(rows(), &[young, old]), [None]);
}

#[test]
fn the_newest_reading_in_range_is_the_one_compared() {
    let old = reading(300, &[(617, 20_000.0, 400_000)]);
    let near = reading(30, &[(617, 21_000.0, 400_000)]);
    let young = reading(5, &[(617, 21_011.0, 400_000)]);
    // 15 seconds of CPU in the 30 since `near`.
    let shares = share(vec![row(617, 21_015.0, 400_030)], &[old, near, young]);
    assert!((shares[0].unwrap() - 50.0).abs() < 1e-9, "{shares:?}");
}

/// A host gets a reading at most once a minute, and readings older than
/// MAX_SECONDS go.
#[test]
fn a_reading_joins_once_a_minute_and_old_ones_go() {
    let mut readings = vec![reading(MAX_SECONDS + 5, &[]), reading(30, &[])];
    assert!(!record(&mut readings, reading(0, &[])));
    assert_eq!(readings.len(), 2);
    let mut readings = vec![reading(MAX_SECONDS + 5, &[]), reading(70, &[])];
    assert!(record(&mut readings, reading(0, &[])));
    assert_eq!(
        readings.iter().map(|r| NOW - r.at).collect::<Vec<_>>(),
        [70, 0]
    );
}

#[test]
fn readings_are_kept_in_a_file_for_each_host() {
    let scratch = crate::testing::Scratch::new("process-readings");
    assert_eq!(read(&scratch.dir, "m1").unwrap(), []);
    let kept = vec![reading(0, &[(1, 2.5, 30)])];
    write(&scratch.dir, "m1", &kept).unwrap();
    assert_eq!(read(&scratch.dir, "m1").unwrap(), kept);
    assert_eq!(read(&scratch.dir, "local").unwrap(), []);
}
