use super::*;

fn row(pid: i32, ppid: i32, uid: u32, pcpu: f64, cpu: f64, age: i64, command: &str) -> Row {
    Row {
        pid,
        ppid,
        uid,
        cpu_percent: Some(pcpu),
        // Steady over the whole span, unless a test says otherwise.
        sustained: Some(Sustained {
            percent: pcpu,
            seconds: 600,
        }),
        cpu_seconds: cpu,
        age_seconds: age,
        zombie: false,
        command: command.to_string(),
    }
}

fn sampling() -> Sampling {
    Sampling {
        home: "/Users/x".into(),
        target: None,
        at: 1_763_100_000,
    }
}

const FSEVENTSD_PATH: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/FSEvents.framework/Support/fseventsd";

const AGENT: &str = "/bin/zsh -c source /Users/x/.claude/shell-snapshots/snapshot-zsh-1.sh && eval 'cargo test' < /dev/null && pwd -P >| /tmp/c";

#[test]
fn parses_ps_lines() {
    let text = "  555     1   202   0.0 823:18.22 24-07:11:48 Ss /usr/bin/a b  c\n\
                  17  1   0  101.5 2586:37.12 06-02:03:04 Rs /usr/libexec/fseventsd\n\
                  18  1   501 3.0 1:02:03.50 05:09 Z (sleep)\n";
    let rows = parse_ps(text).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].command, "/usr/bin/a b  c");
    assert_eq!(rows[0].age_seconds, 24 * 86_400 + 7 * 3600 + 11 * 60 + 48);
    assert_eq!(rows[1].cpu_seconds, 2586.0 * 60.0 + 37.12);
    assert_eq!(rows[1].uid, 0);
    assert_eq!(rows[2].cpu_seconds, 3600.0 + 123.5);
    assert_eq!(rows[2].age_seconds, 5 * 60 + 9);
    assert!(rows[2].zombie && !rows[0].zombie);
    assert_eq!(rows[2].command, "(sleep)");
    assert!(parse_ps("1 2 3\n").is_err());
    // Linux's ps prints CPU time as [dd-]hh:mm:ss.
    let linux = parse_ps("  901     1  1000  2.0 1-02:03:04 3-00:00:01 S /usr/bin/x\n").unwrap();
    assert_eq!(linux[0].cpu_seconds, 86_400.0 + 7_384.0);
}

#[test]
fn lists_a_pegged_system_daemon_with_the_sudo_command() {
    let rows = vec![
        row(1, 0, 0, 0.0, 10.0, 9_000_000, "/sbin/launchd"),
        row(
            17,
            1,
            0,
            140.0,
            2586.0 * 60.0,
            6 * 86_400,
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/FSEvents.framework/Support/fseventsd",
        ),
        row(18, 1, 501, 3.0, 5000.0, 9_000, "/usr/bin/quiet"),
    ];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    assert_eq!(busy.len(), 1);
    assert_eq!(
        busy[0].owner,
        Owner::System {
            name: "fseventsd".into()
        }
    );
    let action = busy[0].action.as_ref().unwrap();
    let sample = action.command.as_deref().unwrap();
    assert!(sample.contains("sudo sample 17 5 -file"), "{sample}");
    assert!(!action.line().contains("killall"), "{action:?}");
    assert!(
        busy[0].line().contains("local pid 17: 140% now, 1d19h CPU"),
        "{}",
        busy[0].line()
    );
}

const WINDOW_SERVER: &str =
    "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer -daemon";

#[test]
fn a_daemon_with_high_lifetime_cpu_and_low_recent_cpu_is_not_flagged() {
    // 5h56m of CPU over 4 days, 12% of a core in the last window.
    let rows = vec![row(617, 1, 0, 12.0, 21_360.0, 4 * 86_400, WINDOW_SERVER)];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    assert_eq!(busy, vec![]);
}

#[test]
fn a_daemon_with_no_recent_share_yet_is_not_flagged() {
    let mut daemon = row(617, 1, 0, 0.0, 21_360.0, 4 * 86_400, WINDOW_SERVER);
    daemon.cpu_percent = None;
    assert_eq!(
        judge("local", &[daemon], &HashMap::new(), &sampling(), None),
        vec![]
    );
}

#[test]
fn a_daemon_with_high_recent_cpu_is_flagged_with_the_sample_command() {
    let rows = vec![row(617, 1, 0, 85.0, 21_360.0, 4 * 86_400, WINDOW_SERVER)];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    assert_eq!(busy.len(), 1);
    let action = busy[0].action.as_ref().unwrap();
    assert!(
        action
            .text
            .contains("stayed over 50% of a core in every interval of the last 10m (at least 85%)"),
        "{action:?}"
    );
    assert!(
        action
            .command
            .as_deref()
            .unwrap()
            .contains("sudo sample 617 5 -file"),
        "{action:?}"
    );
}

#[test]
fn a_daemon_busy_in_one_interval_only_is_not_flagged() {
    // 85% since the last reading, but 5% in an earlier interval of the span.
    let mut daemon = row(617, 1, 0, 85.0, 21_360.0, 4 * 86_400, WINDOW_SERVER);
    daemon.sustained = Some(Sustained {
        percent: 5.0,
        seconds: 600,
    });
    assert_eq!(
        judge("local", &[daemon], &HashMap::new(), &sampling(), None),
        vec![]
    );
}

#[test]
fn a_daemon_with_too_few_readings_for_a_span_is_not_flagged() {
    let mut daemon = row(617, 1, 0, 95.0, 21_360.0, 4 * 86_400, WINDOW_SERVER);
    daemon.sustained = None;
    assert_eq!(
        judge("local", &[daemon], &HashMap::new(), &sampling(), None),
        vec![]
    );
}

#[test]
fn thresholds_pick_busy_processes() {
    let rows = vec![
        // 60% and 11 min of CPU: busy.
        row(1, 0, 501, 60.0, 660.0, 5_000, "/usr/bin/a"),
        // 60% but 5 min of CPU: not busy.
        row(2, 0, 501, 60.0, 300.0, 5_000, "/usr/bin/b"),
        // 95% for 31 min with 20 min of CPU: busy.
        row(3, 0, 501, 95.0, 1_200.0, 1_860, "/usr/bin/c"),
        // 95% for 10 min: not busy.
        row(4, 0, 501, 95.0, 570.0, 600, "/usr/bin/d"),
        // 40% for hours: not busy.
        row(5, 0, 501, 40.0, 9_000.0, 20_000, "/usr/bin/e"),
    ];
    let pids: Vec<i32> = judge("local", &rows, &HashMap::new(), &sampling(), None)
        .iter()
        .map(|b| b.pid)
        .collect();
    assert_eq!(pids.len(), 2);
    assert!(pids.contains(&1) && pids.contains(&3));
}

#[test]
fn a_build_past_its_step_limit_is_flagged_with_its_checkout() {
    let rows = vec![
        row(
            100,
            1,
            501,
            0.0,
            5.0,
            60 * 60,
            "builds app-build bae stuck-cpu macos-tests",
        ),
        row(101, 100, 501, 0.0, 5.0, 60 * 60, "xcodebuild -scheme bae"),
        row(
            102,
            101,
            501,
            120.0,
            4_000.0,
            59 * 60,
            "/usr/bin/swift-frontend -c x",
        ),
    ];
    let cwds = HashMap::from([(
        100,
        "/Users/x/dev/bae-worktrees/side-1/bae-macos".to_string(),
    )]);
    let busy = judge("m1", &rows, &cwds, &sampling(), None);
    assert_eq!(busy.len(), 1);
    assert_eq!(
        busy[0].owner,
        Owner::Builds {
            pid: 100,
            command: "app-build bae stuck-cpu macos-tests".into(),
            checkout: Some("bae/side-1".into()),
            minutes: 60,
        }
    );
    let action = busy[0].action.as_ref().unwrap();
    assert!(
        action.text.contains("past the 45m step limit") && !action.text.contains('`'),
        "{action:?}"
    );
    assert_eq!(action.command.as_deref(), Some("kill 102"));
    assert!(
        action.line().ends_with("; stop it with `kill 102`"),
        "{}",
        action.line()
    );
}

#[test]
fn a_build_inside_its_step_limit_is_listed_not_flagged() {
    let rows = vec![
        row(
            100,
            1,
            501,
            0.0,
            5.0,
            20 * 60,
            "/Users/x/.local/bin/builds app-build bae b macos",
        ),
        row(
            102,
            100,
            501,
            300.0,
            4_000.0,
            19 * 60,
            "rustc --crate-name x",
        ),
    ];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].action, None);
}

#[test]
fn an_agent_process_names_the_shells_worktree() {
    let rows = vec![
        row(50, 1, 501, 0.0, 1.0, 7_000, AGENT),
        row(
            51,
            50,
            501,
            99.0,
            6_000.0,
            6_900,
            "/usr/bin/python3 spin.py",
        ),
    ];
    let cwds = HashMap::from([(50, "/Users/x/dev/bae-worktrees/side-7".to_string())]);
    let busy = judge("local", &rows, &cwds, &sampling(), None);
    assert_eq!(
        busy[0].owner,
        Owner::Agent {
            shell: 50,
            checkout: Some("bae/side-7".into())
        }
    );
    assert!(busy[0].action.is_some());
}

#[test]
fn a_test_binary_is_flagged_after_the_time_a_test_run_takes() {
    let binary =
        "/Volumes/Builds/bae/side-2/target/debug/deps/bae_app-0123456789abcdef --test-threads 4";
    let rows = vec![
        row(70, 1, 501, 100.0, 3_000.0, 40 * 60, binary),
        row(71, 1, 501, 100.0, 700.0, 12 * 60, binary),
    ];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    let long = busy.iter().find(|b| b.pid == 70).unwrap();
    let short = busy.iter().find(|b| b.pid == 71).unwrap();
    assert_eq!(
        long.owner,
        Owner::Test {
            checkout: Some("bae/side-2".into())
        }
    );
    assert!(long.action.as_ref().unwrap().text.contains("40m"));
    assert_eq!(short.action, None);
}

#[test]
fn places_work_by_its_folder() {
    assert_eq!(
        checkout_label("/Users/x/dev/bae-worktrees/side-3/a/b").as_deref(),
        Some("bae/side-3")
    );
    assert_eq!(
        checkout_label("/Volumes/Builds/coven/side-1/target").as_deref(),
        Some("coven/side-1")
    );
    assert_eq!(
        checkout_label("/Users/x/dev/bae/bae-app").as_deref(),
        Some("bae/main")
    );
    assert_eq!(checkout_label("/tmp/x"), None);
}

#[test]
fn reads_lsof_working_folders() {
    let found = parse_lsof("p12\nfcwd\nn/a/b\np13\nfcwd\nn/c\n");
    assert_eq!(found.get(&12).map(String::as_str), Some("/a/b"));
    assert_eq!(found.get(&13).map(String::as_str), Some("/c"));
}

#[test]
fn only_busy_chains_are_placed() {
    let rows = vec![
        row(1, 0, 0, 0.0, 1.0, 100, "/sbin/launchd"),
        row(2, 1, 501, 99.0, 900.0, 2_000, "x"),
        row(3, 1, 501, 0.0, 1.0, 100, "y"),
    ];
    assert_eq!(pids_to_place(&rows), vec![2, 1]);
}

#[test]
fn findings_come_only_from_hosts_that_were_read() {
    let hosts = vec![
        HostBusy {
            host: "m1".into(),
            read: Err("ssh failed".into()),
        },
        HostBusy {
            host: "local".into(),
            read: Ok(HostRead {
                busy: judge(
                    "local",
                    &[row(
                        17,
                        1,
                        0,
                        140.0,
                        200_000.0,
                        500_000,
                        "/usr/libexec/fseventsd",
                    )],
                    &HashMap::new(),
                    &sampling(),
                    None,
                ),
                wedged: None,
                hung: Vec::new(),
                zombie_parents: Vec::new(),
            }),
        },
    ];
    assert_eq!(findings(&hosts).len(), 1);
    let printed = lines(&hosts).join("\n");
    assert!(printed.contains("m1: not read: ssh failed"), "{printed}");
}

#[test]
fn a_busy_fseventsd_is_flagged_at_once_with_the_sample_command() {
    // 98% with 28m of CPU: far under the 2h a daemon needs for the old rule.
    let rows = vec![row(17, 1, 0, 98.0, 28.0 * 60.0, 3 * 3600, FSEVENTSD_PATH)];
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    let sample = "mkdir -p \"/Users/x/Library/Application Support/builds/samples\" && sudo sample 17 5 -file \"/Users/x/Library/Application Support/builds/samples/local-17-1763100000.txt\"";
    let action = busy[0].action.as_ref().unwrap();
    assert_eq!(action.command.as_deref(), Some(sample));
    assert!(
        action
            .text
            .contains("read a sample of it before anyone kills fseventsd")
            && !action.text.contains('`'),
        "{action:?}"
    );
    let finding = busy[0].finding().unwrap();
    assert!(
        finding.text.starts_with("busy: local pid 17 (fseventsd)"),
        "{finding:?}"
    );
    assert_eq!(finding.command.as_deref(), Some(sample));
}

#[test]
fn a_busy_fseventsd_on_another_host_is_sampled_over_ssh_there() {
    let host = Host {
        band: None,
        name: "m1".into(),
        target: Some("me@m1.local".into()),
        cores: 8,
        speed: Some(1.0),
    };
    let sampling = Sampling::new(&host, 1_763_100_000);
    let rows = vec![row(40, 1, 0, 95.0, 1_000.0, 4_000, FSEVENTSD_PATH)];
    let busy = judge("m1", &rows, &HashMap::new(), &sampling, None);
    let action = busy[0].action.as_ref().unwrap();
    assert_eq!(
        action.command.as_deref(),
        Some(
            "ssh -t me@m1.local 'mkdir -p \"$HOME/Library/Application Support/builds/samples\" && sudo sample 40 5 -file \"$HOME/Library/Application Support/builds/samples/m1-40-1763100000.txt\"'"
        ),
    );
}

#[test]
fn an_idle_fseventsd_is_not_flagged() {
    let rows = vec![row(17, 1, 0, 2.0, 5_000.0, 90_000, FSEVENTSD_PATH)];
    assert!(judge("local", &rows, &HashMap::new(), &sampling(), None).is_empty());
    assert_eq!(wedged("local", &rows, 123_456, &sampling()), None);
}

#[test]
fn an_fseventsd_that_answers_with_zero_is_wedged_and_needs_kill_9() {
    let rows = vec![
        row(1, 0, 0, 0.0, 1.0, 9_000_000, "/sbin/launchd"),
        row(17, 1, 0, 100.0, 40.0 * 60.0, 3 * 3600 + 300, FSEVENTSD_PATH),
    ];
    let found = wedged("local", &rows, 0, &sampling()).unwrap();
    assert_eq!(found.pid, 17);
    let text = &found.finding.text;
    assert!(
        text.starts_with("fseventsd does not answer: local pid 17 has run 3h05m"),
        "{text}"
    );
    assert!(text.contains("sudo kill -9 17"), "{text}");
    assert!(
        text.contains("A plain kill does not end an fseventsd that does not answer"),
        "{text}"
    );
    let sample = found.finding.command.as_deref().unwrap();
    assert!(
        sample.contains("sudo sample 17 5 -file") && !text.contains(sample),
        "{sample}"
    );
    // Not running at all: nothing to flag.
    assert_eq!(wedged("local", &rows[..1], 0, &sampling()), None);
}

#[test]
fn a_wedged_fseventsd_replaces_its_busy_finding() {
    let rows = vec![row(17, 1, 0, 100.0, 40.0 * 60.0, 3 * 3600, FSEVENTSD_PATH)];
    let hosts = vec![HostBusy {
        host: "local".into(),
        read: Ok(HostRead {
            busy: judge("local", &rows, &HashMap::new(), &sampling(), None),
            wedged: wedged("local", &rows, 0, &sampling()),
            hung: Vec::new(),
            zombie_parents: Vec::new(),
        }),
    }];
    let found = findings(&hosts);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.starts_with("fseventsd does not answer"),
        "{found:?}"
    );
}

/// A test binary that `builds cargo` runs on a host, and that has used next
/// to no CPU for 20 minutes or more, is hung. Its finding names the worktree,
/// the command and the idle time, with the ssh command that stops it. A test
/// that uses CPU is not hung, nor is a test younger than 20 minutes, nor a
/// test that no `builds cargo` runs.
#[test]
fn an_idle_test_that_builds_cargo_runs_on_a_host_is_hung() {
    const TEST: &str = "/Volumes/Builds/bae/remote-1/target/debug/deps/bae_store-0123456789abcdef --test-threads 4";
    let rows = vec![
        row(1, 0, 0, 0.0, 10.0, 9_000_000, "/sbin/launchd"),
        row(
            10,
            1,
            501,
            0.0,
            0.1,
            2_300,
            "sh -c builds cargo test -p bae-store",
        ),
        row(
            11,
            10,
            501,
            0.0,
            0.5,
            2_300,
            "/Users/d/.local/bin/builds cargo test -p bae-store",
        ),
        row(12, 11, 501, 0.0, 30.0, 2_290, "cargo test -p bae-store"),
        row(13, 12, 501, 0.0, 2.0, 37 * 60, TEST),
        // This test uses CPU, so it still works.
        row(14, 12, 501, 40.0, 900.0, 37 * 60, TEST),
        // This test has run for less than 20 minutes, so the check cannot
        // call it hung.
        row(15, 12, 501, 0.0, 1.0, 10 * 60, TEST),
        // This test runs under an agent's own cargo, so the agent shell's
        // rules cover it and this check skips it.
        row(20, 1, 501, 0.0, 1.0, 2_300, "cargo test"),
        row(21, 20, 501, 0.0, 1.0, 37 * 60, TEST),
    ];
    assert!(pids_to_place(&rows).contains(&13));
    let cwds = HashMap::from([(13, "/Users/d/dev/bae-worktrees/remote-1".to_string())]);
    let on_m1 = Sampling {
        home: "$HOME".into(),
        target: Some("d@m1".into()),
        at: 1_763_100_000,
    };
    let hung = hung_tests("m1", &rows, &cwds, &on_m1);
    assert_eq!(hung.len(), 1, "{hung:?}");
    let finding = hung[0].finding();
    assert_eq!(finding.command.as_deref(), Some("ssh d@m1 'kill 13'"));
    assert!(
        finding
            .text
            .starts_with("hung test: m1 pid 13 in bae/remote-1 has sat idle for 37m")
            && finding
                .text
                .contains("bae_store-0123456789abcdef --test-threads 4"),
        "{}",
        finding.text
    );
    let host = HostBusy {
        host: "m1".into(),
        read: Ok(HostRead {
            busy: Vec::new(),
            wedged: None,
            hung,
            zombie_parents: Vec::new(),
        }),
    };
    assert_eq!(findings(&[host]).len(), 1);
}

/// builds never flags its own dashboard as busy, even after hours of high
/// CPU. Any other builds process with the same CPU use gets a finding.
#[test]
fn the_dashboard_is_never_a_busy_build() {
    let rows = vec![
        row(1, 0, 0, 0.0, 10.0, 9_000_000, "/sbin/launchd"),
        row(
            70525,
            1,
            501,
            80.0,
            3_000.0,
            102 * 60,
            "builds dashboard --listen 192.168.1.166",
        ),
    ];
    assert!(judge("local", &rows, &HashMap::new(), &sampling(), Some(70525)).is_empty());
    let busy = judge("local", &rows, &HashMap::new(), &sampling(), None);
    assert!(busy[0].finding().is_some(), "{busy:?}");
}
