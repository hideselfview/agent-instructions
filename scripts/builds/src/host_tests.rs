use super::*;
use crate::fail::Fail;
use crate::host_band::Band;
use crate::host_memory::{Memory, Pressure};
use crate::testing::Scratch;

/// The scratch's stand-in system with a program in place of ssh: it runs the
/// command it is given in <home> (which stands for the remote Mac's home), and
/// refuses the host named `unreachable` as ssh refuses a host that does not
/// answer. Named `ssh`, so git treats it as ssh.
pub fn fake_ssh(scratch: &Scratch, home: &Path) -> Ssh {
    std::fs::create_dir_all(home).unwrap();
    scratch.stand_in(
        "ssh",
        &format!(
            r#"while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i|-l) shift 2;;
    -*) shift;;
    *) break;;
  esac
done
host=$1
shift
if [ "$host" = unreachable ]; then
  echo "ssh: connect to host unreachable port 22: Connection refused" >&2
  exit 255
fi
cd '{home}' && HOME='{home}' BUILDS_VOLUME='{home}/volume' exec sh -c "$*""#,
            home = home.display()
        ),
    );
    scratch.system().ssh()
}

use std::path::Path;

#[test]
fn reads_what_a_host_prints() {
    let probe = parse_probe(
        "os=Linux\narch=x86_64\ncores=10\nload=1.50 1.20 1.10 \nfree_kib=447000000\nmemory_free_percent=61\nswap_used_mib=12.50\nmemory_pressure_level=2\nprocs=300\nmax_procs=8000\n",
    )
    .unwrap();
    assert_eq!(
        probe,
        Probe {
            platform: crate::host_platform::Platform {
                os: crate::host_platform::Os::Linux,
                arch: "x86_64".to_string(),
            },
            cores: 10,
            load: [1.5, 1.2, 1.1],
            free_kib: 447_000_000,
            memory: Memory {
                free_percent: 61,
                swap_used_mib: 12.5,
                pressure: Pressure::Warning,
            },
            git_fails: None,
            nextest: crate::cargo_program::Has::Nothing,
            mutants: crate::cargo_program::Has::Nothing,
            node: crate::cargo_program::Has::Nothing,
            processes: crate::proc_limit::ProcessUse {
                count: 300,
                limit: 8000,
            },
        }
    );
    assert!((probe.load_per_core() - 0.15).abs() < 1e-9);
    assert!(parse_probe("cores=10\n").is_err());
    assert!(parse_probe("cores=10\nload=1.50\nfree_kib=4\n").is_err());
    let refusing = parse_probe(
        "os=Darwin\narch=arm64\ncores=10\nload=1.50 1.20 1.10\nfree_kib=4\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=1\nmax_procs=2\ngit_fails=You have not agreed to the Xcode license agreements. \n",
    )
    .unwrap();
    assert_eq!(
        refusing.git_fails.as_deref(),
        Some("You have not agreed to the Xcode license agreements.")
    );
}

#[test]
fn a_probe_reads_what_a_host_has_of_cargo_nextest() {
    use crate::cargo_program::Has;
    let said = "os=Darwin\narch=arm64\ncores=10\nload=1.50 1.20 1.10\nfree_kib=4\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=1\nmax_procs=2\n";
    assert_eq!(parse_probe(said).unwrap().nextest, Has::Nothing);
    assert_eq!(
        parse_probe(&format!("{said}nextest=0.9.143\n"))
            .unwrap()
            .nextest,
        Has::Version("0.9.143".to_string())
    );
    assert_eq!(
        parse_probe(&format!("{said}nextest_fails=dyld: missing \n"))
            .unwrap()
            .nextest,
        Has::Broken("dyld: missing".to_string())
    );
}

/// A nextest run skips a host that has no cargo-nextest at the version builds
/// installs, saying which, while a cargo run does not.
#[test]
fn a_nextest_run_passes_over_a_host_without_the_pinned_cargo_nextest() {
    use crate::cargo_program::Has;
    let with = |nextest: Has| {
        Ok(Probe {
            platform: crate::host_platform::Platform::here(),
            cores: 10,
            load: [0.0; 3],
            free_kib: 100,
            memory: crate::host_memory::Memory::at_rest(),
            git_fails: None,
            nextest,
            mutants: Has::Nothing,
            node: Has::Version(crate::cargo_program::NODE.version.to_string()),
            processes: crate::proc_limit::ProcessUse {
                count: 100,
                limit: 10_000,
            },
        })
    };
    let hosts = vec![
        (
            host("local"),
            with(Has::Version(
                crate::cargo_program::NEXTEST.version.to_string(),
            )),
        ),
        (host("m1"), with(Has::Nothing)),
        (host("m2"), with(Has::Version("0.9.1".to_string()))),
        (host("m3"), with(Has::Broken("dyld: missing".to_string()))),
    ];
    let names = |runs| {
        let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), runs);
        let names: Vec<String> = ranked.into_iter().map(|(host, _)| host.name).collect();
        (names, why)
    };
    assert_eq!(names(Runs::Cargo).0, ["local", "m1", "m2", "m3"]);
    let (names, why) = names(Runs::Nextest);
    assert_eq!(names, ["local"]);
    assert!(
        why.contains("m1 has no cargo-nextest (`builds host setup m1` installs 0.9.143)")
            && why.contains(
                "m2 has cargo-nextest 0.9.1, not 0.9.143 (`builds host setup m2` installs 0.9.143)"
            )
            && why.contains("m3's cargo-nextest does not start: dyld: missing"),
        "{why}"
    );
    assert_eq!(
        takes_run(&hosts[1].0, &hosts[1].1, 50, Runs::Nextest).unwrap_err(),
        CannotRun::LacksProgram(LacksProgram {
            host: "m1".to_string(),
            program: &crate::cargo_program::NEXTEST,
            has: Has::Nothing
        })
    );
}

#[test]
fn a_probe_reads_what_a_host_has_of_node() {
    use crate::cargo_program::Has;
    let said = "os=Darwin\narch=arm64\ncores=10\nload=1.50 1.20 1.10\nfree_kib=4\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=1\nmax_procs=2\n";
    assert_eq!(parse_probe(said).unwrap().node, Has::Nothing);
    assert_eq!(
        parse_probe(&format!("{said}node=22.17.0\n")).unwrap().node,
        Has::Version("22.17.0".to_string())
    );
    assert_eq!(
        parse_probe(&format!("{said}node_fails=dyld: missing \n"))
            .unwrap()
            .node,
        Has::Broken("dyld: missing".to_string())
    );
}

/// A run of tests skips a host that has no node at the version builds
/// installs, saying which, while a run that tests nothing does not: builds'
/// own tests run page scripts in node.
#[test]
fn a_test_run_passes_over_a_host_without_the_pinned_node() {
    use crate::cargo_program::{Has, NEXTEST, NODE};
    let with = |node: Has| {
        Ok(Probe {
            platform: crate::host_platform::Platform::here(),
            cores: 10,
            load: [0.0; 3],
            free_kib: 100,
            memory: crate::host_memory::Memory::at_rest(),
            git_fails: None,
            nextest: Has::Version(NEXTEST.version.to_string()),
            mutants: Has::Nothing,
            node,
            processes: crate::proc_limit::ProcessUse {
                count: 100,
                limit: 10_000,
            },
        })
    };
    let hosts = vec![
        (host("local"), with(Has::Version(NODE.version.to_string()))),
        (host("m1"), with(Has::Nothing)),
        (host("m2"), with(Has::Version("20.0.0".to_string()))),
    ];
    for runs in [Runs::Test, Runs::Nextest] {
        let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), runs);
        let names: Vec<String> = ranked.into_iter().map(|(host, _)| host.name).collect();
        assert_eq!(names, ["local"], "{runs:?}");
        assert!(
            why.contains("m1 has no node (`builds host setup m1` installs 22.17.0)")
                && why.contains(
                    "m2 has node 20.0.0, not 22.17.0 (`builds host setup m2` installs 22.17.0)"
                ),
            "{why}"
        );
    }
    let (ranked, _) = rank_by_room(&hosts, 50, &Default::default(), Runs::Cargo);
    assert_eq!(ranked.len(), 3, "a clippy or check run needs no node");
}

#[test]
fn a_host_whose_git_fails_is_passed_over_saying_what_git_said() {
    let refusing = || {
        Ok(Probe {
            platform: crate::host_platform::Platform::here(),
            memory: crate::host_memory::Memory::at_rest(),
            cores: 10,
            load: [0.0; 3],
            free_kib: 100,
            git_fails: Some("no license".to_string()),
            nextest: crate::cargo_program::Has::Nothing,
            mutants: crate::cargo_program::Has::Nothing,
            node: crate::cargo_program::Has::Nothing,
            processes: crate::proc_limit::ProcessUse {
                count: 100,
                limit: 10_000,
            },
        })
    };
    let hosts = vec![
        (host("local"), probe_with(8, 6.0, 100)),
        (host("m1"), refusing()),
    ];
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), crate::host::Runs::Cargo);
    let names: Vec<&str> = ranked.iter().map(|(h, _)| h.name.as_str()).collect();
    assert_eq!(names, ["local"]);
    assert!(
        why.contains("m1 refuses jobs (git fails there: no license)"),
        "{why}"
    );
    let (chosen, why) = choose(&hosts, 50).unwrap();
    assert_eq!(chosen, host("local"));
    assert!(why.contains("m1 refuses jobs"), "{why}");
    assert!(!why.contains("m1 0.00"), "{why}");
}

#[test]
fn refusals_keep_the_first_time_and_go_with_their_host() {
    let scratch = Scratch::new("host-refusals");
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 u@m1 10\n").unwrap();
    record_refusal(&places, Refused::Jobs, "m1", "first").unwrap();
    record_refusal(&places, Refused::Jobs, "m1", "second").unwrap();
    record_refusal(&places, Refused::Xcode, "m1", "plugin").unwrap();
    // `refusals` lists this Mac's record of refusing mac app builds too,
    // because this Mac is always a host.
    record_refusal(&places, Refused::Xcode, LOCAL, "plugin here").unwrap();
    // A record of a host that is not recorded is not listed.
    record_refusal(&places, Refused::Jobs, "old", "gone").unwrap();
    let found = refusals(&places).unwrap();
    let listed: Vec<(&str, Refused, &str)> = found
        .iter()
        .map(|r| (r.host.as_str(), r.refused, r.said.as_str()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (LOCAL, Refused::Xcode, "plugin here"),
            ("m1", Refused::Jobs, "first"),
            ("m1", Refused::Xcode, "plugin"),
        ]
    );
    assert!(found[0].at > 0);
    let mac = |places: &Places| -> Vec<(String, Refused)> {
        refusing_mac_builds(places)
            .unwrap()
            .into_iter()
            .map(|refusal| (refusal.host, refusal.refused))
            .collect()
    };
    record_refusal(&places, Refused::UiAutomation, "m1", "timed out").unwrap();
    assert_eq!(
        mac(&places),
        [
            (LOCAL.to_string(), Refused::Xcode),
            ("m1".to_string(), Refused::Xcode),
            ("m1".to_string(), Refused::UiAutomation),
        ],
        "refusing_mac_builds lists Xcode and UI automation records but not job records"
    );
    clear_refusal(&places, Refused::Jobs, "m1").unwrap();
    assert_eq!(mac(&places).len(), 3, "clearing one kind leaves the others");
    remove(&places, "m1").unwrap();
    std::fs::write(&file, "m1 u@m1 10\n").unwrap();
    assert_eq!(
        mac(&places),
        [(LOCAL.to_string(), Refused::Xcode)],
        "removing m1 removed its records"
    );
}

fn host(name: &str) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

fn probe_of(load: f64, free_kib: u64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        platform: crate::host_platform::Platform::here(),
        memory: crate::host_memory::Memory::at_rest(),
        cores: 10,
        load: [load, load, load],
        free_kib,
        git_fails: None,
        nextest: crate::cargo_program::Has::Nothing,
        mutants: crate::cargo_program::Has::Nothing,
        node: crate::cargo_program::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
    })
}

#[test]
fn picks_the_lowest_load_per_core_above_the_floor() {
    let hosts = vec![
        (host("a"), probe_of(5.0, 100)),
        (host("b"), probe_of(1.0, 100)),
        (host("c"), probe_of(0.1, 5)),
        (host("d"), Err("ssh failed".to_string())),
    ];
    let (picked, why) = choose(&hosts, 50).unwrap();
    assert_eq!(picked.name, "b");
    assert!(why.starts_with("host b: load 0.10 per core"), "{why}");
    assert!(why.contains("a 0.50"), "{why}");
    assert!(why.contains("c has"), "{why}");
    assert!(why.contains("d is unreachable (ssh failed)"), "{why}");
}

/// `builds remote run` runs its command only on a host of this machine's
/// OS, however idle another is.
#[test]
fn a_host_of_another_os_runs_no_remote_command() {
    let mut linux = probe_of(0.0, 100);
    if let Ok(probe) = &mut linux {
        probe.platform.os = crate::host_platform::Os::Linux;
    }
    let hosts = vec![(host("a"), probe_of(5.0, 100)), (host("box"), linux)];
    let (picked, why) = choose(&hosts, 50).unwrap();
    assert_eq!(picked.name, "a");
    assert!(why.contains("box runs Linux, not macOS"), "{why}");
}

#[test]
fn equal_loads_pick_the_first() {
    let hosts = vec![
        (host("a"), probe_of(1.0, 100)),
        (host("b"), probe_of(1.0, 100)),
    ];
    assert_eq!(choose(&hosts, 0).unwrap().0.name, "a");
}

#[test]
fn no_host_fits_names_each() {
    let hosts = vec![
        (host("a"), Err("refused".to_string())),
        (host("b"), probe_of(0.0, 1)),
    ];
    let Err(Fail::Message(message)) = choose(&hosts, 50) else {
        panic!("expected a failure");
    };
    assert!(message.contains("a is unreachable (refused)"), "{message}");
    assert!(message.contains("b has"), "{message}");
}

#[test]
fn adds_lists_and_removes_a_host() {
    let scratch = Scratch::new("host-add");
    let places = scratch.places();
    std::fs::create_dir_all(&places.home).unwrap();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));
    let added = add(&places, &ssh, "m1", "dmitry@example.local").unwrap();
    assert!(added.cores >= 1);
    assert_eq!(read_remote(&places).unwrap(), vec![added.clone()]);
    assert_eq!(all(&places).unwrap().len(), 2);
    let listed = probe_all(&places, &ssh, all(&places).unwrap());
    assert_eq!(listed[0].0.name, "local");
    assert!(listed.iter().all(|(_, probe)| probe.is_ok()));
    assert!(list_line(&listed[1].0, &listed[1].1).contains("m1  dmitry@example.local"));
    assert!(
        add(&places, &ssh, "m1", "x@y").is_err(),
        "a name is taken once"
    );
    assert!(add(&places, &ssh, LOCAL, "x@y").is_err());
    assert!(add(&places, &ssh, "bad name", "x@y").is_err());
    assert!(add(&places, &ssh, "evil", "-oProxyCommand=x").is_err());
    remove(&places, "m1").unwrap();
    assert!(read_remote(&places).unwrap().is_empty());
    assert!(remove(&places, "m1").is_err());
}

#[test]
fn an_unreachable_host_is_not_added() {
    let scratch = Scratch::new("host-unreachable");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));
    let Err(Fail::Message(message)) = add(&places, &ssh, "gone", "unreachable") else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone is not reachable"), "{message}");
    assert!(message.contains("Connection refused"), "{message}");
    assert!(read_remote(&places).unwrap().is_empty());
}

fn probe_with(cores: u32, load: f64, free_kib: u64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        platform: crate::host_platform::Platform::here(),
        memory: crate::host_memory::Memory::at_rest(),
        cores,
        load: [load, load, load],
        free_kib,
        git_fails: None,
        nextest: crate::cargo_program::Has::Nothing,
        mutants: crate::cargo_program::Has::Nothing,
        node: crate::cargo_program::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
    })
}

#[test]
fn ranks_hosts_by_cores_minus_load_and_this_mac_wins_a_tie() {
    let hosts = vec![
        (host("local"), probe_with(8, 6.0, 100)),
        (host("m1"), probe_with(10, 2.0, 100)),
        (host("big"), probe_with(20, 18.0, 100)),
        (host("full"), probe_with(10, 0.0, 5)),
        (host("gone"), Err("refused".to_string())),
    ];
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), crate::host::Runs::Cargo);
    let names: Vec<&str> = ranked.iter().map(|(h, _)| h.name.as_str()).collect();
    assert_eq!(names, ["m1", "local", "big"]);
    assert!(
        why.starts_with(
            "m1 has 8.0 cores of room, worth 8.0 of this Mac's cores at 1.00 times its speed; local has 2.0"
        ),
        "{why}"
    );
    assert!(why.contains("full has"), "{why}");
    assert!(why.contains("gone is unreachable (refused)"), "{why}");

    let tie = vec![
        (host("local"), probe_with(8, 2.0, 100)),
        (host("m1"), probe_with(8, 2.0, 100)),
    ];
    assert_eq!(
        rank_by_room(&tie, 0, &Default::default(), crate::host::Runs::Cargo).0[0]
            .0
            .name,
        "local"
    );

    let loaded = vec![(host("m1"), probe_with(10, 14.0, 100))];
    assert_eq!(
        rank_by_room(&loaded, 0, &Default::default(), crate::host::Runs::Cargo)
            .0
            .len(),
        1,
        "a host with no room still ranks"
    );
    assert!(
        rank_by_room(
            &[(host("m1"), Err("x".into()))],
            0,
            &Default::default(),
            crate::host::Runs::Cargo
        )
        .0
        .is_empty()
    );
}

/// rank_by_room counts a host's room at its core speed against this Mac's,
/// and leaves out a host with no measured speed, naming `builds host setup`
/// for it.
#[test]
fn ranks_hosts_by_their_room_at_their_core_speed() {
    let at = |name: &str, speed: Option<f64>| Host {
        speed,
        ..host(name)
    };
    let hosts = vec![
        (at("local", Some(1.0)), probe_with(8, 6.0, 100)),
        (at("slow", Some(0.5)), probe_with(10, 2.0, 100)),
        (at("slower", Some(0.25)), probe_with(10, 2.0, 100)),
        (at("new", None), probe_with(10, 0.0, 100)),
    ];
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), crate::host::Runs::Cargo);
    let names: Vec<&str> = ranked.iter().map(|(h, _)| h.name.as_str()).collect();
    // 8 free cores at half speed are 4 of this Mac's, ahead of its own 2; at
    // a quarter they are 2, a tie this Mac wins.
    assert_eq!(names, ["slow", "local", "slower"]);
    assert!(
        why.contains(
            "slow has 8.0 cores of room, worth 4.0 of this Mac's cores at 0.50 times its speed"
        ),
        "{why}"
    );
    assert!(
        why.contains("new has no measured core speed (`builds host setup new`)"),
        "{why}"
    );
    // When load is higher than the core count, a slower host ranks below a
    // faster host with the same excess load.
    let busy = vec![
        (at("slow", Some(0.5)), probe_with(10, 14.0, 100)),
        (at("local", Some(1.0)), probe_with(10, 15.0, 100)),
    ];
    let (ranked, _) = rank_by_room(&busy, 50, &Default::default(), crate::host::Runs::Cargo);
    let names: Vec<&str> = ranked.iter().map(|(h, _)| h.name.as_str()).collect();
    assert_eq!(names, ["local", "slow"]);
}

/// The hosts file keeps a host's speed once setup measured it, and a host
/// added and not yet set up has none.
#[test]
fn the_hosts_file_keeps_each_hosts_speed() {
    let scratch = Scratch::new("host-speeds");
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 u@m1 10 0.712\nnew u@new 8\n").unwrap();
    let hosts = read_remote(&places).unwrap();
    let speeds: Vec<Option<f64>> = hosts.iter().map(|h| h.speed).collect();
    assert_eq!(speeds, [Some(0.712), None]);
    change(&places, |_| Ok(())).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "m1 u@m1 10 0.712\nnew u@new 8\n"
    );
    std::fs::write(&file, "m1 u@m1 10 fast\n").unwrap();
    assert!(read_remote(&places).is_err());
    std::fs::write(&file, "m1 u@m1 10 0\n").unwrap();
    assert!(read_remote(&places).is_err());
}

/// The hosts file keeps a band for this Mac and for a recorded host, `all`
/// gives each host its band, and a host with none has none.
#[test]
fn the_hosts_file_keeps_each_hosts_load_band() {
    let scratch = Scratch::new("host-bands");
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 u@m1 10 0.712\nnew u@new 8\n").unwrap();
    assert!(all(&places).unwrap().iter().all(|host| host.band.is_none()));

    let local = Band::parse("25", "30").unwrap();
    let m1 = Band::parse("20", "25").unwrap();
    set_band(&places, LOCAL, Some(local)).unwrap();
    set_band(&places, "m1", Some(m1)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "m1 u@m1 10 0.712\nnew u@new 8\nband local 25 30\nband m1 20 25\n"
    );
    let bands: Vec<Option<Band>> = all(&places).unwrap().iter().map(|h| h.band).collect();
    assert_eq!(bands, [Some(local), Some(m1), None]);

    // Another write to the file keeps the bands.
    record_speed(&places, "new", 0.5).unwrap();
    let bands: Vec<Option<Band>> = all(&places).unwrap().iter().map(|h| h.band).collect();
    assert_eq!(bands, [Some(local), Some(m1), None]);

    // Removing a host removes its band, and clearing a band removes its line.
    remove(&places, "m1").unwrap();
    set_band(&places, LOCAL, None).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "new u@new 8 0.500\n"
    );
    let fault = set_band(&places, "gone", Some(local)).unwrap_err().text();
    assert!(fault.contains("no host named gone"), "{fault}");
}

/// Changes made to the hosts file at once are all kept: here each host's
/// speed and band, each written by a thread of its own.
#[test]
fn changes_to_the_hosts_file_made_at_once_are_all_kept() {
    let scratch = Scratch::new("host-concurrent-changes");
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let names: Vec<String> = (0..16).map(|n| format!("m{n}")).collect();
    let text: String = names.iter().map(|n| format!("{n} u@{n} 8\n")).collect();
    std::fs::write(&file, text).unwrap();
    let band = Band::parse("20", "25").unwrap();
    let start = std::sync::Barrier::new(2 * names.len());
    std::thread::scope(|scope| {
        for name in &names {
            scope.spawn(|| {
                start.wait();
                record_speed(&places, name, 0.5).unwrap();
            });
            scope.spawn(|| {
                start.wait();
                set_band(&places, name, Some(band)).unwrap();
            });
        }
    });
    for host in read_remote(&places).unwrap() {
        assert_eq!(
            (host.speed, host.band),
            (Some(0.5), Some(band)),
            "{}",
            host.name
        );
    }
}

/// A host with no band has the shares of its cores; a band that is not two
/// numbers with the low end under the high end, or that names a host that is
/// not recorded, is an error.
#[test]
fn a_host_without_a_band_keeps_the_shares_of_its_cores_and_a_bad_band_fails() {
    let band = host("m1").band_for(10);
    assert_eq!((band.low, band.high), (7.5, 12.5));
    for (low, high) in [
        ("30", "25"),
        ("25", "25"),
        ("0", "5"),
        ("x", "5"),
        ("5", "inf"),
    ] {
        assert!(Band::parse(low, high).is_err(), "{low} {high}");
    }
    let scratch = Scratch::new("host-bad-bands");
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for text in [
        "m1 u@m1 10\nband m1 30 25\n",
        "m1 u@m1 10\nband gone 20 25\n",
    ] {
        std::fs::write(&file, text).unwrap();
        assert!(read_file(&places).is_err(), "{text}");
    }
}

/// A script run at the terminal runs on the host through `ssh -t`.
#[test]
fn a_script_at_the_terminal_runs_on_the_host() {
    let scratch = Scratch::new("host-terminal");
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch, &home);
    let m1 = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    run_at_terminal(&ssh, &m1, "touch ran && touch 'ran too'").unwrap();
    assert!(home.join("ran").exists() && home.join("ran too").exists());
    let Err(Fail::Message(message)) = run_at_terminal(&ssh, &m1, "exit 1") else {
        panic!("expected a failure");
    };
    assert!(message.starts_with("sudo on host m1 failed"), "{message}");
}

fn sh(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[test]
fn output_of_requires_a_successful_exit() {
    assert_eq!(output_of(sh("echo a"), "x").unwrap(), "a\n");
    assert!(output_of(sh("echo a; exit 1"), "x").is_err());
    let said = output_of(sh("echo a; echo dropped >&2; exit 255"), "ssh").unwrap_err();
    assert!(said.contains("255") && said.contains("dropped"), "{said}");
}

#[test]
fn a_probe_reads_what_a_host_has_of_cargo_mutants() {
    use crate::cargo_program::Has;
    let said = "os=Darwin\narch=arm64\ncores=10\nload=1.50 1.20 1.10\nfree_kib=4\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=1\nmax_procs=2\n";
    assert_eq!(parse_probe(said).unwrap().mutants, Has::Nothing);
    assert_eq!(
        parse_probe(&format!("{said}mutants=27.1.0\nnextest=0.9.143\n"))
            .unwrap()
            .mutants,
        Has::Version("27.1.0".to_string())
    );
    assert_eq!(
        parse_probe(&format!("{said}mutants_fails=dyld: missing \n"))
            .unwrap()
            .mutants,
        Has::Broken("dyld: missing".to_string())
    );
}

/// A mutants run skips a host that has no cargo-mutants at the version builds
/// installs, saying which, while a cargo run does not.
#[test]
fn a_mutants_run_passes_over_a_host_without_the_pinned_cargo_mutants() {
    use crate::cargo_program::{Has, MUTANTS};
    let with = |mutants: Has| {
        Ok(Probe {
            platform: crate::host_platform::Platform::here(),
            cores: 10,
            load: [0.0; 3],
            free_kib: 100,
            memory: crate::host_memory::Memory::at_rest(),
            git_fails: None,
            nextest: Has::Nothing,
            mutants,
            node: Has::Version(crate::cargo_program::NODE.version.to_string()),
            processes: crate::proc_limit::ProcessUse {
                count: 100,
                limit: 10_000,
            },
        })
    };
    let hosts = vec![
        (
            host("local"),
            with(Has::Version(MUTANTS.version.to_string())),
        ),
        (host("m1"), with(Has::Nothing)),
        (host("m2"), with(Has::Version("26.0.0".to_string()))),
    ];
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default(), Runs::Mutants);
    let names: Vec<String> = ranked.into_iter().map(|(host, _)| host.name).collect();
    assert_eq!(names, ["local"]);
    assert!(
        why.contains("m1 has no cargo-mutants (`builds host setup m1` installs 27.1.0)")
            && why.contains(
                "m2 has cargo-mutants 26.0.0, not 27.1.0 (`builds host setup m2` installs 27.1.0)"
            ),
        "{why}"
    );
    let (ranked, _) = rank_by_room(&hosts, 50, &Default::default(), Runs::Cargo);
    assert_eq!(ranked.len(), 3);
}

#[test]
fn a_host_with_its_lid_closed_and_no_display_cannot_draw_a_window_scene() {
    assert_eq!(
        screen_lacking("lid=closed\nscreens=0\n")
            .unwrap()
            .as_deref(),
        Some("its lid is closed and it lists no other display")
    );
    assert_eq!(screen_lacking("lid=open\n").unwrap(), None);
    assert_eq!(
        screen_lacking("lid=none\n").unwrap(),
        None,
        "a Mac with no lid"
    );
    assert_eq!(
        screen_lacking("lid=closed\nscreens=1\n").unwrap(),
        None,
        "another display is attached"
    );
}

#[test]
fn an_answer_that_names_no_lid_or_no_screens_count_is_an_error_not_a_yes() {
    assert!(screen_lacking("").unwrap_err().contains("names no lid"));
    assert!(screen_lacking("lid=ajar\n").is_err());
    assert!(
        screen_lacking("lid=closed\n")
            .unwrap_err()
            .contains("no screens count")
    );
    assert!(screen_lacking("lid=closed\nscreens=many\n").is_err());
}

/// The screen script reads the lid from the system's ioreg and the
/// displays from its system_profiler.
#[test]
fn the_screen_script_reads_the_lid_and_the_displays() {
    let scratch = Scratch::new("screen-script");
    scratch.stand_in("ioreg", "echo '\"AppleClamshellState\" = Yes'");
    scratch.stand_in(
        "system_profiler",
        "echo 'Resolution: 1920 x 1080'; echo 'Resolution: 3024 x 1964'",
    );
    let said = output_at(
        &scratch.system().ssh(),
        &Host::local(),
        SCREEN_SCRIPT,
        "running the screen script here",
    )
    .unwrap();
    assert_eq!(said, "lid=closed\nscreens=2\n");
    assert_eq!(screen_lacking(&said), Ok(None));
}
