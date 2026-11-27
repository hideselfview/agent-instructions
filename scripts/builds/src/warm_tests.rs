use std::os::unix::fs::MetadataExt;

use super::*;
use crate::claim::cmd_release;
use crate::flock::lock_creating;
use crate::manifest::build_dirs;
use crate::setup::setup;
use crate::testing::{Scratch, fill as fill_file, git, repo};
use crate::worktree::{cmd_new, cmd_retire};

fn candidate(name: &str, same_lock: bool, finished: i64) -> Candidate {
    Candidate {
        root: PathBuf::from(name),
        top: PathBuf::from(name),
        units: 0,
        same_lock,
        finished,
    }
}

#[test]
fn the_source_has_origin_mains_dependencies_then_the_newest_build() {
    let mut candidates = vec![
        candidate("/v/r/side-1", false, 500),
        candidate("/v/r/side-2", true, 100),
        candidate("/v/r/main", true, 300),
        candidate("/v/r/side-3", true, 300),
    ];
    rank(&mut candidates);
    let names: Vec<_> = candidates
        .iter()
        .map(|c| c.root.display().to_string())
        .collect();
    assert_eq!(
        names,
        ["/v/r/main", "/v/r/side-3", "/v/r/side-2", "/v/r/side-1"]
    );
}

#[test]
fn a_target_holding_more_units_outranks_a_newer_sparse_one() {
    let mut sparse = candidate("/v/r/side-1", true, 900);
    sparse.units = 216;
    let mut warm = candidate("/v/r/side-2", true, 100);
    warm.units = 2000;
    let mut candidates = vec![sparse, warm];
    rank(&mut candidates);
    assert_eq!(candidates[0].root, PathBuf::from("/v/r/side-2"));
}

/// A repo "demo" with a pushed origin and a main checkout whose build root
/// holds a built `target`.
fn warm_main(scratch: &Scratch, tool: &Tool) -> (PathBuf, PathBuf) {
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
    setup(
        tool,
        &crate::testing::no_ssh(),
        &Checkout::find(&main.display().to_string()).unwrap(),
    )
    .unwrap();
    let target = tool.places.root.join("demo/main/target");
    fill_file(&target.join("debug/.fingerprint/dep-1/fingerprint"), 8);
    fill_file(&target.join("debug/deps/libdep-1.rlib"), 64);
    // A side worktree's builds never read incremental output.
    fill_file(&target.join("debug/incremental/crate-1/state"), 8);
    (main, target)
}

#[test]
fn new_fills_the_empty_target_with_a_clone_of_a_warm_one() {
    let scratch = Scratch::new("warm-new");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string(), "--for".into(), "agent".into()],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-1");
    let target = tool.places.root.join("demo/side-1/target");
    assert!(side.join("target").is_symlink());
    // The root the clone filled is measured, and the dashboard told, once
    // the command ends.
    assert!(
        tool.changed
            .roots()
            .contains(&tool.places.root.join("demo/side-1"))
    );
    let rlib = "debug/deps/libdep-1.rlib";
    assert_eq!(
        std::fs::read(target.join(rlib)).unwrap(),
        std::fs::read(source.join(rlib)).unwrap()
    );
    assert!(
        target
            .join("debug/.fingerprint/dep-1/fingerprint")
            .is_file()
    );
    // The clone is its own folder, not a link to the source's files.
    assert_ne!(
        target.join(rlib).metadata().unwrap().ino(),
        source.join(rlib).metadata().unwrap().ino()
    );
    assert!(
        !target.join("debug/incremental/crate-1").exists(),
        "a side worktree's builds read no incremental folder"
    );
    // The source is as it was, incremental folder included.
    assert!(source.join("debug/incremental/crate-1/state").is_file());
    let left_staged = std::fs::read_dir(tool.places.root.join("demo/side-1"))
        .unwrap()
        .any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".target.warm.")
        });
    assert!(!left_staged, "a staged clone was left behind");
}

#[test]
fn a_free_worktree_whose_target_was_deleted_is_filled_and_one_with_a_target_is_not() {
    let scratch = Scratch::new("warm-free");
    let tool = scratch.tool(0);
    let (main, _) = warm_main(&scratch, &tool);
    let side_arg = scratch
        .dir
        .join("demo-worktrees/side-1")
        .display()
        .to_string();
    let main_arg = main.display().to_string();

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        std::slice::from_ref(&main_arg),
    )
    .unwrap();
    let target = tool.places.root.join("demo/side-1/target");
    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    // The agent built there: the folder holds its own state, which a new
    // hand-out leaves alone.
    fill_file(&target.join("debug/own"), 4);
    std::fs::remove_dir_all(target.join("debug/deps")).unwrap();
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        std::slice::from_ref(&main_arg),
    )
    .unwrap();
    assert!(target.join("debug/own").is_file());
    assert!(!target.join("debug/deps").exists());

    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    std::fs::remove_dir_all(&target).unwrap();
    std::fs::create_dir(&target).unwrap();
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        std::slice::from_ref(&main_arg),
    )
    .unwrap();
    assert!(
        target.join("debug/deps/libdep-1.rlib").is_file(),
        "an emptied folder is filled again"
    );
    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    cmd_retire(&tool, &[side_arg]).unwrap();
}

#[test]
fn with_nothing_warm_to_clone_the_target_stays_empty() {
    let scratch = Scratch::new("warm-none");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);
    std::fs::remove_dir_all(source.join("debug")).unwrap();
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string()],
    )
    .unwrap();
    let target = tool.places.root.join("demo/side-1/target");
    assert!(target.is_dir() && std::fs::read_dir(&target).unwrap().next().is_none());
}

#[test]
fn a_target_over_the_new_ones_cap_is_not_cloned() {
    let scratch = Scratch::new("warm-cap");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);
    fill_file(&source.join("debug/deps/big"), 1024 * 1024 + 16);
    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string()],
    )
    .unwrap();
    let target = tool.places.root.join("demo/side-1/target");
    assert!(std::fs::read_dir(&target).unwrap().next().is_none());
}

#[test]
fn a_source_a_build_holds_is_passed_over_and_the_last_one_is_waited_for() {
    let scratch = Scratch::new("warm-guard");
    let tool = scratch.tool(0);
    let (_, source) = warm_main(&scratch, &tool);
    let root = source.parent().unwrap().to_path_buf();
    let busy = candidate_at(&root);
    // A second checkout's root, built and idle.
    let other = tool.places.root.join("demo/side-9");
    std::fs::create_dir_all(other.join("target/debug/.fingerprint")).unwrap();
    std::fs::write(other.join(".checkout"), "x\n").unwrap();
    let idle = candidate_at(&other);

    // `builds cargo` holds the guard shared while cargo runs.
    let build = lock_creating(&root.join(GUARD), libc::LOCK_SH).unwrap();
    let ranked = [busy.clone(), idle.clone()];
    let held = acquire(&ranked).unwrap().expect("one is idle");
    assert_eq!(held.candidate, idle, "the build's checkout was passed over");
    drop(held);

    // With every candidate building, the call waits for the best one's guard.
    let released = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            let held = acquire(std::slice::from_ref(&busy))
                .unwrap()
                .expect("the build ended");
            assert!(
                released.load(std::sync::atomic::Ordering::SeqCst),
                "took the source while a build held it"
            );
            held.candidate
        });
        released.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(build);
        assert_eq!(waiter.join().unwrap(), busy);
    });
    assert!(acquire(&[]).unwrap().is_none());
}

/// Runs <name>, one of this file's tests, alone in a new process of the test
/// binary. A process another test spawns holds a copy of every open file of
/// its parent from the fork until the exec, and a lock a file holds stays held
/// while any copy is open, so a test that takes a lock and then checks it came
/// free can only check that in a process where no other test spawns.
fn alone_in_a_process(name: &str, body: impl FnOnce()) {
    const MARK: &str = "BUILDS_TEST_ALONE";
    if std::env::var_os(MARK).is_some() {
        body();
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("warm::tests::{name}"),
            "--test-threads=1",
        ])
        .env(MARK, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_source_a_hand_run_cargo_holds_is_passed_over() {
    alone_in_a_process("a_source_a_hand_run_cargo_holds_is_passed_over", || {
        let scratch = Scratch::new("warm-cargo-lock");
        let tool = scratch.tool(0);
        let (_, source) = warm_main(&scratch, &tool);
        let lock = source.join("debug/.cargo-lock");
        std::fs::write(&lock, "").unwrap();
        let hand = crate::evict::try_lock(&lock).unwrap().expect("free");
        let candidate = candidate_at(source.parent().unwrap());
        assert!(try_hold(&candidate).unwrap().is_none());
        drop(hand);
        assert!(try_hold(&candidate).unwrap().is_some());
    });
}

fn candidate_at(root: &Path) -> Candidate {
    Candidate {
        root: root.to_path_buf(),
        top: root.to_path_buf(),
        units: 0,
        same_lock: true,
        finished: 0,
    }
}

#[test]
fn a_new_tree_takes_the_sources_mtimes_for_the_files_that_match() {
    let scratch = Scratch::new("warm-match-times");
    let (source, dest) = (scratch.dir.join("source"), scratch.dir.join("dest"));
    for top in [&source, &dest] {
        repo(top, "target 1G\n");
        std::fs::write(top.join("same.rs"), "fn main() {}\n").unwrap();
        std::fs::write(top.join("changed.rs"), format!("// {}\n", top.display())).unwrap();
        std::fs::write(top.join("only-here-or-there.rs"), "x\n").unwrap();
        git(
            top,
            &["add", "same.rs", "changed.rs", "only-here-or-there.rs"],
        );
        git(top, &["commit", "-q", "-m", "Files"]);
    }
    std::fs::remove_file(source.join("only-here-or-there.rs")).unwrap();
    let built = std::time::UNIX_EPOCH + std::time::Duration::new(1_000_000, 123_456_789);
    for name in ["same.rs", "changed.rs"] {
        File::open(source.join(name))
            .unwrap()
            .set_modified(built)
            .unwrap();
    }
    match_times(&source, &dest).unwrap();
    let mtime = |name: &str| {
        std::fs::metadata(dest.join(name))
            .unwrap()
            .modified()
            .unwrap()
    };
    assert_eq!(mtime("same.rs"), built);
    assert_ne!(mtime("changed.rs"), built);
    assert_ne!(mtime("only-here-or-there.rs"), built);
}

const LANE_MANIFEST: &str = "target 1G app 1G\ntarget-macos 1G app 1G lane mac\ntarget-android 1G app 1G lane android\nxcode 1G app 1G lane mac\ngradle 1G app 1G lane android\n";

/// Two side worktrees of a repo whose lanes build into target-macos, an Xcode
/// tree, target-android and a Gradle folder; the first has built them all.
fn lane_checkouts(scratch: &Scratch, tool: &Tool) -> (Checkout, Checkout) {
    let main = scratch.dir.join("demo");
    repo(&main, LANE_MANIFEST);
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
    for _ in 0..2 {
        cmd_new(
            tool,
            &crate::testing::no_ssh(),
            &[main.display().to_string()],
        )
        .unwrap();
    }
    let side = |n: u32| {
        Checkout::find(
            &scratch
                .dir
                .join(format!("demo-worktrees/side-{n}"))
                .display()
                .to_string(),
        )
        .unwrap()
    };
    let (source, dest) = (side(1), side(2));
    let built = source.build_root(&tool.places);
    for cargo in ["target", "target-macos", "target-android"] {
        fill_file(
            &built
                .join(cargo)
                .join("debug/.fingerprint/dep-1/fingerprint"),
            8,
        );
        fill_file(&built.join(cargo).join("debug/deps/libdep-1.rlib"), 64);
        fill_file(
            &built.join(cargo).join("debug/incremental/crate-1/state"),
            8,
        );
    }
    for other in ["xcode/Build/Products/app", "gradle/caches/state"] {
        fill_file(&built.join(other), 64);
    }
    (source, dest)
}

#[test]
fn a_new_app_checkout_clones_the_cargo_folders_of_its_lane_and_no_others() {
    let scratch = Scratch::new("warm-lane");
    let tool = scratch.tool(0);
    let (source, dest) = lane_checkouts(&scratch, &tool);
    let (from, to) = (
        source.build_root(&tool.places),
        dest.build_root(&tool.places),
    );

    clone_lane(&tool, Lane::MAC, &source, &dest).unwrap();

    let rlib = "debug/deps/libdep-1.rlib";
    for cloned in ["target", "target-macos"] {
        assert_eq!(
            std::fs::read(to.join(cloned).join(rlib)).unwrap(),
            std::fs::read(from.join(cloned).join(rlib)).unwrap(),
            "{cloned}"
        );
        assert_ne!(
            to.join(cloned).join(rlib).metadata().unwrap().ino(),
            from.join(cloned).join(rlib).metadata().unwrap().ino(),
            "{cloned} is a clone, not a link"
        );
        assert!(
            !to.join(cloned).join("debug/incremental/crate-1").exists(),
            "an app checkout builds without incremental compilation"
        );
    }
    for left_empty in ["xcode", "target-android", "gradle"] {
        assert!(
            std::fs::read_dir(to.join(left_empty))
                .unwrap()
                .next()
                .is_none(),
            "{left_empty} is not cloned for the mac lane"
        );
    }
    assert!(
        from.join("target-macos/debug/incremental/crate-1/state")
            .is_file(),
        "the source is as it was"
    );
}

#[test]
fn the_android_lane_clones_target_android() {
    let scratch = Scratch::new("warm-lane-android");
    let tool = scratch.tool(0);
    let (source, dest) = lane_checkouts(&scratch, &tool);
    clone_lane(&tool, Lane::ANDROID, &source, &dest).unwrap();
    let to = dest.build_root(&tool.places);
    assert!(to.join("target-android/debug/deps/libdep-1.rlib").is_file());
    assert!(
        std::fs::read_dir(to.join("target-macos"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn only_folders_that_hold_a_cargo_build_and_are_empty_are_cloned() {
    let scratch = Scratch::new("warm-lane-folders");
    let tool = scratch.tool(0);
    let (source, dest) = lane_checkouts(&scratch, &tool);
    let (from, to) = (
        source.build_root(&tool.places),
        dest.build_root(&tool.places),
    );
    // The destination built in target-macos already.
    fill_file(&to.join("target-macos/debug/own"), 4);
    let dirs = build_dirs(&dest.top).unwrap();
    let paths = |lane| -> Vec<String> {
        cloned_folders(&dirs, lane, &from, &to)
            .unwrap()
            .into_iter()
            .map(|dir| dir.path)
            .collect()
    };
    assert_eq!(paths(Lane::MAC), ["target"]);
    assert_eq!(paths(Lane::ANDROID), ["target", "target-android"]);
}

#[test]
fn a_source_with_no_cargo_build_leaves_the_new_checkout_cold() {
    let scratch = Scratch::new("warm-lane-cold");
    let tool = scratch.tool(0);
    let (source, dest) = lane_checkouts(&scratch, &tool);
    let from = source.build_root(&tool.places);
    for cargo in ["target", "target-macos"] {
        std::fs::remove_dir_all(from.join(cargo).join("debug")).unwrap();
    }
    clone_lane(&tool, Lane::MAC, &source, &dest).unwrap();
    let to = dest.build_root(&tool.places);
    assert!(
        std::fs::read_dir(to.join("target-macos"))
            .unwrap()
            .next()
            .is_none()
    );
}

/// Writes the run of a build script that printed <printed>, in the shared
/// folders layout (build/<package>-<hash>/) when <shared> is set, else in the
/// unit folders layout (build/<package>/<hash>/).
fn build_script_run(profile: &Path, package: &str, hash: &str, shared: bool, printed: &str) {
    if shared {
        let print = profile.join(format!("build/{package}-{hash}/output"));
        std::fs::create_dir_all(print.parent().unwrap()).unwrap();
        std::fs::write(print, printed).unwrap();
        fill_file(
            &profile.join(format!(
                ".fingerprint/{package}-{hash}/run-build-script-build-script-build"
            )),
            1,
        );
    } else {
        let unit = profile.join(format!("build/{package}/{hash}"));
        std::fs::create_dir_all(unit.join("run")).unwrap();
        std::fs::write(unit.join("run/stdout"), printed).unwrap();
        fill_file(
            &unit.join("fingerprint/run-build-script-build-script-build"),
            1,
        );
    }
}

/// Writes into <profile> the runs a clone from <top> must forget and the ones
/// it must keep. Returns the fingerprint folder of each run to forget, then of
/// each to keep.
fn runs_naming(profile: &Path, top: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let top = top.display();
    let dist = format!("cargo:rustc-link-search=native={top}/bae-ffmpeg/dist/lib\n");
    let env = format!("cargo:rustc-env=SOURCE_DIR={top}\n");
    let debug_only = format!("OUT_DIR = Some({top}/target/out)\ncargo:rerun-if-changed=build.rs\n");
    build_script_run(profile, "ffmpeg-sys", "1111111111111111", false, &dist);
    build_script_run(profile, "other-sys", "2222222222222222", false, &env);
    build_script_run(profile, "old-sys", "3333333333333333", true, &dist);
    build_script_run(profile, "quiet-sys", "5555555555555555", false, &debug_only);
    let unit =
        |package: &str, hash: &str| profile.join(format!("build/{package}/{hash}/fingerprint"));
    (
        vec![
            unit("ffmpeg-sys", "1111111111111111"),
            unit("other-sys", "2222222222222222"),
            profile.join(".fingerprint/old-sys-3333333333333333"),
        ],
        vec![unit("quiet-sys", "5555555555555555")],
    )
}

fn assert_runs_forgotten(forgotten: &[PathBuf], kept: &[PathBuf]) {
    for fingerprint in forgotten {
        assert!(
            !fingerprint.exists(),
            "{} names the source checkout's path, so cargo must run its build script again",
            fingerprint.display()
        );
    }
    for fingerprint in kept {
        assert!(
            fingerprint.is_dir(),
            "{} names no path of the source checkout and is still built",
            fingerprint.display()
        );
    }
}

#[test]
fn a_new_target_forgets_the_build_script_runs_that_name_the_sources_paths() {
    let scratch = Scratch::new("warm-forget");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);
    let (forgotten, kept) = runs_naming(&source.join("debug"), &main);
    let new_target = tool.places.root.join("demo/side-1/target");
    let moved = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths
            .iter()
            .map(|path| new_target.join(path.strip_prefix(&source).unwrap()))
            .collect()
    };

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string(), "--for".into(), "agent".into()],
    )
    .unwrap();

    assert_runs_forgotten(&moved(&forgotten), &moved(&kept));
    for fingerprint in forgotten.iter().chain(&kept) {
        assert!(
            fingerprint.exists(),
            "the source keeps {}",
            fingerprint.display()
        );
    }
}

#[test]
fn a_new_app_checkout_forgets_the_build_script_runs_that_name_the_sources_paths() {
    let scratch = Scratch::new("warm-lane-forget");
    let tool = scratch.tool(0);
    let (source, dest) = lane_checkouts(&scratch, &tool);
    let (from, to) = (
        source.build_root(&tool.places),
        dest.build_root(&tool.places),
    );
    let (forgotten, kept) = runs_naming(&from.join("target-macos/debug"), &source.top);
    let moved = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths
            .iter()
            .map(|path| to.join(path.strip_prefix(&from).unwrap()))
            .collect()
    };

    clone_lane(&tool, Lane::MAC, &source, &dest).unwrap();

    assert_runs_forgotten(&moved(&forgotten), &moved(&kept));
}

#[test]
fn a_new_target_forgets_the_build_script_runs_that_name_any_other_checkout() {
    let scratch = Scratch::new("forget-other-checkouts");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);
    let (trees, builds) = (
        scratch.dir.join("demo-worktrees"),
        tool.places.root.join("demo"),
    );
    let new_root = builds.join("side-1");
    let search = |path: &Path| format!("cargo:rustc-link-search=native={}\n", path.display());
    let runs = [
        (
            "gone-tree",
            true,
            search(&trees.join("side-15/bae-ffmpeg/dist/lib")),
        ),
        (
            "gone-root",
            true,
            search(&builds.join("side-15/target/debug/build/x/out")),
        ),
        ("near-tree", true, search(&trees.join("side-10/dist"))),
        ("main-tree", true, search(&main.join("bae-ffmpeg/dist/lib"))),
        (
            "own-tree",
            false,
            search(&trees.join("side-1/bae-ffmpeg/dist/lib")),
        ),
        (
            "own-root",
            false,
            search(&new_root.join("target/debug/build/x/out")),
        ),
        ("system", false, search(Path::new("/opt/ffmpeg/lib"))),
    ];
    // A unit is named by its hash alone, so each run has its own.
    let hash = |at: usize| format!("{at:016x}");
    for (at, (package, _, printed)) in runs.iter().enumerate() {
        build_script_run(&source.join("debug"), package, &hash(at), false, printed);
    }

    cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[main.display().to_string(), "--for".into(), "agent".into()],
    )
    .unwrap();

    for (at, (package, forgotten, _)) in runs.iter().enumerate() {
        let fingerprint = new_root.join(format!(
            "target/debug/build/{package}/{}/fingerprint",
            hash(at)
        ));
        assert_eq!(
            !fingerprint.exists(),
            *forgotten,
            "{package}: a run that names another checkout is forgotten, any other is kept"
        );
    }
}
