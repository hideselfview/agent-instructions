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
            name: "fseventsd".into(),
            span: Sustained {
                percent: 140.0,
                seconds: 600
            }
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
fn a_busy_build_names_its_checkout() {
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
    assert_eq!(busy[0].action, None);
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
    assert_eq!(busy[0].action, None);
}

#[test]
fn a_busy_test_names_its_checkout() {
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
    assert_eq!(long.action, None);
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
    assert_eq!(pids_to_place(&rows, 501), vec![2]);
}

#[test]
fn processes_of_other_users_are_not_placed() {
    let rows = vec![
        row(1, 0, 0, 0.0, 1.0, 100, "/sbin/launchd"),
        row(411, 1, 0, 0.0, 1.0, 100, "/usr/libexec/logd"),
        row(2, 411, 501, 99.0, 900.0, 2_000, "x"),
        row(3, 1, 0, 99.0, 900.0, 2_000, "/usr/sbin/busyd"),
    ];
    assert_eq!(pids_to_place(&rows, 501), vec![2]);
}

#[test]
fn a_host_that_was_not_read_gives_the_reason_and_no_process_findings() {
    let hosts = vec![
        HostBusy {
            host: "m1".into(),
            read: HostRead::ScanFailed("ssh failed".into()),
        },
        HostBusy {
            host: "local".into(),
            read: HostRead::Activity(ProcessActivity {
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
                placement_diagnostic: None,
                wedged: None,
            }),
        },
    ];
    let found = findings(&hosts);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].text, "busy processes on m1: not read: ssh failed");
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
        read: HostRead::Activity(ProcessActivity {
            busy: judge("local", &rows, &HashMap::new(), &sampling(), None),
            placement_diagnostic: None,
            wedged: wedged("local", &rows, 0, &sampling()),
        }),
    }];
    let found = findings(&hosts);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.starts_with("fseventsd does not answer"),
        "{found:?}"
    );
}

/// Builds leaves its dashboard out of the list of busy processes.
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
    assert_eq!(busy.len(), 1, "{busy:?}");
    assert!(busy[0].finding().is_none(), "{busy:?}");
}

#[test]
fn remote_busy_processes_survive_a_cwd_lookup_without_matches() {
    let scratch = crate::testing::Scratch::new("busy-cwd-exit");
    let ssh = crate::testing::ssh_at(&scratch.script(
        "ssh",
        r#"case "${9}" in
  m1) ;;
  denied) echo "lsof: Permission denied" >&2; exit 1 ;;
  invalid) echo "not a folder record"; echo "lsof: failed" >&2; exit 1 ;;
  complete)
    case "${10}" in
      *"lsof -a -d cwd -Fpn -p 42,43,44,45'")
        for pid in 42 43 44 45; do
          printf '%s\n' "p$pid" fcwd n/Users/x/dev/bae-worktrees/side-7
        done
        echo "lsof: Permission denied" >&2
        exit 0 ;;
      *) exit 2 ;;
    esac ;;
  partial|partial-ps-denied|partial-success)
    case "${10}" in
      *"lsof "*) printf '%s\n' p43 fcwd n/Users/x/dev/bae-worktrees/side-7
        echo "lsof: Permission denied" >&2
        if [ "${9}" = partial-success ]; then exit 0; fi
        exit 1 ;;
      *"ps -axww"*) if [ "${9}" = partial-ps-denied ]; then
        echo "ps: Permission denied" >&2; exit 1
      fi ;;
    esac ;;
  ps-denied)
    case "${10}" in
      *"ps -axww"*) echo "ps: Permission denied" >&2; exit 1 ;;
    esac ;;
  unreachable) echo "ssh: Connection refused" >&2; exit 255 ;;
esac
case "${10}" in
  *"lsof -a -d cwd -Fpn -p 42,43,44,45'") exit 1 ;;
  *"ps -axww -o "*) printf '%s\n' now=5003 ps_follows=1 \
    '43 0 0 0.0 16:40.00 40:03 R /usr/bin/live' \
    '44 0 501 0.0 16:40.00 40:03 Z (zombie)' \
    '45 0 501 0.0 00:01.00 00:03 R /usr/bin/new' ;;
  *) exit 2 ;;
esac"#,
    ));
    let mut host = Host {
        name: "m1".into(),
        target: Some("m1".into()),
        cores: 8,
        speed: None,
        band: None,
    };
    let mut zombie = row(46, 43, 501, 90.0, 1_000.0, 2_400, "(already exited)");
    zombie.zombie = true;
    let scan = Ok(crate::host_scan::Scan {
        uid: 501,
        active: Default::default(),
        at: 5_000,
        rows: vec![
            row(42, 0, 501, 90.0, 1_000.0, 2_400, "/usr/bin/exited"),
            row(43, 0, 501, 90.0, 1_000.0, 2_400, "/usr/bin/live"),
            row(44, 0, 501, 90.0, 1_000.0, 2_400, "/usr/bin/zombie"),
            row(45, 0, 501, 90.0, 1_000.0, 2_400, "/usr/bin/old"),
            zombie,
        ],
        detached: Default::default(),
        cpu_idle: Ok(10.0),
        jobs: vec![],
        sccache: Ok(None),
        display: Ok(None),
    });

    let answer = read(&ssh, &host, &scan, None);
    assert!(answer.report().1.unwrap().contains("living PIDs: 43"));
    let answer = answer.activity().unwrap();
    assert_eq!(
        answer.busy.iter().map(|busy| busy.pid).collect::<Vec<_>>(),
        [43]
    );

    host.target = Some("denied".into());
    let denied = read(&ssh, &host, &scan, None)
        .report()
        .1
        .unwrap()
        .to_string();
    assert!(denied.contains("lsof: Permission denied"), "{denied}");

    host.target = Some("invalid".into());
    let invalid = read(&ssh, &host, &scan, None);
    assert!(invalid.activity().is_err());
    assert!(invalid.report().1.unwrap().contains("lsof: failed"));

    host.target = Some("partial".into());
    let partial = read(&ssh, &host, &scan, None);
    assert!(
        partial
            .report()
            .1
            .unwrap()
            .contains("lsof: Permission denied")
    );
    assert_eq!(partial.activity().unwrap().busy[0].pid, 43);

    host.target = Some("partial-success".into());
    let partial = read(&ssh, &host, &scan, None);
    assert!(
        partial
            .report()
            .1
            .unwrap()
            .contains("lsof: Permission denied")
    );
    assert_eq!(partial.activity().unwrap().busy[0].pid, 43);

    host.target = Some("complete".into());
    let complete = read(&ssh, &host, &scan, None);
    let (busy, diagnostic) = complete.report();
    assert_eq!(busy.unwrap().len(), 4);
    assert!(diagnostic.unwrap().contains("lsof: Permission denied"));
    assert!(
        findings(&[complete])
            .iter()
            .any(|finding| { finding.text.contains("lsof: Permission denied") })
    );

    host.target = Some("partial-ps-denied".into());
    let partial = read(&ssh, &host, &scan, None);
    let error = partial.report().1.unwrap();
    assert!(error.contains("lsof: Permission denied") && error.contains("ps: Permission denied"));
    assert_eq!(partial.activity().unwrap().busy[0].pid, 43);

    host.target = Some("ps-denied".into());
    let refused = read(&ssh, &host, &scan, None);
    assert!(
        refused
            .report()
            .1
            .unwrap()
            .contains("ps: Permission denied")
    );

    host.target = Some("unreachable".into());
    let unreachable = read(&ssh, &host, &scan, None)
        .report()
        .1
        .unwrap()
        .to_string();
    assert!(
        unreachable.contains("255") && unreachable.contains("Connection refused"),
        "{unreachable}"
    );
}
