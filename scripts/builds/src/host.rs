//! Hosts: the Macs builds can run a job on. This Mac is always one (`local`);
//! `builds host add` records another, reached over ssh. builds keeps the list
//! in the file `hosts` beside the lists folder, one `<name> <target> <cores>`
//! per line, and asks each host for its load, cores and free disk, and whether
//! git runs there, when `builds host list` or `builds remote run` needs them.
//!
//! A host refuses jobs when it answers but cannot start one: git fails there
//! (as when its Xcode license is not accepted), or sending a job's commit or
//! claiming a worktree for it failed there. builds records each such host in
//! the folder `host-refusals` beside `hosts`, one file per host holding when it
//! first refused and what it said, and passes over a host recorded there. A
//! probe that finds the host answering and git running removes the record, so
//! the next pick may choose the host again.
//!
//! A host refuses mac app builds when its Xcode cannot start (xcode.rs): an
//! app build whose step failed that way records the host, this Mac included,
//! in the folder `host-xcode-failures` the same way, and the mac lane skips
//! the host's app checkouts. No probe can find out whether Xcode starts
//! again, so the record stays until `builds host clear-xcode` removes it,
//! once someone has run the fix xcodebuild named on the host.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::checkout::parallel_map;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::text::{human, local_minute};
use crate::tool::{Tool, volume_space};

/// The name of this Mac among the hosts. No other host may take it.
pub const LOCAL: &str = "local";

/// How long ssh waits to connect before it counts a host as unreachable.
const CONNECT_SECONDS: u32 = 10;

/// The program that runs commands on another Mac: ssh, or in tests a program
/// that runs them in a temporary folder. `BUILDS_SSH` names another.
#[derive(Clone, Debug)]
pub struct Ssh {
    pub program: String,
}

impl Ssh {
    pub fn from_env() -> Ssh {
        Ssh {
            program: std::env::var("BUILDS_SSH")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "ssh".to_string()),
        }
    }

    /// The options every connection takes: never ask for a password, and give
    /// up on a host that does not answer.
    fn options() -> [String; 4] {
        [
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            format!("ConnectTimeout={CONNECT_SECONDS}"),
        ]
    }

    /// `ssh <options> <target> <command>`; ssh hands the command to the
    /// remote shell as one line.
    pub fn command(&self, target: &str, remote_command: &str) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(Self::options())
            .arg(target)
            .arg(remote_command);
        command
    }

    /// What git runs for `ssh` when it pushes to `<target>:<path>`.
    pub fn git_command(&self) -> String {
        format!("{} {}", sh_quote(&self.program), Self::options().join(" "))
    }
}

/// <text> as one word for a POSIX shell.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    pub name: String,
    /// The ssh target (`user@machine`); None for this Mac.
    pub target: Option<String>,
    pub cores: u32,
    /// How fast one of its cores compiles against one of this Mac's
    /// (`core_speed`), measured by `builds host setup`: 1 for this Mac, None
    /// for a host added and not yet set up.
    pub speed: Option<f64>,
}

impl Host {
    /// This Mac, with the cores it has.
    pub fn local() -> Host {
        Host {
            name: LOCAL.to_string(),
            target: None,
            cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            speed: Some(1.0),
        }
    }

    pub fn describe(&self) -> String {
        match &self.target {
            Some(target) => target.clone(),
            None => "this Mac".to_string(),
        }
    }
}

/// What a host said about itself when asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub cores: u32,
    /// The 1, 5 and 15-minute load averages.
    pub load: [f64; 3],
    pub free_kib: u64,
    /// What git printed when it failed on the host, in the shell that a job's
    /// push reaches: every job runs git there, so the host refuses every job.
    /// None when git runs.
    pub git_fails: Option<String>,
}

impl Probe {
    pub fn load_per_core(&self) -> f64 {
        self.load[0] / f64::from(self.cores.max(1))
    }

    /// The cores nothing is using: the cores minus the one-minute load
    /// average, below zero for a host with more work than cores.
    pub fn room(&self) -> f64 {
        f64::from(self.cores) - self.load[0]
    }

    /// `room`, less the load of <runs> cargo runs that started too recently
    /// for the load average to show them (cargo_starts.rs): cargo runs one
    /// compile job per core, so each counts as the host's cores.
    pub fn room_after(&self, runs: u32) -> f64 {
        self.room() - f64::from(runs) * f64::from(self.cores)
    }
}

/// Runs on the host to print what `parse_probe` reads. git runs in the shell
/// ssh starts, with no PATH of builds' own, as the `git-receive-pack` that a
/// job's push starts there does.
const PROBE_SCRIPT: &str = r#"echo cores=$(sysctl -n hw.ncpu)
echo load=$(sysctl -n vm.loadavg | tr -d '{}')
echo free_kib=$(df -k "$HOME" | awk 'NR==2{print $4}')
if ! said=$(git --version 2>&1); then echo "git_fails=$(printf '%s' "$said" | tr '\n' ' ')"; fi"#;

/// Reads what `PROBE_SCRIPT` printed.
pub fn parse_probe(text: &str) -> Result<Probe> {
    let value = |key: &str| -> Result<&str> {
        match text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
        {
            Some(value) => Ok(value.trim()),
            None => fail!("the host's answer has no {key}: {text:?}"),
        }
    };
    let cores: u32 = value("cores")?
        .parse()
        .context(|| "reading the host's cores".to_string())?;
    let averages: Vec<f64> = value("load")?
        .split_whitespace()
        .map(|average| {
            average
                .parse()
                .context(|| "reading the host's load average".to_string())
        })
        .collect::<Result<_>>()?;
    let Ok(load) = <[f64; 3]>::try_from(averages) else {
        fail!(
            "the host's load has not three averages: {:?}",
            value("load")?
        );
    };
    let free_kib: u64 = value("free_kib")?
        .parse()
        .context(|| "reading the host's free disk".to_string())?;
    let git_fails = text
        .lines()
        .find_map(|line| line.strip_prefix("git_fails="))
        .map(|said| said.trim().to_string());
    Ok(Probe {
        cores,
        load,
        free_kib,
        git_fails,
    })
}

/// Asks <target> over ssh for its cores, load and free disk. The error says
/// what ssh printed.
pub fn probe_target(ssh: &Ssh, target: &str) -> std::result::Result<Probe, String> {
    let output = ssh
        .command(target, PROBE_SCRIPT)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("running {}: {error}", ssh.program))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ssh to {target} failed ({}): {}",
            output.status,
            said.trim()
        ));
    }
    parse_probe(&String::from_utf8_lossy(&output.stdout)).map_err(|fail| fail.text())
}

/// This Mac's cores, load and free disk.
pub fn probe_local(places: &Places) -> Result<Probe> {
    let mut loads = [0f64; 3];
    if unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) } != 3 {
        fail!("reading this Mac's load average");
    }
    // A job here runs in this Mac's own clone: nothing is pushed.
    Ok(Probe {
        cores: Host::local().cores,
        load: loads,
        free_kib: volume_space(&places.home)?.free_kib,
        git_fails: None,
    })
}

/// Asks <host>, and keeps its record of refusing jobs current with the
/// answer: `probe` records a host whose git fails as refusing jobs, and
/// removes the record of a host whose git runs. A host that does not answer
/// keeps its record as it is.
pub fn probe(places: &Places, ssh: &Ssh, host: &Host) -> std::result::Result<Probe, String> {
    let Some(target) = &host.target else {
        return probe_local(places).map_err(|fail| fail.text());
    };
    let probe = probe_target(ssh, target)?;
    let kept = match &probe.git_fails {
        Some(said) => record_refusal(
            places,
            Refused::Jobs,
            &host.name,
            &format!("git fails there: {said}"),
        ),
        None => clear_refusal(places, Refused::Jobs, &host.name),
    };
    kept.map_err(|fail| fail.text())?;
    Ok(probe)
}

/// What a host refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refused {
    /// Every job: git fails there, or a job could not start there.
    Jobs,
    /// Mac app builds: its Xcode cannot start.
    MacBuilds,
}

impl Refused {
    const ALL: [Refused; 2] = [Refused::Jobs, Refused::MacBuilds];

    /// The folder of its records, beside the hosts file.
    fn dir(self, places: &Places) -> PathBuf {
        hosts_file(places).with_file_name(match self {
            Refused::Jobs => "host-refusals",
            Refused::MacBuilds => "host-xcode-failures",
        })
    }
}

/// A host that refused work, as its kind's folder records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub host: String,
    pub refused: Refused,
    /// When it first refused, seconds since 1970.
    pub at: i64,
    /// What the host said.
    pub said: String,
}

/// Records that host <name> refused <refused>, saying <said>. A host already
/// recorded keeps the time of its first refusal and what it said then.
pub fn record_refusal(places: &Places, refused: Refused, name: &str, said: &str) -> Result<()> {
    let dir = refused.dir(places);
    let file = dir.join(name);
    if file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let part = dir.join(format!(".{name}.{}", std::process::id()));
    std::fs::write(&part, format!("{at}\n{said}\n"))
        .context(|| format!("writing {}", part.display()))?;
    std::fs::rename(&part, &file).context(|| format!("replacing {}", file.display()))
}

/// Removes host <name>'s record of refusing <refused>, when it has one.
pub fn clear_refusal(places: &Places, refused: Refused, name: &str) -> Result<()> {
    let file = refused.dir(places).join(name);
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => fail!("removing {}: {error}", file.display()),
    }
}

/// Every recorded refusal of a host still recorded (this Mac is always one),
/// by host name, then by what it refuses.
pub fn refusals(places: &Places) -> Result<Vec<Refusal>> {
    let hosts = all(places)?;
    let mut found = Vec::new();
    for refused in Refused::ALL {
        found.extend(refusals_of(places, refused, &hosts)?);
    }
    found.sort_by(|a, b| (&a.host, a.refused).cmp(&(&b.host, b.refused)));
    Ok(found)
}

/// The names of the hosts recorded as refusing <refused>.
pub fn refusing(places: &Places, refused: Refused) -> Result<Vec<String>> {
    Ok(refusals_of(places, refused, &all(places)?)?
        .into_iter()
        .map(|refusal| refusal.host)
        .collect())
}

/// The records of hosts among <hosts> that refuse <refused>.
fn refusals_of(places: &Places, refused: Refused, hosts: &[Host]) -> Result<Vec<Refusal>> {
    let dir = refused.dir(places);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", dir.display()))?
            .path();
        let name = crate::checkout::file_name(&path);
        // A file being written starts with a dot.
        if name.starts_with('.') || !hosts.iter().any(|host| host.name == name) {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        let (at, said) = text.split_once('\n').unwrap_or((&text, ""));
        found.push(Refusal {
            host: name,
            refused,
            at: at
                .trim()
                .parse()
                .context(|| format!("reading the time in {}", path.display()))?,
            said: said.trim().to_string(),
        });
    }
    found.sort_by(|a, b| a.host.cmp(&b.host));
    Ok(found)
}

/// The line `builds health` prints under "needs action" for <refusal>.
pub fn refusal_finding(refusal: &Refusal) -> crate::health::Advice {
    let name = &refusal.host;
    if refusal.refused == Refused::MacBuilds {
        let fix = match crate::xcode::fix_named_in(&refusal.said) {
            Some(fix) => format!("Run sudo {fix} on {name}"),
            None => format!("Fix what xcodebuild said on {name}"),
        };
        return crate::health::Advice::run(
            format!(
                "host {name}'s Xcode cannot start (since {}), so mac app builds skip its app checkouts. xcodebuild said: {}. {fix}, then tell builds to use {name} again with",
                local_minute(refusal.at),
                refusal.said.trim_end_matches('.')
            ),
            format!("builds host clear-xcode {name}"),
        );
    }
    crate::health::Advice::run(
        format!(
            "host {name} refuses jobs (since {}), so builds runs its jobs on this Mac. {name} said: {}. Fix what {name} said on {name}. builds uses {name} again once a probe finds it answering with git running. Each builds cargo that may leave this Mac probes it; to probe every host now, run",
            local_minute(refusal.at),
            refusal.said.trim_end_matches('.')
        ),
        "builds host list".to_string(),
    )
}

/// Why <probe> keeps <host> from taking a job, or None when it can take one:
/// it does not answer, its free disk is under <floor_kib>, or it refuses jobs.
fn passed_over(
    host: &Host,
    probe: &std::result::Result<Probe, String>,
    floor_kib: u64,
) -> Option<String> {
    match probe {
        Err(error) => Some(format!("{} is unreachable ({error})", host.name)),
        Ok(probe) if probe.free_kib < floor_kib => Some(format!(
            "{} has {} free, under the floor of {}",
            host.name,
            human(probe.free_kib),
            human(floor_kib)
        )),
        Ok(Probe {
            git_fails: Some(said),
            ..
        }) => Some(format!(
            "{} refuses jobs (git fails there: {said})",
            host.name
        )),
        Ok(_) => None,
    }
}

/// The file the hosts are kept in, beside the lists folder.
pub fn hosts_file(places: &Places) -> PathBuf {
    places.lists.parent().unwrap_or(&places.lists).join("hosts")
}

/// The recorded remote hosts, in the order they were added.
pub fn read_remote(places: &Places) -> Result<Vec<Host>> {
    let file = hosts_file(places);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    let mut hosts = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (name, target, cores, speed) = match fields[..] {
            [name, target, cores] => (name, target, cores, None),
            [name, target, cores, speed] => (name, target, cores, Some(speed)),
            _ => fail!(
                "{} has a line that is not `<name> <target> <cores> [<speed>]`: {line}",
                file.display()
            ),
        };
        let speed = match speed {
            None => None,
            Some(text) => match text.parse::<f64>() {
                Ok(speed) if speed > 0.0 && speed.is_finite() => Some(speed),
                _ => fail!(
                    "{} has a speed that is not a positive number: {line}",
                    file.display()
                ),
            },
        };
        hosts.push(Host {
            name: name.to_string(),
            target: Some(target.to_string()),
            cores: cores
                .parse()
                .context(|| format!("reading the cores in {}", file.display()))?,
            speed,
        });
    }
    Ok(hosts)
}

fn write_remote(places: &Places, hosts: &[Host]) -> Result<()> {
    let file = hosts_file(places);
    let dir = file.parent().expect("a file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let text: String = hosts
        .iter()
        .map(|host| {
            let target = host
                .target
                .as_deref()
                .expect("a recorded host has a target");
            match host.speed {
                Some(speed) => format!("{} {target} {} {speed:.3}\n", host.name, host.cores),
                None => format!("{} {target} {}\n", host.name, host.cores),
            }
        })
        .collect();
    let part = file.with_extension("part");
    std::fs::write(&part, text).context(|| format!("writing {}", part.display()))?;
    std::fs::rename(&part, &file).context(|| format!("replacing {}", file.display()))
}

/// This Mac, then every recorded host.
pub fn all(places: &Places) -> Result<Vec<Host>> {
    let mut hosts = vec![Host::local()];
    hosts.extend(read_remote(places)?);
    Ok(hosts)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Records <name> as the ssh target <target>, with the cores the host reports.
/// Fails, naming the host and ssh's message, when the host does not answer.
pub fn add(places: &Places, ssh: &Ssh, name: &str, target: &str) -> Result<Host> {
    if !valid_name(name) {
        fail!("host name {name:?} must be lowercase letters, digits and dashes");
    }
    if name == LOCAL {
        fail!("{LOCAL} is this Mac, which is always a host");
    }
    if target.is_empty() || target.starts_with('-') || target.contains(char::is_whitespace) {
        fail!("{target:?} is not an ssh target (user@machine)");
    }
    let mut hosts = read_remote(places)?;
    if hosts.iter().any(|host| host.name == name) {
        fail!("host {name} is already recorded; `builds host remove {name}` first");
    }
    let probe = match probe_target(ssh, target) {
        Ok(probe) => probe,
        Err(error) => fail!("host {name} is not reachable: {error}"),
    };
    let host = Host {
        name: name.to_string(),
        target: Some(target.to_string()),
        cores: probe.cores,
        speed: None,
    };
    hosts.push(host.clone());
    write_remote(places, &hosts)?;
    Ok(host)
}

pub fn remove(places: &Places, name: &str) -> Result<()> {
    let mut hosts = read_remote(places)?;
    let Some(at) = hosts.iter().position(|host| host.name == name) else {
        if name == LOCAL {
            fail!("{LOCAL} is this Mac, which is always a host");
        }
        fail!("no host named {name}");
    };
    hosts.remove(at);
    write_remote(places, &hosts)?;
    for refused in Refused::ALL {
        clear_refusal(places, refused, name)?;
    }
    Ok(())
}

/// What a host has installed of builds' own, as `installed_script` prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The SHA-256 of the host's ~/.local/bin/builds, the program its jobs
    /// and the repos' scripts run there; None when the host has none.
    pub builds: Option<String>,
    /// Whether launchd runs the host's sccache server from the plist builds
    /// writes (`install::run_sccache_under_launchd`), so the server has
    /// builds' cap and never idles out.
    pub sccache_under_launchd: bool,
}

/// Runs on a host, and on this Mac, to print what `parse_installed` reads.
fn installed_script() -> String {
    let label = crate::install::SCCACHE_LABEL;
    format!(
        r#"if [ -e "$HOME/.local/bin/builds" ]; then echo "builds=$(shasum -a 256 "$HOME/.local/bin/builds" | cut -d ' ' -f 1)"; else echo builds=missing; fi
if [ -e "$HOME/Library/LaunchAgents/{label}.plist" ] && launchctl print "gui/$(id -u)/{label}" 2>/dev/null | grep -q 'state = running'; then echo sccache=launchd; else echo sccache=not-launchd; fi"#
    )
}

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
    let sccache_under_launchd = match value("sccache") {
        Some("launchd") => true,
        Some("not-launchd") => false,
        _ => fail!("the host's answer has no sccache line: {text:?}"),
    };
    Ok(Installed {
        builds: match builds {
            "missing" => None,
            "" => fail!("shasum printed nothing for the host's builds: {text:?}"),
            digest => Some(digest.to_string()),
        },
        sccache_under_launchd,
    })
}

/// Asks <host> what it has installed; for this Mac, <places>' home holds
/// what it has. The error says what went wrong asking.
pub fn read_installed(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
) -> std::result::Result<Installed, String> {
    let mut command = match &host.target {
        Some(target) => ssh.command(target, &installed_script()),
        None => {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg(installed_script())
                .env("HOME", &places.home);
            command
        }
    };
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("asking host {}: {error}", host.name))?;
    if !output.status.success() {
        return Err(format!(
            "asking host {} what it has installed failed ({}): {}",
            host.name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_installed(&String::from_utf8_lossy(&output.stdout)).map_err(|fail| fail.text())
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
/// Mac's, and each host, this Mac included, whose sccache server launchd does
/// not run. A host that did not answer adds none, since its load card and
/// `builds host list` already say it does not answer.
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
    for read in reads.iter().filter(|read| read.host != LOCAL) {
        let Ok(installed) = &read.read else { continue };
        let name = &read.host;
        let setup = format!("builds host setup {name}");
        if installed.builds != local.builds {
            let text = match installed.builds {
                None => format!(
                    "host {name} has no builds at ~/.local/bin/builds, so the repos' scripts fail in its jobs. Install this Mac's builds there with"
                ),
                Some(_) => format!(
                    "host {name} runs another builds than this Mac (its ~/.local/bin/builds differs), so its jobs run another version's host logic and store schema. Install this Mac's builds there with"
                ),
            };
            out.push(crate::health::Advice::run(text, setup.clone()));
        }
        if !installed.sccache_under_launchd {
            out.push(crate::health::Advice::run(
                format!("host {name}'s {SCCACHE_OUTSIDE_LAUNCHD} Run it under launchd with"),
                setup,
            ));
        }
    }
    out
}

/// What a host whose sccache server launchd does not run loses.
const SCCACHE_OUTSIDE_LAUNCHD: &str = "sccache server does not run under launchd, so the first compile starts one that takes the client's TMPDIR and open-file limit and exits when idle.";

/// One line of `builds host list`.
pub fn list_line(host: &Host, probe: &std::result::Result<Probe, String>) -> String {
    match probe {
        Ok(probe) => format!(
            "{}  {}  {} cores  load {:.2} per core  free {}  {}",
            host.name,
            host.describe(),
            probe.cores,
            probe.load_per_core(),
            human(probe.free_kib),
            match &probe.git_fails {
                Some(said) => format!("refuses jobs: git fails there: {said}"),
                None => "reachable".to_string(),
            }
        ),
        Err(error) => format!(
            "{}  {}  {} cores  unreachable: {error}",
            host.name,
            host.describe(),
            host.cores
        ),
    }
}

/// The host to run on: the reachable one above the free-disk floor with the
/// lowest load average per core (the first of equals), and the line that says
/// why. Fails, naming each host and what is wrong with it, when none fits.
pub fn choose(
    hosts: &[(Host, std::result::Result<Probe, String>)],
    floor_kib: u64,
) -> Result<(Host, String)> {
    let mut best: Option<(&Host, &Probe)> = None;
    let mut skipped = Vec::new();
    for (host, probe) in hosts {
        if let Some(why) = passed_over(host, probe, floor_kib) {
            skipped.push(why);
        } else if let Ok(probe) = probe
            && best.is_none_or(|(_, held)| probe.load_per_core() < held.load_per_core())
        {
            best = Some((host, probe));
        }
    }
    let Some((host, probe)) = best else {
        fail!("no host can take the job: {}", skipped.join("; "));
    };
    let mut why = format!(
        "host {}: load {:.2} per core, free {}",
        host.name,
        probe.load_per_core(),
        human(probe.free_kib)
    );
    for (other, other_probe) in hosts {
        if other.name != host.name
            && passed_over(other, other_probe, floor_kib).is_none()
            && let Ok(other_probe) = other_probe
        {
            why.push_str(&format!(
                "; {} {:.2}",
                other.name,
                other_probe.load_per_core()
            ));
        }
    }
    for skipped in skipped {
        why.push_str(&format!("; {skipped}"));
    }
    Ok((host.clone(), why))
}

/// The hosts a cargo build may run on, the one with the most room first: the
/// reachable hosts above the free-disk floor with a measured core speed,
/// ordered by `Probe::room_after`, given the number of runs that <recent>
/// counts on each host, times the host's core speed against this Mac's.
/// Hosts whose weighed room is equal to the hundredth of a core keep the
/// order they came in, and `all` puts this Mac first, so this Mac wins a tie.
/// The line says why, with every host's room, recent runs and speed and each
/// host that was left out. A host left out is not a
/// failure: the caller runs on the next, and on this Mac when none is left.
pub fn rank_by_room(
    hosts: &[(Host, std::result::Result<Probe, String>)],
    floor_kib: u64,
    recent: &crate::cargo_starts::Recent,
) -> (Vec<Host>, String) {
    let mut skipped = Vec::new();
    let mut fits: Vec<(&Host, &Probe, f64)> = Vec::new();
    for (host, probe) in hosts {
        match (passed_over(host, probe, floor_kib), host.speed) {
            (Some(why), _) => skipped.push(why),
            (None, None) => skipped.push(format!(
                "{} has no measured core speed (`builds host setup {}`)",
                host.name, host.name
            )),
            (None, Some(speed)) => {
                fits.extend(probe.as_ref().ok().map(|probe| (host, probe, speed)));
            }
        }
    }
    let runs = |host: &Host| recent.get(&host.name).copied().unwrap_or(0);
    // A host's free cores, each counted at its speed against this Mac's: a
    // job finishes first where this is largest. A host with more work than
    // cores has its excess divided by its speed instead, since a slower
    // host takes longer to work through it.
    let weighed = |host: &Host, probe: &Probe, speed: f64| {
        let room = probe.room_after(runs(host));
        if room >= 0.0 {
            room * speed
        } else {
            room / speed
        }
    };
    let hundredths = |host: &Host, probe: &Probe, speed: f64| {
        (weighed(host, probe, speed) * 100.0).round() as i64
    };
    fits.sort_by_key(|(host, probe, speed)| std::cmp::Reverse(hundredths(host, probe, *speed)));
    let mut why: Vec<String> = fits
        .iter()
        .map(|(host, probe, speed)| {
            let at_speed = format!(
                "{:.1} of this Mac's cores ({:.2} speed)",
                weighed(host, probe, *speed),
                speed
            );
            match runs(host) {
                0 => format!(
                    "{} has {:.1} cores of room, {at_speed}",
                    host.name,
                    probe.room()
                ),
                started => format!(
                    "{} has {:.1} cores of room after {} that started in the last minute, {at_speed}",
                    host.name,
                    probe.room_after(started),
                    crate::text::counted(started as usize, "cargo run", "cargo runs")
                ),
            }
        })
        .collect();
    why.extend(skipped);
    (
        fits.into_iter().map(|(host, _, _)| host.clone()).collect(),
        why.join("; "),
    )
}

/// Asks every host at the same time.
pub fn probe_all(
    places: &Places,
    ssh: &Ssh,
    hosts: Vec<Host>,
) -> Vec<(Host, std::result::Result<Probe, String>)> {
    let probes = parallel_map(&hosts, |host| probe(places, ssh, host));
    hosts.into_iter().zip(probes).collect()
}

const USAGE: &str = "usage: builds host add <name> <ssh target> | setup <name> | setup-here | speed-here | list | remove <name> | clear-xcode <name>";

/// The build volume, where a host that has one keeps its builds root, as this
/// Mac does.
pub const VOLUME: &str = "/Volumes/Builds";

/// The builds root on a remote host without a build volume, relative to the
/// host's home: each worktree's build folders live in `<root>/<repo>/<name>`
/// there, as a side worktree's live under /Volumes/Builds here.
pub const REMOTE_ROOT: &str = "builds-root";

/// What `setup_host` did on a host.
pub struct SetUp {
    /// The host's builds root.
    pub root: String,
    /// What `builds host setup-here` said there.
    pub said: String,
    /// The host's core speed against this Mac's, now in the hosts file.
    pub speed: f64,
}

/// Sets <host> up to take jobs: copies this program there (`install_program`),
/// has that copy run `builds host setup-here` there, which runs the host's
/// sccache server under launchd with builds' cap as `builds install` does on
/// this Mac, and records the host's core speed against this Mac's
/// (`core_speed`) in the hosts file, timing the same compile there with
/// `builds host speed-here` and here.
pub fn setup_host(places: &Places, ssh: &Ssh, host: &Host) -> Result<SetUp> {
    let root = install_program(ssh, host)?;
    let (_, said) = run_builds_there(ssh, host, "host setup-here", "setting up")?;
    let (printed, _) = run_builds_there(ssh, host, "host speed-here", "timing a compile on")?;
    let there = crate::core_speed::parse_line(&printed)?;
    let here = crate::core_speed::time_compile(&speed_folder())?;
    let speed = crate::core_speed::factor(here, there);
    let mut hosts = read_remote(places)?;
    let Some(recorded) = hosts.iter_mut().find(|recorded| recorded.name == host.name) else {
        fail!("host {} is no longer recorded", host.name);
    };
    recorded.speed = Some(speed);
    write_remote(places, &hosts)?;
    Ok(SetUp { root, said, speed })
}

/// Where `builds host speed-here` and `setup_host` time their compile.
fn speed_folder() -> PathBuf {
    std::env::temp_dir().join(format!("builds-speed-{}", std::process::id()))
}

/// Runs `~/.local/bin/builds <words>` on <host> with its builds root and
/// PATH; returns what it printed and what it said on stderr. <doing> names
/// the step in errors.
fn run_builds_there(ssh: &Ssh, host: &Host, words: &str, doing: &str) -> Result<(String, String)> {
    let Some(target) = &host.target else {
        fail!("{LOCAL} is this Mac, not a host to run builds on over ssh");
    };
    let script = format!(
        "{root}\"$HOME/.local/bin/builds\" {words}",
        root = crate::remote::root_script(host)
    );
    let output = ssh
        .command(target, &format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running {} to {}", ssh.program, host.name))?;
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        fail!(
            "{doing} host {} ({target}) failed ({}): {said}",
            host.name,
            output.status
        );
    }
    Ok((String::from_utf8_lossy(&output.stdout).into_owned(), said))
}

/// Copies this Mac's `builds` program to `~/.local/bin/builds` on <host> and
/// creates its builds root. The repo's own `scripts/build-layout.sh` and
/// `scripts/cargo.sh` find the program there, so a repo's hooks and scripts
/// work on the host as they do here. Both Macs run arm64 macOS, so the same
/// program runs on both.
pub fn install_program(ssh: &Ssh, host: &Host) -> Result<String> {
    use std::io::Write;
    let Some(target) = &host.target else {
        fail!("{LOCAL} is this Mac and already has builds");
    };
    let exe = std::env::current_exe().context(|| "finding this program".to_string())?;
    let program = std::fs::read(&exe).context(|| format!("reading {}", exe.display()))?;
    let script = format!(
        "{root}mkdir -p \"$HOME/.local/bin\" \"$BUILDS_ROOT\" && cat > \"$HOME/.local/bin/builds.part\" && chmod 755 \"$HOME/.local/bin/builds.part\" && mv \"$HOME/.local/bin/builds.part\" \"$HOME/.local/bin/builds\" && ln -sfn builds \"$HOME/.local/bin/{linker}\" && echo \"$BUILDS_ROOT\"",
        linker = crate::linker::NAME,
        root = crate::remote::root_script(host)
    );
    let mut child = ssh
        .command(target, &format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("running {} to {}", ssh.program, host.name))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let sent = stdin.write_all(&program);
    drop(stdin);
    let output = child
        .wait_with_output()
        .context(|| format!("waiting for ssh to {}", host.name))?;
    if !output.status.success() {
        fail!(
            "installing builds on host {} ({target}) failed ({}): {}",
            host.name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    sent.context(|| format!("sending builds to host {}", host.name))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn cmd_host(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    match args {
        [add_word, name, target] if add_word == "add" => {
            let host = add(&tool.places, ssh, name, target)?;
            println!(
                "added host {} ({}, {} cores)",
                host.name, target, host.cores
            );
            Ok(())
        }
        [word, name] if word == "setup" => {
            let Some(host) = all(&tool.places)?.into_iter().find(|h| h.name == *name) else {
                fail!("no host named {name} (`builds host list`)");
            };
            let set_up = setup_host(&tool.places, ssh, &host)?;
            for line in set_up.said.lines() {
                println!("host {name}: {line}");
            }
            println!(
                "installed builds on host {name}, with its builds root in {}; its cores compile at {:.2} of this Mac's speed",
                set_up.root, set_up.speed
            );
            Ok(())
        }
        [word] if word == "setup-here" => {
            crate::install::run_sccache_under_launchd(&tool.places.home, tool.places.uid)
        }
        [word] if word == "speed-here" => {
            let took = crate::core_speed::time_compile(&speed_folder())?;
            println!("{}", crate::core_speed::line(took));
            Ok(())
        }
        [word, name] if word == "clear-xcode" => {
            if !all(&tool.places)?.iter().any(|host| host.name == *name) {
                fail!("no host named {name} (`builds host list`)");
            }
            clear_refusal(&tool.places, Refused::MacBuilds, name)?;
            println!("mac app builds use the app checkouts on host {name} again");
            Ok(())
        }
        [word, name] if word == "remove" => {
            remove(&tool.places, name)?;
            println!("removed host {name}");
            Ok(())
        }
        [word] if word == "list" => {
            for (host, probe) in probe_all(&tool.places, ssh, all(&tool.places)?) {
                println!("{}", list_line(&host, &probe));
            }
            Ok(())
        }
        _ => fail!("{USAGE}"),
    }
}

/// The folder of <repo>'s clone on a host, relative to the host's home.
pub fn clone_path(repo: &str) -> String {
    format!("dev/{repo}")
}

#[cfg(test)]
#[path = "host_tests.rs"]
pub mod tests;
