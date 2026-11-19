//! What each host, this Mac included, has set up for builds' jobs: the
//! `builds` and `builds-shots` programs its jobs run, whether its service
//! manager (launchd or systemd, sccache_service.rs) runs its sccache server,
//! whether its sccache config holds builds' cap, and the settings that UI
//! tests need on the host (ui_test_settings.rs). `builds
//! health` and the dashboard compare each host with this Mac and say what to
//! run for each one that differs; `ui_test_settings::findings` says what to
//! run for the settings.

use crate::checkout::parallel_map;
use crate::fail;
use crate::fail::Result;
use crate::host::{Host, LOCAL, Ssh};
use crate::places::Places;

/// What a host has set up for builds' jobs, as `installed_script` prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// What the host's ~/.local/bin/builds, the program its jobs and the
    /// repos' scripts run there, says with `builds version`: the sources it
    /// is built from, the same for a Mac's build and a Linux host's; None
    /// when the host has none.
    pub builds: Option<String>,
    /// What a Mac has of what only a Mac runs; None for a Linux host, which
    /// takes no app builds.
    pub mac: Option<MacInstalled>,
    /// Whether the host's service manager runs its sccache server from the
    /// job file builds writes (`install::run_sccache_server`), so the server
    /// keeps the cache under the size limit builds sets and does not exit
    /// when idle: the job file is there, and the sccache server listening on
    /// sccache's port is the manager's job (`sccache::job_serves`, the same
    /// check that setup uses).
    pub sccache_served: bool,
    /// Whether the host's sccache config holds builds' cap
    /// (`install::sccache_cap_kib`), which `install::run_sccache_server`
    /// writes and the server reads when it starts.
    pub sccache_cap: bool,
    /// The max cache size, in bytes, that the host's listening sccache server
    /// runs with, which it read from its config when it started; None when no
    /// server listens. A server that started before builds wrote a new cap
    /// keeps the old one until it starts again.
    pub sccache_running_cap: Option<u64>,
}

/// What a Mac host has of what app builds need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacInstalled {
    /// The SHA-256 of the host's ~/.local/bin/builds-shots, which `builds
    /// shots` runs there; None when the host has none.
    pub shots: Option<String>,
    /// The settings that UI tests need on this host, which `builds host
    /// setup` turns on.
    pub ui_tests: crate::ui_test_settings::UiTestSettings,
}

/// Runs on a host, and on this Mac, to print what `parse_installed` reads:
/// the host's platform, what its builds says with `builds version`, on a Mac
/// the builds-shots digest and the settings that UI tests need, whether the
/// file of the sccache server's job is there, each line of the sccache config
/// after `sccache_config:`, the processes listening on sccache's port, and
/// last, after `job=loaded`, what the
/// service manager says about the server's job (sccache_service.rs). Each
/// OS keeps the job's file and the config in a place of its own.
fn installed_script() -> String {
    use crate::host_platform::Os;
    use crate::sccache_service::Manager;
    let files = |os: Os| {
        format!(
            r#"if [ -e "$HOME/{job}" ]; then echo job_file=present; else echo job_file=missing; fi
  if [ -e "$HOME/{config}" ]; then sed 's/^/{CONFIG_PREFIX}/' "$HOME/{config}"; fi"#,
            job = Manager(os).job_file(),
            config = crate::install::sccache_config_file(os),
        )
    };
    format!(
        r#"{platform}
if [ -e "$HOME/.local/bin/builds" ]; then echo "builds=$("$HOME/.local/bin/builds" version 2>&1 | tr '\n' ' ')"; else echo builds=missing; fi
case "$os" in
Darwin)
  if [ -e "$HOME/.local/bin/{shots}" ]; then echo "shots=$(shasum -a 256 "$HOME/.local/bin/{shots}" | cut -d ' ' -f 1)"; else echo shots=missing; fi
  {mac_files}
{ui_tests}
  ;;
*)
  {linux_files}
  ;;
esac
{listener}
case "$os" in
Darwin) {mac_job} ;;
*) {linux_job} ;;
esac"#,
        platform = crate::host_platform::SCRIPT,
        shots = crate::shots_program::PROGRAM,
        mac_files = files(Os::Mac),
        linux_files = files(Os::Linux),
        listener = crate::sccache::listener_script(),
        ui_tests = crate::ui_test_settings::SCRIPT,
        mac_job = Manager(Os::Mac).job_script(),
        linux_job = Manager(Os::Linux).job_script(),
    )
}

/// What `installed_script` puts before each line of the sccache config.
const CONFIG_PREFIX: &str = "sccache_config:";

/// Reads what `installed_script` printed.
pub fn parse_installed(text: &str) -> Result<Installed> {
    let value = |key: &str| crate::host::answer_value(text, key);
    let Some(builds) = value("builds") else {
        fail!("the host's answer has no builds line: {text:?}");
    };
    let builds = match builds {
        "missing" => None,
        "" => fail!("the host's builds printed no version: {text:?}"),
        said => Some(said.to_string()),
    };
    let os = crate::host_platform::Platform::parse(text)?.os;
    let mac = match os {
        crate::host_platform::Os::Linux => None,
        crate::host_platform::Os::Mac => {
            let shots = match value("shots") {
                Some("missing") => None,
                Some("") => fail!("shasum printed nothing for the host's builds-shots: {text:?}"),
                Some(digest) => Some(digest.to_string()),
                None => fail!("the host's answer has no shots line: {text:?}"),
            };
            Some(MacInstalled {
                shots,
                ui_tests: crate::ui_test_settings::parse(text)?,
            })
        }
    };
    let manager = crate::sccache_service::Manager(os);
    let job_file = match value("job_file") {
        Some("present") => true,
        Some("missing") => false,
        _ => fail!("the host's answer has no job_file line: {text:?}"),
    };
    let listener = crate::sccache::parse_listener_lines(text)?;
    let sccache_running_cap = crate::sccache::parse_stats_line(text)?;
    let config: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix(CONFIG_PREFIX))
        .collect();
    let sccache_cap = crate::install::sccache_config_text_holds(
        &config.join("\n"),
        crate::install::sccache_cap_kib(),
    );
    let job = match text.split_once("\njob=") {
        Some((_, rest)) if rest.starts_with("loaded") => Some(manager.parse_job(rest)?),
        Some((_, rest)) if rest.starts_with("none") => None,
        Some((_, rest)) if rest.starts_with("unread") => fail!(
            "{} could not say what it runs of the sccache server: {}",
            manager.word(),
            rest.trim_start_matches("unread").trim()
        ),
        _ => fail!("the host's answer has no job line: {text:?}"),
    };
    Ok(Installed {
        builds,
        mac,
        sccache_served: job_file && crate::sccache::job_serves(job.as_ref(), listener),
        sccache_cap,
        sccache_running_cap,
    })
}

/// Asks <host> what it has installed; for this Mac, <places>' home holds
/// what it has. The error says what went wrong asking.
pub fn read_installed(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
) -> std::result::Result<Installed, String> {
    let transport = crate::remote::Transport { places, ssh, host };
    let printed = transport
        .ask(
            &format!("{}{}", crate::remote::root_script(host), installed_script()),
            "asking what it has installed",
        )
        .map_err(|fail| fail.text())??;
    parse_installed(&printed).map_err(|fail| fail.text())
}

/// What one host said it has installed, or why asking failed.
pub struct HostInstalled {
    pub host: String,
    pub read: std::result::Result<Installed, String>,
}

/// Asks every host in <hosts>, this Mac included, at the same time.
pub fn read_all_installed(places: &Places, ssh: &Ssh, hosts: &[Host]) -> Vec<HostInstalled> {
    parallel_map(hosts, |host| HostInstalled {
        host: host.name.clone(),
        read: read_installed(places, ssh, host),
    })
}

/// The "needs action" lines for <reads>: each host whose builds is not this
/// Mac's, each host, this Mac included, whose sccache server its service
/// manager does not
/// run or whose sccache config does not hold builds' cap, and each host that
/// builds could not ask, with what went wrong.
pub fn installed_findings(reads: &[HostInstalled]) -> Vec<crate::health::Advice> {
    let Some(local) = reads.iter().find(|read| read.host == LOCAL) else {
        return Vec::new();
    };
    let local = match &local.read {
        Ok(installed) => installed,
        Err(error) => {
            return vec![crate::health::Advice::said(format!(
                "builds could not read what this Mac has installed to compare the hosts with it: {error}"
            ))];
        }
    };
    let mut out = Vec::new();
    if !local.sccache_served {
        out.push(crate::health::Advice::run(
            format!("this Mac's {SCCACHE_OUTSIDE_SERVICE} Run it under launchd with"),
            "builds install".to_string(),
        ));
    }
    if !local.sccache_cap {
        out.push(crate::health::Advice::run(
            format!(
                "this Mac's {} Write builds' cap into the config with",
                sccache_cap_differs()
            ),
            "builds install".to_string(),
        ));
    }
    if let Some(stale) = stale_running_cap(local) {
        out.push(crate::health::Advice::run(
            format!("this Mac's {stale} Restart it with"),
            "builds install".to_string(),
        ));
    }
    for read in reads.iter().filter(|read| read.host != LOCAL) {
        let name = &read.host;
        let installed = match &read.read {
            Ok(installed) => installed,
            Err(error) => {
                out.push(crate::health::Advice::said(format!(
                    "builds could not read what host {name} has installed: {error}"
                )));
                continue;
            }
        };
        let setup = format!("builds host setup {name}");
        if installed.builds != local.builds {
            let text = match &installed.builds {
                None => format!(
                    "host {name} has no builds at ~/.local/bin/builds, so the repos' scripts fail in its jobs. Install this Mac's builds there with"
                ),
                Some(said) => format!(
                    "host {name} runs a different version of builds than this Mac (its ~/.local/bin/builds says {said:?}, and this Mac's says {:?}), so its jobs can behave differently from this Mac's. Install this Mac's builds there with",
                    local.builds.as_deref().unwrap_or("nothing")
                ),
            };
            out.push(crate::health::Advice::run(text, setup.clone()));
        }
        let shots = |installed: &Installed| installed.mac.as_ref().map(|mac| mac.shots.clone());
        if let (Some(theirs), Some(mine)) = (shots(installed), shots(local))
            && theirs != mine
        {
            let text = match theirs {
                None => format!(
                    "host {name} has no builds-shots at ~/.local/bin/builds-shots, so `builds shots` fails in its jobs. Install this Mac's builds-shots there with"
                ),
                Some(_) => format!(
                    "host {name} runs a different version of builds-shots than this Mac (its ~/.local/bin/builds-shots differs), so its shots checks can find other differences than this Mac's. Install this Mac's builds-shots there with"
                ),
            };
            out.push(crate::health::Advice::run(text, setup.clone()));
        }
        if !installed.sccache_served {
            out.push(crate::health::Advice::run(
                format!(
                    "host {name}'s {SCCACHE_OUTSIDE_SERVICE} Run it under its service manager with"
                ),
                setup.clone(),
            ));
        }
        if !installed.sccache_cap {
            out.push(crate::health::Advice::run(
                format!(
                    "host {name}'s {} Write builds' cap into the config with",
                    sccache_cap_differs()
                ),
                setup.clone(),
            ));
        }
        if let Some(stale) = stale_running_cap(installed) {
            out.push(crate::health::Advice::run(
                format!("host {name}'s {stale} Restart it with"),
                setup,
            ));
        }
    }
    out
}

/// What a host whose sccache server its service manager does not run loses.
const SCCACHE_OUTSIDE_SERVICE: &str = "sccache server does not run under its service manager (launchd on macOS, systemd on Linux), so the first compile starts one that takes the client's TMPDIR and open-file limit and exits when idle.";

/// What a Mac whose sccache config does not hold builds' cap loses.
fn sccache_cap_differs() -> String {
    format!(
        "sccache config does not hold builds' cap of {}, so its sccache server keeps a cache of another size.",
        crate::text::human(crate::install::sccache_cap_kib())
    )
}

/// What a Mac loses whose config holds builds' cap while its listening
/// sccache server runs with another, which it read before builds wrote the
/// cap; None when the server runs with the config's cap, no server listens,
/// or the config itself differs (`sccache_cap_differs` says so).
fn stale_running_cap(installed: &Installed) -> Option<String> {
    let cap = crate::install::sccache_cap_kib() * 1024;
    let running = installed.sccache_running_cap?;
    (installed.sccache_cap && running != cap).then(|| {
        format!(
            "sccache server runs with a cap of {}, not the {} its config holds, since it started before builds wrote that cap.",
            crate::text::human_bytes(running),
            crate::text::human_bytes(cap)
        )
    })
}

#[cfg(test)]
#[path = "host_installed_tests.rs"]
mod tests;
