use super::*;
use crate::claim::cmd_release;
use crate::lane::Lane;
use crate::testing::{Scratch, git, repo};

fn refusal(result: Result<()>) -> String {
    match result {
        Err(Fail::Message(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn new_claims_a_worktree_and_retire_removes_it() {
    let scratch = Scratch::new("worktree");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    // A root left by an older side-1 makes the next one side-2.
    std::fs::create_dir_all(tool.places.root.join("demo/side-1")).unwrap();

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string(), "--for".into(), "agent A".into()],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-2");
    assert!(
        side.join("target").is_symlink(),
        "new did not set the worktree up"
    );
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "agent A"
    );

    let side_arg = side.display().to_string();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("is claimed by agent A since")
    );
    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &["mac".to_string(), side_arg.clone()],
    )
    .unwrap();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("one of demo's mac app checkouts")
    );
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &["remove".to_string(), "mac".to_string(), side_arg.clone()],
    )
    .unwrap();
    std::fs::write(side.join("stray"), "x").unwrap();
    assert!(
        refusal(cmd_retire(&tool, std::slice::from_ref(&side_arg)))
            .contains("1 file not committed: stray")
    );
    std::fs::remove_file(side.join("stray")).unwrap();
    cmd_retire(&tool, std::slice::from_ref(&side_arg)).unwrap();
    assert!(!side.exists());
    assert!(!tool.places.root.join("demo/side-2").exists());
    assert!(
        refusal(cmd_retire(&tool, &[main.display().to_string()]))
            .contains("only side worktrees retire")
    );
}

#[test]
fn a_checkout_is_the_app_checkout_of_one_lane_only() {
    let scratch = Scratch::new("app-checkout-lanes");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
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
    let arg = side.display().to_string();
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &["android".to_string(), arg.clone()],
    )
    .unwrap();
    assert_eq!(
        tool.places.app_lane_of("demo", "side-1").unwrap(),
        Some(Lane::Android)
    );
    assert!(
        tool.places
            .app_checkouts_file("demo", Lane::Android)
            .is_file()
    );
    assert!(!tool.places.app_checkouts_file("demo", Lane::Mac).exists());
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &crate::host::Ssh::from_env(),
            &["mac".to_string(), arg.clone()]
        ))
        .contains("android app checkout already")
    );
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &crate::host::Ssh::from_env(),
            &["ios".to_string(), arg]
        ))
        .contains("usage")
    );
    assert!(
        refusal(cmd_retire(&tool, &[side.display().to_string()])).contains("android app checkouts")
    );
}

#[test]
fn a_lane_holds_several_app_checkouts_and_one_comes_out_while_no_build_holds_it() {
    let scratch = Scratch::new("app-checkout-set");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let args = |extra: &[&str], side: &std::path::Path| -> Vec<String> {
        extra
            .iter()
            .map(|word| word.to_string())
            .chain([side.display().to_string()])
            .collect()
    };
    let sides: Vec<_> = (1..=2)
        .map(|n| scratch.dir.join(format!("demo-worktrees/side-{n}")))
        .collect();
    for side in &sides {
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
        cmd_app_checkout(
            &tool,
            &crate::host::Ssh::from_env(),
            &args(&["android"], side),
        )
        .unwrap();
    }
    // Recording one again changes nothing.
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &args(&["android"], &sides[0]),
    )
    .unwrap();
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-1", "side-2"]
    );
    for name in ["side-1", "side-2"] {
        assert_eq!(
            tool.places.app_lane_of("demo", name).unwrap(),
            Some(Lane::Android)
        );
    }
    assert!(
        refusal(cmd_retire(&tool, &[sides[1].display().to_string()]))
            .contains("app-checkout remove android")
    );

    let lock = tool.places.app_lock_file("demo", Lane::Android, "side-2");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    // Close-on-exec: a process another test starts meanwhile must not inherit
    // the lock and keep it past the close.
    let held = crate::queue::open_with(&lock, libc::O_CREAT | libc::O_CLOEXEC).unwrap();
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &crate::host::Ssh::from_env(),
            &args(&["remove", "android"], &sides[1])
        ))
        .contains("holds")
    );
    assert_eq!(
        tool.places
            .app_checkouts_of("demo", Lane::Android)
            .unwrap()
            .len(),
        2
    );
    unsafe { libc::close(held) };
    // A process another test forks meanwhile has the lock until it execs.
    while crate::queue::is_held(&lock).unwrap() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &args(&["remove", "android"], &sides[1]),
    )
    .unwrap();
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["side-1"]
    );
    assert!(!lock.exists(), "its lock went with it");
    assert_eq!(tool.places.app_lane_of("demo", "side-2").unwrap(), None);
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &crate::host::Ssh::from_env(),
            &args(&["remove", "android"], &sides[1])
        ))
        .contains("not one of")
    );
    cmd_app_checkout(
        &tool,
        &crate::host::Ssh::from_env(),
        &args(&["remove", "android"], &sides[0]),
    )
    .unwrap();
    assert!(
        !tool
            .places
            .app_checkouts_file("demo", Lane::Android)
            .exists(),
        "an empty set is no file"
    );
}

#[test]
fn new_and_claim_record_the_project_and_release_forgets_it() {
    use crate::lists::fixtures::make_store;
    let scratch = Scratch::new("worktree-project");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    make_store(&tool.places, &["demo"]);
    // A project that holds no repo of this worktree is refused before anything is made.
    assert!(
        refusal(cmd_new(
            &tool,
            &crate::testing::no_ssh(),
            &[
                main.display().to_string(),
                "--for".into(),
                "agent A".into(),
                "--project".into(),
                "nope".into(),
            ],
        ))
        .contains("project nope holds no repo demo")
    );
    assert!(!scratch.dir.join("demo-worktrees").exists());

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[
            main.display().to_string(),
            "--for".into(),
            "agent A".into(),
            "--project".into(),
            "demo".into(),
        ],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-1");
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert_eq!(
        std::fs::read_to_string(checkout.project_file()).unwrap(),
        "demo\n"
    );
    // Claiming again without --project keeps it; with a project that holds no repo here, refuses.
    let arg = side.display().to_string();
    crate::claim::cmd_claim(&tool.places, &[arg.clone(), "agent A".into()], tool.now).unwrap();
    assert!(checkout.project_file().is_file());
    assert!(
        refusal(crate::claim::cmd_claim(
            &tool.places,
            &[
                arg.clone(),
                "agent A".into(),
                "--project".into(),
                "other".into()
            ],
            tool.now,
        ))
        .contains("holds no repo demo")
    );
    cmd_release(&tool.places, std::slice::from_ref(&arg)).unwrap();
    assert!(!checkout.project_file().exists());
    assert!(!checkout.claim_file().exists());
    // The next worktree handed out without --project has none.
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string()],
    )
    .unwrap();
    assert!(!checkout.project_file().exists());
}

#[test]
fn a_repo_two_projects_hold_gets_no_worktree_or_claim_without_a_project() {
    use crate::lists::fixtures::{NOON, make_store, stamp};
    use crate::lists::project::NewProject;
    use crate::lists::store::Store;
    let scratch = Scratch::new("worktree-two-projects");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    make_store(&tool.places, &["demo"]);
    Store::named(&tool.places, "second")
        .unwrap()
        .create(
            &NewProject {
                name: "second".into(),
                canvas: false,
                repos: vec!["demo".into()],
                places: Default::default(),
                sources: Default::default(),
                specs: Default::default(),
            },
            &stamp(NOON),
            "lists init second",
        )
        .unwrap();
    let main_arg = main.display().to_string();
    // Without --project, new refuses before it makes or claims anything.
    assert!(
        refusal(cmd_new(
            &tool,
            &crate::testing::no_ssh(),
            &[main_arg.clone(), "--for".into(), "agent A".into()]
        ))
        .contains("repo demo is in two projects (demo, second); name the project")
    );
    assert!(!scratch.dir.join("demo-worktrees").exists());

    // With it, the worktree is claimed for that project and released again.
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[
            main_arg.clone(),
            "--for".into(),
            "agent A".into(),
            "--project".into(),
            "second".into(),
        ],
    )
    .unwrap();
    let arg = scratch
        .dir
        .join("demo-worktrees/side-1")
        .display()
        .to_string();
    let checkout = Checkout::find(&arg).unwrap();
    // Claiming it again without --project keeps the project it has.
    crate::claim::cmd_claim(&tool.places, &[arg.clone(), "agent A".into()], tool.now).unwrap();
    cmd_release(&tool.places, std::slice::from_ref(&arg)).unwrap();
    assert!(!checkout.project_file().exists());

    // A fresh claim of the released worktree must name the project too.
    assert!(
        refusal(crate::claim::cmd_claim(
            &tool.places,
            &[arg.clone(), "agent B".into()],
            tool.now,
        ))
        .contains("repo demo is in two projects")
    );
    assert!(!checkout.claim_file().exists());
}

#[test]
fn a_hosts_app_checkout_is_recorded_beside_this_macs_and_taken_out_again() {
    use crate::host::tests::fake_ssh;
    let scratch = Scratch::new("app-checkout-host");
    let tool = scratch.tool(0);
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    crate::host::add(&tool.places, &ssh, "m1", "user@m1").unwrap();
    let words = |words: &[&str]| -> Vec<String> { words.iter().map(|w| w.to_string()).collect() };
    let path = "/Users/me/dev/demo-worktrees/app-1";
    cmd_app_checkout(&tool, &ssh, &words(&["android", "--host", "m1", path])).unwrap();
    cmd_app_checkout(&tool, &ssh, &words(&["android", "--host", "m1", path])).unwrap();
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["m1:app-1"],
        "recording it twice records it once"
    );
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["mac", "--host", "m1", path])
        ))
        .contains("the lanes exist so that their builds share no checkout")
    );
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["android", "--host", "gone", path])
        ))
        .contains("no host named gone")
    );
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["android", "--host", "m1", "/Users/me/app-1"])
        ))
        .contains("-worktrees")
    );
    cmd_app_checkout(
        &tool,
        &ssh,
        &words(&["remove", "android", "--host", "m1", path]),
    )
    .unwrap();
    assert!(
        tool.places
            .app_checkouts_of("demo", Lane::Android)
            .unwrap()
            .is_empty()
    );
}

/// Adding a host's app checkout writes its lane file in the checkout's git
/// dir on the host, and removing it deletes the file, so the host keeps the
/// checkout's builds warm exactly while it is in a lane. When the host does
/// not answer, either fails and leaves the lane as it was; a removal under
/// the wrong lane fails before it touches the host.
#[test]
fn a_hosts_app_checkout_has_its_lane_file_exactly_while_it_is_in_a_lane() {
    use crate::host::tests::fake_ssh;
    let scratch = Scratch::new("app-checkout-host-mark");
    let tool = scratch.tool(0);
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let clone = home.join("dev/demo");
    repo(&clone, "target 1G\n");
    let wt = home.join("dev/demo-worktrees/app-1");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &wt.display().to_string(),
        ],
    );
    let lane_file = Checkout::find(&wt.display().to_string())
        .unwrap()
        .app_lane_file();
    std::fs::write(
        crate::host::hosts_file(&tool.places),
        "m1 user@m1 10\ngone unreachable 4\n",
    )
    .unwrap();
    let words = |words: &[&str]| -> Vec<String> { words.iter().map(|w| w.to_string()).collect() };
    let path = wt.display().to_string();
    cmd_app_checkout(&tool, &ssh, &words(&["android", "--host", "m1", &path])).unwrap();
    assert_eq!(std::fs::read_to_string(&lane_file).unwrap(), "android\n");
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["android", "--host", "gone", &path])
        ))
        .contains("host gone")
    );
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["m1:app-1"],
        "an addition the host did not answer was recorded"
    );
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["remove", "mac", "--host", "m1", &path])
        ))
        .contains("not one of")
    );
    assert!(
        lane_file.exists(),
        "a removal under the wrong lane deleted the lane file"
    );
    let mut set = tool
        .places
        .app_checkouts_recorded("demo", Lane::Android)
        .unwrap();
    set.push(crate::lane::AppCheckout::by_person("gone:app-1"));
    tool.places
        .write_app_checkouts("demo", Lane::Android, &set)
        .unwrap();
    assert!(
        refusal(cmd_app_checkout(
            &tool,
            &ssh,
            &words(&["remove", "android", "--host", "gone", &path])
        ))
        .contains("host gone")
    );
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["m1:app-1", "gone:app-1"],
        "a removal the host did not answer took the checkout out"
    );
    cmd_app_checkout(
        &tool,
        &ssh,
        &words(&["remove", "android", "--host", "m1", &path]),
    )
    .unwrap();
    assert!(!lane_file.exists(), "the host keeps the lane file");
    assert!(wt.join(".git").exists(), "the worktree stays on the host");
    assert_eq!(
        tool.places.app_checkouts_of("demo", Lane::Android).unwrap(),
        vec!["gone:app-1"]
    );
}

/// A repo `demo` under <scratch> with an origin, whose post-checkout hook
/// fails after `new` has made and claimed a worktree: the step after which
/// `builds new` used to leave the worktree unclaimed.
fn demo_with_failing_hook(scratch: &Scratch) -> std::path::PathBuf {
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    main
}

fn push_failing_hook(main: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let hook = main.join("scripts/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho hook failed >&2\nexit 3\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(main, &["add", "scripts/hooks/post-checkout"]);
    git(main, &["commit", "-q", "-m", "Add a failing hook"]);
    git(main, &["push", "-q", "origin", "main"]);
}

fn branch_exists(main: &Path, branch: &str) -> bool {
    !crate::testing::git_out(main, &["branch", "--list", branch]).is_empty()
}

#[test]
fn new_that_fails_after_adding_a_worktree_removes_the_worktree_and_its_branch() {
    let scratch = Scratch::new("worktree-new-undo-add");
    let tool = scratch.tool(0);
    let main = demo_with_failing_hook(&scratch);
    push_failing_hook(&main);

    let error = refusal(cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[
            main.display().to_string(),
            "-b".into(),
            "work".into(),
            "--for".into(),
            "agent A".into(),
        ],
    ));
    assert!(error.contains("post-checkout"), "{error}");
    assert!(!scratch.dir.join("demo-worktrees/side-1").exists());
    assert!(!branch_exists(&main, "work"));
    assert!(!tool.places.root.join("demo/side-1").exists());
    assert!(
        !crate::testing::git_out(&main, &["worktree", "list"]).contains("side-1"),
        "git still lists the worktree"
    );
}

#[test]
fn new_that_fails_in_a_free_worktree_leaves_it_unclaimed_and_without_the_new_branch() {
    let scratch = Scratch::new("worktree-new-undo-reuse");
    let tool = scratch.tool(0);
    let main = demo_with_failing_hook(&scratch);
    let ssh = crate::testing::no_ssh();
    let arg = main.display().to_string();
    cmd_new(
        &tool,
        &ssh,
        &[arg.clone(), "--for".into(), "agent A".into()],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-1");
    cmd_release(&scratch.places(), &[side.display().to_string()]).unwrap();
    push_failing_hook(&main);

    let error = refusal(cmd_new(
        &tool,
        &ssh,
        &[
            arg,
            "-b".into(),
            "work".into(),
            "--for".into(),
            "agent B".into(),
        ],
    ));
    assert!(error.contains("post-checkout"), "{error}");
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    assert!(Claim::read(&checkout.claim_file()).unwrap().is_none());
    assert!(!branch_exists(&main, "work"));
    assert_eq!(
        crate::testing::git_out(&side, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD",
        "the worktree is not detached"
    );
}
