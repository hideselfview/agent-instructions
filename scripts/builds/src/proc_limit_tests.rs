use super::*;

fn row(pid: i32, ppid: i32, zombie: bool, age: i64, command: &str) -> Row {
    Row {
        pid,
        ppid,
        uid: 501,
        cpu_percent: 0.0,
        cpu_seconds: 0.0,
        age_seconds: age,
        zombie,
        command: command.to_string(),
    }
}

fn sampling(target: Option<&str>) -> Sampling {
    Sampling::new(
        &Host {
            name: "m1".into(),
            target: target.map(str::to_string),
            cores: 8,
            speed: Some(1.0),
            band: None,
        },
        0,
    )
}

const TEST: &str = "/Volumes/Builds/x/target/debug/deps/leak-0123456789abcdef";

fn probe(count: u64, limit: u64) -> Probe {
    Probe {
        platform: crate::host_platform::Platform::here(),
        cores: 8,
        load: [0.0; 3],
        free_kib: 1,
        memory: crate::host_memory::Memory::at_rest(),
        git_fails: None,
        nextest: crate::nextest::Has::Nothing,
        processes: ProcessUse { count, limit },
    }
}

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
    assert!(script.contains("kern.maxprocperuid") && script.contains("ps -U"));
    let text = "cores=8\nload=1.0 1.0 1.0\nfree_kib=100\nmemory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=1\nprocs=7300\nmax_procs=8000\n";
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
    let parents = zombie_parents(&zombie_rows(), &sampling(None));
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
    let reads = vec![HostBusy {
        host: "local".into(),
        read: Ok(crate::stuck::HostRead {
            busy: Vec::new(),
            wedged: None,
            hung: Vec::new(),
            zombie_parents: zombie_parents(&zombie_rows(), &sampling(None)),
        }),
    }];
    let hosts = vec![
        (host("local"), Ok(probe(7300, 8000))),
        (host("m1"), Ok(probe(4000, 8000))),
        (host("m2"), Err("down".to_string())),
    ];
    let found = findings(&hosts, &reads);
    assert_eq!(found.len(), 1);
    assert!(found[0].text.contains("this Mac runs 7300 of its 8000"));
    assert!(found[0].text.contains("pid 100"));
    assert_eq!(found[0].command.as_deref(), Some("kill 100"));
}

#[test]
fn a_remote_host_near_the_limit_gets_an_ssh_stop_command() {
    let parents = zombie_parents(&zombie_rows(), &sampling(Some("me@m1")));
    let reads = vec![HostBusy {
        host: "m1".into(),
        read: Ok(crate::stuck::HostRead {
            busy: Vec::new(),
            wedged: None,
            hung: Vec::new(),
            zombie_parents: parents,
        }),
    }];
    let found = findings(&[(host("m1"), Ok(probe(6000, 8000)))], &reads);
    assert_eq!(found[0].command.as_deref(), Some("ssh me@m1 'kill 100'"));
    assert!(found[0].text.starts_with("processes: host m1 runs 6000"));
}

#[test]
fn a_non_test_top_parent_gets_no_stop_command() {
    let rows = vec![
        row(1, 0, false, 9, "/sbin/launchd"),
        row(200, 1, false, 90, "/bin/zsh -l"),
        row(2000, 200, true, 80, "(sh)"),
    ];
    let reads = vec![HostBusy {
        host: "local".into(),
        read: Ok(crate::stuck::HostRead {
            busy: Vec::new(),
            wedged: None,
            hung: Vec::new(),
            zombie_parents: zombie_parents(&rows, &sampling(None)),
        }),
    }];
    let found = findings(&[(host("local"), Ok(probe(7000, 8000)))], &reads);
    assert_eq!(found[0].command, None);
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
    let hosts = vec![
        (host("local"), Ok(probe(7300, 8000))),
        (host("m1"), Ok(probe(7300, 8000))),
    ];
    let reads = vec![HostBusy {
        host: "local".into(),
        read: Err("ps failed".to_string()),
    }];
    let found = findings(&hosts, &reads);
    assert_eq!(found.len(), 2);
    assert!(
        found[0].text.contains("reading its processes failed")
            && found[0].text.contains("ps failed")
    );
    assert!(found[1].text.contains("no reading of its processes exists"));
    assert!(
        found
            .iter()
            .all(|f| !f.text.contains("no process there holds"))
    );
}
