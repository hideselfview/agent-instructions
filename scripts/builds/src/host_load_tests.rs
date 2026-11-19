use super::*;
use crate::host::LOCAL;
use crate::host_band::Band;
use crate::host_memory::{Memory, Pressure};

fn host(name: &str, speed: Option<f64>) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: (name != LOCAL).then(|| format!("u@{name}")),
        cores: 10,
        speed,
    }
}

/// A probe of a host with 10 cores whose 5-minute load is <five>, with 1- and
/// 15-minute loads that would judge the host otherwise.
fn loaded(five: f64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        memory: crate::host_memory::Memory::at_rest(),
        cores: 10,
        load: [30.0, five, 0.0],
        free_kib: 100,
        git_fails: None,
        nextest: crate::nextest::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
    })
}

/// The load part of a line: <five> on 10 cores, against <band>, with the CPU
/// half idle and memory at rest.
fn load(five: &str, band: &str) -> String {
    format!(
        "its 5-minute load is {five} on 10 cores, against a band of {band}; CPU 50% idle; memory pressure normal, 80% free, swap 0M used"
    )
}

/// <host> with a band of <low> to <high>.
fn banded(mut host: Host, low: f64, high: f64) -> Host {
    host.band = Some(Band { low, high });
    host
}

/// <hosts>, each with its CPU half idle.
fn measured(hosts: &[(Host, std::result::Result<Probe, String>)]) -> Vec<Measured> {
    hosts
        .iter()
        .map(|(host, probe)| Measured {
            host: host.clone(),
            probe: probe.clone(),
            idle: Ok(50.0),
        })
        .collect()
}

fn lines(hosts: &[(Host, std::result::Result<Probe, String>)], ready: usize) -> Vec<String> {
    lines_of(&measured(hosts), ready)
}

fn lines_of(hosts: &[Measured], ready: usize) -> Vec<String> {
    findings(hosts, ready, 50)
        .iter()
        .map(Advice::line)
        .collect()
}

const DEFAULT_BAND: &str = "7.5 to 12.5";

/// A host with no band is under it while its 5-minute load is under three
/// quarters of its cores and items are ready: health names the host and the
/// ready items.
#[test]
fn a_host_under_its_band_while_items_are_ready_asks_for_agents() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(2.0)),
        (host("m1", Some(0.8)), loaded(7.4)),
    ];
    assert_eq!(
        lines(&hosts, 3),
        [
            format!(
                "this Mac is under its band while 3 items are ready: {}; start agents",
                load("2.0", DEFAULT_BAND)
            ),
            format!(
                "host m1 is under its band while 3 items are ready: {}; start agents",
                load("7.4", DEFAULT_BAND)
            ),
        ]
    );
    assert_eq!(
        lines(&[(host("m1", Some(0.8)), loaded(2.0))], 1),
        [format!(
            "host m1 is under its band while 1 item is ready: {}; start agents",
            load("2.0", DEFAULT_BAND)
        )]
    );
}

/// Nothing ready needs nothing, and a host loaded inside its band needs
/// nothing either.
#[test]
fn a_host_under_its_band_with_nothing_ready_or_inside_it_needs_nothing() {
    assert!(lines(&[(host(LOCAL, Some(1.0)), loaded(0.0))], 0).is_empty());
    let busy = [
        (host(LOCAL, Some(1.0)), loaded(7.5)),
        (host("m1", Some(0.8)), loaded(12.5)),
    ];
    assert!(lines(&busy, 4).is_empty());
}

/// A host whose 5-minute load is over its band is over it, whether or not
/// items are ready, and holds the queue back when no other host that takes
/// cargo work is under its band.
#[test]
fn a_host_over_its_band_asks_to_start_nothing_new() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", Some(0.8)), loaded(9.0)),
    ];
    let said = format!(
        "this Mac is over its band: {}; start nothing new, except builds items, which this does not hold back",
        load("12.6", DEFAULT_BAND)
    );
    assert_eq!(lines(&hosts, 2), std::slice::from_ref(&said));
    assert_eq!(lines(&hosts, 0), [said]);
}

/// This Mac over its band while m1 is under its own: builds cargo sends the
/// work to m1, so health says to start agents for m1 and does not hold the
/// queue.
#[test]
fn a_host_over_its_band_holds_nothing_back_while_another_host_is_under_its_band() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", Some(0.8)), loaded(2.0)),
    ];
    assert_eq!(
        lines(&hosts, 2),
        [
            format!(
                "this Mac is over its band: {}; builds cargo sends its work to host m1, which is under its band, so this holds nothing back",
                load("12.6", DEFAULT_BAND)
            ),
            format!(
                "host m1 is under its band while 2 items are ready: {}; start agents",
                load("2.0", DEFAULT_BAND)
            ),
        ]
    );
    // An idle host that takes no cargo work gives the other no room.
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", None), loaded(2.0)),
    ];
    assert!(lines(&hosts, 0)[0].contains("start nothing new"));
}

/// A host's own band replaces the shares of its cores: this Mac aims for 25
/// to 30 and m1 for 20 to 25, whatever their cores.
#[test]
fn a_hosts_band_sets_where_it_is_under_and_over() {
    let hosts = |local: f64, m1: f64| {
        [
            (banded(host(LOCAL, Some(1.0)), 25.0, 30.0), loaded(local)),
            (banded(host("m1", Some(0.8)), 20.0, 25.0), loaded(m1)),
        ]
    };
    // Inside both bands, and exactly on their edges: nothing to say.
    assert!(lines(&hosts(25.0, 20.0), 3).is_empty());
    assert!(lines(&hosts(30.0, 25.0), 3).is_empty());
    // A load that the default shares would call over is under this band.
    assert_eq!(
        lines(&hosts(24.9, 25.0), 1),
        [format!(
            "this Mac is under its band while 1 item is ready: {}; start agents",
            load("24.9", "25 to 30")
        )]
    );
    // Over the high end of m1's band while this Mac has room: nothing new
    // waits on m1.
    assert_eq!(
        lines(&hosts(27.0, 25.1), 0),
        [format!(
            "host m1 is over its band: {}; start nothing new, except builds items, which this does not hold back",
            load("25.1", "20 to 25")
        )]
    );
}

/// A host under memory pressure is over its band whatever its load, and its
/// line says why. Swap in use alone is not pressure.
/// Another host under its band takes the work, as for a host over by load.
#[test]
fn a_host_under_memory_pressure_is_over_its_band_whatever_its_load() {
    let swapping = |five: f64| {
        let mut probe = loaded(five).unwrap();
        probe.memory = Memory {
            free_percent: 4,
            swap_used_mib: 2048.0,
            pressure: Pressure::Warning,
        };
        Ok(probe)
    };
    let memory = "memory pressure warning, 4% free, swap 2048M used";
    assert_eq!(
        lines(&[(host(LOCAL, Some(1.0)), swapping(9.0))], 0),
        [format!(
            "this Mac is under memory pressure, which puts it over its band whatever its load: its 5-minute load is 9.0 on 10 cores, against a band of {DEFAULT_BAND}; CPU 50% idle; {memory}; start nothing new, except builds items, which this does not hold back"
        )]
    );
    // A load over the band says so first.
    assert!(
        lines(&[(host(LOCAL, Some(1.0)), swapping(13.0))], 0)[0]
            .starts_with("this Mac is over its band: ")
    );
    // A host with swap in use but normal pressure is judged by its load.
    let mut calm = loaded(1.0).unwrap();
    calm.memory.swap_used_mib = 2400.0;
    assert!(lines(&[(host(LOCAL, Some(1.0)), Ok(calm))], 0).is_empty());
    // A host under pressure is never idle, however low its load.
    let hosts = [
        (host(LOCAL, Some(1.0)), swapping(1.0)),
        (host("m1", Some(0.8)), loaded(9.0)),
    ];
    let said = lines(&hosts, 2);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with("this Mac is under memory pressure"),
        "{said:?}"
    );
}

/// A host whose CPU sits under 5% idle has no room while items are ready,
/// though its load is under its band: the line says so in place of "start
/// agents", and the host gives no room to another over its band. Inside its
/// band, or with nothing ready, it needs nothing. An idle share health could
/// not read leaves the host judged by its load, and the line says why.
#[test]
fn a_host_whose_cpu_sits_idle_under_5_percent_has_no_room() {
    let m1 = |idle| Measured {
        host: banded(host("m1", Some(0.8)), 20.0, 25.0),
        probe: loaded(17.9),
        idle,
    };
    let saturated = "host m1 is under its band by load, but its CPU sits under 5% idle, so it has no room while 3 items are ready: its 5-minute load is 17.9 on 10 cores, against a band of 20 to 25; CPU 0% idle; memory pressure normal, 80% free, swap 0M used";
    assert_eq!(
        lines_of(&[m1(Ok(0.4))], 3),
        [format!(
            "{saturated}; start nothing new, except builds items, which this does not hold back"
        )]
    );
    assert!(lines_of(&[m1(Ok(0.4))], 0).is_empty());
    let over_here = Measured {
        host: host(LOCAL, Some(1.0)),
        probe: loaded(12.6),
        idle: Ok(30.0),
    };
    let said = lines_of(&[over_here, m1(Ok(0.4))], 3);
    assert!(
        said[0].ends_with("start nothing new, except builds items, which this does not hold back"),
        "{said:?}"
    );
    assert!(said[1].starts_with(saturated), "{said:?}");
    let unknown = lines_of(&[m1(Err("top failed".to_string()))], 3);
    assert_eq!(
        unknown,
        [
            "host m1 is under its band while 3 items are ready: its 5-minute load is 17.9 on 10 cores, against a band of 20 to 25; CPU idle share unknown (top failed); memory pressure normal, 80% free, swap 0M used; start agents"
        ]
    );
    let inside = Measured {
        probe: loaded(22.0),
        ..m1(Ok(0.0))
    };
    assert!(lines_of(&[inside], 3).is_empty());
}

/// Idle cores that `builds cargo` never uses ask for no agents. Health says
/// why `builds cargo` passes the host over and what fixes it: a host under the
/// free-disk floor, one whose git fails, and one with no measured core speed.
#[test]
fn a_host_that_takes_no_cargo_job_says_why_and_how_to_fix_it() {
    let roomy = Probe {
        free_kib: 100 * 1024 * 1024,
        ..loaded(0.0).unwrap()
    };
    let refusing = Ok(Probe {
        git_fails: Some("no license".to_string()),
        nextest: crate::nextest::Has::Nothing,
        ..roomy.clone()
    });
    let full = Ok(Probe {
        free_kib: 29 * 1024 * 1024,
        ..loaded(0.0).unwrap()
    });
    let hosts = [
        (host("refusing", Some(1.0)), refusing),
        (host("full", Some(1.0)), full),
        (host("new", None), Ok(roomy)),
    ];
    let floor_kib = 30 * 1024 * 1024;
    let lines: Vec<String> = findings(&measured(&hosts), 5, floor_kib)
        .iter()
        .map(Advice::line)
        .collect();
    assert_eq!(
        lines,
        [
            "host refusing is under its band while 5 items are ready, but builds cargo passes it over: refusing refuses jobs (git fails there: no license). Fix git on refusing",
            "host full is under its band while 5 items are ready, but builds cargo passes it over: full has 29.0G free, under the floor of 30.0G. Free disk there (builds help, DISK), starting with `builds prune`",
            "host new is under its band while 5 items are ready, but builds cargo passes it over: new has no measured core speed (`builds host setup new`)",
        ]
    );
}

/// A host `builds cargo` passes over gets no line while it is busy or while
/// no item is ready, as for any other host.
#[test]
fn a_host_that_takes_no_cargo_job_needs_nothing_unless_idle_with_items_ready() {
    let full = |five| {
        Ok(Probe {
            free_kib: 10,
            ..loaded(five).unwrap()
        })
    };
    assert!(lines(&[(host("full", Some(1.0)), full(0.0))], 0).is_empty());
    assert!(lines(&[(host("full", Some(1.0)), full(8.0))], 5).is_empty());
}

/// A host that does not say its load is a finding that names the host and
/// what went wrong, since health cannot tell whether its cores are idle or
/// over.
#[test]
fn a_host_whose_probe_fails_is_named_with_the_error() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(9.0)),
        (host("m1", Some(0.8)), Err("ssh to u@m1 failed".to_string())),
    ];
    assert_eq!(
        lines(&hosts, 0),
        [
            "host m1 did not say its load, so health cannot tell whether it is under or over its band: ssh to u@m1 failed"
        ]
    );
}

/// Ready items of project builds are listed whatever the hosts' load, and
/// other projects' items and running items are not.
#[test]
fn ready_builds_items_are_listed_and_other_projects_are_not() {
    let db = crate::lists::fixtures::db();
    let id = crate::lists::fixtures::item(&db, Vec::new());
    let store = |project: &str| Loaded {
        project: project.to_string(),
        lists: Ok(crate::lists::fixtures::read(&db)),
    };
    assert!(builds_items_ready(&Ok(vec![store("bae")])).is_none());
    assert_eq!(
        builds_items_ready(&Ok(vec![store("builds")]))
            .unwrap()
            .line(),
        format!(
            "builds items ready and not running: {id} Do the work numbered {}; start them (builds work goes first)",
            id.0
        )
    );
}
