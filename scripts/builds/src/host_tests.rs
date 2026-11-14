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
cd '{home}' && HOME='{home}' exec sh -c "$*"
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
            load: 1.5,
            free_kib: 447_000_000
        }
    );
    assert!((probe.load_per_core() - 0.15).abs() < 1e-9);
    assert!(parse_probe("cores=10\n").is_err());
}

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
    }
}

fn probe_of(load: f64, free_kib: u64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        cores: 10,
        load,
        free_kib,
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
fn setup_copies_builds_to_the_host_and_makes_its_root() {
    let scratch = Scratch::new("host-setup");
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let host = Host {
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
    };
    setup_host(&ssh, &host).unwrap();
    let installed = home.join(".local/bin/builds");
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(std::env::current_exe().unwrap()).unwrap()
    );
    assert!(home.join(REMOTE_ROOT).is_dir());
    assert!(!home.join(".local/bin/builds.part").exists());
    assert!(setup_host(&ssh, &Host::local()).is_err());
    let gone = Host {
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
    };
    let Err(Fail::Message(message)) = setup_host(&ssh, &gone) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone"), "{message}");
}
