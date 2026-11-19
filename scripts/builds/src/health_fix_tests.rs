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
        memory: crate::host_memory::Memory::at_rest(),
        cores: 10,
        load: [1.0; 3],
        free_kib: (free_gib * GIB as f64) as u64,
        git_fails: git_fails.map(str::to_string),
        nextest: crate::nextest::Has::Nothing,
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
