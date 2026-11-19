use super::*;
use crate::testing::{Scratch, git, repo};
use std::os::unix::fs::PermissionsExt;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// A repo `demo` with a Lean package in `proofs`, its main checkout and one
/// side worktree.
fn checkouts(scratch: &Scratch) -> (Checkout, Checkout) {
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\nproofs/.lake 1G\n");
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
    (
        Checkout::at(&main).unwrap().unwrap(),
        Checkout::at(&side).unwrap().unwrap(),
    )
}

#[test]
fn a_lake_build_in_a_side_worktree_may_leave_this_mac() {
    let scratch = Scratch::new("lake-stays");
    let (main, side) = checkouts(&scratch);
    assert_eq!(stays_here(&args(&["build"]), Some(&side)), None);
    assert_eq!(
        stays_here(&args(&["build", "narrowingRunner"]), Some(&side)),
        None
    );
    for (words, checkout, why) in [
        (&["build"][..], Some(&main), "the person's"),
        (&["build"], None, "no git checkout"),
        (&["build", "/x/Main.lean"], Some(&side), "path on this Mac"),
    ] {
        let said = stays_here(&args(words), checkout).unwrap();
        assert!(said.contains(why), "{words:?}: {said}");
    }
}

#[test]
fn a_host_runs_builds_lake_as_local_in_the_callers_folder() {
    let scratch = Scratch::new("lake-command");
    let (_main, side) = checkouts(&scratch);
    let none = |_: &str| None;
    assert_eq!(
        host_command(
            &args(&["build", "narrowingRunner"]),
            &side,
            &side.top.join("proofs"),
            &none
        ),
        args(&[
            "sh",
            "-c",
            "cd 'proofs' || exit $?\nexec \"$@\"",
            "sh",
            "env",
            "BUILDS_CARGO_HOST=local",
            "builds",
            "lake",
            "build",
            "narrowingRunner"
        ])
    );
    assert_eq!(
        host_command(&args(&["build"]), &side, &side.top, &none),
        args(&["env", "BUILDS_CARGO_HOST=local", "builds", "lake", "build"])
    );
}

/// Writes under <home> a `lake` that writes down its arguments, its folder
/// and its elan home in <record>, and exits with 3 when an argument is
/// `broken`.
fn fake_lake(home: &Path, record: &Path) {
    let lake = crate::lean::lake(home);
    std::fs::create_dir_all(lake.parent().unwrap()).unwrap();
    std::fs::write(
        &lake,
        format!(
            "#!/bin/sh\necho \"$* $(pwd) $ELAN_HOME\" > '{}'\ncase \" $* \" in *\" broken \"*) exit 3;; esac\n",
            record.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&lake, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn here_lake_runs_in_the_folder_with_elans_home_and_its_status_comes_back() {
    let scratch = Scratch::new("lake-here");
    let places = scratch.places();
    let (_main, side) = checkouts(&scratch);
    let proofs = side.top.join("proofs");
    std::fs::create_dir_all(&proofs).unwrap();
    let none = |_: &str| None;

    // Without elan's lake, the run fails and names setup. The package has no
    // lean-toolchain, so nothing is installed first.
    let message = run_here(&places, Some(&side), &proofs, &args(&["build"]), &none)
        .unwrap_err()
        .text();
    assert!(
        message.contains("`builds setup` installs elan"),
        "{message}"
    );

    let record = scratch.dir.join("ran.txt");
    fake_lake(&places.home, &record);
    let code = run_here(
        &places,
        Some(&side),
        &proofs,
        &args(&["build", "narrowingRunner"]),
        &none,
    )
    .unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(&record).unwrap().trim(),
        format!(
            "build narrowingRunner {} {}",
            proofs.display(),
            places.home.join(".elan").display()
        )
    );
    let failed = run_here(
        &places,
        Some(&side),
        &proofs,
        &args(&["build", "broken"]),
        &none,
    );
    assert_eq!(failed.unwrap(), 3);
}
