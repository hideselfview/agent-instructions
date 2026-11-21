use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::{Scratch, git, host_builds};
use std::os::unix::fs::PermissionsExt;

#[test]
fn an_entry_names_its_host_and_its_checkout() {
    assert_eq!(entry_of("m1", "app-1"), "m1:app-1");
    assert_eq!(split("m1:app-1"), Some(("m1", "app-1")));
    assert_eq!(split("side-1"), None);
}

#[test]
fn a_hosts_checkout_path_names_its_repo_and_its_name() {
    assert_eq!(
        name_in("/Users/dima/dev/bae-worktrees/app-1").unwrap(),
        ("bae".to_string(), "app-1".to_string())
    );
    assert_eq!(
        name_in("dev/coven-worktrees/droid_2").unwrap(),
        ("coven".to_string(), "droid_2".to_string())
    );
    for refused in [
        "app-1",
        "/Users/dima/bae-worktrees/app-1",
        "/Users/dima/dev/bae/app-1",
        "/Users/dima/dev/-worktrees/app-1",
        "/Users/dima/dev/bae-worktrees/remote-1",
        "/Users/dima/dev/bae-worktrees/a b",
    ] {
        assert!(name_in(refused).is_err(), "{refused} is refused");
    }
}

/// A source repo `demo` with a post-checkout hook, a Gradle project and a
/// `.app-build`, a clone of it as the remote home's `dev/demo`, a recorded
/// host `m1` that reaches the remote home through a fake ssh, the host's
/// Android SDK folder, and the host's `builds`. The hook and that `builds`
/// each add a line to the remote home's `runs.txt` when they run.
struct Setup {
    // Held so the scratch folder stays until the test ends.
    _scratch: Scratch,
    source: PathBuf,
    home: PathBuf,
    places: Places,
    ssh: Ssh,
}

fn setup(name: &str) -> Setup {
    let scratch = Scratch::new(name);
    let source = scratch.dir.join("source/demo");
    crate::testing::repo(&source, "target 1G\n");
    std::fs::create_dir_all(source.join("scripts/hooks")).unwrap();
    std::fs::create_dir_all(source.join("droid")).unwrap();
    std::fs::write(
        source.join("scripts/hooks/post-checkout"),
        "#!/bin/sh\necho \"$BUILDS_ROOT\" > hook-ran\necho \"hook $(pwd)\" >> \"$HOME/runs.txt\"\n",
    )
    .unwrap();
    std::fs::set_permissions(
        source.join("scripts/hooks/post-checkout"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(source.join("droid/gradlew"), "#!/bin/sh\n").unwrap();
    std::fs::write(source.join("version"), "one\n").unwrap();
    // A repo ignores what its setup and builds write, as bae's does.
    let ignore = source.join(".gitignore");
    let ignored = std::fs::read_to_string(&ignore).unwrap();
    std::fs::write(
        &ignore,
        format!("{ignored}hook-ran\nlocal.properties\nkept\n"),
    )
    .unwrap();
    git(
        &source,
        &["add", ".gitignore", "scripts", "droid", "version"],
    );
    git(&source, &["commit", "-q", "-m", "Add the hook"]);
    let home = scratch.dir.join("remote-home");
    std::fs::create_dir_all(home.join("dev")).unwrap();
    std::fs::create_dir_all(home.join("Library/Android/sdk")).unwrap();
    git(
        &scratch.dir,
        &[
            "clone",
            "-q",
            &source.display().to_string(),
            &home.join("dev/demo").display().to_string(),
        ],
    );
    host_builds(&home, r#"echo "builds $*" >> "$HOME/runs.txt""#);
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let places = scratch.places();
    host::add(&places, &ssh, "m1", "user@m1").unwrap();
    Setup {
        _scratch: scratch,
        source,
        home,
        places,
        ssh,
    }
}

fn head_of(dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn remote_app(s: &Setup, lane: Lane) -> RemoteApp {
    RemoteApp::find(&s.places, &s.ssh, "demo", lane, "m1:app-1")
        .unwrap()
        .unwrap()
}

#[test]
fn a_local_entry_is_no_remote_checkout() {
    let s = setup("remote-app-local");
    assert!(
        RemoteApp::find(&s.places, &s.ssh, "demo", Lane::Android, "side-1")
            .unwrap()
            .is_none()
    );
    let error = RemoteApp::find(&s.places, &s.ssh, "demo", Lane::Android, "gone:app-1")
        .expect_err("an unknown host fails");
    let text = error.text();
    assert!(text.contains("host gone"), "{text}");
}

#[test]
fn the_first_switch_makes_the_checkout_as_a_side_worktree_is_made() {
    let s = setup("remote-app-make");
    let mut app = remote_app(&s, Lane::Android);
    let sha = head_of(&s.source);
    app.switch_to(&s.source, &sha).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    assert_eq!(head_of(&wt), sha, "the checkout is on the commit");
    assert_eq!(app.head().unwrap(), sha);
    assert_eq!(
        std::fs::read_to_string(wt.join("hook-ran")).unwrap().trim(),
        s.home.join("builds-root.noindex").display().to_string(),
        "the post-checkout ran with the host's builds root"
    );
    // The host's `builds setup` links the build folders first, since a repo
    // without a post-checkout has nothing else that links them.
    assert_eq!(
        std::fs::read_to_string(s.home.join("runs.txt")).unwrap(),
        format!("builds setup {wt}\nhook {wt}\n", wt = wt.display())
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("droid/local.properties"))
            .unwrap()
            .trim(),
        format!("sdk.dir={}/Library/Android/sdk", s.home.display()),
        "Gradle finds the host's SDK"
    );
    let refs = Command::new("git")
        .arg("-C")
        .arg(s.home.join("dev/demo"))
        .args(["for-each-ref", "refs/builds"])
        .output()
        .unwrap();
    assert!(refs.stdout.is_empty(), "the job's ref is deleted");
}

#[test]
fn the_mac_lane_gets_no_local_properties() {
    let s = setup("remote-app-mac");
    let mut app = remote_app(&s, Lane::Mac);
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
    assert!(
        !s.home
            .join("dev/demo-worktrees/app-1/droid/local.properties")
            .exists()
    );
}

#[test]
fn a_later_switch_moves_the_checkout_and_keeps_its_files() {
    let s = setup("remote-app-switch");
    let mut app = remote_app(&s, Lane::Android);
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    std::fs::write(wt.join("kept"), "").unwrap();
    std::fs::write(s.source.join("version"), "two\n").unwrap();
    git(&s.source, &["commit", "-q", "-a", "-m", "Two"]);
    let two = head_of(&s.source);
    app.switch_to(&s.source, &two).unwrap();
    assert_eq!(head_of(&wt), two);
    assert_eq!(
        std::fs::read_to_string(wt.join("version")).unwrap(),
        "two\n"
    );
    assert!(wt.join("kept").exists(), "ignored or untracked files stay");
}

#[test]
fn a_checkout_with_changes_is_not_switched() {
    let s = setup("remote-app-dirty");
    let mut app = remote_app(&s, Lane::Android);
    let one = head_of(&s.source);
    app.switch_to(&s.source, &one).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    std::fs::write(wt.join("version"), "edited\n").unwrap();
    std::fs::write(s.source.join("version"), "two\n").unwrap();
    git(&s.source, &["commit", "-q", "-a", "-m", "Two"]);
    let error = app
        .switch_to(&s.source, &head_of(&s.source))
        .expect_err("changes stop the switch");
    let text = error.text();
    assert!(text.contains("has changes"), "{text}");
    assert_eq!(head_of(&wt), one, "the checkout did not move");
}

#[test]
fn a_checkout_whose_setup_fails_is_removed() {
    let s = setup("remote-app-nosdk");
    std::fs::remove_dir_all(s.home.join("Library/Android/sdk")).unwrap();
    let mut app = remote_app(&s, Lane::Android);
    let error = app
        .switch_to(&s.source, &head_of(&s.source))
        .expect_err("no SDK fails");
    let text = error.text();
    assert!(text.contains("no Android SDK"), "{text}");
    assert!(
        !s.home.join("dev/demo-worktrees/app-1").exists(),
        "the next build sets it up again"
    );
}

#[test]
fn a_host_without_a_clone_says_so() {
    let s = setup("remote-app-noclone");
    std::fs::remove_dir_all(s.home.join("dev/demo")).unwrap();
    let mut app = remote_app(&s, Lane::Mac);
    let error = app
        .switch_to(&s.source, &head_of(&s.source))
        .expect_err("no clone fails");
    // The push to the missing clone fails first, naming the host.
    let text = error.text();
    assert!(text.contains("m1"), "{text}");
}

#[test]
fn results_come_back_to_the_checkouts_folder_on_this_mac() {
    let s = setup("remote-app-pull");
    let mut app = remote_app(&s, Lane::Android);
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    // As in a real checkout, the results folder is a link into the builds
    // root.
    let real = s.home.join("builds-root.noindex/demo/app-1/shots");
    std::fs::create_dir_all(real.join("android")).unwrap();
    std::fs::write(real.join("android/a.png"), "pixels").unwrap();
    std::os::unix::fs::symlink(&real, wt.join("shots")).unwrap();
    let local = s.places.root.join("demo/m1:app-1");
    std::fs::create_dir_all(local.join("shots")).unwrap();
    std::fs::write(local.join("shots/stale"), "").unwrap();
    app.pull(&["shots".to_string(), "never-written".to_string()])
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(local.join("shots/android/a.png")).unwrap(),
        "pixels"
    );
    assert!(
        !local.join("shots/stale").exists(),
        "an old copy does not mix with the new"
    );
    assert!(!local.join("never-written").exists());
    // The link the build made is no change to the checkout.
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
}

#[test]
fn a_built_app_comes_back_with_its_links_as_links() {
    let s = setup("remote-app-pull-bundle");
    let mut app = remote_app(&s, Lane::Mac);
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    let bundle = wt.join("out/Demo.app/Contents/Frameworks/F.framework/Versions");
    std::fs::create_dir_all(bundle.join("A")).unwrap();
    std::fs::write(bundle.join("A/F"), "code").unwrap();
    std::os::unix::fs::symlink("A", bundle.join("Current")).unwrap();
    let local = s.places.root.join("demo/m1:app-1");
    app.pull_bundle("out/Demo.app").unwrap();
    let copied = local.join("out/Demo.app/Contents/Frameworks/F.framework/Versions");
    assert_eq!(
        std::fs::read_link(copied.join("Current")).unwrap(),
        Path::new("A"),
        "a link the bundle holds stays a link"
    );
    assert_eq!(std::fs::read_to_string(copied.join("A/F")).unwrap(), "code");
}

/// The lane a switch wrote in the checkout's git dir.
fn lane_mark(wt: &Path) -> String {
    let git_dir = crate::testing::git_out(wt, &["rev-parse", "--absolute-git-dir"]);
    std::fs::read_to_string(Path::new(git_dir.trim()).join(crate::checkout::APP_LANE_FILE)).unwrap()
}

#[test]
fn a_new_checkout_gets_its_lane_file_before_the_hosts_setup_runs() {
    let s = setup("remote-app-lane");
    // The host's builds says which lane the checkout's git dir names when
    // its setup runs.
    host_builds(
        &s.home,
        r#"echo "builds $* $(cat "$(git -C "$2" rev-parse --absolute-git-dir)/builds-app-lane")" >> "$HOME/runs.txt""#,
    );
    let mut app = remote_app(&s, Lane::Android);
    app.switch_to(&s.source, &head_of(&s.source)).unwrap();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    assert_eq!(lane_mark(&wt), "android\n");
    assert_eq!(
        std::fs::read_to_string(s.home.join("runs.txt")).unwrap(),
        format!("builds setup {wt} android\nhook {wt}\n", wt = wt.display())
    );
}

#[test]
fn marking_or_unmarking_a_checkout_git_cannot_read_fails() {
    let s = setup("remote-app-badmark");
    let app = remote_app(&s, Lane::Android);
    let wt = s.home.join("dev/demo-worktrees/app-1");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: /nowhere\n").unwrap();
    let marked = app.mark().expect_err("a checkout git cannot read").text();
    assert!(marked.contains("m1"), "{marked}");
    let unmarked = app.unmark().expect_err("a checkout git cannot read").text();
    assert!(unmarked.contains("m1"), "{unmarked}");
}

#[test]
fn marking_says_whether_the_host_had_the_worktree() {
    let s = setup("remote-app-marked");
    let app = remote_app(&s, Lane::Mac);
    assert!(!app.mark().unwrap(), "the host has no worktree yet");
    let wt = s.home.join("dev/demo-worktrees/app-1");
    git(
        &s.home.join("dev/demo"),
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &wt.display().to_string(),
        ],
    );
    assert!(app.mark().unwrap());
    assert!(app.mark().unwrap(), "marking again marks again");
    assert_eq!(lane_mark(&wt), "mac\n");
}

#[test]
fn marking_reads_only_the_hosts_two_answers() {
    assert!(marked_in("lane-file=written\n").unwrap());
    assert!(!marked_in("lane-file=no-worktree\n").unwrap());
    assert!(
        marked_in("motd from the host\nlane-file=written\n").unwrap(),
        "the last answer counts"
    );
    for said in ["", "marked\n", "lane-file=\n", "lane-file=maybe\n"] {
        let error = marked_in(said).expect_err("not an answer").text();
        assert!(error.contains("lane file"), "{error}");
    }
}

#[test]
fn a_picture_folders_parts_go_to_a_temporary_folder_on_the_host_and_come_back() {
    let s = setup("remote-app-pictures");
    let mut app = remote_app(&s, Lane::Mac);
    let local = s.places.root.join("pictures");
    std::fs::create_dir_all(local.join("boards")).unwrap();
    std::fs::write(local.join("boards/h.png"), "png").unwrap();
    std::fs::create_dir_all(local.join("android/other")).unwrap();
    std::fs::write(local.join("android/other/x.png"), "x").unwrap();
    let parts = ["boards".to_string(), "mac/tree1".to_string()];
    let there = app.put_folder(&local, &parts).unwrap();
    let there_path = Path::new(&there);
    assert!(there_path.join("boards/h.png").is_file());
    assert!(
        !there_path.join("android").exists(),
        "only the parts named go"
    );
    std::fs::create_dir_all(there_path.join("mac/tree1")).unwrap();
    std::fs::write(there_path.join("mac/tree1/s.png"), "shot").unwrap();
    let back = s.places.root.join("back");
    app.fetch_folder(&there, &back).unwrap();
    assert!(back.join("mac/tree1/s.png").is_file() && back.join("boards/h.png").is_file());
    app.remove_temporary(&there).unwrap();
    assert!(!there_path.exists());
}

/// A lane's step on a host runs this build's programs there: the first step
/// copies this version's folder to the host, and each step runs under
/// `builds programs hold` from that folder.
#[test]
fn a_step_on_a_host_runs_under_this_builds_programs_there() {
    let s = setup("remote-app-programs");
    let version = env!("BUILDS_SOURCE");
    let (builds, shots) = crate::testing::started_programs(
        &s.places.home.join("bin"),
        version,
        crate::shots_program::VERSION,
    );
    let pinned = crate::testing::pinned(
        crate::programs::pin(
            &s.places.home,
            &crate::programs::Sources {
                builds: &builds,
                shots: &shots,
                version,
                shots_version: crate::shots_program::VERSION,
            },
        )
        .unwrap(),
    );
    let mut app = remote_app(&s, Lane::Mac);
    let there = s.home.join(crate::programs::FOLDER).join(version);
    let words = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    let held = app
        .with_programs(&pinned, &words(&["builds", "shots", "run"]))
        .unwrap();
    assert_eq!(
        held,
        words(&[
            &there.join("builds").display().to_string(),
            "programs",
            "hold",
            "--",
            "builds",
            "shots",
            "run"
        ])
    );
    for (name, copied) in [("builds", &builds), (crate::shots_program::PROGRAM, &shots)] {
        assert_eq!(
            std::fs::read(there.join(name)).unwrap(),
            std::fs::read(copied).unwrap()
        );
    }
    // The next step finds the folder there and copies nothing.
    std::fs::write(there.join("builds"), "kept").unwrap();
    app.with_programs(&pinned, &words(&["true"])).unwrap();
    assert_eq!(
        std::fs::read_to_string(there.join("builds")).unwrap(),
        "kept"
    );
}
