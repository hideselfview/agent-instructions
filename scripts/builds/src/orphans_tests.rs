use super::*;

fn m1() -> Host {
    Host {
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

fn scan(ps: &str, jobs: Vec<JobRun>) -> std::result::Result<Scan, String> {
    Ok(Scan {
        uid: 501,
        at: 0,
        rows: crate::stuck::parse_ps(ps).unwrap(),
        cpu_idle: Ok(50.0),
        jobs,
        sccache: Ok(None),
        front_app: Ok(None),
        screen_locked: Ok(None),
    })
}

/// This Mac's processes under launchd: the tools whose caller is gone, and
/// the processes that run there on purpose or belong to another user.
const HERE: &str = "    1     0     0   0.0   1:00.00 9-00:00:00 S /sbin/launchd\n\
  300     1   501   0.0   0:01.00 2-07:16:47 S /usr/bin/git -C /tmp/builds-test-1/demo-worktrees/side-1 -c x commit\n\
  301   300   501   0.0   0:01.00 2-07:16:47 S /usr/bin/vim .git/COMMIT_EDITMSG\n\
  302     1   501   0.0   0:01.00    03:19 S builds dashboard --listen 192.168.1.166\n\
  303     1   501   0.0   0:01.00    01:19 S /Users/x/.local/bin/builds units after-build /x\n\
  312     1   501   0.0   0:01.00    05:19 S builds review audit 77 --project bae\n\
  304     1   501   0.0   0:01.00 1-06:22:49 S /Users/x/.cargo/bin/sccache\n\
  305     1   501   0.0   0:01.00 1-06:22:49 S git fsmonitor--daemon run\n\
  306     1   501   0.0   0:01.00    00:30 S cargo test\n\
  307     1   501   0.0   0:01.00    40:00 S builds land side-3\n\
  308     1   501   0.0   0:01.00    40:00 S ssh -o BatchMode=yes -o ConnectTimeout=10 u@m1 sh -c x\n\
  309     1     0   0.0   0:01.00    40:00 S cargo test\n\
  310     1   501   0.0   0:01.00    40:00 S /Applications/Safari.app/Contents/MacOS/Safari\n\
  311     1   501   0.0   0:01.00    40:00 S /x/dev/bae-worktrees/side-2/target/debug/deps/bae_views-0123456789abcdef\n\
  313     1   501   0.0   0:00.00    40:00 Z cargo test\n\
  314     1   501   0.0   0:01.00    12:19 S /Users/x/.local/bin/builds build-app bae mac main\n";

/// The orphans among each host's processes, and the jobs whose caller on
/// this Mac is gone; a job whose caller runs is none.
#[test]
fn finds_the_processes_whose_caller_is_gone() {
    // A microsecond before the reading: the clock can read the same time
    // twice, and a claim made at the reading's time counts as running.
    let made = crate::job_claim::now_nanos() - 1_000;
    let holders = Holders::read().unwrap();
    let job = |caller: u32, leader: i32| JobRun {
        worktree: "/Users/u/dev/bae-worktrees/remote-side-5".to_string(),
        leader,
        run: format!("{caller}-{made}"),
    };
    let gone = job(999_999, 6000);
    let scans = vec![
        (Host::local(), scan(HERE, Vec::new())),
        (
            m1(),
            scan(
                "6000 5999 502 0.0 0:01.00 40:00 S cargo test\n",
                vec![gone.clone(), job(std::process::id(), 6001)],
            ),
        ),
    ];
    let found = find(&scans, &holders);
    let shown: Vec<(String, Option<String>)> = found
        .iter()
        .map(|orphan| (orphan.host.name.clone(), orphan.stop_command()))
        .collect();
    assert_eq!(
        shown,
        [
            ("local".to_string(), Some("kill -TERM 300 301".to_string())),
            ("local".to_string(), None),
            ("local".to_string(), Some("kill -TERM 308".to_string())),
            ("local".to_string(), Some("kill -TERM 311".to_string())),
            (
                "m1".to_string(),
                Some("ssh u@m1 'kill -TERM -6000'".to_string())
            ),
        ]
    );
    assert_eq!(
        found[4].kind,
        Kind::Job {
            job: gone,
            caller: 999_999
        }
    );
}

/// A finding says what the orphan is and, for one health stops, the command
/// that stops it on its host; a `builds` command that still does its work
/// gets no command.
#[test]
fn a_finding_names_the_orphan_and_how_to_stop_it() {
    let rows = crate::stuck::parse_ps(HERE).unwrap();
    let git = Orphan {
        host: m1(),
        kind: Kind::Process {
            row: rows[1].clone(),
            tree: vec![300, 301],
            stops: true,
        },
    };
    assert_eq!(
        git.finding().line(),
        "orphan: host m1 pid 300 (/usr/bin/git -C /tmp/builds-test-1/demo-worktrees/side-1 -c x commit), 2d7h old, with 1 process under it: its caller is gone, so nothing reads what it prints; stop it with `ssh u@m1 'kill -TERM 300 301'`"
    );
    let land = Orphan {
        host: Host::local(),
        kind: Kind::Process {
            row: rows[9].clone(),
            tree: vec![307],
            stops: false,
        },
    };
    assert_eq!(
        land.finding().line(),
        "orphan: this Mac pid 307 (builds land side-3), 40m old: its caller is gone; it does its work to its end, and nothing reads what it prints"
    );
    let job = Orphan {
        host: m1(),
        kind: Kind::Job {
            caller: 999999,
            job: JobRun {
                worktree: "/Users/u/dev/bae-worktrees/remote-side-5".to_string(),
                leader: 6000,
                run: "999999-1795067690799282000".to_string(),
            },
        },
    };
    assert_eq!(
        job.finding().line(),
        "orphan: the job in bae/remote-side-5 on host m1 (process group 6000), which process 999999 on this Mac ran: its caller is gone, so nothing reads what it prints; stop it with `ssh u@m1 'kill -TERM -6000'`"
    );
}

/// The stop script signals each target on its own and names only a target
/// that refused the signal and still runs: a pid that ended first counts as
/// stopped (no pid reaches 999999 on macOS). launchd (pid 1) refuses a
/// signal from any user but root.
#[test]
fn the_stop_script_names_only_a_target_that_refused() {
    // As root, the signal to launchd would go through.
    if unsafe { libc::getuid() } == 0 {
        return;
    }
    let rows = crate::stuck::parse_ps("1 0 0 0.0 0:01.00 40:00 S cargo test\n").unwrap();
    let orphan = Orphan {
        host: Host::local(),
        kind: Kind::Process {
            row: rows[0].clone(),
            tree: vec![999_999, 1],
            stops: true,
        },
    };
    let output = std::process::Command::new("sh")
        .args(["-c", &orphan.stop_script().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "refused=1\n");
}

/// The dashboard starts each `builds build-app` as its child, and a restart
/// of the dashboard leaves it under launchd with its work to do: it is no
/// orphan, while another `builds` command with no caller is listed.
#[test]
fn a_build_app_the_dashboard_started_is_no_orphan() {
    let ps = "314     1   501   0.0   0:01.00    12:19 S /Users/x/.local/bin/builds build-app bae mac main\n\
              307     1   501   0.0   0:01.00    40:00 S builds land side-3\n";
    let scans = vec![(Host::local(), scan(ps, Vec::new()))];
    let pids: Vec<i32> = find(&scans, &Holders::read().unwrap())
        .iter()
        .filter_map(|orphan| match &orphan.kind {
            Kind::Process { row, .. } => Some(row.pid),
            Kind::Job { .. } => None,
        })
        .collect();
    assert_eq!(pids, [307]);
}
