//! The app in front on each Mac host, which each host's scan reads
//! (host_scan.rs). A system alert in front, such as the notification alert
//! that UserNotificationCenter shows, keeps the focus from every window the
//! shot renderer brings forward there, so every Mac window shot on that host
//! fails until someone answers or closes the alert. `builds health` and the
//! dashboard name such a host under "needs action", with the alert's name
//! and pid.

use crate::health::Advice;
use crate::host::Host;
use crate::host_scan::Scan;

/// The apps that show macOS's system alerts: notifications that ask for an
/// answer, password prompts and the alerts of the Finder's services, such as
/// the one that asks whether to open an app downloaded from the internet.
const SYSTEM_ALERTS: [&str; 3] = [
    "UserNotificationCenter",
    "SecurityAgent",
    "CoreServicesUIAgent",
];

/// The line the scan script runs on a Mac (host_scan.rs) to print what
/// `parse` reads: lsappinfo's description of the front app, its name in
/// quotes on the first line and its pid after `pid =`, on one line.
pub const SCRIPT_LINE: &str =
    r#"echo "front_app=$(lsappinfo info -only name,pid "$(lsappinfo front)" 2>&1 | tr '\n' ' ')""#;

/// The app that holds the focus on a Mac, as `lsappinfo` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp {
    /// Its name, or None when lsappinfo names none.
    pub name: Option<String>,
    pub pid: i32,
}

impl FrontApp {
    /// The name of the system alert this app shows, or None when it shows
    /// none.
    fn system_alert(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|name| SYSTEM_ALERTS.contains(name))
    }
}

/// Reads the scan's `front_app` line, <line>: None when the host printed
/// none, as a host that runs no lsappinfo does, and an error that quotes
/// the line when it names no pid.
pub fn parse(line: Option<&str>) -> std::result::Result<Option<FrontApp>, String> {
    let Some(line) = line else {
        return Ok(None);
    };
    let pid = line
        .split_once("pid = ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|pid| pid.parse().ok());
    let Some(pid) = pid else {
        return Err(format!("lsappinfo named no front app: {line:?}"));
    };
    let name = line
        .trim_start()
        .strip_prefix('"')
        .and_then(|rest| rest.split_once('"'))
        .map(|(name, _)| name.to_string());
    Ok(Some(FrontApp { name, pid }))
}

/// The lines for <scans>: each host whose front app is a system alert, with
/// the command that ends the alert's process there, and each host whose
/// front app health could not read. A host whose scan failed gets no line
/// here.
pub fn findings(scans: &[(Host, std::result::Result<Scan, String>)]) -> Vec<Advice> {
    let mut found = Vec::new();
    for (host, scan) in scans {
        let Ok(scan) = scan else { continue };
        let place = crate::host::place(&host.name);
        match &scan.front_app {
            Ok(Some(app)) => {
                if let Some(alert) = app.system_alert() {
                    let pid = app.pid;
                    let end = match &host.target {
                        Some(target) => format!("ssh {target} kill {pid}"),
                        None => format!("kill {pid}"),
                    };
                    found.push(Advice::run(
                        format!(
                            "{place} shows the system alert {alert} (pid {pid}) in front of every app, so every Mac window shot there fails; answer or close the alert on {place}, or end it with"
                        ),
                        end,
                    ));
                }
            }
            Ok(None) => {}
            Err(error) => found.push(Advice::said(format!(
                "health could not tell which app is in front on {place}: {error}"
            ))),
        }
    }
    found
}

#[cfg(test)]
#[path = "host_front_app_tests.rs"]
mod tests;
