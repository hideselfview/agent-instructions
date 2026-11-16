use super::*;
use crate::fail::Fail;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;

/// A program in place of ssh: it runs the command it is given in <home>
/// (which stands for the remote Mac's home), and refuses the host named
/// `unreachable` as ssh refuses a host that does not answer. Named `ssh`, so
/// git treats it as ssh.
pub fn fake_ssh(dir: &Path, home: &Path) -> Ssh {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::create_dir_all(home).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!(
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
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
cd '{home}' && HOME='{home}' BUILDS_VOLUME='{home}/volume' exec sh -c "$*"
"#,
            home = home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Ssh {
        program: program.display().to_string(),
    }
}

use std::path::Path;

#[test]
fn reads_what_a_host_prints() {
    let probe = parse_probe("cores=10\nload=1.50 1.20 1.10 \nfree_kib=447000000\n").unwrap();
    assert_eq!(
        probe,
        Probe {
            cores: 10,
            load: [1.5, 1.2, 1.1],
            free_kib: 447_000_000,
            git_fails: None,
        }
    );
    assert!((probe.load_per_core() - 0.15).abs() < 1e-9);
    assert!(parse_probe("cores=10\n").is_err());
    assert!(parse_probe("cores=10\nload=1.50\nfree_kib=4\n").is_err());
    let refusing = parse_probe(
        "cores=10\nload=1.50 1.20 1.10\nfree_kib=4\ngit_fails=You have not agreed to the Xcode license agreements. \n",
    )
    .unwrap();
    assert_eq!(
        refusing.git_fails.as_deref(),
        Some("You have not agreed to the Xcode license agreements.")
    );
}

#[test]
fn a_host_whose_git_fails_is_passed_over_saying_what_git_said() {
    let refusing = || {
        Ok(Probe {
            cores: 10,
            load: [0.0; 3],
            free_kib: 100,
            git_fails: Some("no license".to_string()),
        })
    };
    let hosts = vec![
        (host("local"), probe_with(8, 6.0, 100)),
        (host("m1"), refusing()),
    ];
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default());
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
    record_refusal(&places, Refused::MacBuilds, "m1", "plugin").unwrap();
    // `refusals` lists this Mac's record of refusing mac app builds too,
    // because this Mac is always a host.
    record_refusal(&places, Refused::MacBuilds, LOCAL, "plugin here").unwrap();
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
            (LOCAL, Refused::MacBuilds, "plugin here"),
            ("m1", Refused::Jobs, "first"),
            ("m1", Refused::MacBuilds, "plugin"),
        ]
    );
    assert!(found[0].at > 0);
    assert_eq!(
        refusing(&places, Refused::MacBuilds).unwrap(),
        vec![LOCAL.to_string(), "m1".to_string()]
    );
    clear_refusal(&places, Refused::Jobs, "m1").unwrap();
    assert_eq!(
        refusing(&places, Refused::MacBuilds).unwrap(),
        vec![LOCAL.to_string(), "m1".to_string()],
        "clearing one kind leaves the other"
    );
    remove(&places, "m1").unwrap();
    std::fs::write(&file, "m1 u@m1 10\n").unwrap();
    assert_eq!(
        refusing(&places, Refused::MacBuilds).unwrap(),
        vec![LOCAL.to_string()],
        "removing m1 removed its records"
    );
}

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

fn probe_of(load: f64, free_kib: u64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        cores: 10,
        load: [load, load, load],
        free_kib,
        git_fails: None,
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
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
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
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    let Err(Fail::Message(message)) = add(&places, &ssh, "gone", "unreachable") else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone is not reachable"), "{message}");
    assert!(message.contains("Connection refused"), "{message}");
    assert!(read_remote(&places).unwrap().is_empty());
}

#[test]
fn install_program_copies_builds_to_the_host_and_makes_its_root() {
    let scratch = Scratch::new("host-setup");
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let host = Host {
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    install_program(&ssh, &host).unwrap();
    let installed = home.join(".local/bin/builds");
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(std::env::current_exe().unwrap()).unwrap()
    );
    assert!(home.join(REMOTE_ROOT).is_dir());
    assert!(!home.join(".local/bin/builds.part").exists());
    assert_eq!(
        std::fs::read_link(crate::linker::path(&home)).unwrap(),
        Path::new("builds"),
        "builds-linker runs the host's builds"
    );
    // A second install leaves the link as it is.
    install_program(&ssh, &host).unwrap();
    assert_eq!(
        std::fs::read_link(crate::linker::path(&home)).unwrap(),
        Path::new("builds")
    );
    assert!(install_program(&ssh, &Host::local()).is_err());
    let gone = Host {
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
        speed: Some(1.0),
    };
    let Err(Fail::Message(message)) = install_program(&ssh, &gone) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone"), "{message}");
}

fn probe_with(cores: u32, load: f64, free_kib: u64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        cores,
        load: [load, load, load],
        free_kib,
        git_fails: None,
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
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default());
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
        rank_by_room(&tie, 0, &Default::default()).0[0].0.name,
        "local"
    );

    let loaded = vec![(host("m1"), probe_with(10, 14.0, 100))];
    assert_eq!(
        rank_by_room(&loaded, 0, &Default::default()).0.len(),
        1,
        "a host with no room still ranks"
    );
    assert!(
        rank_by_room(&[(host("m1"), Err("x".into()))], 0, &Default::default())
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
    let (ranked, why) = rank_by_room(&hosts, 50, &Default::default());
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
    let (ranked, _) = rank_by_room(&busy, 50, &Default::default());
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
    write_remote(&places, &hosts).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "m1 u@m1 10 0.712\nnew u@new 8\n"
    );
    std::fs::write(&file, "m1 u@m1 10 fast\n").unwrap();
    assert!(read_remote(&places).is_err());
    std::fs::write(&file, "m1 u@m1 10 0\n").unwrap();
    assert!(read_remote(&places).is_err());
}

#[test]
fn reads_what_a_host_has_installed() {
    assert_eq!(
        parse_installed("builds=ab12\nsccache=launchd\n").unwrap(),
        Installed {
            builds: Some("ab12".to_string()),
            sccache_under_launchd: true,
        }
    );
    assert_eq!(
        parse_installed("builds=missing\nsccache=not-launchd\n").unwrap(),
        Installed {
            builds: None,
            sccache_under_launchd: false,
        }
    );
    assert!(parse_installed("builds=\nsccache=launchd\n").is_err());
    assert!(parse_installed("builds=ab12\n").is_err());
    assert!(parse_installed("").is_err());
}

/// A host with no builds, or another builds than this Mac's, is a finding
/// that names `builds host setup`; a host that got this Mac's copy is none.
#[test]
fn a_host_whose_builds_is_not_this_macs_is_a_finding() {
    let scratch = Scratch::new("host-installed");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let mine = places.home.join(".local/bin/builds");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &mine).unwrap();
    let m1 = host("m1");
    let gone = Host {
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
        speed: Some(1.0),
    };
    let hosts = [Host::local(), m1.clone(), gone];
    // A host that does not answer is a finding that says why.
    let all = installed_findings(&read_all_installed(&places, &ssh, &hosts));
    assert!(
        all.iter().any(|advice| advice
            .text
            .starts_with("builds could not read what host gone has installed")
            && advice.text.contains("Connection refused")),
        "{all:?}"
    );
    // Neither home has builds' sccache plist, so each also has its sccache
    // finding; this test reads m1's builds ones.
    let findings = || -> Vec<crate::health::Advice> {
        installed_findings(&read_all_installed(&places, &ssh, &hosts))
            .into_iter()
            .filter(|advice| advice.text.starts_with("host m1") && !advice.text.contains("sccache"))
            .collect()
    };

    let found = findings();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.starts_with("host m1 has no builds"),
        "{found:?}"
    );
    assert_eq!(found[0].command.as_deref(), Some("builds host setup m1"));

    install_program(&ssh, &m1).unwrap();
    assert_eq!(findings(), [], "m1 has this Mac's builds");

    std::fs::write(home.join(".local/bin/builds"), "older").unwrap();
    let found = findings();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .text
            .starts_with("host m1 runs a different version of builds"),
        "{found:?}"
    );
}

/// A Mac whose sccache server launchd does not run is a finding: a host's
/// finding names `builds host setup`, this Mac's `builds install`.
/// read_all_installed reports that launchd does not run sccache when the
/// home has no plist from builds.
#[test]
fn a_host_whose_sccache_runs_outside_launchd_is_a_finding() {
    let installed = |sccache_under_launchd| Installed {
        builds: Some("ab12".to_string()),
        sccache_under_launchd,
    };
    let read = |host: &str, under: bool| HostInstalled {
        host: host.to_string(),
        read: Ok(installed(under)),
    };
    assert_eq!(
        installed_findings(&[read(LOCAL, true), read("m1", true)]),
        []
    );
    let found = installed_findings(&[read(LOCAL, false), read("m1", false)]);
    let shown: Vec<(bool, Option<&str>)> = found
        .iter()
        .map(|advice| (advice.text.contains("sccache"), advice.command.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            (true, Some("builds install")),
            (true, Some("builds host setup m1"))
        ]
    );

    let scratch = Scratch::new("host-sccache");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    let reads = read_all_installed(&places, &ssh, &[Host::local(), host("m1")]);
    let under: Vec<bool> = reads
        .iter()
        .map(|read| read.read.as_ref().unwrap().sccache_under_launchd)
        .collect();
    assert_eq!(under, [false, false]);
}

/// A builds command run on a host comes back with what it printed on stdout
/// and stderr, and a failing one fails with all of that, naming the host.
#[test]
fn a_builds_command_on_a_host_returns_its_output_or_fails_with_it() {
    let scratch = Scratch::new("host-run-builds");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let builds = home.join(".local/bin/builds");
    std::fs::create_dir_all(builds.parent().unwrap()).unwrap();
    std::fs::write(
        &builds,
        "#!/bin/sh\necho \"said $*\" >&2\necho ms=5\n[ \"$1\" = fail ] && exit 3\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&builds, std::fs::Permissions::from_mode(0o755)).unwrap();
    let m1 = host("m1");
    let printed =
        run_builds_there(&places, &ssh, &m1, "host speed-here", "timing a compile").unwrap();
    assert_eq!(printed, "said host speed-here\nms=5\n");
    let Err(Fail::Message(message)) = run_builds_there(&places, &ssh, &m1, "fail", "setting up")
    else {
        panic!("expected a failure");
    };
    assert!(
        message.contains("setting up failed on host m1") && message.contains("said fail"),
        "{message}"
    );
}
