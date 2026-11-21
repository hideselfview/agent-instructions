//! The service manager that runs the shared sccache server on each host:
//! launchd on macOS, the user's systemd on Linux. Each starts the server at
//! login (systemd at boot, for a user whose services linger), starts it again
//! when it exits, and gives it no idle timeout, so no client has a reason to
//! start a server of its own (install.rs `run_sccache_server`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::host_platform::Os;
use crate::sccache::Job;

/// The name of the server's job: launchd's label, and the stem of systemd's
/// unit file.
pub const LABEL: &str = "local.sccache-server";

/// systemd's name for the server's unit.
fn unit_name() -> String {
    format!("{LABEL}.service")
}

/// The service manager of OS <os>.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manager(pub Os);

impl Manager {
    /// This machine's.
    pub fn here() -> Manager {
        Manager(Os::here())
    }

    /// `launchd` or `systemd`.
    pub fn word(self) -> &'static str {
        match self.0 {
            Os::Mac => "launchd",
            Os::Linux => "systemd",
        }
    }

    /// The file that defines the server's job, relative to the home folder.
    pub fn job_file(self) -> String {
        match self.0 {
            Os::Mac => format!("Library/LaunchAgents/{LABEL}.plist"),
            Os::Linux => format!(".config/systemd/user/{}", unit_name()),
        }
    }

    /// The file the server writes its errors to, relative to the home folder.
    pub fn log_file(self) -> &'static str {
        match self.0 {
            Os::Mac => "Library/Logs/sccache-server.log",
            Os::Linux => ".cache/sccache-server.log",
        }
    }

    /// What the job's file holds for the server at <sccache>, in <home>,
    /// whose scratch files go to <tmp>, a folder of the build volume.
    pub fn job_text(self, sccache: &Path, home: &Path, tmp: &Path) -> String {
        let log = home.join(self.log_file());
        match self.0 {
            Os::Mac => plist(sccache, &log, tmp),
            Os::Linux => unit(sccache, &log, tmp),
        }
    }

    /// What the manager says about the server's job, or None when it has
    /// not loaded the job. <uid> is the user's.
    pub fn read_job(self, uid: u32) -> Result<Option<Job>> {
        match self.0 {
            Os::Mac => crate::sccache::read_job(&format!("gui/{uid}/{LABEL}")),
            Os::Linux => {
                let output = Command::new("systemctl")
                    .args(["--user", "show", &unit_name()])
                    .args(SYSTEMD_PROPERTIES.iter().flat_map(|name| ["-p", name]))
                    .stderr(Stdio::null())
                    .output()
                    .context(|| "running systemctl".to_string())?;
                if !output.status.success() {
                    fail!(
                        "systemctl --user show {} failed ({})",
                        unit_name(),
                        output.status
                    );
                }
                parse_systemd_job(&String::from_utf8_lossy(&output.stdout))
            }
        }
    }

    /// Stops the server's job, which the manager has loaded.
    pub fn stop(self, uid: u32) -> Result<()> {
        match self.0 {
            Os::Mac => run("launchctl", &["bootout", &format!("gui/{uid}/{LABEL}")]),
            Os::Linux => run("systemctl", &["--user", "stop", &unit_name()]),
        }
    }

    /// Loads the job from its file and starts it.
    pub fn start(self, uid: u32, job_file: &Path) -> Result<()> {
        match self.0 {
            Os::Mac => run(
                "launchctl",
                &[
                    "bootstrap",
                    &format!("gui/{uid}"),
                    &job_file.to_string_lossy(),
                ],
            ),
            Os::Linux => {
                run("systemctl", &["--user", "daemon-reload"])?;
                run("systemctl", &["--user", "enable", &unit_name()])?;
                run("systemctl", &["--user", "restart", &unit_name()])
            }
        }
    }

    /// A shell script, for a host of this manager's OS, that prints `job=none`
    /// when the manager has not loaded the server's job, else `job=loaded`
    /// and then what the manager says about it, which `parse_job` reads.
    pub fn job_script(self) -> String {
        match self.0 {
            Os::Mac => format!(
                r#"if job=$(launchctl print "gui/$(id -u)/{LABEL}" 2>/dev/null); then echo job=loaded; printf '%s\n' "$job"; else echo job=none; fi"#
            ),
            Os::Linux => format!(
                r#"if job=$(systemctl --user show {unit} {properties} 2>&1); then case "$job" in *LoadState=not-found*) echo job=none;; *) echo job=loaded; printf '%s\n' "$job";; esac; else echo job=unread; printf '%s\n' "$job" | tr '\n' ' '; echo; fi"#,
                unit = unit_name(),
                properties = SYSTEMD_PROPERTIES
                    .iter()
                    .map(|name| format!("-p {name}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }

    /// Reads what `job_script` printed after `job=loaded`.
    pub fn parse_job(self, text: &str) -> Result<Job> {
        match self.0 {
            Os::Mac => crate::sccache::parse_job(text),
            Os::Linux => match parse_systemd_job(text)? {
                Some(job) => Ok(job),
                None => fail!("systemd has not loaded {}: {text:?}", unit_name()),
            },
        }
    }
}

/// The properties of the server's unit that builds reads from `systemctl
/// show`.
const SYSTEMD_PROPERTIES: [&str; 4] = ["LoadState", "SubState", "MainPID", "ExecMainStatus"];

/// Reads what `systemctl --user show -p ...` printed for the server's unit:
/// None when systemd has no such unit, else its sub-state (`running` while
/// the server runs), its main process and the status that process last
/// exited with.
pub fn parse_systemd_job(text: &str) -> Result<Option<Job>> {
    let field = |key: &str| crate::host::answer_value(text, key);
    let Some(load) = field("LoadState") else {
        fail!("systemctl show printed no LoadState for the unit: {text:?}");
    };
    if load == "not-found" {
        return Ok(None);
    }
    let Some(state) = field("SubState") else {
        fail!("systemctl show printed no SubState for the unit: {text:?}");
    };
    let pid = match field("MainPID") {
        None | Some("0") => None,
        Some(pid) => match pid.parse() {
            Ok(pid) => Some(pid),
            Err(_) => fail!("systemctl show printed a MainPID that is not a number: {pid:?}"),
        },
    };
    Ok(Some(Job {
        state: state.to_string(),
        pid,
        last_exit: field("ExecMainStatus").map(str::to_string),
    }))
}

/// Runs <program> <args>, and fails with what it said when it fails.
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .context(|| format!("running {program}"))?;
    if !output.status.success() {
        fail!(
            "{program} {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&[output.stdout, output.stderr].concat()).trim()
        );
    }
    Ok(())
}

/// launchd's job for the server at <sccache>, writing its errors to <log> and
/// its scratch files to <tmp>.
fn plist(sccache: &Path, log: &Path, tmp: &Path) -> String {
    // One server on sccache's default TCP port, started with a TMPDIR of its
    // own on the build volume, where fseventsd keeps no history of its
    // writes: a server inherits the TMPDIR of whichever client starts it, and
    // a run's TMPDIR, deleted when the run ends, fails every compile after.
    // The job makes the folder first and fails when it cannot, as when the
    // volume is not mounted yet at login; KeepAlive starts it again. launchd
    // starts it at login, restarts it if it exits, and it never idles out, so
    // no client has a reason to start one of its own. SCCACHE_START_SERVER=1
    // makes this process the server and SCCACHE_NO_DAEMON=1 keeps it from
    // forking, so it is the process launchd watches. launchd starts a job at
    // 256 open files; the server holds every source file it hashes open, and
    // a crate like aws-sdk-s3 runs out at that, so the job raises its own
    // limit.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by builds install. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>mkdir -p "{tmp}" &amp;&amp; TMPDIR="{tmp}" exec "{sccache}"</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>SCCACHE_START_SERVER</key>
    <string>1</string>
    <key>SCCACHE_NO_DAEMON</key>
    <string>1</string>
    <key>SCCACHE_IDLE_TIMEOUT</key>
    <string>0</string>
    <key>SCCACHE_ERROR_LOG</key>
    <string>{log}</string>
  </dict>
  <key>SoftResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key>
    <integer>65536</integer>
  </dict>
  <key>HardResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key>
    <integer>65536</integer>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#,
        sccache = sccache.display(),
        log = log.display(),
        tmp = tmp.display()
    )
}

/// systemd's unit for the server at <sccache>, writing its errors to <log>
/// and its scratch files to <tmp>: the same server as launchd's job
/// (`plist`).
fn unit(sccache: &Path, log: &Path, tmp: &Path) -> String {
    format!(
        "# Written by builds install.\n[Unit]\nDescription=The sccache server every build on this host uses\n\n[Service]\nExecStartPre=/bin/mkdir -p {tmp}\nExecStart={sccache}\nEnvironment=TMPDIR={tmp}\nEnvironment=SCCACHE_START_SERVER=1\nEnvironment=SCCACHE_NO_DAEMON=1\nEnvironment=SCCACHE_IDLE_TIMEOUT=0\nEnvironment=SCCACHE_ERROR_LOG={log}\nLimitNOFILE=65536\nRestart=always\n\n[Install]\nWantedBy=default.target\n",
        sccache = sccache.display(),
        log = log.display(),
        tmp = tmp.display()
    )
}

/// The job file in <home> on this machine.
pub fn job_path(home: &Path) -> PathBuf {
    home.join(Manager::here().job_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launchd job runs the server with TMPDIR on the build volume: the
    /// shell command it names makes the folder and starts the server there.
    #[test]
    fn the_launchd_job_starts_the_server_with_a_tmpdir_on_the_build_volume() {
        let dir = PathBuf::from(format!("/private/tmp/sccache-job-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (volume, tmp) = (dir.join("volume"), dir.join("volume/.sccache-tmp"));
        std::fs::create_dir_all(&volume).unwrap();
        let record = dir.join("record");
        let server = dir.join("sccache");
        std::fs::write(
            &server,
            format!("#!/bin/sh\necho \"$TMPDIR\" > {}\n", record.display()),
        )
        .unwrap();
        std::fs::set_permissions(&server, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let text = Manager(Os::Mac).job_text(&server, &dir, &tmp);
        let (_, after) = text.split_once("<string>-c</string>").unwrap();
        let command = after
            .split_once("<string>")
            .and_then(|(_, rest)| rest.split_once("</string>"))
            .unwrap()
            .0
            .replace("&amp;", "&");
        let ran = Command::new("/bin/sh")
            .args(["-c", &command])
            .env("TMPDIR", "/private/tmp")
            .status()
            .unwrap();
        assert!(ran.success());
        assert_eq!(
            std::fs::read_to_string(&record).unwrap().trim(),
            tmp.display().to_string()
        );
        assert!(tmp.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The systemd unit gives the server the same folder.
    #[test]
    fn the_systemd_unit_gives_the_server_a_tmpdir_on_the_build_volume() {
        let text = Manager(Os::Linux).job_text(
            Path::new("/usr/bin/sccache"),
            Path::new("/home/u"),
            Path::new("/home/u/builds-root/.sccache-tmp"),
        );
        assert!(text.contains("Environment=TMPDIR=/home/u/builds-root/.sccache-tmp\n"));
        assert!(text.contains("ExecStartPre=/bin/mkdir -p /home/u/builds-root/.sccache-tmp\n"));
    }

    /// What `systemctl --user show` prints for the unit while it runs, after
    /// its server exited, and when systemd has no such unit.
    #[test]
    fn reads_what_systemd_says_about_the_servers_unit() {
        let running = parse_systemd_job(
            "LoadState=loaded\nSubState=running\nMainPID=812\nExecMainStatus=0\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(running.running_pid(), Some(812));
        let restarting = parse_systemd_job(
            "LoadState=loaded\nSubState=auto-restart\nMainPID=0\nExecMainStatus=2\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(restarting.running_pid(), None);
        assert_eq!(
            restarting.text(),
            "state = auto-restart, last exit code = 2"
        );
        assert_eq!(
            parse_systemd_job("LoadState=not-found\nSubState=dead\nMainPID=0\n").unwrap(),
            None
        );
        assert!(parse_systemd_job("SubState=running\n").is_err());
        assert!(parse_systemd_job("LoadState=loaded\nSubState=running\nMainPID=x\n").is_err());
    }
}
