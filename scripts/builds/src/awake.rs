//! Keeping each Mac awake while builds works on it or for it, with macOS's
//! power assertions as IOKit's IOPMLib.h describes them. This module takes
//! every assertion builds holds.
//!
//! A Mac that idle-sleeps does no work: agents, builds and reviews stop until
//! someone wakes it. So each builds process that runs work
//! (`Command::keeps_awake`)
//! holds PreventUserIdleSystemSleep from its start to its end, which keeps
//! the Mac from sleeping because nobody uses it, on battery as on power. A
//! command that runs on a host for another Mac has no builds process of its
//! own there, so the script that runs it starts one that holds the host's
//! assertion until the script ends (`hold_while`, remote.rs `run_script`).
//!
//! A Mac that sleeps also answers nothing on its connections. A host that
//! keeps sending a command's output and hears nothing back ends the
//! connection within about twenty seconds, which stops the command there: the
//! step or job fails with ssh's "Connection reset by peer" once this Mac
//! wakes. So while builds drives work on a host (a lane's app checkout there,
//! app_remote.rs, or a job of `builds remote run` and `builds cargo`,
//! remote.rs), it also holds NetworkClientActive, which on power keeps it
//! awake through a maintenance wake (a dark wake), where the first has no
//! effect. Neither keeps a Mac on battery awake once its lid closes or
//! through a maintenance wake; when such a sleep ends a connection, the
//! failure names it (sleeps.rs).
//!
//! macOS releases a process's assertions when the process ends, however it
//! ends. A Linux machine is only ever a host, and builds holds nothing there.

use std::path::PathBuf;

use crate::fail::Result;
use crate::host::Host;

/// The command line word that makes builds hold a host's assertion for
/// another Mac's command (`hold_while`).
pub const HOLD_COMMAND: &str = "__hold-awake";

/// What `hold_while` prints once it holds its assertion.
pub const HELD: &str = "held";

/// Where builds takes its assertions.
#[derive(Clone, Debug)]
pub enum Power {
    /// macOS's power management daemon.
    Powerd,
    /// A file of a stand-in system (system.rs), with a line `take <kind>
    /// <pid> <name>` for each assertion taken and `release <kind> <pid>
    /// <name>` for each released.
    Record(PathBuf),
}

/// Holds this Mac awake until dropped.
#[derive(Debug)]
pub struct KeepAwake {
    _held: Option<Assertion>,
}

impl KeepAwake {
    /// Keeps this Mac from idle sleep while builds does <what>. The
    /// assertion carries <what> as its name, which `pmset -g assertions`
    /// shows.
    pub fn while_working(power: &Power, what: &str) -> Result<KeepAwake> {
        Ok(KeepAwake {
            _held: Assertion::on_a_mac(power, "PreventUserIdleSystemSleep", what)?,
        })
    }

    /// Keeps this Mac answering its connection while builds does <what> on
    /// <host>; for this Mac's own work it holds nothing.
    pub fn for_host(power: &Power, host: &Host, what: &str) -> Result<KeepAwake> {
        let held = match host.target {
            Some(_) => Assertion::on_a_mac(
                power,
                "NetworkClientActive",
                &format!("{what} on host {}", host.name),
            )?,
            None => None,
        };
        Ok(KeepAwake { _held: held })
    }
}

/// Holds the assertion for <what> from now until process <pid> ends, and
/// prints `HELD` once it holds it.
pub fn hold_while(power: &Power, pid: i32, what: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;

        use crate::fail::Context;

        let queue = crate::kqueue::Queue::new()
            .context(|| "Builds could not open an event queue to keep this Mac awake".into())?;
        // Watched before the caller learns the assertion is held, so its end
        // is never missed.
        queue
            .watch_exit(pid)
            .context(|| format!("Builds could not watch process {pid} to keep this Mac awake"))?;
        let _awake = KeepAwake::while_working(power, what)?;
        let mut out = crate::output::answer_handle();
        writeln!(out, "{HELD}")
            .and_then(|()| out.flush())
            .context(|| "Builds could not say that it keeps this Mac awake".into())?;
        queue
            .wait(&mut [crate::kqueue::event(0, 0, 0, 0)], None)
            .context(|| format!("Builds could not wait for process {pid} to end"))?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (power, pid, what);
        crate::fail!("builds keeps only a Mac awake, and this machine runs Linux")
    }
}

/// One assertion this process holds; dropping it releases it.
#[derive(Debug)]
enum Assertion {
    #[cfg(target_os = "macos")]
    Powerd { _held: os::Assertion },
    #[cfg(target_os = "macos")]
    Recorded {
        record: PathBuf,
        /// The line that took it, past the word `take`.
        taken: String,
    },
}

impl Assertion {
    /// Takes an assertion of <kind> named for <what>; none on Linux.
    fn on_a_mac(power: &Power, kind: &str, what: &str) -> Result<Option<Assertion>> {
        let name = format!("builds: {what}");
        #[cfg(target_os = "macos")]
        {
            Ok(Some(match power {
                Power::Powerd => Assertion::Powerd {
                    _held: os::Assertion::take(kind, &name)?,
                },
                Power::Record(record) => {
                    // One line per assertion: a name may hold a command's
                    // argument with a newline.
                    let taken =
                        format!("{kind} {} {}", std::process::id(), name.replace('\n', " "));
                    note_in(record, &format!("take {taken}"))?;
                    Assertion::Recorded {
                        record: record.clone(),
                        taken,
                    }
                }
            }))
        }
        #[cfg(target_os = "linux")]
        {
            let _ = (power, kind, name);
            Ok(None)
        }
    }
}

/// Appends <line> to the record file <record>, in one write.
#[cfg(target_os = "macos")]
fn note_in(record: &std::path::Path, line: &str) -> Result<()> {
    use std::io::Write;

    use crate::fail::Context;

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record)
        .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()))
        .context(|| format!("writing {}", record.display()))
}

impl Drop for Assertion {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Assertion::Recorded { record, taken } = self
            && let Err(error) = note_in(record, &format!("release {taken}"))
        {
            crate::output::note(error.text());
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::CString;

    use crate::core_foundation::{CFStringRef, CfString};
    use crate::fail;
    use crate::fail::Result;

    /// kIOPMAssertionLevelOn.
    const ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// One power assertion this process holds; dropping it releases it.
    #[derive(Debug)]
    pub struct Assertion(u32);

    impl Assertion {
        /// Takes an assertion of <kind> named <name>; fails, naming the
        /// kind and IOKit's return code, when powerd refuses it.
        pub fn take(kind: &str, name: &str) -> Result<Assertion> {
            let kind_text = CfString::new(&CString::new(kind).expect("a kind holds no NUL"));
            let name_text = CfString::new(&CString::new(name).expect("a name holds no NUL"));
            let mut id = 0;
            let code = unsafe {
                IOPMAssertionCreateWithName(kind_text.as_ptr(), ON, name_text.as_ptr(), &mut id)
            };
            if code != 0 {
                fail!(
                    "macOS refused builds the power assertion {kind} ({name}): IOKit returned {code:#x}"
                );
            }
            Ok(Assertion(id))
        }
    }

    impl Drop for Assertion {
        /// Releases the assertion. A drop cannot fail its caller, so a
        /// release that IOKit refuses is said on stderr: the assertion then
        /// lasts until this process ends.
        fn drop(&mut self) {
            let code = unsafe { IOPMAssertionRelease(self.0) };
            if code != 0 {
                crate::output::note(format!(
                    "macOS refused to release builds' power assertion {}: IOKit returned {code:#x}, so this Mac stays awake until this process ends",
                    self.0
                ));
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "awake_tests.rs"]
mod tests;

impl crate::command::Command {
    /// Whether the command keeps this Mac awake while it runs (awake.rs): it
    /// runs work. A command that only reads, or only waits for a person or
    /// another process, runs none, and neither does the dashboard, which runs
    /// until it is stopped and starts each of its own runs as a builds
    /// process.
    pub fn keeps_awake(self) -> bool {
        use crate::command::Command::*;
        use crate::command::*;
        match self {
            Land
            | Health
            | Setup
            | New
            | AppClone
            | Prune
            | Install
            | Tool
            | Toolchain
            | Web(web::Picture)
            | Agent(agent::Run | agent::Resume)
            | Review(
                review::Run
                | review::Copy
                | review::Replay
                | review::Audit
                | review::CasePropose
                | review::CaseAdd,
            )
            | Checks(checks::Run)
            | Item(item::Take)
            | AppCheckout(app_checkout::Add | app_checkout::Remove | app_checkout::Reset)
            | Host(
                host::Add
                | host::Setup
                | host::SetupHere
                | host::SpeedHere
                | host::Band
                | host::List
                | host::Cache
                | host::Remove
                | host::ClearXcode,
            ) => true,
            Help
            | Ack
            | RulesOnEdit
            | Lake
            | Claim
            | Note
            | Release
            | Retire
            | Discard
            | Cargo
            | AppBuild(
                app_build::Build
                | app_build::LandingCoverage
                | app_build::DropShots
                | app_build::RecordHanded,
            )
            | BuildApp
            | Lock
            | Programs
            | Queue
            | Takeover
            | SccacheTmpdir
            | Du
            | Status
            | InstallHook
            | Waits
            | Brief
            | Version
            | Next
            | Agent(agent::Start | agent::List | agent::Show)
            | Shots(shots::Findings | shots::Picture | shots::CheckProgram | shots::Checker)
            | Remote(remote::Run | remote::Step)
            | Units(units::Prune | units::GitCheckouts | units::AfterBuild)
            | Dashboard(dashboard::Stage | dashboard::Serve)
            | Checks(checks::Whole | checks::Stats | checks::List | checks::Tests)
            | Review(
                review::Settle
                | review::Wait
                | review::Dispute
                | review::Keep
                | review::Show
                | review::Key
                | review::CheckLanding
                | review::Landed
                | review::Groups
                | review::Misses
                | review::Rates
                | review::Scores,
            )
            | Judge(judge::Listen)
            | Question(
                question::Ask
                | question::Answer
                | question::Settle
                | question::Done
                | question::Drop
                | question::Place
                | question::Edit
                | question::Show
                | question::List,
            )
            | Item(
                item::Add
                | item::Edit
                | item::Note
                | item::Changes
                | item::Wait
                | item::Backlog
                | item::Start
                | item::CheckBrief
                | item::Handoff
                | item::Stop
                | item::Finished
                | item::Drop
                | item::Place
                | item::CheckLanding
                | item::Landed
                | item::Show
                | item::List
                | item::Times,
            )
            | Area(area::Add | area::Rename | area::Move | area::Remove)
            | Decision(
                decision::Add | decision::Edit | decision::Find | decision::Show | decision::List,
            )
            | Design(
                design::Places
                | design::Only
                | design::Record
                | design::Amend
                | design::Approve
                | design::Check
                | design::Landed
                | design::Title
                | design::Drop
                | design::Show
                | design::List
                | design::Render
                | design::TokensCss
                | design::PublishFind
                | design::PublishFiles
                | design::PublishLast
                | design::Listen
                | design::PublishTake
                | design::PublishDone
                | design::PublishFailed,
            )
            | Lists(
                lists::Init | lists::DesignGlobs | lists::AddRepo | lists::Show | lists::ProjectOf,
            ) => self.makes_run_folder(),
        }
    }
}
