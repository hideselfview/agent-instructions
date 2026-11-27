use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;

fn host(name: &str) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

/// What `installed_script` prints for the sccache config `builds install`
/// writes for a cap of <cap_kib>.
fn config_lines(cap_kib: u64) -> String {
    format!(
        "sccache_config:# Written by builds install.\nsccache_config:[cache.disk]\nsccache_config:size = {}\n",
        cap_kib * 1024
    )
}

/// What `installed_script` prints for a Mac with builds' plist, an sccache
/// config with builds' cap, the server <listener> on sccache's port, the
/// settings that UI tests need all on, and launchd's job as `launchctl print`
/// printed <job>, or no job.
fn answer(builds: &str, plist: &str, listener: &str, job: Option<&str>) -> String {
    let job = match job {
        Some(printed) => format!("job=loaded\n{printed}"),
        None => "job=none\n".to_string(),
    };
    let (status, lsof, stats) = if listener.is_empty() {
        (1, "listener=\n".to_string(), String::new())
    } else {
        (
            0,
            format!("listener=p{listener}\nlistener=R1\nlistener=csccache\nlistener=f5\n"),
            stats_line(cap_bytes()),
        )
    };
    format!(
        "os=Darwin\narch=arm64\nbuilds={builds}\njob_file={plist}\n{config}listener_status={status}\n{lsof}listener_said=\n{stats}{UI_TESTS}{job}",
        config = config_lines(crate::install::sccache_cap_kib())
    )
}

/// builds' cap for the sccache cache, in bytes.
fn cap_bytes() -> u64 {
    crate::install::sccache_cap_kib() * 1024
}

fn listening(pid: u32, cap: u64, served: bool) -> ListeningSccache {
    ListeningSccache {
        pid,
        served,
        cap: Ok(cap),
        tmp_issue: None,
    }
}

/// What `installed_script` prints for a listening server whose max cache size
/// is <bytes>, cut to the fields that matter.
fn stats_line(bytes: u64) -> String {
    format!(
        "sccache_stats={{\"stats\":{{\"compile_requests\":5}},\"cache_size\":10,\"max_cache_size\":{bytes},\"version\":\"0.13.0\"}}\n"
    )
}

/// What `ui_test_settings::SCRIPT` prints on a Mac with every setting on.
const UI_TESTS: &str = "ui_user=u
ui_automation=This device DOES NOT REQUIRE user authentication to enable Automation Mode.
ui_developer_mode=Developer mode is currently enabled.
ui_developer_group=yes u is a member of _developer
";

/// The settings that UI tests need, all on, for user u.
fn all_on() -> crate::ui_test_settings::UiTestSettings {
    crate::ui_test_settings::UiTestSettings {
        user: "u".to_string(),
        missing: Vec::new(),
    }
}

#[test]
fn reads_what_a_host_has_installed() {
    use crate::sccache::tests::RUNNING;
    assert_eq!(
        parse_installed(&answer("ab12", "present", "2914", Some(RUNNING))).unwrap(),
        Installed {
            builds: Some("ab12".to_string()),
            mac: Some(crate::host_installed::MacInstalled { ui_tests: all_on() }),
            sccache_cap: true,
            sccache: Some(listening(2914, cap_bytes(), true)),
        }
    );
    assert_eq!(
        parse_installed(&answer("missing", "present", "", None)).unwrap(),
        Installed {
            builds: None,
            mac: Some(crate::host_installed::MacInstalled { ui_tests: all_on() }),
            sccache_cap: true,
            sccache: None,
        }
    );
    assert!(parse_installed(&answer("", "present", "", None)).is_err());
    assert!(parse_installed("builds=ab12\n").is_err());
    assert!(parse_installed("builds=ab12\nplist=present\nlistener=\n").is_err());
    let lsof_fails = answer("ab12", "present", "", None)
        .replace("listener_said=", "listener_said=lsof: cannot run");
    assert!(
        parse_installed(&lsof_fails).is_err(),
        "a failing lsof is an error, not a free port"
    );
    assert!(parse_installed("").is_err());
}

/// launchd runs a host's sccache server only when launchd's running job is
/// the process listening on sccache's port, the same check that setup uses.
/// The test checks the cases where launchd does not run the server: a
/// client's server holds the port while the job runs, nothing listens,
/// launchd keeps starting the job again, launchd has no job, or the host
/// has no plist from builds.
#[test]
fn launchd_runs_a_hosts_sccache_only_when_its_job_listens() {
    use crate::sccache::tests::{RESPAWNING, RUNNING};
    let under = |plist, listener, job| {
        parse_installed(&answer("ab12", plist, listener, job))
            .unwrap()
            .sccache_served()
    };
    assert!(under("present", "2914", Some(RUNNING)));
    assert!(!under("present", "77", Some(RUNNING)), "a client's server");
    assert!(!under("present", "", Some(RUNNING)), "nothing listens");
    assert!(!under("present", "77", Some(RESPAWNING)));
    assert!(!under("present", "77", None), "launchd has no job");
    assert!(
        !under("missing", "2914", Some(RUNNING)),
        "no plist from builds"
    );
}

/// A host with no builds program, or a different version than this Mac's,
/// gets a finding that names `builds host setup`; a host with this Mac's
/// copy gets no finding.
#[test]
fn a_host_whose_builds_is_not_this_macs_is_a_finding() {
    let scratch = Scratch::new("host-installed");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch, &home);
    let mine = places.home.join(".local/bin/builds");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::copy(scratch.builds_program(), &mine).unwrap();
    let m1 = host("m1");
    let gone = Host {
        band: None,
        name: "gone".to_string(),
        target: Some("unreachable".to_string()),
        cores: 1,
        speed: Some(1.0),
    };
    let hosts = [Host::local(), m1.clone(), gone];
    // A host that does not answer gets a finding that says why.
    let all = installed_findings(&read_all_installed(&places, &ssh, &hosts));
    assert!(
        all.iter().any(|advice| advice
            .text
            .starts_with("builds could not read what host gone has installed")
            && advice.text.contains("Connection refused")),
        "{all:?}"
    );
    // Neither home has builds' sccache plist, so each also has its sccache
    // finding; this test reads m1's builds ones.
    let findings = || -> Vec<crate::health::Advice> {
        installed_findings(&read_all_installed(&places, &ssh, &hosts))
            .into_iter()
            .filter(|advice| {
                advice.text.starts_with("host m1 has no builds")
                    || advice.text.starts_with("host m1 runs a different version")
            })
            .collect()
    };

    let found = findings();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].text.starts_with("host m1 has no builds at"),
        "{found:?}"
    );
    assert_eq!(found[0].command.as_deref(), Some("builds host setup m1"));

    crate::host_setup::install_program(&ssh, &m1, &scratch.builds_program()).unwrap();
    assert_eq!(findings(), [], "m1 has this Mac's builds");

    std::fs::write(home.join(".local/bin/builds"), "older").unwrap();
    let found = findings();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .text
            .starts_with("host m1 runs a different version of builds"),
        "{found:?}"
    );
}

/// When launchd does not run a Mac's sccache server, that Mac gets a
/// finding. A host's finding names `builds host setup`, and this Mac's
/// finding names `builds install`. read_all_installed reports that launchd
/// does not run sccache when the home has no plist from builds.
#[test]
fn a_host_whose_sccache_runs_outside_launchd_is_a_finding() {
    let installed = |sccache_under_launchd| Installed {
        builds: Some("ab12".to_string()),
        mac: Some(crate::host_installed::MacInstalled { ui_tests: all_on() }),
        sccache_cap: true,
        sccache: Some(listening(2914, cap_bytes(), sccache_under_launchd)),
    };
    let read = |host: &str, under: bool| HostInstalled {
        host: host.to_string(),
        read: Ok(installed(under)),
    };
    assert_eq!(
        installed_findings(&[read(LOCAL, true), read("m1", true)]),
        []
    );
    let found = installed_findings(&[read(LOCAL, false), read("m1", false)]);
    let shown: Vec<(bool, Option<&str>)> = found
        .iter()
        .map(|advice| (advice.text.contains("sccache"), advice.command.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            (true, Some("builds install")),
            (true, Some("builds host setup m1"))
        ]
    );

    let scratch = Scratch::new("host-sccache");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));
    let reads = read_all_installed(&places, &ssh, &[Host::local(), host("m1")]);
    let under: Vec<bool> = reads
        .iter()
        .map(|read| read.read.as_ref().unwrap().sccache_served())
        .collect();
    assert_eq!(under, [false, false]);
}

#[test]
fn health_names_the_server_and_its_bad_temp_folder() {
    use crate::sccache::tests::RUNNING;
    let scratch = Scratch::new("host-sccache-tmpdir");
    let places = scratch.places();
    let answer = answer("ab12", "present", "2914", Some(RUNNING));
    let missing_builds = answer.replace("builds=ab12", "builds=missing");
    let run_tmp = places.root.join(".tmp/1-2");
    let missing = places.root.join("gone folder");
    let run_issue = serde_json::json!(format!("uses a run folder ({run_tmp:?})"));
    let missing_issue = serde_json::json!(format!("uses a missing folder ({missing:?})"));
    let fake_ssh = scratch.script(
        "ssh",
        &format!(
            r#"case "$*" in
*' sccache-tmpdir 2914'*)
  case "$*" in
    *u@m1*) printf '%s\n' {run_issue} ;;
    *u@m2*) printf '%s\n' {missing_issue} ;;
    *u@m3*) echo null ;;
    *) exit 2 ;;
  esac
  ;;
*u@m4*) cat <<'EOF'
{missing_builds}EOF
  ;;
*) cat <<'EOF'
{answer}EOF
  ;;
esac"#,
            run_issue = crate::host::sh_quote(&run_issue.to_string()),
            missing_issue = crate::host::sh_quote(&missing_issue.to_string()),
        ),
    );
    let ssh = crate::testing::ssh_at(&fake_ssh);
    let mut installed = vec![HostInstalled {
        host: LOCAL.to_string(),
        read: Ok(parse_installed(&answer).unwrap()),
    }];
    installed.extend(read_all_installed(
        &places,
        &ssh,
        &[host("m1"), host("m2"), host("m3"), host("m4")],
    ));
    let findings = installed_findings(&installed);
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(
        findings[0].text.contains("host m1 (pid 2914)")
            && findings[0].text.contains(&*run_tmp.to_string_lossy()),
        "{findings:?}"
    );
    assert!(
        findings[1].text.contains("host m2 (pid 2914)")
            && findings[1].text.contains(&*missing.to_string_lossy()),
        "{findings:?}"
    );
    assert!(findings[2].text.starts_with("host m4 has no builds at"));
    installed[0].read = Err("the local probe failed".into());
    let without_local = installed_findings(&installed);
    assert_eq!(&without_local[1..], &findings);
}

/// A Mac whose sccache config does not hold builds' cap, such as a host that
/// kept the old cap when builds raised it, gets a finding: a host's finding
/// names `builds host setup`, and this Mac's finding names `builds install`.
/// read_all_installed reads each home's config.
#[test]
fn a_host_whose_sccache_config_does_not_hold_builds_cap_is_a_finding() {
    use crate::sccache::tests::RUNNING;
    let cap = crate::install::sccache_cap_kib();
    let holds = |config: &str| {
        let text =
            answer("ab12", "present", "2914", Some(RUNNING)).replace(&config_lines(cap), config);
        parse_installed(&text).unwrap().sccache_cap
    };
    assert!(holds(&config_lines(cap)));
    assert!(!holds(&config_lines(30 * 1024 * 1024)), "the old cap");
    assert!(!holds(""), "no config");
    assert!(
        holds(&config_lines(cap).replace("Written by builds install.", "an older comment")),
        "a comment is no setting"
    );

    let read = |host: &str, sccache_cap: bool| HostInstalled {
        host: host.to_string(),
        read: Ok(Installed {
            builds: Some("ab12".to_string()),
            mac: Some(crate::host_installed::MacInstalled { ui_tests: all_on() }),
            sccache_cap,
            sccache: Some(listening(2914, cap_bytes(), true)),
        }),
    };
    assert_eq!(
        installed_findings(&[read(LOCAL, true), read("m1", true)]),
        []
    );
    let found = installed_findings(&[read(LOCAL, false), read("m1", false)]);
    let shown: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|advice| (advice.text.as_str(), advice.command.as_deref()))
        .collect();
    let says = format!(
        "sccache config does not hold builds' cap of {}",
        crate::text::human(cap)
    );
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(
        shown[0].0.starts_with(&format!("this Mac's {says}")),
        "{shown:?}"
    );
    assert_eq!(shown[0].1, Some("builds install"));
    assert!(
        shown[1].0.starts_with(&format!("host m1's {says}")),
        "{shown:?}"
    );
    assert_eq!(shown[1].1, Some("builds host setup m1"));

    let scratch = Scratch::new("host-sccache-cap");
    let places = scratch.places();
    let home = scratch.dir.join("remote-home");
    let ssh = fake_ssh(&scratch, &home);
    let config = crate::install::sccache_config_path(&home);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, format!("[cache.disk]\nsize = {}\n", cap * 1024)).unwrap();
    let reads = read_all_installed(&places, &ssh, &[Host::local(), host("m1")]);
    let caps: Vec<bool> = reads
        .iter()
        .map(|read| read.read.as_ref().unwrap().sccache_cap)
        .collect();
    assert_eq!(caps, [false, true], "only m1's home has a config");
}

/// A server that runs with another cap than the config's, as one that read
/// its config before builds wrote the new cap, is a finding: a host's names
/// `builds host setup`, and this Mac's names `builds install`. A server that
/// runs with builds' cap, or no listening server, is none.
#[test]
fn a_server_whose_running_cap_differs_from_its_config_is_a_finding() {
    use crate::sccache::tests::RUNNING;
    let old = 30 * 1024 * 1024 * 1024;
    let stale = answer("ab12", "present", "2914", Some(RUNNING))
        .replace(&stats_line(cap_bytes()), &stats_line(old));
    assert_eq!(
        parse_installed(&stale).unwrap().sccache,
        Some(listening(2914, old, true))
    );
    let read = |host: &str, running: Option<u64>| HostInstalled {
        host: host.to_string(),
        read: Ok(Installed {
            builds: Some("ab12".to_string()),
            mac: Some(crate::host_installed::MacInstalled { ui_tests: all_on() }),
            sccache_cap: true,
            sccache: running.map(|cap| listening(2914, cap, true)),
        }),
    };
    let without_listener = installed_findings(&[read(LOCAL, Some(cap_bytes())), read("m1", None)]);
    assert_eq!(without_listener.len(), 1, "{without_listener:?}");
    assert!(without_listener[0].text.contains(SCCACHE_OUTSIDE_SERVICE));
    let found = installed_findings(&[read(LOCAL, Some(old)), read("m1", Some(old))]);
    let shown: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|advice| (advice.text.as_str(), advice.command.as_deref()))
        .collect();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(
        shown[0]
            .0
            .starts_with("this Mac's sccache server runs with a cap of 30.0G"),
        "{shown:?}"
    );
    assert_eq!(shown[0].1, Some("builds install"));
    assert!(
        shown[1]
            .0
            .starts_with("host m1's sccache server runs with a cap of 30.0G"),
        "{shown:?}"
    );
    assert_eq!(shown[1].1, Some("builds host setup m1"));
}

/// A Linux host says nothing of the settings UI tests need,
/// and its sccache server's job is systemd's.
#[test]
fn reads_what_a_linux_host_has_installed() {
    let said = format!(
        "os=Linux\narch=x86_64\nbuilds=builds ab12\njob_file=present\n{config}listener_status=0\nlistener=p812\nlistener=R700\nlistener=csccache\nlistener=f5\nlistener_said=\n{stats}job=loaded\nLoadState=loaded\nSubState=running\nMainPID=812\nExecMainStatus=0\n",
        config = config_lines(crate::install::sccache_cap_kib()),
        stats = stats_line(cap_bytes()),
    );
    assert_eq!(
        parse_installed(&said).unwrap(),
        Installed {
            builds: Some("builds ab12".to_string()),
            mac: None,
            sccache_cap: true,
            sccache: Some(listening(812, cap_bytes(), true)),
        }
    );
}
