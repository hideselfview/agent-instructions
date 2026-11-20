//! Hosts: the Macs builds can run a job on. This Mac is always one (`local`);
//! `builds host add` records another, reached over ssh. builds keeps the list
//! in the file `hosts` beside the lists folder, one `<name> <target> <cores>
//! [<speed>]` per line, with a line `band <name> <low> <high>` for each host
//! (this Mac included) that has a load band (host_band.rs). builds asks each
//! host for its load, cores, free disk and memory, and whether
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
//! once someone has run the fix xcodebuild named on the host. An app build
//! whose step timed out turning on automation mode for UI tests records the
//! host in the folder `host-ui-automation-failures`, with the same effect,
//! until `builds host setup` finds the settings that UI tests need in place there
//! (ui_test_settings.rs).
//!
//! host_installed.rs checks what each host has set up for builds' jobs, and
//! host_setup.rs sets a host up to take jobs.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::checkout::parallel_map;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host_band::Band;
use crate::host_memory::Memory;
use crate::places::Places;
use crate::text::{human, local_minute};
use crate::tool::{Tool, volume_space};

/// The name of this Mac among the hosts. No other host may take it.
pub const LOCAL: &str = "local";

/// How long ssh waits to connect before it counts a host as unreachable.
const CONNECT_SECONDS: u32 = 10;

/// How often ssh asks a connected host whether it still answers, and how many
/// questions in a row may go unanswered before ssh ends the connection: a
/// connection to a host that stopped answering ends within a minute.
const ALIVE_SECONDS: u32 = 15;
const ALIVE_MISSES: u32 = 4;

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
    /// up on a host that does not answer, whether while connecting or later.
    fn options() -> [String; 8] {
        [
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            format!("ConnectTimeout={CONNECT_SECONDS}"),
            "-o".to_string(),
            format!("ServerAliveInterval={ALIVE_SECONDS}"),
            "-o".to_string(),
            format!("ServerAliveCountMax={ALIVE_MISSES}"),
        ]
    }

    /// `ssh <options> [-t] <target> <command>`; ssh hands the command to
    /// the remote shell as one line, with a terminal there when <terminal>
    /// says so.
    pub fn command(&self, target: &str, remote_command: &str, terminal: Terminal) -> Command {
        let mut command = Command::new(&self.program);
        command.args(Self::options());
        if terminal == Terminal::Person {
            command.arg("-t");
        }
        command.arg(target).arg(remote_command);
        command
    }

    /// What git runs for `ssh` when it pushes to `<target>:<path>`.
    pub fn git_command(&self) -> String {
        format!("{} {}", sh_quote(&self.program), Self::options().join(" "))
    }
}

/// Whether a command run over ssh gets a terminal on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    /// No terminal: builds reads what the command prints.
    None,
    /// A terminal (`ssh -t`) joined to this one, so a program on the host,
    /// such as sudo, can ask the person at this terminal for a password.
    Person,
}

/// The command that runs <script> on <host>: `ssh` to its target, or `sh -c`
/// here. <terminal> says whether a person at this terminal can answer it.
pub fn command_at(ssh: &Ssh, host: &Host, script: &str, terminal: Terminal) -> Command {
    match &host.target {
        Some(target) => ssh.command(target, &format!("sh -c {}", sh_quote(script)), terminal),
        None => {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            command
        }
    }
}

/// Which exits of a command `output_of` counts as success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accept {
    /// Exit 0 only.
    Zero,
    /// Exit 0, or exit 1 with output: `lsof` exits 1 when a process it was
    /// asked about has ended, and still prints the others. Any other status
    /// (ssh's 255 when the connection drops, a signal) fails.
    ZeroOrOneWithOutput,
}

/// What <command> printed. It fails, naming <what>, the status and what the
/// command wrote to stderr, when it cannot run or exits in a way <accept> does
/// not allow.
pub fn output_of(
    mut command: Command,
    what: &str,
    accept: Accept,
) -> std::result::Result<String, String> {
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("running {what}: {error}"))?;
    let ok = output.status.success()
        || (accept == Accept::ZeroOrOneWithOutput
            && output.status.code() == Some(1)
            && !output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !ok {
        return Err(format!(
            "{what} failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    // An accepted exit 1 may also be a per-process error (lsof's permission
    // denied), so say what the command wrote.
    if !output.status.success() && !stderr.trim().is_empty() {
        crate::text::note(format!(
            "{what} exited with {} and wrote: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs <script> on <host> with this terminal as its input and output, so
/// sudo there can ask the person for the password.
pub fn run_at_terminal(ssh: &Ssh, host: &Host, script: &str) -> Result<()> {
    let status = command_at(ssh, host, script, Terminal::Person)
        .status()
        .context(|| format!("running sudo on {}", place(&host.name)))?;
    if !status.success() {
        fail!("sudo on {} failed ({status})", place(&host.name));
    }
    Ok(())
}

/// One sudo step of `builds host setup` on host <name>: <lacking> says what
/// is off there, naming the host, and <commands> turn it on. With a person
/// at this terminal (<at_terminal>), it runs the commands there in one
/// script through <run>, which fails when sudo or a command exits non-zero,
/// and that failure fails the step. With no one at the terminal, it runs
/// nothing and fails. Each failure names the commands to run on the host.
pub fn sudo_step(
    name: &str,
    at_terminal: bool,
    lacking: &str,
    commands: &[String],
    run: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    let place = place(name);
    let shown = shown_commands(commands);
    let then = format!("Run {shown} on {place}, then `builds host setup {name}` checks again");
    if !at_terminal {
        fail!("{lacking}, and no one is at this terminal to type the sudo password. {then}");
    }
    crate::text::note(format!(
        "running {shown} on {place} through sudo, which asks for the password there"
    ));
    if let Err(failed) = run(&commands.join(" && ")) {
        fail!("{lacking}, and {}. {then}", failed.text());
    }
    Ok(())
}

/// <commands>, each in backticks, for a person to read and run.
pub fn shown_commands(commands: &[String]) -> String {
    commands
        .iter()
        .map(|command| format!("`{command}`"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The value of the first line `<key>=<value>` in <text>, which a script
/// run on a host printed, without the spaces around it.
pub fn answer_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(str::trim)
}

/// <text> as one word for a POSIX shell.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The command a person runs to run <script> on the host whose ssh target
/// is <target>: over ssh, with the script quoted, or as it is on this Mac
/// (no target).
pub fn person_command(target: Option<&str>, script: &str) -> String {
    match target {
        Some(target) => format!("ssh {target} {}", sh_quote(script)),
        None => script.to_string(),
    }
}

/// `host m1`, or `this Mac` for this Mac: how a line names host <name>.
pub fn place(name: &str) -> String {
    if name == LOCAL {
        "this Mac".to_string()
    } else {
        format!("host {name}")
    }
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
    /// The 5-minute load that health aims to keep the host in
    /// (`builds host band`); None for a host that keeps the shares of its
    /// cores.
    pub band: Option<Band>,
}

impl Host {
    /// This Mac, with the cores it has.
    pub fn local() -> Host {
        Host {
            name: LOCAL.to_string(),
            target: None,
            cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            speed: Some(1.0),
            band: None,
        }
    }

    /// The host's band, or the shares of <cores> when it has none.
    pub fn band_for(&self, cores: u32) -> Band {
        self.band.unwrap_or_else(|| Band::of_cores(cores))
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
    /// The host's OS and processor.
    pub platform: crate::host_platform::Platform,
    pub cores: u32,
    /// The 1, 5 and 15-minute load averages.
    pub load: [f64; 3],
    pub free_kib: u64,
    pub memory: Memory,
    /// What git printed when it failed on the host, in the shell that a job's
    /// push reaches: every job runs git there, so the host refuses every job.
    /// None when git runs.
    pub git_fails: Option<String>,
    /// What the host has of cargo-nextest and of cargo-mutants
    /// (`cargo_program::NEXTEST.version` and `MUTANTS.version` are the
    /// versions builds installs).
    pub nextest: crate::cargo_program::Has,
    pub mutants: crate::cargo_program::Has,
    /// The user's processes and the limit on them (proc_limit.rs).
    pub processes: crate::proc_limit::ProcessUse,
}

/// A host's cores and its one-minute load average at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Load {
    pub cores: u32,
    pub one_minute: f64,
}

impl Load {
    /// This Mac's cores and load now.
    pub fn here() -> Load {
        Load {
            cores: crate::procs::cores() as u32,
            one_minute: crate::procs::load_average()[0],
        }
    }

    /// The load that a store row's `cores` and `load` columns hold, when the
    /// row has both.
    pub fn from_columns(cores: Option<i64>, one_minute: Option<f64>) -> Option<Load> {
        cores.zip(one_minute).map(|(cores, one_minute)| Load {
            cores: cores as u32,
            one_minute,
        })
    }

    /// `load 21/14`: the load average, rounded, over the cores.
    pub fn text(&self) -> String {
        format!("load {:.0}/{}", self.one_minute, self.cores)
    }
}

impl Probe {
    /// The host's cores and one-minute load when it answered.
    pub fn load(&self) -> Load {
        Load {
            cores: self.cores,
            one_minute: self.load[0],
        }
    }

    pub fn load_per_core(&self) -> f64 {
        self.load[0] / f64::from(self.cores.max(1))
    }

    /// The cores nothing is using: the cores minus the one-minute load
    /// average, below zero for a host with more work than cores.
    pub fn room(&self) -> f64 {
        f64::from(self.cores) - self.load[0]
    }

    /// `room`, less the load of <runs> builds that started too recently
    /// for the load average to show them (cargo_starts.rs): cargo runs one
    /// compile job per core, so each counts as the host's cores.
    pub fn room_after(&self, runs: u32) -> f64 {
        self.room() - f64::from(runs) * f64::from(self.cores)
    }
}

/// Runs on the host to print what `parse_probe` reads, after the lines that
/// name its platform (`host_platform::SCRIPT`), which set `os`: macOS's
/// sysctl names the cores and load, and Linux's nproc and /proc/loadavg do.
/// git runs in the shell ssh starts, with no PATH of builds' own, as the
/// `git-receive-pack` that a job's push starts there does.
const PROBE_SCRIPT: &str = r#"case "$os" in
Darwin)
  echo cores=$(sysctl -n hw.ncpu)
  echo load=$(sysctl -n vm.loadavg | tr -d '{}')
  ;;
*)
  echo cores=$(nproc)
  echo load=$(cut -d ' ' -f 1-3 /proc/loadavg)
  ;;
esac
echo free_kib=$(df -Pk "$HOME" | awk 'NR==2{print $4}')
if ! said=$(git --version 2>&1); then echo "git_fails=$(printf '%s' "$said" | tr '\n' ' ')"; fi
"#;

/// The script a remote host runs: the platform's lines, `PROBE_SCRIPT`, then
/// the memory script, then the lines of each program builds installs and the
/// process lines.
fn probe_script() -> String {
    format!(
        "{}\n{PROBE_SCRIPT}{}\n{}\n{}\n{}",
        crate::host_platform::SCRIPT,
        crate::host_memory::SCRIPT,
        crate::cargo_program::NEXTEST.probe_lines(),
        crate::cargo_program::MUTANTS.probe_lines(),
        crate::proc_limit::PROBE_LINES
    )
}

/// Reads what `probe_script` printed.
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
        platform: crate::host_platform::Platform::parse(text)?,
        cores,
        load,
        free_kib,
        memory: Memory::parse(text)?,
        git_fails,
        nextest: crate::cargo_program::NEXTEST.parse_has(text),
        mutants: crate::cargo_program::MUTANTS.parse_has(text),
        processes: crate::proc_limit::parse(text)?,
    })
}

/// Asks <target> over ssh for its cores, load and free disk. The error says
/// what ssh printed.
pub fn probe_target(ssh: &Ssh, target: &str) -> std::result::Result<Probe, String> {
    let output = ssh
        .command(target, &probe_script(), Terminal::None)
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
        platform: crate::host_platform::Platform::here(),
        cores: Host::local().cores,
        load: loads,
        free_kib: volume_space(&places.home)?.free_kib,
        memory: crate::host_memory::here()?,
        git_fails: None,
        nextest: crate::cargo_program::NEXTEST.has(&places.home),
        mutants: crate::cargo_program::MUTANTS.has(&places.home),
        processes: crate::proc_limit::local()?,
    })
}

/// The lines a host runs to say whether it can draw a window scene (a shot
/// of a screen): `lid=closed` with `screens=<n>`, how many displays
/// `system_profiler` lists with a resolution, `lid=open`, or `lid=none` for a
/// Mac with no lid (`ioreg` lists no clamshell state). A Mac whose lid is
/// closed makes no virtual display for the shot renderer unless another
/// display is attached. A command that fails fails the script.
pub const SCREEN_SCRIPT: &str = r#"lid=$(ioreg -r -k AppleClamshellState -d 1) || exit 1
case "$lid" in
*'"AppleClamshellState" = Yes'*)
    displays=$(system_profiler SPDisplaysDataType) || exit 1
    echo lid=closed
    echo "screens=$(printf '%s\n' "$displays" | grep -c 'Resolution:')";;
*'"AppleClamshellState" = No'*) echo lid=open;;
*) echo lid=none;;
esac"#;

/// Why the host that printed <text> for `SCREEN_SCRIPT` cannot draw a window
/// scene: its lid is closed and it lists no display. None when it can, and
/// an error when <text> is no answer of the script's.
pub fn screen_lacking(text: &str) -> std::result::Result<Option<String>, String> {
    let line = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .map(str::trim)
    };
    match line("lid=") {
        Some("open" | "none") => Ok(None),
        Some("closed") => {
            let screens = line("screens=").and_then(|count| count.parse::<u32>().ok());
            match screens {
                Some(0) => Ok(Some(
                    "its lid is closed and it lists no other display".to_string(),
                )),
                Some(_) => Ok(None),
                None => Err(format!(
                    "its answer to the lid script has no screens count: {text:?}"
                )),
            }
        }
        _ => Err(format!(
            "its answer to the lid script names no lid: {text:?}"
        )),
    }
}

/// Asks <host> whether it can draw a window scene: None when it can, or the
/// reason it cannot (`screen_lacking`), and what went wrong when it cannot
/// say.
pub fn screen_lacks(ssh: &Ssh, host: &Host) -> std::result::Result<Option<String>, String> {
    let what = format!("asking host {} about its displays", host.name);
    let said = output_of(
        command_at(ssh, host, SCREEN_SCRIPT, Terminal::None),
        &what,
        Accept::Zero,
    )?;
    screen_lacking(&said).map_err(|why| format!("{what}: {why}"))
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
    Xcode,
    /// Mac app builds: its UI tests timed out turning on automation mode,
    /// which asks for a password there (ui_test_settings.rs).
    UiAutomation,
}

impl Refused {
    const ALL: [Refused; 3] = [Refused::Jobs, Refused::Xcode, Refused::UiAutomation];

    /// The folder of its records, beside the hosts file.
    fn dir(self, places: &Places) -> PathBuf {
        hosts_file(places).with_file_name(match self {
            Refused::Jobs => "host-refusals",
            Refused::Xcode => "host-xcode-failures",
            Refused::UiAutomation => "host-ui-automation-failures",
        })
    }

    /// Why a host recorded this way refuses mac app builds, or None for a
    /// kind that leaves mac app builds alone.
    pub fn mac_builds_cause(self) -> Option<&'static str> {
        match self {
            Refused::Jobs => None,
            Refused::Xcode => Some("Xcode cannot start"),
            Refused::UiAutomation => Some("UI tests time out turning on automation mode"),
        }
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

/// The records of hosts that refuse mac app builds, of every kind that
/// refuses them (`Refused::mac_builds_cause`).
pub fn refusing_mac_builds(places: &Places) -> Result<Vec<Refusal>> {
    Ok(refusals(places)?
        .into_iter()
        .filter(|refusal| refusal.refused.mac_builds_cause().is_some())
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

/// The line for <refusal> under "needs action". For a host whose UI tests
/// timed out, this line says nothing about the host's settings, so `builds
/// health` prints the line that `ui_test_settings::findings` writes with
/// them instead.
pub fn refusal_finding(refusal: &Refusal) -> crate::health::Advice {
    let name = &refusal.host;
    let at = local_minute(refusal.at);
    let said = refusal.said.trim_end_matches('.');
    match refusal.refused {
        Refused::Jobs => crate::health::Advice::run(
            format!(
                "host {name} refuses jobs (since {at}), so builds runs its jobs on this Mac. {name} said: {said}. Fix what {name} said on {name}. builds uses {name} again once a probe finds it answering with git running. Each builds cargo that may leave this Mac probes it; to probe every host now, run"
            ),
            "builds host list".to_string(),
        ),
        Refused::Xcode => {
            let fix = match crate::xcode::fix_named_in(&refusal.said) {
                Some(fix) => format!("Run sudo {fix} on {name}"),
                None => format!("Fix what xcodebuild said on {name}"),
            };
            crate::health::Advice::run(
                format!(
                    "host {name}'s Xcode cannot start (since {at}), so mac app builds skip its app checkouts. xcodebuild said: {said}. {fix}, then tell builds to use {name} again with"
                ),
                format!("builds host clear-xcode {name}"),
            )
        }
        Refused::UiAutomation => crate::ui_test_settings::finding(name, Some(refusal), None),
    }
}

/// Why a host cannot take a job.
#[derive(Clone, Debug, PartialEq)]
pub enum PassedOver {
    /// The host did not answer its probe; <error> says why.
    Unreachable { host: String, error: String },
    /// The host has less free disk than the floor.
    UnderFloor {
        host: String,
        free_kib: u64,
        floor_kib: u64,
    },
    /// Git fails on the host, as <said>, so it refuses every job.
    GitFails { host: String, said: String },
    /// `builds host setup` has not measured the host's core speed.
    NoSpeed { host: String },
}

impl std::fmt::Display for PassedOver {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PassedOver::Unreachable { host, error } => {
                write!(out, "{host} is unreachable ({error})")
            }
            PassedOver::UnderFloor {
                host,
                free_kib,
                floor_kib,
            } => write!(
                out,
                "{host} has {} free, under the floor of {}",
                human(*free_kib),
                human(*floor_kib)
            ),
            PassedOver::GitFails { host, said } => {
                write!(out, "{host} refuses jobs (git fails there: {said})")
            }
            PassedOver::NoSpeed { host } => write!(
                out,
                "{host} has no measured core speed (`builds host setup {host}`)"
            ),
        }
    }
}

/// Why <probe> keeps <host> from taking a job, or None when it can take one:
/// it does not answer, its free disk is under <floor_kib>, or it refuses jobs.
pub fn passed_over(
    host: &Host,
    probe: &std::result::Result<Probe, String>,
    floor_kib: u64,
) -> Option<PassedOver> {
    let name = host.name.clone();
    match probe {
        Err(error) => Some(PassedOver::Unreachable {
            host: name,
            error: error.clone(),
        }),
        Ok(probe) if probe.free_kib < floor_kib => Some(PassedOver::UnderFloor {
            host: name,
            free_kib: probe.free_kib,
            floor_kib,
        }),
        Ok(Probe {
            git_fails: Some(said),
            ..
        }) => Some(PassedOver::GitFails {
            host: name,
            said: said.clone(),
        }),
        Ok(_) => None,
    }
}

/// The file the hosts are kept in, beside the lists folder.
pub fn hosts_file(places: &Places) -> PathBuf {
    places.lists.parent().unwrap_or(&places.lists).join("hosts")
}

/// What the hosts file holds: the recorded remote hosts, in the order they
/// were added, each with its band, and this Mac's band.
#[derive(Clone, Debug, PartialEq)]
pub struct HostsFile {
    pub remote: Vec<Host>,
    pub local_band: Option<Band>,
}

/// The recorded remote hosts, in the order they were added.
pub fn read_remote(places: &Places) -> Result<Vec<Host>> {
    Ok(read_file(places)?.remote)
}

/// Reads the hosts file; a file that is not there holds nothing.
pub fn read_file(places: &Places) -> Result<HostsFile> {
    let file = hosts_file(places);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HostsFile {
                remote: Vec::new(),
                local_band: None,
            });
        }
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    let mut hosts: Vec<Host> = Vec::new();
    let mut bands: Vec<(String, Band)> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if let [word, name, low, high] = fields[..]
            && word == crate::host_band::LINE_WORD
        {
            let band = match Band::parse(low, high) {
                Ok(band) => band,
                Err(fault) => fail!(
                    "{} has a band that is not valid ({}): {line}",
                    file.display(),
                    fault.text()
                ),
            };
            bands.push((name.to_string(), band));
            continue;
        }
        let (name, target, cores, speed) = match fields[..] {
            [name, target, cores] => (name, target, cores, None),
            [name, target, cores, speed] => (name, target, cores, Some(speed)),
            _ => fail!(
                "{} has a line that is not `<name> <target> <cores> [<speed>]` or `band <name> <low> <high>`: {line}",
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
            band: None,
        });
    }
    let mut local_band = None;
    for (name, band) in bands {
        if name == LOCAL {
            local_band = Some(band);
        } else if let Some(host) = hosts.iter_mut().find(|host| host.name == name) {
            host.band = Some(band);
        } else {
            fail!(
                "{} has a band for {name}, which is not a recorded host",
                file.display()
            );
        }
    }
    Ok(HostsFile {
        remote: hosts,
        local_band,
    })
}

fn write_file(places: &Places, hosts: &HostsFile) -> Result<()> {
    let file = hosts_file(places);
    let dir = file.parent().expect("a file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let mut text: String = hosts
        .remote
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
    let bands = std::iter::once((LOCAL, hosts.local_band)).chain(
        hosts
            .remote
            .iter()
            .map(|host| (host.name.as_str(), host.band)),
    );
    for (name, band) in bands {
        if let Some(band) = band {
            text.push_str(&band.line(name));
        }
    }
    crate::whole_file::write(&file, text)
}

/// Records <speed> as host <name>'s core speed against this Mac's.
pub fn record_speed(places: &Places, name: &str, speed: f64) -> Result<()> {
    let mut file = read_file(places)?;
    let Some(recorded) = file
        .remote
        .iter_mut()
        .find(|recorded| recorded.name == name)
    else {
        fail!("host {name} is no longer recorded");
    };
    recorded.speed = Some(speed);
    write_file(places, &file)
}

/// Records <band> as host <name>'s load band, or removes the band when
/// <band> is None. <name> is `local` for this Mac or a recorded host.
pub fn set_band(places: &Places, name: &str, band: Option<Band>) -> Result<()> {
    let mut file = read_file(places)?;
    if name == LOCAL {
        file.local_band = band;
    } else if let Some(host) = file.remote.iter_mut().find(|host| host.name == name) {
        host.band = band;
    } else {
        fail!("no host named {name} (`builds host list`)");
    }
    write_file(places, &file)
}

/// This Mac with the load band `builds host band local` set, if any.
pub fn local_host(places: &Places) -> Result<Host> {
    let mut local = Host::local();
    local.band = read_file(places)?.local_band;
    Ok(local)
}

/// This Mac, then every recorded host.
pub fn all(places: &Places) -> Result<Vec<Host>> {
    let mut hosts = vec![local_host(places)?];
    hosts.extend(read_file(places)?.remote);
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
    if name == crate::host_band::LINE_WORD {
        fail!(
            "{name:?} starts the lines of the hosts file that hold load bands, so no host may take it"
        );
    }
    if target.is_empty() || target.starts_with('-') || target.contains(char::is_whitespace) {
        fail!("{target:?} is not an ssh target (user@machine)");
    }
    let mut file = read_file(places)?;
    if file.remote.iter().any(|host| host.name == name) {
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
        band: None,
    };
    file.remote.push(host.clone());
    write_file(places, &file)?;
    Ok(host)
}

pub fn remove(places: &Places, name: &str) -> Result<()> {
    let mut file = read_file(places)?;
    let Some(at) = file.remote.iter().position(|host| host.name == name) else {
        if name == LOCAL {
            fail!("{LOCAL} is this Mac, which is always a host");
        }
        fail!("no host named {name}");
    };
    file.remote.remove(at);
    write_file(places, &file)?;
    for refused in Refused::ALL {
        clear_refusal(places, refused, name)?;
    }
    Ok(())
}

/// One line of `builds host list`.
pub fn list_line(host: &Host, probe: &std::result::Result<Probe, String>) -> String {
    match probe {
        Ok(probe) => format!(
            "{}  {}  {}  {} cores  load {:.2} per core  load band {}  {}  free {}  {}",
            host.name,
            host.describe(),
            probe.platform.text(),
            probe.cores,
            probe.load_per_core(),
            host.band_for(probe.cores).text(),
            probe.memory.text(),
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

/// The host to run on: the reachable one above the free-disk floor, of this
/// machine's OS, with the lowest load average per core (the first of
/// equals), and the line that says why. A command `builds remote run` is
/// given is one for this machine's OS. Fails, naming each host and what is
/// wrong with it, when none fits.
pub fn choose(
    hosts: &[(Host, std::result::Result<Probe, String>)],
    floor_kib: u64,
) -> Result<(Host, String)> {
    let here = crate::host_platform::Os::here();
    let mut best: Option<(&Host, &Probe)> = None;
    let mut skipped = Vec::new();
    for (host, probe) in hosts {
        if let Some(why) = passed_over(host, probe, floor_kib) {
            skipped.push(why.to_string());
        } else if let Ok(probe) = probe
            && probe.platform.os != here
        {
            skipped.push(format!(
                "{} runs {}, not {}",
                host.name,
                probe.platform.os.word(),
                here.word()
            ));
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
            && other_probe.platform.os == here
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

/// Whether `builds cargo` may run a build on <host>, given its <probe>: its
/// probe and its measured core speed when it may, else why not. A host may
/// take one when `passed_over` finds nothing against it and `builds host
/// setup` measured its core speed.
pub fn takes_cargo<'a>(
    host: &Host,
    probe: &'a std::result::Result<Probe, String>,
    floor_kib: u64,
) -> std::result::Result<(&'a Probe, f64), PassedOver> {
    if let Some(why) = passed_over(host, probe, floor_kib) {
        return Err(why);
    }
    let Ok(probe) = probe else {
        unreachable!("passed_over passes over a host that did not answer");
    };
    match host.speed {
        Some(speed) => Ok((probe, speed)),
        None => Err(PassedOver::NoSpeed {
            host: host.name.clone(),
        }),
    }
}

/// What a `builds cargo` or `builds lake` command runs, as far as the hosts
/// that may take it are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// A subcommand that cargo itself provides.
    Cargo,
    /// `cargo nextest`, which needs cargo-nextest at its pinned version.
    Nextest,
    /// `cargo mutants`, which needs cargo-mutants at its pinned version.
    Mutants,
    /// `lake build`, which needs Lean's toolchains: the host's setup for the
    /// job installs them (lean.rs).
    Lake,
}

/// Why a host cannot take a run that needs <program>: it has no <program> at
/// its pinned version, and has <has>.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LacksProgram {
    pub host: String,
    pub program: crate::cargo_program::Program,
    pub has: crate::cargo_program::Has,
}

impl std::fmt::Display for LacksProgram {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::cargo_program::Has;
        let LacksProgram { host, program, has } = self;
        let (name, version) = (program.name, program.version);
        let fix = format!("`builds host setup {host}` installs {version}");
        match has {
            Has::Nothing => write!(out, "{host} has no {name} ({fix})"),
            Has::Version(has) => write!(out, "{host} has {name} {has}, not {version} ({fix})"),
            Has::Broken(said) => write!(out, "{host}'s {name} does not start: {said}"),
        }
    }
}

/// Why `builds cargo` cannot run <runs> on a host.
#[derive(Clone, Debug, PartialEq)]
pub enum CannotRun {
    /// The host takes no cargo job.
    PassedOver(PassedOver),
    /// The host takes cargo jobs but lacks what the run needs.
    LacksProgram(LacksProgram),
}

impl std::fmt::Display for CannotRun {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CannotRun::PassedOver(why) => why.fmt(out),
            CannotRun::LacksProgram(why) => why.fmt(out),
        }
    }
}

/// Whether `builds cargo` may run <runs> on <host>, given its <probe>: what
/// `takes_cargo` returns when the host may take cargo builds, and for a
/// nextest or mutants run also has that program at its pinned version.
pub fn takes_run<'a>(
    host: &Host,
    probe: &'a std::result::Result<Probe, String>,
    floor_kib: u64,
    runs: Runs,
) -> std::result::Result<(&'a Probe, f64), CannotRun> {
    let (probe, speed) = takes_cargo(host, probe, floor_kib).map_err(CannotRun::PassedOver)?;
    has_what_runs_needs(host, probe, runs).map_err(CannotRun::LacksProgram)?;
    Ok((probe, speed))
}

/// Whether <host>, which answered with <probe>, has the program <runs>
/// needs, at its pinned version: cargo-nextest for a nextest run and
/// cargo-mutants for a mutants run.
pub fn has_what_runs_needs(
    host: &Host,
    probe: &Probe,
    runs: Runs,
) -> std::result::Result<(), LacksProgram> {
    use crate::cargo_program::{Has, MUTANTS, NEXTEST};
    let (program, has) = match runs {
        Runs::Nextest => (NEXTEST, &probe.nextest),
        Runs::Mutants => (MUTANTS, &probe.mutants),
        Runs::Cargo | Runs::Lake => return Ok(()),
    };
    if *has != Has::Version(program.version.to_string()) {
        return Err(LacksProgram {
            host: host.name.clone(),
            program,
            has: has.clone(),
        });
    }
    Ok(())
}

/// The hosts a cargo build may run on, each with its probe, the one with the
/// most room first: the reachable hosts above the free-disk floor with a
/// measured core speed (and, for a nextest run, cargo-nextest: `takes_run`),
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
    runs: Runs,
) -> (Vec<(Host, Probe)>, String) {
    let mut skipped = Vec::new();
    let mut fits: Vec<(&Host, &Probe, f64)> = Vec::new();
    for (host, probe) in hosts {
        match takes_run(host, probe, floor_kib, runs) {
            Ok((probe, speed)) => fits.push((host, probe, speed)),
            Err(why) => skipped.push(why.to_string()),
        }
    }
    let runs = |host: &Host| recent.get(&host.name).copied().unwrap_or(0);
    // A host's free cores, each counted at its speed against this Mac's: a
    // job finishes first where this is largest. For a host with more work
    // than cores, the ranking divides the excess by the host's speed
    // instead, since a slower host takes longer to work through it.
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
                "worth {:.1} of this Mac's cores at {:.2} times its speed",
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
                    crate::text::counted(started as usize, "build", "builds")
                ),
            }
        })
        .collect();
    why.extend(skipped);
    (
        fits.into_iter()
            .map(|(host, probe, _)| (host.clone(), probe.clone()))
            .collect(),
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

const USAGE: &str = "usage: builds host add <name> <ssh target> | setup <name> | setup-here | speed-here | band <name> <low> <high> | band <name> clear | list | remove <name> | clear-xcode <name>";

/// The build volume, where a host that has one keeps its builds root, as this
/// Mac does.
pub const VOLUME: &str = "/Volumes/Builds";

/// The builds root on a Mac host without a build volume, relative to the
/// host's home: each worktree's build folders live in `<root>/<repo>/<name>`
/// there, as a side worktree's live under /Volumes/Builds here. Spotlight
/// skips a folder whose name ends in `.noindex` (spotlight.rs).
pub const REMOTE_ROOT: &str = "builds-root.noindex";

/// The builds root on a Linux host, relative to the host's home.
pub const LINUX_ROOT: &str = "builds-root";

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
        [word, name] if word == "setup" => crate::host_setup::cmd_setup(tool, ssh, name),
        [word] if word == "setup-here" => {
            tool.places.mark_host()?;
            crate::cargo_program::install_all(&tool.places.home)?;
            crate::install::run_sccache_server(&tool.places.home, tool.places.uid)
        }
        [word] if word == "speed-here" => crate::host_setup::cmd_speed_here(),
        [word, rest @ ..] if word == "band" => crate::host_band::cmd_band(tool, rest),
        [word, name] if word == "clear-xcode" => {
            if !all(&tool.places)?.iter().any(|host| host.name == *name) {
                fail!("no host named {name} (`builds host list`)");
            }
            clear_refusal(&tool.places, Refused::Xcode, name)?;
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
