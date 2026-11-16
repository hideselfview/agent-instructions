use super::*;
use crate::fail::Fail;
use crate::host::REMOTE_ROOT;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
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
    let exe = std::env::current_exe().unwrap();
    install_program(&ssh, &host, &exe).unwrap();
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
    install_program(&ssh, &host, &exe).unwrap();
    assert_eq!(
        std::fs::read_link(crate::linker::path(&home)).unwrap(),
        Path::new("builds")
    );
    assert!(install_program(&ssh, &Host::local(), &exe).is_err());
    let gone = Host {
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
        speed: Some(1.0),
    };
    let Err(Fail::Message(message)) = install_program(&ssh, &gone, &exe) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone"), "{message}");
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
