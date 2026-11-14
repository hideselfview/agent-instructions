//! Hosts: the Macs builds can run a job on. This Mac is always one (`local`);
//! `builds host add` records another, reached over ssh. builds keeps the list
//! in the file `hosts` beside the lists folder, one `<name> <target> <cores>`
//! per line, and asks each host for its load, cores and free disk when
//! `builds host list` or `builds remote run` needs them.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::checkout::parallel_map;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::text::human;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    /// The ssh target (`user@machine`); None for this Mac.
    pub target: Option<String>,
    pub cores: u32,
}

impl Host {
    /// This Mac, with the cores it has.
    pub fn local() -> Host {
        Host {
            name: LOCAL.to_string(),
            target: None,
            cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
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
    /// The one-minute load average.
    pub load: f64,
    pub free_kib: u64,
}

impl Probe {
    pub fn load_per_core(&self) -> f64 {
        self.load / f64::from(self.cores.max(1))
    }
}

/// Runs on the host to print what `parse_probe` reads.
const PROBE_SCRIPT: &str = r#"echo cores=$(sysctl -n hw.ncpu)
echo load=$(sysctl -n vm.loadavg | tr -d '{}')
echo free_kib=$(df -k "$HOME" | awk 'NR==2{print $4}')"#;

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
    let load: f64 = value("load")?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .parse()
        .context(|| "reading the host's load average".to_string())?;
    let free_kib: u64 = value("free_kib")?
        .parse()
        .context(|| "reading the host's free disk".to_string())?;
    Ok(Probe {
        cores,
        load,
        free_kib,
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
    let mut loads = [0f64; 1];
    if unsafe { libc::getloadavg(loads.as_mut_ptr(), 1) } != 1 {
        fail!("reading this Mac's load average");
    }
    Ok(Probe {
        cores: Host::local().cores,
        load: loads[0],
        free_kib: volume_space(&places.home)?.free_kib,
    })
}

/// Asks <host>.
pub fn probe(places: &Places, ssh: &Ssh, host: &Host) -> std::result::Result<Probe, String> {
    match &host.target {
        Some(target) => probe_target(ssh, target),
        None => probe_local(places).map_err(|fail| fail.text()),
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
        let [name, target, cores] = fields[..] else {
            fail!(
                "{} has a line that is not `<name> <target> <cores>`: {line}",
                file.display()
            );
        };
        hosts.push(Host {
            name: name.to_string(),
            target: Some(target.to_string()),
            cores: cores
                .parse()
                .context(|| format!("reading the cores in {}", file.display()))?,
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
            format!(
                "{} {} {}\n",
                host.name,
                host.target
                    .as_deref()
                    .expect("a recorded host has a target"),
                host.cores
            )
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
    write_remote(places, &hosts)
}

/// One line of `builds host list`.
pub fn list_line(host: &Host, probe: &std::result::Result<Probe, String>) -> String {
    match probe {
        Ok(probe) => format!(
            "{}  {}  {} cores  load {:.2} per core  free {}  reachable",
            host.name,
            host.describe(),
            probe.cores,
            probe.load_per_core(),
            human(probe.free_kib)
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
        match probe {
            Err(error) => skipped.push(format!("{} is unreachable ({error})", host.name)),
            Ok(probe) if probe.free_kib < floor_kib => skipped.push(format!(
                "{} has {} free, under the floor of {}",
                host.name,
                human(probe.free_kib),
                human(floor_kib)
            )),
            Ok(probe) => {
                if best.is_none_or(|(_, held)| probe.load_per_core() < held.load_per_core()) {
                    best = Some((host, probe));
                }
            }
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
            && let Ok(other_probe) = other_probe
            && other_probe.free_kib >= floor_kib
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

/// Asks every host at the same time.
pub fn probe_all(
    places: &Places,
    ssh: &Ssh,
    hosts: Vec<Host>,
) -> Vec<(Host, std::result::Result<Probe, String>)> {
    let probes = parallel_map(&hosts, |host| probe(places, ssh, host));
    hosts.into_iter().zip(probes).collect()
}

const USAGE: &str =
    "usage: builds host add <name> <ssh target> | setup <name> | list | remove <name>";

/// The builds root on a remote host, relative to the host's home: the host
/// has no build volume, so each `remote-N` worktree's build folders live in
/// `<root>/<repo>/remote-N` there, as a side worktree's live under
/// /Volumes/Builds here.
pub const REMOTE_ROOT: &str = "builds-root";

/// Copies this Mac's `builds` program to `~/.local/bin/builds` on <host> and
/// creates its builds root. The repo's own `scripts/build-layout.sh` and
/// `scripts/cargo.sh` find the program there, so a repo's hooks and scripts
/// work on the host as they do here. Both Macs run arm64 macOS, so the same
/// program runs on both.
pub fn setup_host(ssh: &Ssh, host: &Host) -> Result<()> {
    use std::io::Write;
    let Some(target) = &host.target else {
        fail!("{LOCAL} is this Mac and already has builds");
    };
    let exe = std::env::current_exe().context(|| "finding this program".to_string())?;
    let program = std::fs::read(&exe).context(|| format!("reading {}", exe.display()))?;
    let script = format!(
        "mkdir -p \"$HOME/.local/bin\" \"$HOME/{REMOTE_ROOT}\" && cat > \"$HOME/.local/bin/builds.part\" && chmod 755 \"$HOME/.local/bin/builds.part\" && mv \"$HOME/.local/bin/builds.part\" \"$HOME/.local/bin/builds\""
    );
    let mut child = ssh
        .command(target, &format!("sh -c {}", sh_quote(&script)))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
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
    sent.context(|| format!("sending builds to host {}", host.name))
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
            setup_host(ssh, &host)?;
            println!("installed builds on host {name}, with its builds root in ~/{REMOTE_ROOT}");
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
