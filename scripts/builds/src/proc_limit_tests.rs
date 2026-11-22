use super::*;

fn row(pid: i32, ppid: i32, zombie: bool, age: i64, command: &str) -> Row {
    Row {
        pid,
        ppid,
        uid: 501,
        cpu_percent: Some(0.0),
        sustained: None,
        cpu_seconds: 0.0,
        age_seconds: age,
        zombie,
        command: command.to_string(),
    }
}

const TEST: &str = "/Volumes/Builds/x/target/debug/deps/leak-0123456789abcdef";

fn host(name: &str) -> Host {
    Host {
        name: name.to_string(),
        target: (name != "local").then(|| format!("me@{name}")),
        cores: 8,
        speed: Some(1.0),
        band: None,
    }
}

fn zombie_rows() -> Vec<Row> {
    let mut rows = vec![
        row(1, 0, false, 9_000, "/sbin/launchd"),
        row(100, 1, false, 600, TEST),
        row(200, 1, false, 90, "/bin/zsh -l"),
    ];
    rows.extend((1000..1005).map(|pid| row(pid, 100, true, 500, "(sh)")));
    rows.extend((2000..2002).map(|pid| row(pid, 200, true, 80, "(sh)")));
    rows.extend((3000..3003).map(|pid| row(pid, 1, true, 80, "(sh)")));
    rows
}

#[test]
fn reads_the_count_and_the_limit_from_the_probe_lines() {
    let used = parse("cores=8\nprocs=7300\nmax_procs=8000\n").unwrap();
    assert_eq!(
        used,
        ProcessUse {
            count: 7300,
            limit: 8000
        }
    );
    assert!(used.near_limit());
    assert!(parse("procs=5\n").is_err());
}

#[test]
fn this_mac_counts_its_processes_with_the_same_lines() {
    let used = local().unwrap();
    assert!(used.count > 0 && used.limit > used.count / 2);
}

#[test]
fn the_remote_probe_script_asks_for_the_process_lines() {
    let script = PROBE_LINES;
    assert!(
        script.contains("kern.maxprocperuid")
            && script.contains("/proc/self/limits")
            && script.contains("ps -U")
    );
    let text = "os=Linux\narch=x86_64\ncores=8\nload=1.0 1.0 1.0\nfree_kib=100\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=7300\nmax_procs=8000\n";
    let probe = crate::host::parse_probe(text).unwrap();
    assert_eq!(
        probe.processes,
        ProcessUse {
            count: 7300,
            limit: 8000
        }
    );
}

#[test]
fn names_the_parents_with_the_most_zombies_and_skips_init() {
    let parents = zombie_parents(&zombie_rows(), None);
    assert_eq!(
        parents
            .iter()
            .map(|p| (p.pid, p.zombies))
            .collect::<Vec<_>>(),
        vec![(100, 5), (200, 2)]
    );
    assert!(parents[0].test_binary && !parents[1].test_binary);
    assert_eq!(parents[0].stop, "kill 100");
}

#[test]
fn warns_for_a_host_over_sixty_percent_and_not_under() {
    let host = host("local");
    let found = finding(
        &host,
        ProcessUse {
            count: 7300,
            limit: 8000,
        },
        Ok(&zombie_rows()),
    )
    .unwrap();
    assert!(found.text.contains("this Mac runs 7300 of its 8000"));
    assert!(found.text.contains("pid 100"));
    assert_eq!(found.command.as_deref(), Some("kill 100"));
    assert!(
        finding(
            &host,
            ProcessUse {
                count: 4000,
                limit: 8000
            },
            Ok(&zombie_rows())
        )
        .is_none()
    );
}

#[test]
fn a_remote_host_near_the_limit_gets_an_ssh_stop_command() {
    let found = finding(
        &host("m1"),
        ProcessUse {
            count: 6000,
            limit: 8000,
        },
        Ok(&zombie_rows()),
    )
    .unwrap();
    assert_eq!(found.command.as_deref(), Some("ssh me@m1 'kill 100'"));
    assert!(found.text.starts_with("processes: host m1 runs 6000"));
}

#[test]
fn a_non_test_top_parent_gets_no_stop_command() {
    let rows = vec![
        row(1, 0, false, 9, "/sbin/launchd"),
        row(200, 1, false, 90, "/bin/zsh -l"),
        row(2000, 200, true, 80, "(sh)"),
    ];
    let found = finding(
        &host("local"),
        ProcessUse {
            count: 7000,
            limit: 8000,
        },
        Ok(&rows),
    )
    .unwrap();
    assert_eq!(found.command, None);
}

#[test]
fn a_failed_ps_fails_the_probe_instead_of_counting_zero() {
    let error = parse("procs_fails=fork: Resource temporarily unavailable \nmax_procs=8000\n")
        .unwrap_err()
        .text();
    assert!(
        error.contains("Resource temporarily unavailable"),
        "{error}"
    );
    let script = format!("ps() {{ echo 'ps: fork failed' >&2; return 1; }}\n{PROBE_LINES}");
    let printed = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&printed.stdout);
    assert!(text.contains("procs_fails=ps: fork failed"), "{text}");
    assert!(
        !text.lines().any(|line| line.starts_with("procs=")),
        "{text}"
    );
}

#[test]
fn a_host_whose_processes_could_not_be_read_says_so() {
    let found = finding(
        &host("local"),
        ProcessUse {
            count: 7300,
            limit: 8000,
        },
        Err("ps failed"),
    )
    .unwrap();
    assert!(
        found.text.contains("reading its processes failed") && found.text.contains("ps failed")
    );
}
