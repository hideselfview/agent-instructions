use super::*;
use crate::fail::Fail;
use crate::host::REMOTE_ROOT;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn host(name: &str) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

#[test]
fn install_program_copies_both_programs_to_the_host_and_makes_its_root() {
    let scratch = Scratch::new("host-setup");
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let host = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    let programs = scratch.programs();
    install_program(&ssh, &host, &programs).unwrap();
    let installed = crate::links::path(&home, "builds");
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(&programs.builds).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(crate::links::path(&home, "builds-shots")).unwrap(),
        "shots"
    );
    assert!(home.join(REMOTE_ROOT).is_dir());
    assert!(!crate::links::path(&home, "builds.part").exists());
    for link in crate::links::LINKS {
        let name = link.name;
        assert_eq!(
            std::fs::read_link(crate::links::path(&home, name)).unwrap(),
            Path::new("builds"),
            "{name} runs the host's builds"
        );
    }
    // A second install leaves the link as it is.
    install_program(&ssh, &host, &programs).unwrap();
    assert_eq!(
        std::fs::read_link(crate::links::path(&home, crate::linker::NAME)).unwrap(),
        Path::new("builds")
    );
    assert!(install_program(&ssh, &Host::local(), &programs).is_err());
    let gone = Host {
        band: None,
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
        speed: Some(1.0),
    };
    let Err(Fail::Message(message)) = install_program(&ssh, &gone, &programs) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone"), "{message}");
}

/// Installs that run at the same time on one host, as two landings' installs
/// do, each write their own temporary file, so each succeeds and leaves the
/// whole program and its link.
#[test]
fn installs_at_the_same_time_on_one_host_all_succeed() {
    let scratch = Scratch::new("host-setup-together");
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let m1 = host("m1");
    let programs = scratch.programs();
    let results: Vec<Result<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| install_program(&ssh, &m1, &programs)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    for result in results {
        result.unwrap();
    }
    assert_eq!(
        std::fs::read(crate::links::path(&home, "builds")).unwrap(),
        std::fs::read(&programs.builds).unwrap()
    );
    for link in crate::links::LINKS {
        let name = link.name;
        assert_eq!(
            std::fs::read_link(crate::links::path(&home, name)).unwrap(),
            Path::new("builds")
        );
    }
    let left: Vec<String> = std::fs::read_dir(crate::links::bin(&home))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name != "builds"
                && !crate::links::LINKS.iter().any(|link| link.name == name)
                && name != crate::shots_program::PROGRAM
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// A builds command run on a host comes back with what it printed on stdout
/// and stderr, and a failing one fails with all of that, naming the host.
#[test]
fn a_builds_command_on_a_host_returns_its_output_or_fails_with_it() {
    let scratch = Scratch::new("host-run-builds");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let builds = crate::links::path(&home, "builds");
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
