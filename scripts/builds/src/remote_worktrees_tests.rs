use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::{Scratch, git};
use std::path::Path;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn worktree(name: &str, claim: Claim) -> HostWorktree {
    HostWorktree {
        repo: "bae".to_string(),
        name: name.to_string(),
        claim,
    }
}

#[test]
fn reads_what_a_host_lists() {
    assert_eq!(
        parse_list(
            "worktree bae remote-3 -\nworktree bae remote-side-2 41-17\nworktree coven remote-1 ?\nnoise\n"
        ),
        vec![
            worktree("remote-3", Claim::Free),
            worktree("remote-side-2", Claim::Job("41-17".to_string())),
            HostWorktree {
                repo: "coven".to_string(),
                name: "remote-1".to_string(),
                claim: Claim::Starting,
            },
        ]
    );
}

/// A worktree belongs to a checkout when it is `remote-<checkout>` or one of
/// the `remote-<checkout>.<n>` that jobs take while another holds the first.
#[test]
fn a_worktree_belongs_to_the_checkout_it_is_named_after() {
    let here = names(&["bae", "side-1", "side-12"]);
    assert!(belongs("remote-bae", &here));
    assert!(belongs("remote-side-1", &here));
    assert!(belongs("remote-side-1.2", &here));
    assert!(belongs("remote-side-12", &here));
    assert!(!belongs("remote-side-2", &here));
    assert!(!belongs("remote-side-1.", &here));
    assert!(!belongs("remote-side-1.x", &here));
    assert!(!belongs("remote-3", &here));
    assert!(!belongs("side-1", &here));
}

/// An unused worktree goes unless a job that still runs claims it, or a job
/// is claiming it now; a worktree of a checkout here always stays. A claim
/// whose job id names no pid keeps the worktree, and prune says why.
#[test]
fn an_unused_worktree_goes_unless_a_running_job_claims_it() {
    let here = names(&["bae", "side-1"]);
    let runs = |id: &str| match id {
        "live-1" => Some(true),
        "dead-1" => Some(false),
        _ => None,
    };
    assert_eq!(
        judge(&worktree("remote-3", Claim::Free), &here, runs),
        Judged::Unused("no checkout of bae here is named 3".to_string())
    );
    assert_eq!(
        judge(&worktree("remote-3", Claim::Job("dead-1".into())), &here, runs),
        Judged::Unused("no checkout of bae here is named 3, and the process of job dead-1, which claimed it, is gone".to_string())
    );
    assert_eq!(
        judge(
            &worktree("remote-3", Claim::Job("live-1".into())),
            &here,
            runs
        ),
        Judged::Used
    );
    assert_eq!(
        judge(&worktree("remote-3", Claim::Job("junk".into())), &here, runs),
        Judged::UnreadableClaim("no checkout of bae here is named 3, but the job id \"junk\" in its claim names no pid builds can check".to_string())
    );
    assert_eq!(
        judge(&worktree("remote-3", Claim::Starting), &here, runs),
        Judged::Used
    );
    assert_eq!(
        judge(&worktree("remote-side-1.2", Claim::Free), &here, runs),
        Judged::Used
    );
}

/// The pid of a process that has exited.
fn gone_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn a_job_runs_while_its_process_on_this_mac_runs() {
    assert_eq!(job_runs(&format!("{}-17", std::process::id())), Some(true));
    assert_eq!(job_runs(&format!("{}-17", gone_pid())), Some(false));
    assert_eq!(job_runs("unreadable"), None);
    assert_eq!(job_runs("x-17"), None);
}

/// Adds worktree <name> of the host's clone <clone>, with a build root in the
/// host's builds root, and claims it with <claim> when one is given.
fn host_worktree(home: &Path, clone: &Path, name: &str, claim: Option<&str>) {
    let wt = home.join("dev/demo-worktrees").join(name);
    git(
        clone,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &wt.display().to_string(),
        ],
    );
    let root = home.join(crate::host::REMOTE_ROOT).join("demo").join(name);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("built"), "x").unwrap();
    if let Some(id) = claim {
        let job = home.join(format!("dev/demo-worktrees/{name}.job"));
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("id"), format!("{id}\n")).unwrap();
    }
}

/// prune removes each worktree on a host that belongs to no checkout here,
/// with its build root, including one whose claim's job is gone, and keeps
/// the worktrees of the checkouts here, the one a running job claims, an app
/// checkout, and everything with --dry-run. A host that does not answer is
/// passed over.
#[test]
fn prune_removes_the_worktrees_on_a_host_that_no_checkout_uses() {
    let scratch = Scratch::new("remote-worktrees");
    let places = scratch.places();
    let clone_here = places.home.join("dev/demo");
    crate::testing::repo(&clone_here, "target 1G\n");
    git(
        &clone_here,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &places
                .home
                .join("dev/demo-worktrees/side-1")
                .display()
                .to_string(),
        ],
    );
    let home = scratch.dir.join("remote-home");
    std::fs::create_dir_all(home.join("dev")).unwrap();
    let clone = home.join("dev/demo");
    git(
        &scratch.dir,
        &[
            "clone",
            "-q",
            &clone_here.display().to_string(),
            &clone.display().to_string(),
        ],
    );
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let hosts = crate::host::hosts_file(&places);
    std::fs::create_dir_all(hosts.parent().unwrap()).unwrap();
    std::fs::write(&hosts, "m1 u@m1 10\ngone unreachable 4\n").unwrap();
    let live = format!("{}-17", std::process::id());
    let dead = format!("{}-17", gone_pid());
    host_worktree(&home, &clone, "remote-demo", None);
    host_worktree(&home, &clone, "remote-side-1", None);
    host_worktree(&home, &clone, "remote-side-1.2", None);
    host_worktree(&home, &clone, "remote-3", None);
    host_worktree(&home, &clone, "remote-4", Some(&dead));
    host_worktree(&home, &clone, "remote-5", Some(&live));
    host_worktree(&home, &clone, "remote-app", None);
    places
        .write_app_checkouts("demo", Lane::Android, &names(&["m1:remote-app"]))
        .unwrap();
    let all = [
        "remote-demo",
        "remote-side-1",
        "remote-side-1.2",
        "remote-3",
        "remote-4",
        "remote-5",
        "remote-app",
    ];
    let present = || -> Vec<&str> {
        all.iter()
            .copied()
            .filter(|name| home.join("dev/demo-worktrees").join(name).exists())
            .collect()
    };

    prune(&places, &ssh, Run::Dry).unwrap();
    assert_eq!(present(), all, "a dry run removes nothing");

    prune(&places, &ssh, Run::Real).unwrap();
    assert_eq!(
        present(),
        [
            "remote-demo",
            "remote-side-1",
            "remote-side-1.2",
            "remote-5",
            "remote-app"
        ]
    );
    let roots = home.join(crate::host::REMOTE_ROOT).join("demo");
    assert!(!roots.join("remote-3").exists());
    assert!(!roots.join("remote-4").exists());
    assert!(roots.join("remote-5/built").exists());
    assert!(!home.join("dev/demo-worktrees/remote-3.job").exists());
    assert!(!home.join("dev/demo-worktrees/remote-4.job").exists());
    assert!(home.join("dev/demo-worktrees/remote-5.job/id").exists());
    let listed = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(!listed.contains("remote-3"), "{listed}");
}

/// A worktree another job claimed since the listing stays.
#[test]
fn a_worktree_a_job_claimed_since_the_listing_stays() {
    let scratch = Scratch::new("remote-worktrees-claimed");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let clone = home.join("dev/demo");
    crate::testing::repo(&clone, "target 1G\n");
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    host_worktree(&home, &clone, "remote-3", Some("41-17"));
    let host = Host {
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    // The listing saw job 40's claim; job 41 holds it now.
    let seen = worktree("remote-3", Claim::Job("40-17".to_string()));
    let seen = HostWorktree {
        repo: "demo".to_string(),
        ..seen
    };
    let transport = Transport {
        places: &places,
        ssh: &ssh,
        host: &host,
    };
    let said = transport
        .ask(&remove_script(&host, &seen), "removing a job worktree")
        .unwrap()
        .unwrap_err();
    assert!(said.contains("a job claimed"), "{said}");
    assert!(home.join("dev/demo-worktrees/remote-3/.git").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("dev/demo-worktrees/remote-3.job/id")).unwrap(),
        "41-17\n"
    );
}
