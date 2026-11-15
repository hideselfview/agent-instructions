use super::*;

fn row(pid: i32, ppid: i32, uid: u32, pcpu: f64, cpu: f64, age: i64, command: &str) -> Row {
    Row {
        pid,
        ppid,
        uid,
        cpu_percent: pcpu,
        cpu_seconds: cpu,
        age_seconds: age,
        command: command.to_string(),
    }
}

const AGENT: &str = "/bin/zsh -c source /Users/x/.claude/shell-snapshots/snapshot-zsh-1.sh && eval 'cargo test' < /dev/null && pwd -P >| /tmp/c";

#[test]
fn parses_ps_lines() {
    let text = "  555     1   202   0.0 823:18.22 24-07:11:48 /usr/bin/a b  c\n\
                  17  1   0  101.5 2586:37.12 06-02:03:04 /usr/libexec/fseventsd\n\
                  18  1   501 3.0 1:02:03.50 05:09 sleep 1\n";
    let rows = parse_ps(text).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].command, "/usr/bin/a b  c");
    assert_eq!(rows[0].age_seconds, 24 * 86_400 + 7 * 3600 + 11 * 60 + 48);
    assert_eq!(rows[1].cpu_seconds, 2586.0 * 60.0 + 37.12);
    assert_eq!(rows[1].uid, 0);
    assert_eq!(rows[2].cpu_seconds, 3600.0 + 123.5);
    assert_eq!(rows[2].age_seconds, 5 * 60 + 9);
    assert!(parse_ps("1 2 3\n").is_err());
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
    let busy = judge("local", &rows, &HashMap::new());
    assert_eq!(busy.len(), 1);
    assert_eq!(
        busy[0].owner,
        Owner::System {
            name: "fseventsd".into()
        }
    );
    let action = busy[0].action.as_ref().unwrap();
    assert!(action.contains("sudo killall fseventsd"), "{action}");
    assert!(
        busy[0].line().contains("local pid 17: 140% now, 1d19h CPU"),
        "{}",
        busy[0].line()
    );
}

#[test]
fn a_busy_system_daemon_with_little_cpu_time_is_listed_not_flagged() {
    let rows = vec![row(9, 1, 0, 95.0, 700.0, 2_000, "/usr/libexec/mds")];
    let busy = judge("local", &rows, &HashMap::new());
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].action, None);
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
    let pids: Vec<i32> = judge("local", &rows, &HashMap::new())
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
    let busy = judge("m1", &rows, &cwds);
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
        action.contains("past the 45m step limit") && action.contains("kill 102"),
        "{action}"
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
    let busy = judge("local", &rows, &HashMap::new());
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
    let busy = judge("local", &rows, &cwds);
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
    let busy = judge("local", &rows, &HashMap::new());
    let long = busy.iter().find(|b| b.pid == 70).unwrap();
    let short = busy.iter().find(|b| b.pid == 71).unwrap();
    assert_eq!(
        long.owner,
        Owner::Test {
            checkout: Some("bae/side-2".into())
        }
    );
    assert!(long.action.as_ref().unwrap().contains("40m"));
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
            busy: Err("ssh failed".into()),
        },
        HostBusy {
            host: "local".into(),
            busy: Ok(judge(
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
            )),
        },
    ];
    assert_eq!(findings(&hosts).len(), 1);
    let printed = lines(&hosts).join("\n");
    assert!(printed.contains("m1: not read: ssh failed"), "{printed}");
}
