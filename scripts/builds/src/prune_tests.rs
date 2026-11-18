use super::*;
use crate::testing::{Scratch, age, age_all, fill};

#[test]
fn test_temps_go_after_a_day_and_scratch_entries_after_three() {
    let scratch = Scratch::new("prune-ages");
    let mut tool = scratch.tool(0);
    let tmpdir = scratch.dir.join("T");
    tool.places.tmpdir = Some(tmpdir.clone());
    for (name, hours) in [
        (".tmpOld", 25),
        (".tmpNew", 23),
        ("TemporaryDirectory.Old", 25),
        ("TemporaryDirectory.New", 23),
        ("other", 48),
    ] {
        fill(&tmpdir.join(name).join("f"), 1);
        age(&tmpdir.join(name), hours);
    }
    let pad = tool
        .places
        .scratch_base()
        .join("project/session/scratchpad");
    for (name, hours) in [("old", 73), ("new", 71)] {
        fill(&pad.join(name), 1);
        age(&pad.join(name), hours);
    }
    prune_test_temps(&tool, Run::Real).unwrap();
    prune_scratchpads(&tool, Run::Real).unwrap();
    assert!(
        !tmpdir.join(".tmpOld").exists(),
        "a test temp folder a day old was kept"
    );
    assert!(
        !tmpdir.join("TemporaryDirectory.Old").exists(),
        "a Swift tool's temporary folder a day old was kept"
    );
    assert!(
        tmpdir.join(".tmpNew").exists()
            && tmpdir.join("TemporaryDirectory.New").exists()
            && tmpdir.join("other").exists()
    );
    assert!(
        !pad.join("old").exists(),
        "a scratch entry three days old was kept"
    );
    assert!(pad.join("new").exists());
}

#[test]
fn a_scratch_entry_goes_only_when_its_session_ended_and_nothing_inside_changed_for_three_days() {
    let scratch = Scratch::new("prune-scratch");
    let tool = scratch.tool(0);
    let ended = tool.places.scratch_base().join("project/ended/scratchpad");
    let running = tool
        .places
        .scratch_base()
        .join("project/running/scratchpad");
    // A copy refreshed in place: the folder's own time is old, a file inside is new.
    fill(&ended.join("refreshed/inner/f"), 1);
    age_all(&ended.join("refreshed"), 80);
    age(&ended.join("refreshed"), 80);
    fill(&ended.join("refreshed/inner/g"), 1);
    age(&ended.join("refreshed/inner"), 80);
    age(&ended.join("refreshed"), 80);
    fill(&ended.join("idle/f"), 1);
    age_all(&ended.join("idle"), 80);
    age(&ended.join("idle"), 80);
    fill(&running.join("idle/f"), 1);
    age_all(&running.join("idle"), 80);
    age(&running.join("idle"), 80);
    let sessions = tool.places.claude_sessions();
    std::fs::create_dir_all(&sessions).unwrap();
    let me = unsafe { libc::getpid() };
    std::fs::write(
        sessions.join(format!("{me}.json")),
        format!(r#"{{"pid":{me},"sessionId":"running"}}"#),
    )
    .unwrap();
    prune_scratchpads(&tool, Run::Real).unwrap();
    assert!(
        !ended.join("idle").exists(),
        "an ended session's entry idle three days was kept"
    );
    assert!(
        ended.join("refreshed").exists(),
        "an entry written inside lately was deleted"
    );
    assert!(
        running.join("idle").exists(),
        "a running session's entry was deleted"
    );
}

/// A side worktree, whose builds run without incremental compilation, keeps
/// the incremental folders a lint wrapper writes until nothing wrote them for
/// the rules' idle hours, as main does.
#[test]
fn a_side_worktree_keeps_its_incremental_folders_for_the_idle_hours() {
    let scratch = Scratch::new("prune-incremental");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    crate::setup::setup(&tool, &checkout).unwrap();
    let profile = checkout.build_root(&tool.places).join("target/debug");
    std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    let increment = |name: &str, hours: u64| {
        let dir = profile.join("incremental").join(name);
        fill(&dir.join("s-1/query-cache.bin"), 64);
        age_all(&dir, hours);
        dir
    };
    let recent = increment("linted-1a2b", 1);
    let idle = increment("linted-3c4d", tool.rules.incremental_hours + 1);
    prune_after_build(&tool, &checkout).unwrap();
    assert!(
        recent.exists(),
        "pruning deleted an incremental folder written an hour ago"
    );
    assert!(!idle.exists(), "pruning kept an idle incremental folder");
}

/// Pruning deletes an installed tool's version once no live checkout's file
/// names it. It keeps the version that the file of a checkout without the
/// `installed` line names, and an install that a script is still writing.
#[test]
fn pruning_deletes_an_installed_version_once_no_checkout_names_it() {
    let scratch = Scratch::new("prune-installed");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\ninstalled checker scripts/checker.sh\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    for top in [&main, &side] {
        crate::setup::setup(&tool, &Checkout::find(&top.display().to_string()).unwrap()).unwrap();
    }
    std::fs::write(side.join(".build-dirs"), "target 1G\n").unwrap();
    for (top, version) in [(&main, "aaa111"), (&side, "ddd444")] {
        std::fs::create_dir_all(top.join("scripts")).unwrap();
        std::fs::write(
            top.join("scripts/checker.sh"),
            format!("checker_revision=\"{version}\"\n"),
        )
        .unwrap();
    }
    let folder = tool.places.cache_home.join("checker");
    for name in ["aaa111", "ccc333", "ddd444", ".install.x"] {
        fill(&folder.join(name), 1);
    }
    let roots = tool.roots().unwrap();
    prune_installed(&tool, &roots, Run::Dry).unwrap();
    assert!(
        folder.join("ccc333").exists(),
        "a dry run of pruning deleted a version"
    );
    prune_installed(&tool, &roots, Run::Real).unwrap();
    assert!(
        !folder.join("ccc333").exists(),
        "pruning kept a version no checkout names"
    );
    assert!(
        folder.join("aaa111").exists(),
        "pruning deleted the version that main's file names"
    );
    assert!(
        folder.join("ddd444").exists(),
        "pruning deleted the version that a checkout without the installed line names"
    );
    assert!(
        folder.join(".install.x").exists(),
        "pruning deleted an install that a script is still writing"
    );
    for top in [&main, &side] {
        std::fs::remove_file(top.join("scripts/checker.sh")).unwrap();
    }
    prune_installed(&tool, &roots, Run::Real).unwrap();
    assert!(
        folder.join("aaa111").exists() && folder.join("ddd444").exists(),
        "with no file to read, pruning deleted a version"
    );
}

/// Pruning that can't list a tool's folder deletes nothing in it and lets the
/// build go on.
#[test]
fn pruning_deletes_nothing_in_a_folder_it_cannot_list() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("prune-installed-unlisted");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\ninstalled checker scripts/checker.sh\n");
    crate::setup::setup(&tool, &Checkout::find(&main.display().to_string()).unwrap()).unwrap();
    std::fs::create_dir_all(main.join("scripts")).unwrap();
    std::fs::write(
        main.join("scripts/checker.sh"),
        "checker_revision=\"aaa111\"\n",
    )
    .unwrap();
    let folder = tool.places.cache_home.join("checker");
    fill(&folder.join("ccc333"), 1);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    let pruned = prune_installed(&tool, &tool.roots().unwrap(), Run::Real);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    pruned.unwrap();
    assert!(
        folder.join("ccc333").exists(),
        "pruning deleted a version in a folder it could not list"
    );
}

/// A host's app checkouts have no line in a lane's set, which stays on the Mac
/// that sends the host its builds: the host's prune reads the lane that the
/// checkout's git dir names, keeps that lane's folders warm under --idle,
/// and lets the other lane's folders and a job worktree's go.
#[test]
fn a_hosts_prune_keeps_its_app_checkouts_lane_folders_warm() {
    let scratch = Scratch::new("prune-host-app");
    let tool = scratch.tool(0);
    tool.places.mark_host().unwrap();
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "xcode 4M lane mac\ngradle 4M lane android\n");
    let mut roots = Vec::new();
    for name in ["app-1", "remote-side-1"] {
        let dir = scratch.dir.join("demo-worktrees").join(name);
        crate::testing::git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &dir.display().to_string(),
            ],
        );
        let checkout = Checkout::find(&dir.display().to_string()).unwrap();
        crate::setup::setup(&tool, &checkout).unwrap();
        roots.push((checkout.clone(), checkout.build_root(&tool.places)));
    }
    std::fs::write(roots[0].0.app_lane_file(), "mac\n").unwrap();
    assert_eq!(
        roots[0].0.app_lane(&tool.places).unwrap(),
        Some(crate::lane::Lane::Mac)
    );
    assert_eq!(roots[1].0.app_lane(&tool.places).unwrap(), None);
    for (_, root) in &roots {
        for folder in ["xcode", "gradle"] {
            fill(&root.join(folder).join("blob"), 2048);
            age_all(&root.join(folder), 2);
        }
    }
    for (checkout, root) in &roots {
        prune_root(&tool, root, checkout, Run::Real, 60, 0).unwrap();
    }
    assert!(
        roots[0].1.join("xcode/blob").exists(),
        "the host deleted its mac app checkout's idle mac folder"
    );
    assert!(
        !roots[0].1.join("gradle/blob").exists(),
        "the host kept its mac app checkout's idle android folder"
    );
    assert!(
        !roots[1].1.join("xcode/blob").exists(),
        "the host kept a job worktree's idle folder"
    );
}

/// This Mac's lanes' sets name its app checkouts; a lane file in a checkout's
/// git dir names one only on a host.
#[test]
fn this_mac_reads_its_app_checkouts_from_the_lanes_sets() {
    let scratch = Scratch::new("prune-mac-app");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\n");
    let dir = scratch.dir.join("demo-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &dir.display().to_string(),
        ],
    );
    let checkout = Checkout::find(&dir.display().to_string()).unwrap();
    std::fs::write(checkout.app_lane_file(), "mac\n").unwrap();
    assert_eq!(checkout.app_lane(&tool.places).unwrap(), None);
    std::fs::create_dir_all(tool.places.root.join("demo")).unwrap();
    std::fs::write(
        tool.places
            .app_checkouts_file("demo", crate::lane::Lane::Android),
        "side-1\n",
    )
    .unwrap();
    assert_eq!(
        checkout.app_lane(&tool.places).unwrap(),
        Some(crate::lane::Lane::Android)
    );
}

/// `builds prune` runs each host's own prune with its options, and a host
/// that does not answer or whose prune fails leaves the rest to run.
#[test]
fn a_prune_runs_each_hosts_own_prune_with_its_options() {
    let scratch = Scratch::new("prune-hosts");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = crate::host::tests::fake_ssh(&scratch.dir.join("bin"), &home);
    crate::testing::host_builds(
        &home,
        r#"echo "builds $*" >> "$HOME/runs.txt"
echo "builds: deleted a folder" >&2"#,
    );
    std::fs::write(
        crate::host::hosts_file(&places),
        "gone unreachable 4\nm1 user@m1 10\n",
    )
    .unwrap();
    let options = ["--idle".to_string(), "60".to_string()];
    prune_hosts(&places, &ssh, &options).unwrap();
    assert_eq!(
        std::fs::read_to_string(home.join("runs.txt")).unwrap(),
        "builds prune --idle 60\n"
    );
    crate::testing::host_builds(&home, "echo 'builds: another prune runs' >&2; exit 1");
    prune_hosts(&places, &ssh, &[]).unwrap();
}
