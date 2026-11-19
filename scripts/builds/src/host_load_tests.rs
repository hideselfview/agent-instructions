use super::*;
use crate::host::LOCAL;

fn host(name: &str, speed: Option<f64>) -> Host {
    Host {
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
        cores: 10,
        load: [30.0, five, 0.0],
        free_kib: 100,
        git_fails: None,
    })
}

fn lines(hosts: &[(Host, std::result::Result<Probe, String>)], ready: usize) -> Vec<String> {
    findings(hosts, ready, 50)
        .iter()
        .map(Advice::line)
        .collect()
}

/// Cores sit idle on a host whose 5-minute load is under three quarters of its
/// cores while items are ready: health names the host and the ready items.
#[test]
fn idle_cores_on_a_host_while_items_are_ready_ask_for_agents() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(2.0)),
        (host("m1", Some(0.8)), loaded(7.4)),
    ];
    assert_eq!(
        lines(&hosts, 3),
        [
            "cores idle on this Mac while 3 items are ready: its 5-minute load is 2.0 on 10 cores; start agents",
            "cores idle on host m1 while 3 items are ready: its 5-minute load is 7.4 on 10 cores; start agents",
        ]
    );
    assert_eq!(
        lines(&[(host("m1", Some(0.8)), loaded(2.0))], 1),
        [
            "cores idle on host m1 while 1 item is ready: its 5-minute load is 2.0 on 10 cores; start agents"
        ]
    );
}

/// Idle cores with nothing ready need nothing, and a host loaded between
/// three quarters and five quarters of its cores needs nothing either.
#[test]
fn idle_cores_with_nothing_ready_or_a_busy_host_need_nothing() {
    assert!(lines(&[(host(LOCAL, Some(1.0)), loaded(0.0))], 0).is_empty());
    let busy = [
        (host(LOCAL, Some(1.0)), loaded(7.5)),
        (host("m1", Some(0.8)), loaded(12.5)),
    ];
    assert!(lines(&busy, 4).is_empty());
}

/// A host whose 5-minute load is over five quarters of its cores is over
/// them, whether or not items are ready, and holds the queue back when no
/// other host that takes cargo work has idle cores.
#[test]
fn a_host_over_its_cores_asks_to_start_nothing_new() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", Some(0.8)), loaded(9.0)),
    ];
    assert_eq!(
        lines(&hosts, 2),
        [
            "this Mac is over its cores: its 5-minute load is 12.6 on 10 cores; start nothing new, except builds items, which this does not hold back"
        ]
    );
    assert_eq!(
        lines(&hosts, 0),
        [
            "this Mac is over its cores: its 5-minute load is 12.6 on 10 cores; start nothing new, except builds items, which this does not hold back"
        ]
    );
}

/// This Mac over its cores while m1 sits idle: builds cargo sends the work to
/// m1, so health says to start agents for m1 and does not hold the queue.
#[test]
fn a_host_over_its_cores_holds_nothing_back_while_another_host_has_room() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", Some(0.8)), loaded(2.0)),
    ];
    assert_eq!(
        lines(&hosts, 2),
        [
            "this Mac is over its cores: its 5-minute load is 12.6 on 10 cores; builds cargo sends its work to host m1, which has idle cores, so this holds nothing back",
            "cores idle on host m1 while 2 items are ready: its 5-minute load is 2.0 on 10 cores; start agents",
        ]
    );
    // An idle host that takes no cargo work gives the other no room.
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", None), loaded(2.0)),
    ];
    assert!(lines(&hosts, 0)[0].contains("start nothing new"));
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
    let lines: Vec<String> = findings(&hosts, 5, floor_kib)
        .iter()
        .map(Advice::line)
        .collect();
    assert_eq!(
        lines,
        [
            "cores idle on host refusing while 5 items are ready, but builds cargo passes it over: refusing refuses jobs (git fails there: no license). Fix git on refusing",
            "cores idle on host full while 5 items are ready, but builds cargo passes it over: full has 29.0G free, under the floor of 30.0G. Free disk there (builds help, DISK), starting with `builds prune`",
            "cores idle on host new while 5 items are ready, but builds cargo passes it over: new has no measured core speed (`builds host setup new`)",
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
            "host m1 did not say its load, so health cannot tell whether its cores are idle or over: ssh to u@m1 failed"
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
