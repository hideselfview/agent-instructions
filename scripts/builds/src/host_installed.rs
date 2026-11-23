//! What each host, this Mac included, has set up for builds' jobs: the
//! `builds` program its jobs run, whether its service
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
use crate::host_platform::Os;
use crate::places::Places;
use crate::remote::Transport;

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
    /// Whether the host's sccache config holds builds' cap
    /// (`install::sccache_cap_kib`), which `install::run_sccache_server`
    /// writes and the server reads when it starts.
    pub sccache_cap: bool,
    /// The server on the shared port, when one listens.
    pub(crate) sccache: Option<ListeningSccache>,
}

impl Installed {
    fn sccache_served(&self) -> bool {
        self.sccache.as_ref().is_some_and(|server| server.served)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListeningSccache {
    pid: u32,
    /// The service manager runs this process from the job builds installs.
    served: bool,
    /// The cache size limit in bytes that the server read when it started.
    cap: std::result::Result<u64, String>,
    tmp_issue: Option<String>,
}

/// What a Mac host has of what app builds need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacInstalled {
    /// The settings that UI tests need on this host, which `builds host
    /// setup` turns on.
    pub ui_tests: crate::ui_test_settings::UiTestSettings,
}

/// Runs on a host, and on this Mac, to print what `parse_installed` reads:
/// the host's platform, what its builds says with `builds version`, on a Mac
/// the settings that UI tests need, whether the
/// file of the sccache server's job is there, each line of the sccache config
/// after `sccache_config:`, the processes listening on sccache's port, and
/// last, after `job=loaded`, what the
/// service manager says about the server's job (sccache_service.rs). Each
/// OS keeps the job's file and the config in a place of its own.
fn installed_script() -> String {
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
fn parse_installed(text: &str) -> Result<Installed> {
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
        Os::Linux => None,
        Os::Mac => Some(MacInstalled {
            ui_tests: crate::ui_test_settings::parse(text)?,
        }),
    };
    let manager = crate::sccache_service::Manager(os);
    let job_file = match value("job_file") {
        Some("present") => true,
        Some("missing") => false,
        _ => fail!("the host's answer has no job_file line: {text:?}"),
    };
    let listener = crate::sccache::parse_listener_lines(text)?;
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
        sccache_cap,
        sccache: listener.map(|pid| ListeningSccache {
            pid,
            served: job_file && crate::sccache::job_serves(job.as_ref(), Some(pid)),
            cap: crate::sccache::parse_stats_line(text).map_err(|error| error.text()),
            tmp_issue: None,
        }),
    })
}

/// Asks <host> what it has installed; for this Mac, <places>' home holds
/// what it has. The error says what went wrong asking.
pub fn read_installed(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
) -> std::result::Result<Installed, String> {
    let transport = Transport { places, ssh, host };
    let printed = transport
        .ask(
            &format!("{}{}", crate::remote::root_script(host), installed_script()),
            "asking what it has installed",
        )
        .map_err(|fail| fail.text())??;
    let mut installed = parse_installed(&printed).map_err(|fail| fail.text())?;
    if (host.target.is_none() || installed.builds.is_some())
        && let Some(server) = &mut installed.sccache
    {
        server.tmp_issue = match crate::sccache::tmpdir_issue(&transport, server.pid) {
            Ok(issue) => issue,
            Err(error) => Some(format!(
                "has a TMPDIR that builds could not read ({})",
                error.text()
            )),
        };
    }
    Ok(installed)
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
    let local = reads.iter().find(|read| read.host == LOCAL);
    let mut out = Vec::new();
    for read in reads {
        let name = &read.host;
        let place = crate::host::place(name);
        let installed = match &read.read {
            Ok(installed) => installed,
            Err(error) => {
                out.push(crate::health::Advice::said(format!(
                    "builds could not read what {place} has installed: {error}"
                )));
                continue;
            }
        };
        let setup = if name == LOCAL {
            "builds install".to_string()
        } else {
            format!("builds host setup {name}")
        };
        out.extend(sccache_running_findings(name, installed));
        if name != LOCAL {
            let text = match (&installed.builds, local) {
                (None, _) => Some(format!(
                    "host {name} has no builds at ~/.local/bin/builds. Builds needs that program to run jobs and inspect sccache there. Install this Mac's builds there with"
                )),
                (
                    Some(said),
                    Some(HostInstalled {
                        read: Ok(local), ..
                    }),
                ) if installed.builds != local.builds => Some(format!(
                    "host {name} runs a different version of builds than this Mac (its ~/.local/bin/builds says {said:?}, and this Mac's says {:?}), so its jobs can behave differently from this Mac's. Install this Mac's builds there with",
                    local.builds.as_deref().unwrap_or("nothing")
                )),
                _ => None,
            };
            if let Some(text) = text {
                out.push(crate::health::Advice::run(text, setup.clone()));
            }
        }
        if !installed.sccache_served() {
            out.push(crate::health::Advice::run(
                format!(
                    "{place}'s {SCCACHE_OUTSIDE_SERVICE} Run it under its service manager with"
                ),
                setup.clone(),
            ));
        }
        if !installed.sccache_cap {
            out.push(crate::health::Advice::run(
                format!(
                    "{place}'s {} Write builds' cap into the config with",
                    sccache_cap_differs()
                ),
                setup.clone(),
            ));
        }
        if let Some(stale) = stale_running_cap(installed) {
            out.push(crate::health::Advice::run(
                format!("{place}'s {stale} Restart it with"),
                setup,
            ));
        }
    }
    out
}

/// Why builds asks the user to set up sccache as a service.
const SCCACHE_OUTSIDE_SERVICE: &str = "sccache server runs outside its service manager (launchd on macOS, systemd on Linux) or has yet to start.";

fn sccache_running_findings(
    host: &str,
    installed: &Installed,
) -> impl Iterator<Item = crate::health::Advice> {
    installed.sccache.iter().flat_map(move |server| {
        let place = crate::host::place(host);
        let pid = server.pid;
        let tmp = server.tmp_issue.as_ref().map(|issue| {
            crate::health::Advice::said(format!("The sccache server on {place} (pid {pid}) {issue}."))
        });
        let stats = match &server.cap {
            Ok(_) => None,
            Err(error) => Some(crate::health::Advice::said(format!(
                "Builds could not read the cache size limit of the sccache server on {place} (pid {pid}). {error}"
            ))),
        };
        tmp.into_iter().chain(stats)
    })
}

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
    let Ok(running) = installed.sccache.as_ref()?.cap else {
        return None;
    };
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
