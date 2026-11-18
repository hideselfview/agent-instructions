use super::*;
use crate::fail::Fail;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;

/// What `SCRIPT` prints for a build volume whose indexing is off, as this
/// Mac printed it.
const VOLUME_OFF: &str = "spotlight_root=/Volumes/Builds
spotlight_kind=volume
spotlight_said=Indexing and searching disabled.
";

/// What `SCRIPT` prints for a build volume Spotlight indexes, as m1 printed
/// it.
const VOLUME_ON: &str = "spotlight_root=/Volumes/Builds
spotlight_kind=volume
spotlight_said=Indexing enabled.
";

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

fn volume(said: &str) -> Root {
    Root::Volume {
        root: "/Volumes/Builds".to_string(),
        said: said.to_string(),
    }
}

#[test]
fn a_volume_is_off_only_when_mdutil_says_its_indexing_is_disabled() {
    let off = parse(VOLUME_OFF).unwrap();
    assert_eq!(off, volume("Indexing and searching disabled."));
    let on = parse(VOLUME_ON).unwrap();
    assert_eq!(on, volume("Indexing enabled."));
    assert!(indexing_off("Indexing and searching disabled."));
    assert!(indexing_off("Indexing disabled."));
    assert!(!indexing_off("Indexing enabled."));
    assert!(!indexing_off("Error: unknown indexing state."));
}

#[test]
fn a_folder_is_named_only_when_its_name_ends_in_noindex() {
    let named =
        parse("spotlight_root=/Users/d/builds-root.noindex\nspotlight_kind=folder\n").unwrap();
    assert_eq!(
        named,
        Root::Named {
            root: "/Users/d/builds-root.noindex".to_string()
        }
    );
    let inside = parse("spotlight_root=/tmp/x.noindex/root\nspotlight_kind=folder\n").unwrap();
    assert!(matches!(inside, Root::Named { .. }), "{inside:?}");
    let folder = parse("spotlight_root=/tmp/noindex\nspotlight_kind=folder\n").unwrap();
    assert_eq!(
        folder,
        Root::Folder {
            root: "/tmp/noindex".to_string()
        }
    );
}

#[test]
fn an_answer_without_a_root_that_is_there_fails() {
    let Err(Fail::Message(message)) =
        parse("spotlight_root=/Volumes/Builds\nspotlight_kind=missing\n")
    else {
        panic!("expected a failure");
    };
    assert!(
        message.contains("the build root /Volumes/Builds is not there"),
        "{message}"
    );
    assert!(parse("spotlight_kind=folder\n").is_err());
    assert!(parse("spotlight_root=/Volumes/Builds\nspotlight_kind=volume\n").is_err());
}

/// `read` runs the real script on this Mac against the build root, which in
/// a test is a folder of the scratch's volume.
#[test]
fn reads_this_macs_build_root() {
    let scratch = Scratch::new("spotlight-local");
    let mut places = scratch.places();
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    let found = read(&places, &ssh, &Host::local()).unwrap();
    assert_eq!(
        found,
        Root::Folder {
            root: places.root.display().to_string()
        }
    );
    places.root = scratch.dir.join("it's a root.noindex");
    std::fs::create_dir_all(&places.root).unwrap();
    let found = read(&places, &ssh, &Host::local()).unwrap();
    assert_eq!(
        found,
        Root::Named {
            root: places.root.display().to_string()
        }
    );
}

/// Over ssh, the script reads the host's builds root: here the fake host has
/// no build volume, so the root is ~/builds-root.noindex, which Spotlight
/// skips.
#[test]
fn reads_a_hosts_build_root_over_ssh() {
    let scratch = Scratch::new("spotlight-remote");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    std::fs::create_dir_all(home.join(crate::host::REMOTE_ROOT)).unwrap();
    let found = read(&places, &ssh, &host("m1")).unwrap();
    assert_eq!(
        found,
        Root::Named {
            root: home.join("builds-root.noindex").display().to_string()
        }
    );
    let gone = Host {
        target: Some("unreachable".to_string()),
        ..host("gone")
    };
    assert!(read(&places, &ssh, &gone).is_err());
}

#[test]
fn setup_of_a_root_spotlight_skips_runs_nothing() {
    for (root, said) in [
        (
            volume("Indexing and searching disabled."),
            "Spotlight indexing is off on its build volume /Volumes/Builds",
        ),
        (
            Root::Named {
                root: "/Users/d/builds-root.noindex".to_string(),
            },
            "Spotlight skips its build root /Users/d/builds-root.noindex, whose name ends in .noindex",
        ),
    ] {
        let got = set_up_with("m1", false, root, &mut |_| panic!("nothing to run")).unwrap();
        assert_eq!(got, said);
    }
}

#[test]
fn setup_with_no_one_at_the_terminal_fails_naming_the_command() {
    let Err(Fail::Message(message)) =
        set_up_with("m1", false, volume("Indexing enabled."), &mut |_| {
            panic!("no one can type the password")
        })
    else {
        panic!("expected a failure");
    };
    assert_eq!(
        message,
        "Spotlight indexes the build volume /Volumes/Builds on host m1 (mdutil -s says: Indexing enabled.), so its indexer reads every file a build writes there, and no one is at this terminal to type the sudo password. Run `sudo mdutil -i off '/Volumes/Builds'` on host m1, then `builds host setup m1` checks again"
    );
}

#[test]
fn setup_at_a_terminal_turns_indexing_off() {
    let mut ran = Vec::new();
    let said = set_up_with("local", true, volume("Indexing enabled."), &mut |script| {
        ran.push(script.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(ran, ["sudo mdutil -i off '/Volumes/Builds'"]);
    assert_eq!(
        said,
        "turned off Spotlight indexing on its build volume /Volumes/Builds"
    );
}

#[test]
fn setup_fails_when_sudo_fails() {
    let Err(Fail::Message(message)) =
        set_up_with("m1", true, volume("Indexing enabled."), &mut |_| {
            fail!("sudo on host m1 failed (exit status: 1)")
        })
    else {
        panic!("expected a failure");
    };
    assert_eq!(
        message,
        "Spotlight indexes the build volume /Volumes/Builds on host m1 (mdutil -s says: Indexing enabled.), so its indexer reads every file a build writes there, and sudo on host m1 failed (exit status: 1). Run `sudo mdutil -i off '/Volumes/Builds'` on host m1, then `builds host setup m1` checks again"
    );
}

/// builds never runs mdutil on a folder: mdutil's setting is the whole
/// volume's, and the volume of a folder root holds other files.
#[test]
fn setup_of_a_folder_root_not_named_noindex_fails_and_runs_nothing() {
    let Err(Fail::Message(message)) = set_up_with(
        "m1",
        true,
        Root::Folder {
            root: "/Users/d/elsewhere".to_string(),
        },
        &mut |_| panic!("builds never turns off a volume that holds other files"),
    ) else {
        panic!("expected a failure");
    };
    assert_eq!(
        message,
        "Spotlight may index the build root /Users/d/elsewhere on host m1: the root is a folder whose name does not end in .noindex, on a volume that holds other files. builds keeps Spotlight off a build volume through mdutil, and off a build root in a home folder by its name, ~/builds-root.noindex"
    );
}

#[test]
fn status_says_whether_spotlight_indexes_the_build_root() {
    assert_eq!(
        status_line(Ok(volume("Indexing and searching disabled.")), false),
        "spotlight: indexing is off on the build volume /Volumes/Builds"
    );
    assert_eq!(
        status_line(
            Ok(Root::Named {
                root: "/Users/d/builds-root.noindex".to_string()
            }),
            true
        ),
        "spotlight: skips the build root /Users/d/builds-root.noindex, whose name ends in .noindex"
    );
    assert_eq!(
        status_line(Ok(volume("Indexing enabled.")), false),
        "spotlight: INDEXES the build volume /Volumes/Builds (mdutil -s says: Indexing enabled.), so its indexer reads every file a build writes here. Turn it off from a terminal with `builds host setup local`"
    );
    assert_eq!(
        status_line(Ok(volume("Indexing enabled.")), true),
        "spotlight: INDEXES the build volume /Volumes/Builds (mdutil -s says: Indexing enabled.), so its indexer reads every file a build writes here. Turn it off from a terminal on the Mac that sends jobs here, with `builds host setup <this host's name>`"
    );
    assert_eq!(
        status_line(
            Ok(Root::Folder {
                root: "/tmp/root".to_string()
            }),
            false
        ),
        "spotlight: may index the build root /tmp/root, a folder whose name does not end in .noindex, on a volume that holds other files"
    );
    assert_eq!(
        status_line(Err(Fail::Message("ssh failed".to_string())), false),
        "spotlight: builds could not read whether Spotlight indexes the build root: ssh failed"
    );
}
