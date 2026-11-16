//! What each host, this Mac included, has set up for builds' jobs: the
//! `builds` program its jobs run, whether launchd runs its sccache server,
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
    /// The SHA-256 of the host's ~/.local/bin/builds, the program its jobs
    /// and the repos' scripts run there; None when the host has none.
    pub builds: Option<String>,
    /// Whether launchd runs the host's sccache server from the plist builds
    /// writes (`install::run_sccache_under_launchd`), so the server keeps the
    /// cache under the size limit builds sets and does not exit when idle:
    /// the plist is there, and the process listening on sccache's port is
    /// launchd's job (`sccache::launchd_serves`, the same check that setup
    /// uses).
    pub sccache_under_launchd: bool,
    /// Whether the host's sccache config holds builds' cap
    /// (`install::sccache_cap_kib`), which `install::run_sccache_under_launchd`
    /// writes and the server reads when it starts.
    pub sccache_cap: bool,
    /// The settings that UI tests need on this host, which `builds host
    /// setup` turns on.
    pub ui_tests: crate::ui_test_settings::UiTestSettings,
}

/// Runs on a host, and on this Mac, to print what `parse_installed` reads:
/// the builds digest, whether builds' plist is there, each line of the
/// sccache config after `sccache_config:`, the pid listening on sccache's
/// port, the settings that UI tests need, and last, after `job=loaded`, what
/// `launchctl print` says about launchd's job.
fn installed_script() -> String {
    let label = crate::install::SCCACHE_LABEL;
    format!(
        r#"if [ -e "$HOME/.local/bin/builds" ]; then echo "builds=$(shasum -a 256 "$HOME/.local/bin/builds" | cut -d ' ' -f 1)"; else echo builds=missing; fi
if [ -e "$HOME/Library/LaunchAgents/{label}.plist" ]; then echo plist=present; else echo plist=missing; fi
if [ -e "$HOME/{config}" ]; then sed 's/^/{CONFIG_PREFIX}/' "$HOME/{config}"; fi
{listener}
{ui_tests}
if job=$(launchctl print "gui/$(id -u)/{label}" 2>/dev/null); then echo job=loaded; printf '%s\n' "$job"; else echo job=none; fi"#,
        config = crate::install::SCCACHE_CONFIG,
        listener = crate::sccache::listener_script(),
        ui_tests = crate::ui_test_settings::SCRIPT
    )
}

/// What `installed_script` puts before each line of the sccache config.
const CONFIG_PREFIX: &str = "sccache_config:";

/// Reads what `installed_script` printed.
pub fn parse_installed(text: &str) -> Result<Installed> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let Some(builds) = value("builds") else {
        fail!("the host's answer has no builds line: {text:?}");
    };
    let plist = match value("plist") {
        Some("present") => true,
        Some("missing") => false,
        _ => fail!("the host's answer has no plist line: {text:?}"),
    };
    let listener = crate::sccache::parse_listener_lines(text)?;
    let ui_tests = crate::ui_test_settings::parse(text)?;
    let config: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix(CONFIG_PREFIX))
        .collect();
    let sccache_cap = crate::install::sccache_config_text_holds(
        &config.join("\n"),
        crate::install::sccache_cap_kib(),
    );
    let job = match text.split_once("\njob=") {
        Some((_, rest)) if rest.starts_with("loaded") => Some(crate::sccache::parse_job(rest)?),
        Some((_, rest)) if rest.starts_with("none") => None,
        _ => fail!("the host's answer has no job line: {text:?}"),
    };
    Ok(Installed {
        builds: match builds {
            "missing" => None,
            "" => fail!("shasum printed nothing for the host's builds: {text:?}"),
            digest => Some(digest.to_string()),
        },
        sccache_under_launchd: plist && crate::sccache::launchd_serves(job.as_ref(), listener),
        sccache_cap,
        ui_tests,
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
        .ask(&installed_script(), "asking what it has installed")
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
/// Mac's, each host, this Mac included, whose sccache server launchd does not
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
    if !local.sccache_under_launchd {
        out.push(crate::health::Advice::run(
            format!("this Mac's {SCCACHE_OUTSIDE_LAUNCHD} Run it under launchd with"),
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
            let text = match installed.builds {
                None => format!(
                    "host {name} has no builds at ~/.local/bin/builds, so the repos' scripts fail in its jobs. Install this Mac's builds there with"
                ),
                Some(_) => format!(
                    "host {name} runs a different version of builds than this Mac (its ~/.local/bin/builds differs), so its jobs can behave differently from this Mac's. Install this Mac's builds there with"
                ),
            };
            out.push(crate::health::Advice::run(text, setup.clone()));
        }
        if !installed.sccache_under_launchd {
            out.push(crate::health::Advice::run(
                format!("host {name}'s {SCCACHE_OUTSIDE_LAUNCHD} Run it under launchd with"),
                setup.clone(),
            ));
        }
        if !installed.sccache_cap {
            out.push(crate::health::Advice::run(
                format!(
                    "host {name}'s {} Write builds' cap into the config with",
                    sccache_cap_differs()
                ),
                setup,
            ));
        }
    }
    out
}

/// What a host whose sccache server launchd does not run loses.
const SCCACHE_OUTSIDE_LAUNCHD: &str = "sccache server does not run under launchd, so the first compile starts one that takes the client's TMPDIR and open-file limit and exits when idle.";

/// What a Mac whose sccache config does not hold builds' cap loses.
fn sccache_cap_differs() -> String {
    format!(
        "sccache config does not hold builds' cap of {}, so its sccache server keeps a cache of another size.",
        crate::text::human(crate::install::sccache_cap_kib())
    )
}

#[cfg(test)]
#[path = "host_installed_tests.rs"]
mod tests;
