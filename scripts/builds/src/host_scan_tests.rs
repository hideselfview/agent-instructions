use super::*;

const PS: &str = "    1     0     0   0.0   1:00.00 9-00:00:00 /sbin/launchd\n44790     1   501   0.0  10:00.00 2-07:02:40 /Users/x/.cargo/bin/sccache\n";

#[test]
fn reads_what_a_host_printed() {
    let scan = parse(&format!(
        "uid=501\ncpu=CPU usage: 95.17% user, 4.9% sys, 0.72% idle \njob=/Users/x/dev/bae-worktrees/remote-side-5 76647 17546-1795067690799282000\n{PS_FOLLOWS}\n{PS}"
    ))
    .unwrap();
    assert_eq!(scan.uid, 501);
    assert_eq!(scan.cpu_idle, Ok(0.72));
    assert_eq!(scan.rows.len(), 2);
    assert_eq!(
        scan.jobs,
        [JobRun {
            worktree: "/Users/x/dev/bae-worktrees/remote-side-5".to_string(),
            leader: 76647,
            run: "17546-1795067690799282000".to_string(),
        }]
    );
    let job = &scan.jobs[0];
    assert_eq!(job.caller(), Some(17546));
    assert_eq!(job.started(), Some(1_795_067_690));
    assert_eq!(job.label(), "bae/remote-side-5");
    assert!(job.is_cargo_job());
    let app = JobRun {
        worktree: "/Users/x/dev/bae-worktrees/mac-1".to_string(),
        ..job.clone()
    };
    assert!(!app.is_cargo_job());
}

/// A top that printed no idle share leaves the rest of the reading whole;
/// a reading with no process listing is an error.
#[test]
fn a_missing_idle_share_is_an_error_of_its_own() {
    let scan = parse(&format!(
        "uid=501\ncpu=CPU usage: 95.17% user, 4.9% sys\n{PS_FOLLOWS}\n{PS}"
    ))
    .unwrap();
    assert!(scan.cpu_idle.unwrap_err().contains("no idle share"));
    assert_eq!(scan.rows.len(), 2);
    assert!(parse("uid=501\ncpu=\n").is_err());
    assert!(parse(&format!("cpu=\n{PS_FOLLOWS}\n{PS}")).is_err());
}

#[test]
fn finds_a_processs_tree() {
    let rows = parse_ps(
        "10 1 501 0.0 0:01.00 01:00 builds cargo test\n11 10 501 0.0 0:01.00 01:00 cargo test\n12 11 501 0.0 0:01.00 01:00 rustc a\n13 1 501 0.0 0:01.00 01:00 other\n",
    )
    .unwrap();
    let pids: Vec<i32> = tree(&rows, 10).iter().map(|row| row.pid).collect();
    assert_eq!(pids, [10, 11, 12]);
    assert_eq!(program(&rows[0]), "builds");
    assert_eq!(arguments(&rows[0]), ["cargo", "test"]);
}

/// The script, run on this Mac with a stand-in for top, reads top's second
/// sample, which covers the last second, lists the processes, and prints
/// the job claim whose process group runs, not the one whose group ended.
#[test]
fn the_script_reads_the_last_second_of_the_cpu_and_the_jobs_that_run() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let scratch = crate::testing::Scratch::new("host-scan");
    let bin = scratch.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let top = bin.join("top");
    std::fs::write(
        &top,
        "#!/bin/sh\necho 'CPU usage: 10.0% user, 5.0% sys, 85.0% idle'\necho 'CPU usage: 50.0% user, 10.0% sys, 40.0% idle'\n",
    )
    .unwrap();
    std::fs::set_permissions(&top, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut group = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let home = scratch.dir.join("home");
    let worktrees = home.join("dev/demo-worktrees");
    for (name, pid) in [("remote-side-1", group.id()), ("remote-side-2", 999_999)] {
        let claim = worktrees.join(format!("{name}.job"));
        std::fs::create_dir_all(&claim).unwrap();
        std::fs::write(claim.join("pid"), format!("{pid} 77-1795067690799282000\n")).unwrap();
    }
    let output = std::process::Command::new("sh")
        .args(["-c", &script(&crate::host::Host::local())])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &home)
        .output()
        .unwrap();
    let _ = group.kill();
    let _ = group.wait();
    let scan = parse(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(scan.uid, unsafe { libc::getuid() });
    assert_eq!(scan.cpu_idle, Ok(40.0));
    assert_eq!(
        scan.jobs,
        [JobRun {
            worktree: worktrees.join("remote-side-1").display().to_string(),
            leader: group.id() as i32,
            run: "77-1795067690799282000".to_string(),
        }]
    );
    assert!(
        scan.rows
            .iter()
            .any(|row| row.pid == std::process::id() as i32)
    );
}
