use super::*;

/// The lines of `pmset -g assertions` that this process holds under <name>.
fn held_by_this_process(name: &str) -> Vec<String> {
    let output = std::process::Command::new("pmset")
        .args(["-g", "assertions"])
        .output()
        .expect("pmset runs");
    let owner = format!("pid {}(", std::process::id());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(&owner) && line.contains(&format!("named: \"{name}\"")))
        .map(str::to_string)
        .collect()
}

fn host(target: Option<&str>) -> Host {
    Host {
        name: "m1".to_string(),
        target: target.map(str::to_string),
        cores: 10,
        speed: None,
        band: None,
    }
}

#[test]
fn work_on_a_host_keeps_this_mac_awake_until_it_ends() {
    let what = format!("test {}", crate::job_claim::now_nanos());
    let name = format!("builds: {what} on host m1");
    let awake = KeepAwake::for_host(&host(Some("u@m1")), &what).unwrap();
    let held = held_by_this_process(&name);
    for kind in ["PreventUserIdleSystemSleep", "NetworkClientActive"] {
        assert!(
            held.iter()
                .any(|line| line.contains(&format!(" {kind} named:"))),
            "{kind} is held: {held:?}"
        );
    }
    drop(awake);
    assert_eq!(held_by_this_process(&name), Vec::<String>::new());
}

#[test]
fn work_on_this_mac_holds_nothing() {
    let what = format!("test {}", crate::job_claim::now_nanos());
    let awake = KeepAwake::for_host(&host(None), &what).unwrap();
    assert_eq!(
        held_by_this_process(&format!("builds: {what} on host m1")),
        Vec::<String>::new()
    );
    drop(awake);
}
