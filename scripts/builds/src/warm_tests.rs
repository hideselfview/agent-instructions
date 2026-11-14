use std::os::unix::fs::MetadataExt;

use super::*;
use crate::claim::cmd_release;
use crate::evict::lock_shared_creating;
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
    setup(tool, &Checkout::find(&main.display().to_string()).unwrap()).unwrap();
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
        &[main.display().to_string(), "--for".into(), "agent".into()],
    )
    .unwrap();
    let side = scratch.dir.join("demo-worktrees/side-1");
    let target = tool.places.root.join("demo/side-1/target");
    assert!(side.join("target").is_symlink());
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

    cmd_new(&tool, std::slice::from_ref(&main_arg)).unwrap();
    let target = tool.places.root.join("demo/side-1/target");
    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    // The agent built there: the folder holds its own state, which a new
    // hand-out leaves alone.
    fill_file(&target.join("debug/own"), 4);
    std::fs::remove_dir_all(target.join("debug/deps")).unwrap();
    cmd_new(&tool, std::slice::from_ref(&main_arg)).unwrap();
    assert!(target.join("debug/own").is_file());
    assert!(!target.join("debug/deps").exists());

    cmd_release(&scratch.places(), std::slice::from_ref(&side_arg)).unwrap();
    std::fs::remove_dir_all(&target).unwrap();
    std::fs::create_dir(&target).unwrap();
    cmd_new(&tool, std::slice::from_ref(&main_arg)).unwrap();
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
    cmd_new(&tool, &[main.display().to_string()]).unwrap();
    let target = tool.places.root.join("demo/side-1/target");
    assert!(target.is_dir() && std::fs::read_dir(&target).unwrap().next().is_none());
}

#[test]
fn a_target_over_the_new_ones_cap_is_not_cloned() {
    let scratch = Scratch::new("warm-cap");
    let tool = scratch.tool(0);
    let (main, source) = warm_main(&scratch, &tool);
    fill_file(&source.join("debug/deps/big"), 1024 * 1024 + 16);
    cmd_new(&tool, &[main.display().to_string()]).unwrap();
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
    let build = lock_shared_creating(&root.join(GUARD)).unwrap();
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
fn a_revision_without_a_lock_is_empty_and_git_failing_is_an_error() {
    let scratch = Scratch::new("warm-lock-blob");
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    assert_eq!(lock_blob(&main, "HEAD").unwrap(), "");
    assert_eq!(lock_blob(&main, "origin/main").unwrap(), "");
    std::fs::write(main.join("Cargo.lock"), "# lock\n").unwrap();
    git(&main, &["add", "Cargo.lock"]);
    git(&main, &["commit", "-q", "-m", "Lock"]);
    assert_eq!(lock_blob(&main, "HEAD").unwrap().len(), 40);

    let not_a_repo = scratch.dir.join("plain");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    let fault = lock_blob(&not_a_repo, "HEAD").unwrap_err().text();
    assert!(
        fault.contains("git failed to find Cargo.lock at HEAD"),
        "{fault}"
    );

    std::fs::write(main.join(".git/HEAD"), "garbage\n").unwrap();
    let broken = lock_blob(&main, "HEAD").unwrap_err().text();
    assert!(
        broken.contains("git failed to find Cargo.lock at HEAD"),
        "{broken}"
    );
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
