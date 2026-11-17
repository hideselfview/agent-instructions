//! The gates that keep a builds program off a store of another schema.
//!
//! `builds install` moves each store to its program's schema and puts that
//! program on PATH. A builds program refuses a store of another schema
//! (db.rs), so a command that started before an install could not write a
//! store after the install moved it. The gates make install wait for each
//! such command first. When every store already has the new program's
//! schema, install moves no store and closes no gate: a command that started
//! before the install still writes the schema that the new program reads.
//!
//! A command uses the stores in one of three ways. A use is a locked file of
//! its own in its kind's folder that says which process runs what and since
//! when.
//! - A command that writes a store after long work, with what it measured
//!   since it started (main.rs's `holds_for_life` names them: a hook run and
//!   a review that asks reviewers), holds a command use for its life.
//! - An app build's lane holds a command use while it holds the lane's lock
//!   (`hold`): from the moment it takes the lock until it ends, or until a
//!   takeover or a host fault sends it back to the queue. A build waiting in
//!   the queue builds nothing and writes nothing, so it holds no use. When
//!   install ran while the lane waited, the lane leaves the queue and runs
//!   the installed builds in its place.
//! - Every other command takes a connection use while each store connection
//!   it opens is open. Its writes are each one short transaction, so a
//!   command such as `builds cargo test` holds no use while it builds.
//!
//! Install first takes the install lock, which it holds until its program
//! is on PATH, so a second install waits for it, and no install moves a
//! store or replaces the program while another reads the stores' schemas. Then it
//! reads each store's schema (migrate.rs) and, when every store has its
//! program's, puts the program on PATH with both gates open.
//!
//! A command use and a connection use each have a gate: a file that a use
//! holds a shared lock on while it starts and that install locks
//! exclusively. An install that moves a store, or that finds no store with a
//! schema, closes the commands gate and waits for every command use, then
//! closes the connections gate and waits for every connection use, then
//! counts one more install in the generation file, moves the stores, puts
//! its program on PATH and opens both gates. So while install waits for a
//! long command, only a command that would hold a command use waits at the
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

/// The commands that take no use of the stores: install, which closes the
/// gates and moves the stores, the dashboard, which install restarts, and
/// the help.
pub const STORELESS: [&str; 5] = ["install", "dashboard", "help", "-h", "--help"];

/// Runs the program at this program's path with this process's arguments in
/// its place; returns only when it cannot, with the error that says why.
pub fn run_again() -> Fail {
    use std::os::unix::process::CommandExt;
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => return Fail::Message(format!("finding this program: {error}")),
    };
    // The program takes its own use. A process that holds a use (`hold`)
    // names itself in VARIABLE, and the exec closes the use's file, which
    // ends that use.
    let error = std::process::Command::new(&program)
        .args(std::env::args_os().skip(1))
        .env_remove(VARIABLE)
        .exec();
    Fail::Message(format!("running {} again: {error}", program.display()))
}

/// The file that counts the installs that closed the gates.
fn generation_file(places: &Places) -> PathBuf {
    places.install_dir().join("generation")
}

/// How many installs closed the gates: 0 before the first.
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

/// A use of the stores: its file, which the use holds locked until the
/// process drops the use and the use removes the file. Install removes the
/// file of a use whose process ended without dropping it.
///
/// Only a process that holds a use file's lock removes the file, and only
/// after it checks that the path still names the file it locked, since
/// another process may have made a new file there. A process that takes a
/// use checks the same once it holds the lock, and starts again when the
/// path names another file or none: a file removed while it waited for the
/// lock is no use install can see.
pub struct Use {
    _file: File,
    path: PathBuf,
}

impl Drop for Use {
    fn drop(&mut self) {
        // The use holds the lock, so the path names its file. A file that
        // stays is one that install removes later, since its lock is free.
        if let Err(error) = std::fs::remove_file(&self.path) {
            crate::text::note(format!(
                "removing {}: {error}; the next builds install removes it",
                self.path.display()
            ));
        }
    }
}

/// Whether the path <path> names the open file <file>.
fn names(file: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let open = file
        .metadata()
        .context(|| format!("reading {}", path.display()))?;
    match std::fs::metadata(path) {
        Ok(named) => Ok(named.dev() == open.dev() && named.ino() == open.ino()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// Removes the use file at <path>, whose lock <file> holds, when the path
/// still names that file.
fn remove_use_file(file: &File, path: &Path) -> Result<()> {
    if names(file, path)? {
        std::fs::remove_file(path).context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
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
    let mut file = loop {
        let file = open(&path)?;
        lock(&file, libc::LOCK_EX, &path)?;
        if names(&file, &path)? {
            break file;
        }
    };
    file.set_len(0)
        .context(|| format!("writing {}", path.display()))?;
    file.write_all(format!("process {pid}, builds {what}, since {since}\n").as_bytes())
        .context(|| format!("writing {}", path.display()))?;
    // Closing the gate's file releases the shared lock; the use is in place.
    drop(gate);
    Ok(Use { _file: file, path })
}

const RUNS_AGAIN: &str =
    "builds install runs: this command waits until it finishes, then runs the installed builds";

/// How a command that holds a use for its life enters.
pub enum Entered {
    /// The command holds this use for its life.
    Holds(Use),
    /// The command waited while an install ran, and must run the installed
    /// program in its place.
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
/// process's use, and the last one to close ends it, except while the process
/// holds a command use (`hold`).
pub struct PerConnection {
    places: Places,
    pid: u32,
    what: String,
    since: String,
    /// The installs counted when this process started. An install that
    /// closes the gates counts one more before it moves the stores and
    /// replaces the program, so a count that differs says such an install
    /// ran since. An install that closes no gate moves no store and counts
    /// none.
    generation: u64,
    /// The use that the process's open connections share. It ends when the
    /// last of them closes and drops it.
    shared: Mutex<Weak<Use>>,
    /// Whether the process holds a command use (`hold`) now.
    holding: AtomicBool,
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
            holding: AtomicBool::new(false),
        })
    }

    /// Takes a command use that the process holds until it drops the value
    /// returned, waiting at the gate while an install runs. No connection of
    /// the process may be open: install waits for it before it opens the
    /// gate.
    pub fn hold(&self) -> Result<Holding<'_>> {
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
        self.holding.store(true, Ordering::SeqCst);
        Ok(Holding {
            per_connection: self,
            named: false,
            _use: taken,
        })
    }

    /// A use for one connection, waiting at the gate while an install runs;
    /// none while the process holds a command use.
    pub fn take(&self) -> Result<Option<ConnectionUse>> {
        self.take_saying(&mut |line| crate::text::note(line))
    }

    /// `take`, giving <say> the line that says it waits at the closed gate.
    fn take_saying(&self, say: &mut dyn FnMut(&str)) -> Result<Option<ConnectionUse>> {
        if self.holding.load(Ordering::SeqCst) {
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

    /// Whether an install that closed the gates ran since this process
    /// started.
    fn install_ran(&self) -> Result<bool> {
        Ok(generation(&self.places)? != self.generation)
    }
}

/// A command use that a process holds for a stretch of its life
/// (`PerConnection::hold`). While it lasts, the process's connections take no
/// use of their own; once it ends, they take theirs again.
pub struct Holding<'a> {
    per_connection: &'a PerConnection,
    /// Whether `hold` named this process in VARIABLE, which the end of the
    /// use removes.
    named: bool,
    _use: Use,
}

impl Drop for Holding<'_> {
    // Rust drops the fields after this runs, so the use ends after the
    // connections may take theirs again.
    fn drop(&mut self) {
        self.per_connection.holding.store(false, Ordering::SeqCst);
        if self.named {
            // SAFETY: as in `hold`: the other threads an app build's lane runs
            // here read no environment.
            unsafe { std::env::remove_var(VARIABLE) };
        }
    }
}

/// One connection's share of its process's use.
pub struct ConnectionUse {
    _use: Arc<Use>,
}

/// This process's connection uses, when it takes them. `use_per_connection`
/// sets it once, when the process starts.
static PER_CONNECTION: OnceLock<PerConnection> = OnceLock::new();

/// Has this process take a use for each store connection it opens.
pub fn use_per_connection(per_connection: PerConnection) {
    if let Err(second) = PER_CONNECTION.set(per_connection) {
        let first = PER_CONNECTION.get().expect("set returned the second");
        panic!(
            "use_per_connection ran twice in this process: for process {}, builds {}, then for process {}, builds {}",
            first.pid, first.what, second.pid, second.what
        );
    }
}

/// The use of the stores that a store connection this process opens takes:
/// None while the process holds a command use, a holder's use covers it,
/// or it takes none (install, the dashboard, tests).
pub fn for_connection() -> Result<Option<ConnectionUse>> {
    match PER_CONNECTION.get() {
        Some(per_connection) => per_connection.take(),
        None => Ok(None),
    }
}

/// Takes a command use of the stores that this process holds until it drops
/// the value returned, for a command that holds none at start
/// (`PerConnection::hold`). Every builds command the process starts while it
/// holds the use runs under it. None for a process that takes no use per
/// connection: a holder's use covers it, or it runs in a test.
pub fn hold() -> Result<Option<Holding<'static>>> {
    let Some(per_connection) = PER_CONNECTION.get() else {
        return Ok(None);
    };
    let mut holding = per_connection.hold()?;
    // SAFETY: the other threads an app build's lane runs here wait in `open`
    // on the queue's tickets, or copy a finished step's output, and read no
    // environment.
    unsafe { std::env::set_var(VARIABLE, per_connection.pid.to_string()) };
    holding.named = true;
    Ok(Some(holding))
}

/// Whether an install that closed the gates ran since this process started;
/// false for a process that takes no use per connection.
pub fn install_ran() -> Result<bool> {
    match PER_CONNECTION.get() {
        Some(per_connection) => per_connection.install_ran(),
        None => Ok(false),
    }
}

/// The install lock, held by one install from before it reads the stores'
/// schemas until its program is on PATH.
pub struct Installing {
    places: Places,
    lock: File,
}

/// Takes the install lock, waiting while another install holds it, and gives
/// <say> the line that says it waits.
pub fn begin(places: &Places, say: &mut dyn FnMut(String)) -> Result<Installing> {
    let path = places.install_dir().join("install.lock");
    let file = open(&path)?;
    if !try_lock(&file, libc::LOCK_EX, &path)? {
        say("another builds install runs: waiting for it".to_string());
        lock(&file, libc::LOCK_EX, &path)?;
    }
    Ok(Installing {
        places: places.clone(),
        lock: file,
    })
}

impl Installing {
    /// Removes the file of each use whose process ended without removing it,
    /// with both gates open: for an install that waits for no use.
    pub fn remove_ended_uses(&self) -> Result<()> {
        remove_ended_uses(&self.places, Kind::Command)?;
        remove_ended_uses(&self.places, Kind::Connection)
    }

    /// Closes the commands gate and waits for every command use, then closes
    /// the connections gate and waits for every connection use, telling
    /// <say> each use it waits on; then counts one more install.
    pub fn close(self, say: &mut dyn FnMut(String)) -> Result<Closed> {
        let commands = close_gate(&self.places, Kind::Command)?;
        wait_for_uses(&self.places, Kind::Command, say)?;
        let connections = close_gate(&self.places, Kind::Connection)?;
        wait_for_uses(&self.places, Kind::Connection, say)?;
        advance_generation(&self.places)?;
        Ok(Closed {
            _install: self.lock,
            _gates: [commands, connections],
        })
    }
}

/// The closed gates and the install lock: no use starts and no other install
/// begins until install drops this value, which releases their locks.
pub struct Closed {
    _install: File,
    _gates: [File; 2],
}

/// Closes <kind>'s gate. The install lock keeps every other install out, so
/// only a command that passes the gate holds a lock on it: a shared lock,
/// held while the command takes its use. A child process that another thread
/// of the command starts holds the same lock until the child calls exec,
/// which closes the gate's file in the child.
fn close_gate(places: &Places, kind: Kind) -> Result<File> {
    let path = kind.gate(places);
    let gate = open(&path)?;
    lock(&gate, libc::LOCK_EX, &path)?;
    Ok(gate)
}

/// The use files of <kind>, each open, in the order of their names. The list
/// skips a file that its use removed after the listing.
fn use_files(places: &Places, kind: Kind) -> Result<Vec<(File, PathBuf)>> {
    let dir = kind.uses(places);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        paths.push(entry.path());
    }
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        match OpenOptions::new().read(true).open(&path) {
            Ok(file) => files.push((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => fail!("opening {}: {error}", path.display()),
        }
    }
    Ok(files)
}

/// Waits until every use of <kind> ends, telling <say> each use it waits
/// on, and removes the file of each use whose process ended without
/// removing it.
fn wait_for_uses(places: &Places, kind: Kind, say: &mut dyn FnMut(String)) -> Result<()> {
    for (mut file, path) in use_files(places, kind)? {
        if !try_lock(&file, libc::LOCK_EX, &path)? {
            let mut text = String::new();
            file.read_to_string(&mut text)
                .context(|| format!("reading which process holds {}", path.display()))?;
            say(format!("waiting for {}", text.trim()));
            lock(&file, libc::LOCK_EX, &path)?;
        }
        remove_use_file(&file, &path)?;
    }
    Ok(())
}

/// Removes the file of each use of <kind> whose lock is free: its process
/// ended without removing it, so nothing reads the file. Leaves the file of
/// each use that holds its lock.
fn remove_ended_uses(places: &Places, kind: Kind) -> Result<()> {
    for (file, path) in use_files(places, kind)? {
        if try_lock(&file, libc::LOCK_EX, &path)? {
            remove_use_file(&file, &path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "install_gate_tests.rs"]
mod tests;
