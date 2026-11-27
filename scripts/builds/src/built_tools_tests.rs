use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::checkout::Checkout;
use crate::host::tests::fake_ssh;
use crate::setup::Prepare;
use crate::testing::{Scratch, git, repo};

const MANIFEST: &str = "target 1G\ntool lints tool.sh built\n";

/// A tool program that prints the key in key.txt and builds a folder holding
/// `tool` and the check in check.sh, counting its builds in the work
/// folder's builds.txt.
const PROGRAM: &str = r#"#!/bin/sh
case "$1" in
key) cat key.txt ;;
build)
  mkdir -p "$2"
  echo built >> "$BUILDS_TOOL_WORK/builds.txt"
  echo tool > "$2/tool"
  cp check.sh "$2/check"
  ;;
esac
"#;

/// A check that passes where the folder holds `tool`, and counts its runs in
/// the folder's checks.txt.
const PASSING: &str = "#!/bin/sh\n[ -f tool ] && echo ran >> checks.txt\n";

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A repo at <top> that declares tool `lints` with key <key> and <check>.
fn tool_repo(top: &Path, key: &str, check: &str) -> BuiltTool {
    repo(top, MANIFEST);
    executable(&top.join("tool.sh"), PROGRAM);
    executable(&top.join("check.sh"), check);
    std::fs::write(top.join("key.txt"), format!("{key}\n")).unwrap();
    built_tools(top).unwrap().remove(0)
}

fn no_ssh() -> Ssh {
    crate::testing::no_ssh()
}

fn key_of(text: &str) -> Key {
    parse_key(text).unwrap()
}

fn builds_count(places: &Places, toolchain: &str) -> usize {
    std::fs::read_to_string(
        tool_dir(places, "lints")
            .join(toolchain)
            .join(WORK)
            .join("builds.txt"),
    )
    .map(|text| text.lines().count())
    .unwrap_or(0)
}

#[test]
fn a_key_is_a_toolchain_and_a_sources_hash() {
    assert_eq!(
        parse_key("abc123 f00d\n").unwrap(),
        Key {
            toolchain: "abc123".into(),
            sources: "f00d".into()
        }
    );
    for bad in ["", "abc", "a b c", ".a b", "a b/c"] {
        assert!(parse_key(bad).is_err(), "{bad:?} passed");
    }
}

#[test]
fn builds_a_missing_key_once_and_links_every_checkout_to_it() {
    let scratch = Scratch::new("tools-build-once");
    let places = scratch.places();
    let main = scratch.dir.join("demo");
    let line = tool_repo(&main, "tc1 src1", PASSING);
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    for file in ["tool.sh", "check.sh", "key.txt"] {
        std::fs::copy(main.join(file), side.join(file)).unwrap();
    }

    let dest = ensure(&places, &no_ssh(), &main, &line, Prepare::Yes).unwrap();
    assert_eq!(dest, key_dir(&places, "lints", &key_of("tc1 src1")));
    assert!(dest.join("tool").is_file());
    assert_eq!(std::fs::read_link(main.join("built")).unwrap(), dest);
    assert_eq!(
        std::fs::read_to_string(dest.join("checks.txt")).unwrap(),
        "ran\n",
        "the build ran its check before it took its place"
    );

    assert_eq!(
        ensure(&places, &no_ssh(), &side, &line, Prepare::Yes).unwrap(),
        dest
    );
    assert_eq!(std::fs::read_link(side.join("built")).unwrap(), dest);
    assert_eq!(
        builds_count(&places, "tc1"),
        1,
        "a second checkout with the key built it again"
    );

    std::fs::write(side.join("key.txt"), "tc1 src2\n").unwrap();
    let other = ensure(&places, &no_ssh(), &side, &line, Prepare::Yes).unwrap();
    assert_eq!(other, key_dir(&places, "lints", &key_of("tc1 src2")));
    assert_eq!(std::fs::read_link(side.join("built")).unwrap(), other);
    assert_eq!(builds_count(&places, "tc1"), 2);
}

#[test]
fn a_build_whose_check_fails_never_takes_its_place() {
    let scratch = Scratch::new("tools-check-fails");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(
        &top,
        "tc1 src1",
        "#!/bin/sh\necho no toolchain >&2\nexit 1\n",
    );
    let Err(Fail::Message(message)) = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes) else {
        panic!("a failing check passed");
    };
    assert!(message.contains("no toolchain"), "{message}");
    assert!(!key_dir(&places, "lints", &key_of("tc1 src1")).exists());
    assert_eq!(
        listed(&tool_dir(&places, "lints").join("tc1")).unwrap(),
        Vec::<String>::new()
    );
    assert!(!top.join("built").exists());
}

/// A host m1 whose home is <scratch>/remote-home, with its build volume, and
/// where it keeps tool `lints`.
fn host_m1(scratch: &Scratch, places: &Places) -> (Ssh, Host, PathBuf) {
    let home = scratch.dir.join("remote-home");
    std::fs::create_dir_all(home.join("volume")).unwrap();
    let ssh = fake_ssh(scratch, &home);
    let file = crate::host::hosts_file(places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 user@m1 10 1.0\n").unwrap();
    let host = Host {
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
        band: None,
    };
    (ssh, host, home.join("volume/caches/tools/lints"))
}

#[test]
fn copies_a_key_from_a_host_that_has_it_instead_of_building_it() {
    let scratch = Scratch::new("tools-fetch");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    let (ssh, _, there) = host_m1(&scratch, &places);
    let remote = there.join("tc1/src1");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::write(remote.join("tool"), "tool\n").unwrap();
    executable(&remote.join("check"), PASSING);

    let dest = ensure(&places, &ssh, &top, &line, Prepare::Yes).unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.join("tool")).unwrap(),
        "tool\n"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("checks.txt")).unwrap(),
        "ran\n",
        "the copy ran its check here"
    );
    assert_eq!(
        builds_count(&places, "tc1"),
        0,
        "builds built a key a host had"
    );
}

#[test]
fn sends_a_key_the_host_lacks_and_checks_it_there() {
    let scratch = Scratch::new("tools-send");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    let (ssh, host, there) = host_m1(&scratch, &places);
    let here = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes).unwrap();

    send_to(&places, &ssh, &host, &top);
    let remote = there.join("tc1/src1");
    assert_eq!(
        std::fs::read_to_string(remote.join("tool")).unwrap(),
        "tool\n"
    );
    assert_eq!(
        std::fs::read_to_string(remote.join("checks.txt")).unwrap(),
        "ran\nran\n",
        "the check ran here and again on the host"
    );
    assert_eq!(
        listed(&there.join("tc1")).unwrap(),
        vec!["src1".to_string()]
    );

    // A host that has the key gets no second copy.
    std::fs::write(remote.join("tool"), "the host's own\n").unwrap();
    send_to(&places, &ssh, &host, &top);
    assert_eq!(
        std::fs::read_to_string(remote.join("tool")).unwrap(),
        "the host's own\n"
    );

    // A copy whose check fails on the host leaves nothing there.
    std::fs::write(top.join("key.txt"), "tc1 src2\n").unwrap();
    let failing = here.with_file_name("src2");
    std::fs::create_dir_all(&failing).unwrap();
    executable(&failing.join("check"), "#!/bin/sh\nexit 1\n");
    send_to(&places, &ssh, &host, &top);
    assert_eq!(
        listed(&there.join("tc1")).unwrap(),
        vec!["src1".to_string()]
    );
    assert!(
        std::fs::read_dir(there.join("tc1")).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".new-")
        }),
        "the failed copy was left on the host"
    );
}

#[test]
fn prune_deletes_the_keys_and_toolchains_no_checkout_names() {
    let scratch = Scratch::new("tools-prune");
    let tool = scratch.tool(0);
    let places = &tool.places;
    let top = scratch.dir.join("demo");
    tool_repo(&top, "tc1 src1", PASSING);
    crate::setup::setup(
        &tool,
        &crate::testing::no_ssh(),
        &Checkout::find(&top.display().to_string()).unwrap(),
    )
    .unwrap();
    assert!(key_dir(places, "lints", &key_of("tc1 src1")).is_dir());
    for (toolchain, sources) in [("tc1", "old"), ("tc0", "src1")] {
        std::fs::create_dir_all(tool_dir(places, "lints").join(toolchain).join(sources)).unwrap();
    }

    prune_tools(places, &tool.roots().unwrap(), Run::Real).unwrap();
    assert_eq!(
        listed(&tool_dir(places, "lints")).unwrap(),
        vec!["tc1".to_string()]
    );
    assert_eq!(
        listed(&tool_dir(places, "lints").join("tc1")).unwrap(),
        vec!["src1".to_string()]
    );
    assert!(
        tool_dir(places, "lints").join("tc1").join(WORK).is_dir(),
        "prune deleted the work folder of a toolchain a checkout names"
    );

    // A checkout whose key can't be read keeps every build of the tool.
    std::fs::write(top.join("key.txt"), "not a key\n").unwrap();
    std::fs::create_dir_all(tool_dir(places, "lints").join("tc0")).unwrap();
    prune_tools(places, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(tool_dir(places, "lints").join("tc0").is_dir());
}

#[test]
fn setup_runs_each_prepare_program_and_fails_naming_the_one_that_fails() {
    let scratch = Scratch::new("tools-prepare");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "target 1G\nprepare a.sh\nprepare b.sh\n");
    executable(&top.join("a.sh"), "#!/bin/sh\necho a >> ran.txt\n");
    executable(&top.join("b.sh"), "#!/bin/sh\necho b >> ran.txt\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    crate::setup::setup(&tool, &crate::testing::no_ssh(), &checkout).unwrap();
    assert_eq!(
        std::fs::read_to_string(top.join("ran.txt")).unwrap(),
        "a\nb\n"
    );

    executable(&top.join("a.sh"), "#!/bin/sh\nexit 3\n");
    let Err(Fail::Message(message)) =
        crate::setup::setup(&tool, &crate::testing::no_ssh(), &checkout)
    else {
        panic!("a failing prepare program passed");
    };
    assert!(message.contains("a.sh"), "{message}");
    assert_eq!(
        std::fs::read_to_string(top.join("ran.txt")).unwrap(),
        "a\nb\n",
        "a program after the failing one ran"
    );
}

#[test]
fn fetch_tells_a_host_that_lacks_the_key_from_one_it_could_not_ask() {
    let scratch = Scratch::new("tools-fetch-misses");
    let places = scratch.places();
    let (ssh, _, _) = host_m1(&scratch, &places);
    let toolchain = tool_dir(&places, "lints").join("tc1");
    std::fs::create_dir_all(&toolchain).unwrap();
    let key = key_of("tc1 src1");

    let fetched = fetch(&places, &ssh, "lints", &key, &toolchain).unwrap();
    assert!(fetched.copy.is_none());
    assert_eq!(fetched.misses, vec!["host m1 does not have it".to_string()]);

    std::fs::write(crate::host::hosts_file(&places), "m1 unreachable 10 1.0\n").unwrap();
    let fetched = fetch(&places, &ssh, "lints", &key, &toolchain).unwrap();
    assert!(fetched.copy.is_none());
    assert!(
        fetched.misses[0].starts_with("builds could not ask host m1 for it:"),
        "{:?}",
        fetched.misses
    );
}

#[test]
fn a_rejected_copy_builds_cannot_delete_is_an_error() {
    let scratch = Scratch::new("tools-discard");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    let (ssh, _, there) = host_m1(&scratch, &places);
    let remote = there.join("tc1/src1");
    std::fs::create_dir_all(&remote).unwrap();
    // A check that fails and leaves a folder nothing can delete files from.
    executable(
        &remote.join("check"),
        "#!/bin/sh\nmkdir locked && touch locked/file && chmod 555 locked\nexit 1\n",
    );

    let Err(Fail::Message(message)) = ensure(&places, &ssh, &top, &line, Prepare::Yes) else {
        panic!("a copy that could not be deleted passed");
    };
    assert!(
        message.starts_with("the copy from host m1 failed: "),
        "{message}"
    );
    assert!(message.contains("; deleting "), "{message}");
    assert_eq!(builds_count(&places, "tc1"), 0, "builds built beside it");
    let toolchain = tool_dir(&places, "lints").join("tc1");
    for entry in std::fs::read_dir(&toolchain).unwrap() {
        let locked = entry.unwrap().path().join("locked");
        if locked.exists() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[test]
fn prune_keeps_every_build_while_a_checkouts_manifest_cannot_be_read() {
    let scratch = Scratch::new("tools-prune-unread");
    let tool = scratch.tool(0);
    let places = &tool.places;
    let top = scratch.dir.join("demo");
    tool_repo(&top, "tc1 src1", PASSING);
    crate::setup::setup(
        &tool,
        &crate::testing::no_ssh(),
        &Checkout::find(&top.display().to_string()).unwrap(),
    )
    .unwrap();
    let old = tool_dir(places, "lints").join("tc0").join("src0");
    std::fs::create_dir_all(&old).unwrap();
    let roots = tool.roots().unwrap();
    std::fs::write(top.join(".build-dirs"), "target 1G\ntool lints\n").unwrap();
    prune_tools(places, &roots, Run::Real).unwrap();
    assert!(
        old.is_dir(),
        "prune deleted a build while a manifest was unread"
    );
}

#[test]
fn a_failed_build_builds_cannot_delete_says_both_failures() {
    let scratch = Scratch::new("tools-build-discard");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    executable(
        &top.join("tool.sh"),
        "#!/bin/sh\ncase \"$1\" in\nkey) cat key.txt ;;\nbuild) mkdir \"$2/locked\" && touch \"$2/locked/file\" && chmod 555 \"$2/locked\"; echo no compiler >&2; exit 4 ;;\nesac\n",
    );
    let Err(Fail::Message(message)) = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes) else {
        panic!("a failed build passed");
    };
    assert!(message.contains("build failed"), "{message}");
    assert!(message.contains("; deleting "), "{message}");
    let toolchain = tool_dir(&places, "lints").join("tc1");
    for entry in std::fs::read_dir(&toolchain).unwrap() {
        let locked = entry.unwrap().path().join("locked");
        if locked.exists() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[test]
fn a_hook_check_only_links_a_build_this_mac_has() {
    let scratch = Scratch::new("tools-hook");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    let Err(Fail::Message(message)) = ensure(&places, &no_ssh(), &top, &line, Prepare::No) else {
        panic!("a missing build passed");
    };
    assert!(message.contains("builds setup"), "{message}");
    assert_eq!(
        builds_count(&places, "tc1"),
        0,
        "a hook check built the tool"
    );
    assert!(!top.join("built").exists());

    let dest = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes).unwrap();
    std::fs::remove_file(top.join("built")).unwrap();
    assert_eq!(
        ensure(&places, &no_ssh(), &top, &line, Prepare::No).unwrap(),
        dest
    );
    assert_eq!(std::fs::read_link(top.join("built")).unwrap(), dest);
}

#[test]
fn a_workspace_wrapper_under_a_tools_link_names_the_build_of_the_checkouts_key() {
    let scratch = Scratch::new("tools-workspace-wrapper");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    let line = tool_repo(&top, "tc1 src1", PASSING);
    let wrapper = top.join("built/tool");
    let Err(Fail::Message(message)) = cargo_workspace_wrapper(&top, &wrapper) else {
        panic!("a wrapper under a link that points nowhere passed");
    };
    assert!(message.contains("builds tool lints"), "{message}");

    let first = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes).unwrap();
    assert_eq!(
        cargo_workspace_wrapper(&top, &wrapper).unwrap(),
        Some(first.join("tool"))
    );
    std::fs::write(top.join("key.txt"), "tc1 src2\n").unwrap();
    let second = ensure(&places, &no_ssh(), &top, &line, Prepare::Yes).unwrap();
    assert_eq!(
        cargo_workspace_wrapper(&top, &wrapper).unwrap(),
        Some(second.join("tool")),
        "the wrapper names the build the link points at now"
    );
    assert_ne!(first, second);

    for other in [top.join("scripts/wrapper.sh"), top.join("builtin/tool")] {
        assert_eq!(
            cargo_workspace_wrapper(&top, &other).unwrap(),
            None,
            "{} is under no tool's link",
            other.display()
        );
    }
}

#[test]
fn setup_copies_this_macs_builds_to_each_host_and_without_prepare_does_nothing_of_it() {
    let scratch = Scratch::new("tools-setup-hosts");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    tool_repo(&top, "tc1 src1", PASSING);
    executable(
        &top.join("prepare.sh"),
        "#!/bin/sh\necho ran >> prepared.txt\n",
    );
    std::fs::write(
        top.join(".build-dirs"),
        format!("{MANIFEST}prepare prepare.sh\n"),
    )
    .unwrap();
    let (ssh, _, there) = host_m1(&scratch, &tool.places);
    let checkout = Checkout::find(&top.display().to_string()).unwrap();

    crate::setup::setup_with(&tool, &ssh, &checkout, crate::setup::Prepare::No).unwrap();
    assert!(!top.join("prepared.txt").exists());
    assert!(!top.join("built").exists());
    assert!(!there.join("tc1/src1").exists());

    crate::setup::setup(&tool, &ssh, &checkout).unwrap();
    assert!(top.join("prepared.txt").is_file());
    assert!(top.join("built").is_symlink());
    assert!(
        there.join("tc1/src1/tool").is_file(),
        "setup copied nothing to the host"
    );
}

#[test]
fn copying_to_the_hosts_fails_on_a_manifest_it_cannot_read() {
    let scratch = Scratch::new("tools-send-unread");
    let places = scratch.places();
    let top = scratch.dir.join("demo");
    tool_repo(&top, "tc1 src1", PASSING);
    let (ssh, _, _) = host_m1(&scratch, &places);
    std::fs::write(top.join(".build-dirs"), "target 1G\ntool lints\n").unwrap();
    let Err(Fail::Message(message)) = send_to_hosts(&places, &ssh, &top) else {
        panic!("an unread manifest counted as no tools");
    };
    assert!(message.contains("'tool' takes a name"), "{message}");
}
