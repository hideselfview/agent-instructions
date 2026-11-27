//! The programs builds runs on a machine's OS, in one table (`Program`), and
//! where builds finds them (`System`): the machine's own, or a test's
//! stand-in folder, where each name resolves to `<folder>/<name>`.
//!
//! The table holds each program whose effect or answer reaches past the files
//! builds names: the service managers, power, displays, settings, the screen's
//! apps, Xcode, other processes, sudo, other machines and the agent CLIs. A
//! test that ran one of the machine's own would act on the person's Mac. The
//! POSIX tools, git and the build tools (sh, ps, df, sysctl, git, tar, cargo,
//! sccache) only read files and counts or act on paths builds names, and
//! builds runs them by name.
//!
//! main reads `VARIABLE` once, and every builds process a command starts
//! inherits it. Under a stand-in folder, builds refuses to run a name the
//! folder lacks, and names it, and the scripts builds runs on a host
//! (`script`) run each name in the table from the folder too: a test's
//! stand-in ssh runs them on this Mac.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fail;
use crate::fail::Result;

/// The variable that names a stand-in folder; builds' tests set it.
pub const VARIABLE: &str = "BUILDS_SYSTEM";

/// A program of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    /// Reaches the other hosts (host.rs `Ssh`).
    Ssh,
    /// Claude Code: the agents `builds agent run` starts, and a reviewer.
    Claude,
    /// The Codex CLI: thread notifications, and a reviewer.
    Codex,
    /// launchd, which runs the sccache server on a Mac (sccache_service.rs).
    Launchctl,
    /// The user's systemd, which runs it on Linux.
    Systemctl,
    /// The registry that says whether a Mac's screen is locked and its lid
    /// closed (host_screen_lock.rs, host.rs `SCREEN_SCRIPT`).
    Ioreg,
    /// The displays a Mac lists (host.rs `SCREEN_SCRIPT`).
    SystemProfiler,
    /// The log of a Mac's sleeps (sleeps.rs).
    Pmset,
    /// Holds a host's display awake while a step opens windows
    /// (app_remote.rs `keeping_display_on`).
    Caffeinate,
    /// Xcode's settings (install.rs, status.rs).
    Defaults,
    /// The apps on a Mac's screen (build_app.rs).
    Osascript,
    /// Xcode's version (xcode_cache.rs, app_platform.rs).
    Xcodebuild,
    /// The simulators (idle_daemons.rs).
    Xcrun,
    /// The files and ports other processes hold open.
    Lsof,
    /// Another process's children (sccache.rs `compiles`).
    Pgrep,
    /// What `builds host setup` turns on as root.
    Sudo,
}

impl Program {
    pub const ALL: [Program; 16] = [
        Program::Ssh,
        Program::Claude,
        Program::Codex,
        Program::Launchctl,
        Program::Systemctl,
        Program::Ioreg,
        Program::SystemProfiler,
        Program::Pmset,
        Program::Caffeinate,
        Program::Defaults,
        Program::Osascript,
        Program::Xcodebuild,
        Program::Xcrun,
        Program::Lsof,
        Program::Pgrep,
        Program::Sudo,
    ];

    /// The name the OS gives it, which a stand-in folder's file has too.
    pub fn name(self) -> &'static str {
        match self {
            Program::Ssh => "ssh",
            Program::Claude => "claude",
            Program::Codex => "codex",
            Program::Launchctl => "launchctl",
            Program::Systemctl => "systemctl",
            Program::Ioreg => "ioreg",
            Program::SystemProfiler => "system_profiler",
            Program::Pmset => "pmset",
            Program::Caffeinate => "caffeinate",
            Program::Defaults => "defaults",
            Program::Osascript => "osascript",
            Program::Xcodebuild => "xcodebuild",
            Program::Xcrun => "xcrun",
            Program::Lsof => "lsof",
            Program::Pgrep => "pgrep",
            Program::Sudo => "sudo",
        }
    }
}

/// Where builds finds the programs of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum System {
    /// The machine's own, by name on PATH, and macOS's power assertions.
    Real,
    /// A test's folder: each program is `<folder>/<name>`, and builds writes
    /// each power assertion it takes and releases to `<folder>/power`
    /// (awake.rs `Power::Record`).
    StandIn(PathBuf),
}

impl System {
    /// The system `VARIABLE` names.
    pub fn from_env() -> Result<System> {
        System::from_setting(std::env::var_os(VARIABLE).as_deref())
    }

    /// The system that <setting>, the value of `VARIABLE`, names.
    pub fn from_setting(setting: Option<&OsStr>) -> Result<System> {
        match setting {
            None => Ok(System::Real),
            Some(folder) if folder.is_empty() => {
                fail!("Set {VARIABLE} to a folder of stand-in programs or leave it unset.")
            }
            Some(folder) => Ok(System::StandIn(PathBuf::from(folder))),
        }
    }

    /// Where <program> is: its name, found on PATH, or its file in the
    /// stand-in folder, which may lack it. A command that runs it refuses
    /// then (`path`), and a program that takes this as an argument, such as
    /// the reviewer (review/provider.rs), fails naming it.
    pub fn location(&self, program: Program) -> PathBuf {
        match self {
            System::Real => PathBuf::from(program.name()),
            System::StandIn(folder) => folder.join(program.name()),
        }
    }

    /// The path builds runs <program> at; under a stand-in folder that lacks
    /// it, a failure that names the program and the folder.
    pub fn path(&self, program: Program) -> Result<PathBuf> {
        let path = self.location(program);
        if let System::StandIn(folder) = self
            && !path.is_file()
        {
            fail!("{}", lacking(folder, program));
        }
        Ok(path)
    }

    /// A command that runs <program>.
    pub fn command(&self, program: Program) -> Result<Command> {
        Ok(Command::new(self.path(program)?))
    }

    /// <body> as a script for a host. Under a stand-in folder, the lines
    /// before it make each name of the table run the folder's program, or
    /// fail, naming the program and the folder, when the folder lacks it.
    pub fn script(&self, body: &str) -> String {
        let System::StandIn(folder) = self else {
            return body.to_string();
        };
        let mut script = String::new();
        for program in Program::ALL {
            let name = program.name();
            let path = folder.join(name);
            let run = if path.is_file() {
                format!("{} \"$@\"", crate::host::sh_quote(&path.to_string_lossy()))
            } else {
                format!(
                    "echo {} >&2; return 127",
                    crate::host::sh_quote(&lacking(folder, program))
                )
            };
            script.push_str(&format!("{name}() {{ {run}; }}\n"));
        }
        script.push_str(body);
        script
    }

    /// `sh -c <script>` on this Mac, with the table's programs (`script`).
    pub fn sh(&self, script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(self.script(script));
        command
    }

    /// Where builds takes the power assertions that keep this Mac awake.
    pub fn power(&self) -> crate::awake::Power {
        match self {
            System::Real => crate::awake::Power::Powerd,
            System::StandIn(folder) => crate::awake::Power::Record(folder.join("power")),
        }
    }

    /// How builds runs scripts on hosts with these programs.
    pub fn ssh(&self) -> crate::host::Ssh {
        crate::host::Ssh::of(self.clone())
    }
}

/// Why builds runs no <program> under the stand-in folder <folder>.
fn lacking(folder: &Path, program: Program) -> String {
    format!(
        "{VARIABLE} names the stand-in folder {}, which has no {}, so builds runs none",
        folder.display(),
        program.name()
    )
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
