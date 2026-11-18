use super::*;
use crate::testing::{Scratch, gone_pid};
use std::path::Path;

/// Runs <script> after the claim functions for <holders>, and returns its
/// exit status and what it printed.
fn shell(holders: &Holders, script: &str) -> (bool, String) {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{}{script}", holders.script()))
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    )
}

fn claim(dir: &Path, name: &str, id: &str) -> std::path::PathBuf {
    let folder = dir.join(format!("{name}.job"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("id"), format!("{id}\n")).unwrap();
    folder
}

/// A claim's process runs when a process with its pid runs and started no
/// later than the claim; a claim made after builds read the processes counts
/// as held; the shell judges each claim as builds does.
#[test]
fn a_claim_is_gone_once_the_process_that_made_it_is() {
    let scratch = Scratch::new("job-claim-holders");
    let made_now = crate::remote::job_id();
    let gone = format!("{}-{}", gone_pid(), now_nanos());
    let holders = Holders::read().unwrap();
    let reused = format!("{}-17", std::process::id());
    let later = format!("{}-{}", gone_pid(), holders.read_at + 1);
    let no_such_pid = format!("99999999999-{}", now_nanos() - 1_000_000_000);
    for (id, holder) in [
        (made_now.as_str(), Holder::Runs),
        (gone.as_str(), Holder::Gone),
        (reused.as_str(), Holder::Gone),
        (later.as_str(), Holder::Runs),
        (no_such_pid.as_str(), Holder::Gone),
    ] {
        assert_eq!(holders.holder(id), Some(holder), "{id}");
        let folder = claim(&scratch.dir, "wt", id);
        let (gone, said) = shell(&holders, &format!("claim_gone '{}'", folder.display()));
        assert_eq!(gone, holder == Holder::Gone, "the shell judges {id}");
        if gone {
            assert_eq!(said, id);
        }
    }
    let empty = scratch.dir.join("starting.job");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(!shell(&holders, &format!("claim_gone '{}'", empty.display())).0);
    for junk in [
        "junk", "12345", "x-17", "17-x", "1-2-3", "-17", "17-", "+1-17",
    ] {
        assert_eq!(holders.holder(junk), None, "{junk}");
        let folder = claim(&scratch.dir, "junk", junk);
        assert!(
            !shell(&holders, &format!("claim_gone '{}'", folder.display())).0,
            "the shell keeps a claim whose id {junk:?} it cannot read"
        );
    }
}

/// release_gone removes a claim that still holds the id it was given, and
/// keeps one that another job holds now or that another script is releasing.
#[test]
fn release_gone_removes_only_the_claim_it_judged() {
    let scratch = Scratch::new("job-claim-release");
    let holders = Holders::read().unwrap();
    let gone = claim(&scratch.dir, "gone", "40-17");
    assert!(
        shell(
            &holders,
            &format!("release_gone '{}' 40-17", gone.display())
        )
        .0
    );
    assert!(!gone.exists());

    let taken = claim(&scratch.dir, "taken", "41-17");
    assert!(
        !shell(
            &holders,
            &format!("release_gone '{}' 40-17", taken.display())
        )
        .0
    );
    assert_eq!(
        std::fs::read_to_string(taken.join("id")).unwrap(),
        "41-17\n"
    );
    assert!(!taken.join("taking").exists(), "the script leaves no lock");

    let releasing = claim(&scratch.dir, "releasing", "40-17");
    std::fs::create_dir(releasing.join("taking")).unwrap();
    assert!(
        !shell(
            &holders,
            &format!("release_gone '{}' 40-17", releasing.display())
        )
        .0
    );
    assert!(releasing.join("id").exists());
}
