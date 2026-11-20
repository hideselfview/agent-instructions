use super::*;
use crate::host::Host;
use crate::host_scan::Scan;

/// What `lsappinfo info -only name,pid "$(lsappinfo front)"` printed on m1
/// while macOS's notification alert held the focus, with the script's
/// newlines turned into spaces.
const ALERT: &str = r#""UserNotificationCenter" ASN:0x0-0x1a01a: (in front)      bundleID=[ NULL ]      bundle path=[ NULL ]      executable path=[ NULL ]      pid = 812 !cgsConnection !signalled type=[ NULL ]  flavor=[ NULL ]  Version=[ NULL ]  Arch=!!none  "#;

fn m1() -> Host {
    Host {
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

fn scan(front_app: std::result::Result<Option<FrontApp>, String>) -> Scan {
    Scan {
        uid: 501,
        at: 0,
        rows: Vec::new(),
        cpu_idle: Ok(50.0),
        jobs: Vec::new(),
        sccache: Ok(None),
        front_app,
    }
}

fn app(name: &str, pid: i32) -> FrontApp {
    FrontApp {
        name: Some(name.to_string()),
        pid,
    }
}

/// lsappinfo's answer names the front app in quotes and its pid after
/// `pid =`; a host that printed no line runs no lsappinfo, and an answer
/// with no pid is an error that quotes it.
#[test]
fn reads_the_front_app_from_lsappinfo() {
    assert_eq!(
        parse(Some(ALERT)),
        Ok(Some(app("UserNotificationCenter", 812)))
    );
    assert_eq!(
        parse(Some(
            "[ NULL ]  [ NULL ]       bundleID=[ NULL ]      pid = 43839 !cgsConnection "
        )),
        Ok(Some(FrontApp {
            name: None,
            pid: 43839
        }))
    );
    assert_eq!(parse(None), Ok(None));
    assert!(
        parse(Some("lsappinfo: no such app"))
            .unwrap_err()
            .contains("lsappinfo: no such app")
    );
}

/// A host whose front app is a system alert gets a line that names the
/// alert and its pid and the command that ends it there; a host with
/// another app in front, or with none, gets none, and a host whose front
/// app health could not read gets a line that says why.
#[test]
fn a_host_whose_front_app_is_a_system_alert_needs_action() {
    let here = Host::local();
    let found = findings(&[
        (m1(), Ok(scan(Ok(Some(app("UserNotificationCenter", 812)))))),
        (here.clone(), Ok(scan(Ok(Some(app("SecurityAgent", 90)))))),
        (m1(), Ok(scan(Ok(Some(app("Terminal", 43839)))))),
        (m1(), Ok(scan(Ok(None)))),
        (m1(), Err("ssh failed".to_string())),
        (
            m1(),
            Ok(scan(Err("lsappinfo named no front app".to_string()))),
        ),
    ]);
    let lines: Vec<String> = found.iter().map(|advice| advice.line()).collect();
    assert_eq!(
        lines,
        [
            "host m1 shows the system alert UserNotificationCenter (pid 812) in front of every app, so every Mac window shot there fails; answer or close the alert on host m1, or end it with `ssh u@m1 kill 812`",
            "this Mac shows the system alert SecurityAgent (pid 90) in front of every app, so every Mac window shot there fails; answer or close the alert on this Mac, or end it with `kill 90`",
            "health could not tell which app is in front on host m1: lsappinfo named no front app",
        ]
    );
}
