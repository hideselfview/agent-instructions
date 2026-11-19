use super::*;

/// JSON stats as sccache 0.13 prints them, cut to the fields builds reads
/// and a few it does not.
const STATS: &str = r#"{"stats":{"compile_requests":112611,"requests_executed":92569,"cache_hits":{"counts":{"Assembler":6440,"C/C++":14727,"Rust":35361},"adv_counts":{}},"cache_misses":{"counts":{"C/C++":565,"Rust":34774},"adv_counts":{}}},"cache_size":53670801410,"max_cache_size":53687091200}"#;

/// The lines the listener script prints when the server (pid 44790) listens.
fn listening() -> String {
    format!(
        "listener_status=0\nlistener=p44790\nlistener=R1\nlistener=csccache\nlistener=f12\nlistener_said=\nsccache_stats={STATS}\n"
    )
}

const PS: &str = "    1     0     0   0.0   1:00.00 9-00:00:00 S /sbin/launchd\n44790     1   501   0.0  10:00.00 2-07:02:40 S /Users/x/.cargo/bin/sccache\n";

#[test]
fn reads_what_a_host_printed() {
    let scan = parse(&format!(
        "uid=501\ncpu=CPU usage: 95.17% user, 4.9% sys, 0.72% idle \njob=/Users/x/dev/bae-worktrees/remote-side-5 76647 17546-1795067690799282000\n{}{PS_FOLLOWS}\n{PS}",
        listening()
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
    assert_eq!(
        scan.sccache,
        Ok(Some(CacheServer {
            pid: 44790,
            stats: crate::sccache::CacheStats {
                executed: 92569,
                rust_hits: 35361,
                rust_misses: 34774,
                cache_size: 53670801410,
                max_cache_size: 53687091200,
            }
        }))
    );
}

/// A host where no server listens has no server; stats that hold no counts
/// are an error that names them, not a server with none.
#[test]
fn reads_a_host_with_no_server_and_stats_it_cannot_read() {
    let none =
        format!("uid=501\ncpu=\nlistener_status=1\nlistener=\nlistener_said=\n{PS_FOLLOWS}\n{PS}");
    assert_eq!(parse(&none).unwrap().sccache, Ok(None));
    let broken = format!(
        "uid=501\ncpu=\n{}{PS_FOLLOWS}\n{PS}",
        listening().replace(STATS, "{\"stats\":{}}")
    );
    assert!(
        parse(&broken)
            .unwrap()
            .sccache
            .unwrap_err()
            .contains("no counts of hits and misses")
    );
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

/// A Linux host prints the idle share it read from /proc/stat itself.
#[test]
fn a_linux_host_prints_its_idle_share() {
    let scan = parse(&format!("uid=1000\ncpu_idle=12.5\n{PS_FOLLOWS}\n{PS}")).unwrap();
    assert_eq!(scan.cpu_idle, Ok(12.5));
    let scan = parse(&format!("uid=1000\ncpu_idle=\n{PS_FOLLOWS}\n{PS}")).unwrap();
    assert!(scan.cpu_idle.unwrap_err().contains("no idle share"));
}

#[test]
fn finds_a_processs_tree() {
    let rows = parse_ps(
        "10 1 501 0.0 0:01.00 01:00 S builds cargo test\n11 10 501 0.0 0:01.00 01:00 S cargo test\n12 11 501 0.0 0:01.00 01:00 S rustc a\n13 1 501 0.0 0:01.00 01:00 S other\n",
    )
    .unwrap();
    let pids: Vec<i32> = tree(&rows, 10).iter().map(|row| row.pid).collect();
    assert_eq!(pids, [10, 11, 12]);
    assert_eq!(program(&rows[0]), "builds");
    assert_eq!(arguments(&rows[0]), ["cargo", "test"]);
}

/// The script, run on this Mac with a stand-in for top, reads top's second
/// sample, which covers the last second, lists the processes, and prints
/// the job that the run script (`remote::run_script`) runs, not a claim
/// whose process group ended. Once the job's command ends, the script lists
/// it no more.
#[test]
fn the_script_reads_the_last_second_of_the_cpu_and_the_jobs_that_run() {
    use std::os::unix::fs::PermissionsExt;
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
    // lsof's exit 1 with nothing printed: no server listens.
    let lsof = bin.join("lsof");
    std::fs::write(
        &lsof,
        "#!/bin/sh
exit 1
",
    )
    .unwrap();
    std::fs::set_permissions(&lsof, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = scratch.dir.join("home");
    let worktrees = home.join("dev/demo-worktrees");
    let (running, ended) = (
        worktrees.join("remote-side-1"),
        worktrees.join("remote-side-2"),
    );
    for wt in [&running, &ended] {
        std::fs::create_dir_all(wt).unwrap();
        std::fs::create_dir_all(wt.with_extension("job")).unwrap();
    }
    // A claim whose run script was killed: its record names a group that
    // no longer runs.
    std::fs::write(
        ended.with_extension("job").join(crate::remote::RUN_FILE),
        format!("{} 78-1795067690799282000\n", crate::testing::gone_pid()),
    )
    .unwrap();
    let local = crate::host::Host::local();
    let scan = || {
        let output = std::process::Command::new("sh")
            .args(["-c", &script(&local)])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", &home)
            .output()
            .unwrap();
        parse(&String::from_utf8_lossy(&output.stdout)).unwrap()
    };
    let (started, release) = (running.join("started"), running.join("release"));
    crate::testing::fifo(&started);
    crate::testing::fifo(&release);
    let run = "77-1795067690799282000";
    let mut job = std::process::Command::new("sh")
        .args([
            "-c",
            &crate::remote::run_script(
                &local,
                &running.display().to_string(),
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "echo $$ > started; read line < release".to_string(),
                ],
                run,
            ),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The read waits until the command has started.
    let group: i32 = std::fs::read_to_string(&started)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let scan_while_running = scan();
    std::fs::write(&release, "\n").unwrap();
    let status = job.wait().unwrap();
    drop(job.stdin.take());
    assert!(status.success(), "{status}");
    assert_eq!(scan_while_running.uid, unsafe { libc::getuid() });
    assert_eq!(scan_while_running.cpu_idle, Ok(40.0));
    assert_eq!(scan_while_running.sccache, Ok(None));
    assert_eq!(
        scan_while_running.jobs,
        [JobRun {
            worktree: running.display().to_string(),
            leader: group,
            run: run.to_string(),
        }]
    );
    assert!(
        scan_while_running
            .rows
            .iter()
            .any(|row| row.pid == std::process::id() as i32)
    );
    assert_eq!(scan().jobs, []);
}
