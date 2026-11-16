//! The gates that keep a builds program off a store of another schema.
//!
//! `builds install` moves each store to its program's schema and puts that
//! program on PATH. A builds program refuses a store of another schema
//! (db.rs), so a command that started before an install could not write a
//! store after the install moved it. The gates make install wait for each
//! such command first.
//!
//! A command uses the stores in one of three ways. A use is a locked file of
//! its own in its kind's folder that says which process runs what and since
//! when.
//! - A command that writes a store after long work, with what it measured
//!   since it started (main.rs's `holds_for_life` names them: a hook run and
//!   a review that asks reviewers), holds a command use for its life.
//! - An app build's lane holds a command use from the moment it holds the
//!   lane's lock until it ends (`hold_from_now`): before that it has done no
//!   work and written nothing, so a build waiting for the lane holds no use.
//!   When install ran while the lane waited, the lane leaves the queue and
//!   runs the installed builds in its place.
//! - Every other command takes a connection use while each store connection
//!   it opens is open. Its writes are each one short transaction, so a
//!   command such as `builds cargo test` holds no use while it builds.
//!
//! A command use and a connection use each have a gate: a file that a use
//! holds a shared lock on while it starts and that install locks
//! exclusively. Install closes the commands gate and waits for every command
//! use, then closes the connections gate and waits for every connection use,
//! then counts one more install in the generation file, moves the stores,
//! puts its program on PATH and opens both gates. So while install waits for
//! a long command, only a command that would hold a command use waits at the
//! gate; the others keep working.
//!
//! A command that finds a gate closed when it starts waits until the gate
//! opens, then runs the program at its own path in its place, since install
//! replaced that program. A connection that finds the gate closed waits too,
//! then opens the store; when install moved it to another schema, db.rs
//! refuses it and says to run the command again.
//!
//! A builds command that a command use's holder starts runs under the
//! holder's use (`BUILDS_USE` in its environment names the holder) and takes
//! none of its own: waiting at a gate, it would wait for install, which waits
//! for the holder, which waits for it. A holder that starts a builds command
//! it does not wait for removes `BUILDS_USE` from that command's
//! environment. For the same reason install refuses to run under a holder.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::flock::{lock, try_lock};
use crate::places::Places;

/// The variable that names the process whose use covers a builds command.
pub const VARIABLE: &str = "BUILDS_USE";

/// Runs the program at this program's path with this process's arguments in
/// its place; returns only when it cannot, with the error that says why.
pub fn run_again() -> Fail {
    use std::os::unix::process::CommandExt;
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => return Fail::Message(format!("finding this program: {error}")),
    };
    // The program takes its own use. A process that took a use for the rest
    // of its life (`hold_from_now`) names itself in VARIABLE, and the exec
    // closes the use's file, which ends that use.
    let error = std::process::Command::new(&program)
        .args(std::env::args_os().skip(1))
        .env_remove(VARIABLE)
        .exec();
    Fail::Message(format!("running {} again: {error}", program.display()))
}

/// The file that counts the installs that moved the stores.
fn generation_file(places: &Places) -> PathBuf {
    places.install_dir().join("generation")
}

/// How many installs moved the stores: 0 before the first.
fn generation(places: &Places) -> Result<u64> {
    let path = generation_file(places);
    match std::fs::read_to_string(&path) {
        Ok(text) => match text.trim().parse() {
            Ok(count) => Ok(count),
            Err(_) => fail!("{} holds {text:?}, not a count", path.display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// Counts one more install, in one rename. Only install calls it, with both
/// gates closed.
fn advance_generation(places: &Places) -> Result<()> {
    let next = generation(places)? + 1;
    crate::whole_file::write(&generation_file(places), format!("{next}\n"))
}

/// What a use spans.
#[derive(Clone, Copy)]
enum Kind {
    /// A command's life.
    Command,
    /// An open store connection.
    Connection,
}

impl Kind {
    fn gate(self, places: &Places) -> PathBuf {
        places.install_dir().join(match self {
            Kind::Command => "commands.gate",
            Kind::Connection => "connections.gate",
        })
    }

    fn uses(self, places: &Places) -> PathBuf {
        places.install_dir().join(match self {
            Kind::Command => "commands",
            Kind::Connection => "connections",
        })
    }
}

fn open(path: &Path) -> Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    }
    crate::flock::open_creating(path)
}

/// A use of the stores: its file, locked until dropped. The file stays;
/// install removes the file of each use that ended.
pub struct Use {
    _file: File,
}

/// Passes <kind>'s gate: returns its file, holding a shared lock, and
/// whether the gate was open. When it is closed, gives <say> the line
/// <waiting> and waits until it opens.
fn pass(
    places: &Places,
    kind: Kind,
    waiting: &str,
    say: &mut dyn FnMut(&str),
) -> Result<(File, bool)> {
    let path = kind.gate(places);
    let gate = open(&path)?;
    if try_lock(&gate, libc::LOCK_SH, &path)? {
        return Ok((gate, true));
    }
    say(waiting);
    lock(&gate, libc::LOCK_SH, &path)?;
    Ok((gate, false))
}

/// Takes a use of <kind> for process <pid>, which runs `builds <what>` since
/// the local time <since>, while the gate held open by <gate> stays open.
fn take(places: &Places, kind: Kind, gate: File, pid: u32, what: &str, since: &str) -> Result<Use> {
    let path = kind.uses(places).join(pid.to_string());
    let mut file = open(&path)?;
    lock(&file, libc::LOCK_EX, &path)?;
    file.set_len(0)
        .context(|| format!("writing {}", path.display()))?;
    file.write_all(format!("process {pid}, builds {what}, since {since}\n").as_bytes())
        .context(|| format!("writing {}", path.display()))?;
    // Closing the gate's file releases the shared lock; the use is in place.
    drop(gate);
    Ok(Use { _file: file })
}

const RUNS_AGAIN: &str =
    "builds install runs: this command waits until it finishes, then runs the installed builds";

/// How a command that holds a use for its life enters.
pub enum Entered {
    /// It holds this use for its life.
    Holds(Use),
    /// It waited while an install ran, and must run the installed program in
    /// its place.
    RunAgain,
}

/// Passes the commands gate and takes a use for the life of the command
/// <what> of process <pid>, started at the local time <since>; gives <say>
/// the line that says it waits at the closed gate.
pub fn enter_for_life(
    places: &Places,
    pid: u32,
    what: &str,
    since: &str,
    say: &mut dyn FnMut(&str),
) -> Result<Entered> {
    match pass(places, Kind::Command, RUNS_AGAIN, say)? {
        (gate, true) => Ok(Entered::Holds(take(
            places,
            Kind::Command,
            gate,
            pid,
            what,
            since,
        )?)),
        (_, false) => Ok(Entered::RunAgain),
    }
}

/// Whether a command that takes a use per connection may start: false when
/// it waited at the closed gate while an install ran, and must run the
/// installed program in its place. Gives <say> the line that says it waits.
pub fn may_start(places: &Places, say: &mut dyn FnMut(&str)) -> Result<bool> {
    Ok(pass(places, Kind::Connection, RUNS_AGAIN, say)?.1)
}

/// The connection uses of one process: the first open connection takes the
/// process's use, and the last one to close ends it, until the process holds
/// a use for the rest of its life (`hold_from_now`).
pub struct PerConnection {
    places: Places,
    pid: u32,
    what: String,
    since: String,
    /// The installs counted when this process started. Install counts one
    /// more before it moves the stores and replaces the program, so a count
    /// that differs says an install ran since.
    generation: u64,
    /// The use that the process's open connections share. It ends when the
    /// last of them closes and drops it.
    shared: Mutex<Weak<Use>>,
    held_for_life: AtomicBool,
}

impl PerConnection {
    pub fn new(places: Places, pid: u32, what: &str, since: &str) -> Result<PerConnection> {
        Ok(PerConnection {
            generation: generation(&places)?,
            places,
            pid,
            what: what.to_string(),
            since: since.to_string(),
            shared: Mutex::new(Weak::new()),
            held_for_life: AtomicBool::new(false),
        })
    }

    /// Takes a use for the rest of the process's life, waiting at the gate
    /// while an install runs. No connection of the process may be open:
    /// install waits for it before it opens the gate.
    pub fn hold_from_now(&self) -> Result<Use> {
        let (gate, _) = pass(
            &self.places,
            Kind::Command,
            "builds install runs: this command waits until it finishes",
            &mut |line| crate::text::note(line),
        )?;
        let taken = take(
            &self.places,
            Kind::Command,
            gate,
            self.pid,
            &self.what,
            &self.since,
        )?;
        self.held_for_life.store(true, Ordering::SeqCst);
        Ok(taken)
    }

    /// A use for one connection, waiting at the gate while an install runs;
    /// none once the process holds a use for its life.
    pub fn take(&self) -> Result<Option<ConnectionUse>> {
        self.take_saying(&mut |line| crate::text::note(line))
    }

    /// `take`, giving <say> the line that says it waits at the closed gate.
    fn take_saying(&self, say: &mut dyn FnMut(&str)) -> Result<Option<ConnectionUse>> {
        if self.held_for_life.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = shared.upgrade() {
            return Ok(Some(ConnectionUse { _use: open }));
        }
        let (gate, _) = pass(
            &self.places,
            Kind::Connection,
            "builds install moves the stores: this command waits until it finishes",
            say,
        )?;
        let taken = Arc::new(take(
            &self.places,
            Kind::Connection,
            gate,
            self.pid,
            &self.what,
            &self.since,
        )?);
        *shared = Arc::downgrade(&taken);
        Ok(Some(ConnectionUse { _use: taken }))
    }

    /// Whether an install ran since this process started.
    fn install_ran(&self) -> Result<bool> {
        Ok(generation(&self.places)? != self.generation)
    }
}

/// One connection's share of its process's use.
pub struct ConnectionUse {
    _use: Arc<Use>,
}

/// This process's connection uses, when it takes them; set once at start.
static PER_CONNECTION: OnceLock<PerConnection> = OnceLock::new();

/// Has this process take a use for each store connection it opens.
pub fn use_per_connection(per_connection: PerConnection) {
    if let Err(second) = PER_CONNECTION.set(per_connection) {
        let first = PER_CONNECTION.get().expect("set returned the second");
        panic!(
            "this process's use of the stores was set twice: for process {}, builds {}, then for process {}, builds {}",
            first.pid, first.what, second.pid, second.what
        );
    }
}

/// The use of the stores that a store connection this process opens takes:
/// None when the process holds a use for its life, a holder's use covers it,
/// or it takes none (install, the dashboard, tests).
pub fn for_connection() -> Result<Option<ConnectionUse>> {
    match PER_CONNECTION.get() {
        Some(per_connection) => per_connection.take(),
        None => Ok(None),
    }
}

/// Takes a use of the stores for the rest of this process's life, for a
/// command that holds none at start (`PerConnection::hold_from_now`). Every
/// builds command the process starts from then on runs under it. None for a
/// process that takes no use per connection: a holder's use covers it, or it
/// runs in a test.
pub fn hold_from_now() -> Result<Option<Use>> {
    let Some(per_connection) = PER_CONNECTION.get() else {
        return Ok(None);
    };
    let taken = per_connection.hold_from_now()?;
    // SAFETY: the only other threads an app build's lane runs here wait in
    // `open` on the queue's tickets and read no environment.
    unsafe { std::env::set_var(VARIABLE, per_connection.pid.to_string()) };
    Ok(Some(taken))
}

/// Whether an install ran since this process started; false for a process
/// that takes no use per connection.
pub fn install_ran() -> Result<bool> {
    match PER_CONNECTION.get() {
        Some(per_connection) => per_connection.install_ran(),
        None => Ok(false),
    }
}

/// The closed gates: no use starts until install drops this value, which
/// releases the gates' locks.
pub struct Closed {
    _gates: [File; 2],
}

/// Closes <kind>'s gate, waiting for whoever holds it.
fn close_gate(places: &Places, kind: Kind, say: &mut dyn FnMut(String)) -> Result<File> {
    let path = kind.gate(places);
    let gate = open(&path)?;
    if !try_lock(&gate, libc::LOCK_EX, &path)? {
        if closed_by_install(&gate, &path)? {
            say("another builds install runs: waiting for it".to_string());
        }
        lock(&gate, libc::LOCK_EX, &path)?;
    }
    Ok(gate)
}

/// Whether another install holds the gate at <path>, which <gate> opens,
/// closed. Only an install locks a gate exclusively. A command that passes
/// the gate holds a shared lock on it for a moment, and so does a process
/// that another thread of that command starts, until the process runs its
/// own program and closes the gate's file. When no install holds the gate,
/// <gate> holds a shared lock on return.
fn closed_by_install(gate: &File, path: &Path) -> Result<bool> {
    Ok(!try_lock(gate, libc::LOCK_SH, path)?)
}

/// Waits until every use of <kind> ends, telling <say> each use it waits
/// on, and removes each use's file. Only install removes a use's file, and
/// it holds the gate that keeps a second install out, so each file it lists
/// is there until it removes it.
fn wait_for_uses(places: &Places, kind: Kind, say: &mut dyn FnMut(String)) -> Result<()> {
    let dir = kind.uses(places);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let mut file = OpenOptions::new()
            .read(true)
            .open(&path)
            .context(|| format!("opening {}", path.display()))?;
        if !try_lock(&file, libc::LOCK_EX, &path)? {
            let mut text = String::new();
            file.read_to_string(&mut text)
                .context(|| format!("reading which process holds {}", path.display()))?;
            say(format!("waiting for {}", text.trim()));
            lock(&file, libc::LOCK_EX, &path)?;
        }
        // The use ended, or its process died.
        std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Closes the commands gate and waits for every command use, then closes
/// the connections gate and waits for every connection use, telling <say>
/// each use it waits on; then counts one more install.
pub fn close(places: &Places, say: &mut dyn FnMut(String)) -> Result<Closed> {
    let commands = close_gate(places, Kind::Command, say)?;
    wait_for_uses(places, Kind::Command, say)?;
    let connections = close_gate(places, Kind::Connection, say)?;
    wait_for_uses(places, Kind::Connection, say)?;
    advance_generation(places)?;
    Ok(Closed {
        _gates: [commands, connections],
    })
}

#[cfg(test)]
#[path = "install_gate_tests.rs"]
mod tests;
