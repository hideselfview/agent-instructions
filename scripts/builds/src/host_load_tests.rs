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
/// them, whether or not items are ready.
#[test]
fn a_host_over_its_cores_asks_to_start_nothing_new() {
    let hosts = [
        (host(LOCAL, Some(1.0)), loaded(12.6)),
        (host("m1", Some(0.8)), loaded(2.0)),
    ];
    assert_eq!(
        lines(&hosts, 0),
        [
            "this Mac is over its cores: its 5-minute load is 12.6 on 10 cores; start nothing new, except builds items, which this does not hold back"
        ]
    );
    assert_eq!(
        lines(&hosts, 2),
        [
            "this Mac is over its cores: its 5-minute load is 12.6 on 10 cores; start nothing new, except builds items, which this does not hold back",
            "cores idle on host m1 while 2 items are ready: its 5-minute load is 2.0 on 10 cores; start agents",
        ]
    );
}

/// Idle cores that `builds cargo` never uses ask for no agents: a host that
/// refuses jobs, is under the free-disk floor or has no measured core speed
/// (`host::takes_cargo` says no to each).
#[test]
fn a_host_that_takes_no_cargo_job_asks_for_no_agents() {
    let refusing = Ok(Probe {
        git_fails: Some("no license".to_string()),
        ..loaded(0.0).unwrap()
    });
    let full = Ok(Probe {
        free_kib: 10,
        ..loaded(0.0).unwrap()
    });
    let hosts = [
        (host("refusing", Some(1.0)), refusing),
        (host("full", Some(1.0)), full),
        (host("new", None), loaded(0.0)),
    ];
    assert!(lines(&hosts, 5).is_empty());
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
