use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
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

    crate::host_setup::install_program(&ssh, &m1).unwrap();
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
