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
//! - A landing (`builds land`) holds a command use (`hold`) from its start
//!   until it has written the record of its run, then ends it before it
//!   fast-forwards the main checkout, whose
//!   post-merge hook in agent-instructions runs install: install would wait
//!   forever for a use the landing still held. A landing ends its use while
//!   it waits for its turn among the landings that change the apps, and
//!   while it waits for an app build of its result, and takes it again
//!   before it pushes; when install ran meanwhile, the landing runs the
//!   installed builds in its place.
//! - Every other command takes a connection use while each store connection
//!   it opens is open. Its writes are each one short transaction, so a
//!   command such as `builds cargo test` holds no use while it builds. An
//!   app build's lane is such a command: it refuses to start if an install
//!   changed the generation since its parent started. After the lane starts,
//!   an install waits for none of its steps. The lane hands its run to the
//!   installed builds if the generation changes while it builds
//!   (handed_runs.rs).
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
//! schema, closes the commands gate, waits for every command use, then
//! closes the connections gate and waits for every connection use, then
//! counts one more install in the generation file, moves the stores, puts
//! its program on PATH and opens both gates. So while install waits for a
//! long command, only a command that would hold a command use waits at the
//! gate; the others keep working.
//!
//! A command that finds a gate closed when it starts waits until the gate
//! opens. A lane started by an app build compares generations while holding
//! the connections gate open. Other commands run the program at their own
//! path in their place. A connection that finds the gate closed waits too,
//! then opens the store; when install moved it to another schema, db.rs
//! refuses it and says to run the command again.
//!
//! A builds command that runs under a parent's use (`BUILDS_USE` in its
//! environment names the holder) takes none of its own. Waiting at a gate
//! would deadlock: install waits for the holder, which waits for its child.
//! A holder removes `BUILDS_USE` from any child it does not wait for.
//! For the same reason install refuses to run under a holder.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::flock::{lock, names, try_lock};
use crate::places::Places;

/// The variable that names the process whose use covers a builds command.
pub const VARIABLE: &str = "BUILDS_USE";

/// The install generation an app build passes to each lane it starts.
const LANE_GENERATION: &str = "BUILDS_APP_BUILD_GENERATION";

/// The commands that take no use of the stores: install, which closes the
/// gates and moves the stores, the dashboard, which install restarts, the
/// help, `brief`, `agent`, which registers an agent outside the stores
/// (ack/agent.rs), `version`, which prints only what the program was built
/// from, `programs`, which runs a lane's step on a host (programs.rs), and
/// `sccache-tmpdir`, which checks a host's sccache server.
pub const STORELESS: [&str; 10] = [
    "install",
    "dashboard",
    "help",
    "brief",
    "-h",
    "--help",
    "agent",
    "version",
    "programs",
    "sccache-tmpdir",
];

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
    /// A command's life, or the stretch of it that writes the stores after
    /// long work. Install waits for it to end.
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

    fn waiters(self, places: &Places) -> PathBuf {
        places.install_dir().join(match self {
            Kind::Command => "commands-waiting",
            Kind::Connection => "connections-waiting",
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
    pid: u32,
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
    pid: u32,
    waiting: &str,
    say: &mut dyn FnMut(&str),
) -> Result<(File, bool)> {
    let path = kind.gate(places);
    let gate = open(&path)?;
    let was_open = pass_lock(&gate, &path, libc::LOCK_SH, || {
        let waiter = locked_use(kind.waiters(places), pid)?;
        say(waiting);
        Ok(waiter)
    })?;
    Ok((gate, was_open))
}

fn pass_lock(
    file: &File,
    path: &Path,
    how: libc::c_int,
    register_waiter: impl FnOnce() -> Result<Use>,
) -> Result<bool> {
    if try_lock(file, how, path)? {
        return Ok(true);
    }
    let _waiter = register_waiter()?;
    lock(file, how, path)?;
    Ok(false)
}

/// Takes a use of <kind> for process <pid>, which runs `builds <what>` since
/// the local time <since>, while the gate held open by <gate> stays open.
fn take(places: &Places, kind: Kind, gate: File, pid: u32, what: &str, since: &str) -> Result<Use> {
    let mut usage = locked_use(kind.uses(places), pid)?;
    usage
        ._file
        .set_len(0)
        .context(|| format!("writing {}", usage.path.display()))?;
    usage
        ._file
        .write_all(format!("process {pid}, builds {what}, since {since}\n").as_bytes())
        .context(|| format!("writing {}", usage.path.display()))?;
    // Closing the gate's file releases the shared lock; the use is in place.
    drop(gate);
    Ok(usage)
}

fn locked_use(dir: PathBuf, pid: u32) -> Result<Use> {
    let path = dir.join(pid.to_string());
    loop {
        let file = open(&path)?;
        lock(&file, libc::LOCK_EX, &path)?;
        if names(&file, &path)? {
            return Ok(Use {
                _file: file,
                path,
                pid,
            });
        }
    }
}

const RUNS_AGAIN: &str =
    "builds install runs: this command waits until it finishes, then runs the installed builds";

const WAITS_FOR_MOVE: &str =
    "builds install moves the stores: this command waits until it finishes";

/// How a command enters the stores.
pub enum Entered<T> {
    /// The command starts with its use or the state for its connection uses.
    Start(T),
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
) -> Result<Entered<Use>> {
    match pass(places, Kind::Command, pid, RUNS_AGAIN, say)? {
        (gate, true) => Ok(Entered::Start(take(
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
    /// Captures the starting generation while holding the connections gate.
    /// A lane compares it with its parent's generation to check that it can
    /// start on the program its parent selected. Other commands get RunAgain
    /// if they had to wait.
    pub fn enter(
        places: Places,
        pid: u32,
        what: &str,
        since: &str,
        parent_generation: Option<u64>,
        say: &mut dyn FnMut(&str),
    ) -> Result<Entered<Self>> {
        let waiting = if parent_generation.is_some() {
            WAITS_FOR_MOVE
        } else {
            RUNS_AGAIN
        };
        let (_gate, was_open) = pass(&places, Kind::Connection, pid, waiting, say)?;
        if !was_open && parent_generation.is_none() {
            return Ok(Entered::RunAgain);
        }
        let uses = Self::new(places, pid, what, since)?;
        if parent_generation.is_some_and(|parent| parent != uses.generation) {
            fail!(
                "builds install ran before the app build lane started. Run builds app-build again."
            );
        }
        Ok(Entered::Start(uses))
    }

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
    /// returned, which a running install waits for, waiting at the gate while
    /// an install runs. No connection of the process may be open: install
    /// waits for it before it opens the gate.
    pub fn hold(&self) -> Result<Holding<'_>> {
        let (gate, _) = pass(
            &self.places,
            Kind::Command,
            self.pid,
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
            return Ok(Some(ConnectionUse { held: open }));
        }
        let (gate, _) = pass(
            &self.places,
            Kind::Connection,
            self.pid,
            WAITS_FOR_MOVE,
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
        Ok(Some(ConnectionUse { held: taken }))
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

/// A share of the process's use of the stores.
pub struct ConnectionUse {
    held: Arc<Use>,
}

impl ConnectionUse {
    /// Covers a child for which the holder waits. Taking a separate use in
    /// the child could deadlock if install already waits for this use.
    pub fn cover(&self, child: &mut std::process::Command) {
        child.env(VARIABLE, self.held.pid.to_string());
    }
}

/// This process's connection uses, when it takes them. `use_per_connection`
/// sets it once, when the process starts.
static PER_CONNECTION: OnceLock<PerConnection> = OnceLock::new();

/// Gives a lane the install generation of the app build that starts it.
/// A parent covered by another command's use needs no generation to compare.
pub fn set_lane_generation(child: &mut std::process::Command) {
    if let Some(uses) = PER_CONNECTION.get() {
        child.env(LANE_GENERATION, uses.generation.to_string());
    }
}

/// Reads the parent app build's generation at process startup and removes it
/// from the environment before the lane starts any children.
pub fn take_lane_generation() -> Result<Option<u64>> {
    let word = match std::env::var(LANE_GENERATION) {
        Ok(word) => word,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => fail!("Builds could not read {LANE_GENERATION}: {error}"),
    };
    // SAFETY: the command reads the variable before starting any thread.
    unsafe { std::env::remove_var(LANE_GENERATION) };
    let generation = word
        .parse()
        .context(|| format!("Builds could not read the install generation in {LANE_GENERATION}"))?;
    Ok(Some(generation))
}

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

/// Takes a use of the stores that this process holds until it drops the
/// value returned, which a running install waits for, for a command that
/// holds none at start (`PerConnection::hold`). Every builds command the
/// process starts while it holds the use runs under it. None for a process
/// that takes no use per connection: a holder's use covers it, or it runs in
/// a test.
fn hold() -> Result<Option<Holding<'static>>> {
    let Some(per_connection) = PER_CONNECTION.get() else {
        return Ok(None);
    };
    let mut holding = per_connection.hold()?;
    // SAFETY: a landing, the one command that takes this use, runs on one
    // thread, so no other thread reads the environment.
    unsafe { std::env::set_var(VARIABLE, per_connection.pid.to_string()) };
    holding.named = true;
    Ok(Some(holding))
}

/// What `hold_current` found.
pub enum Hold {
    /// The process holds this use until it drops it; None for a process that
    /// takes no use per connection.
    Holds(Option<Holding<'static>>),
    /// An install that closed the gates ran since this process started, so
    /// this program writes a store of another schema. The use has ended: the
    /// caller undoes what it must, then runs the installed program in its
    /// place (`run_installed`).
    InstallRan,
}

/// Takes a use (`hold`), which a running install waits for, for a command
/// that writes the stores after long work from here on, unless an install
/// that closed the gates ran since this process started.
pub fn hold_current() -> Result<Hold> {
    let held = hold()?;
    if install_ran()? {
        drop(held);
        return Ok(Hold::InstallRan);
    }
    Ok(Hold::Holds(held))
}

/// Says <why> and runs the installed program in this one's place
/// (`run_again`), for a command that `hold_current` found an install ran
/// under.
pub fn run_installed(why: &str) -> Fail {
    crate::text::note(why);
    run_again()
}

/// Whether some process holds the lock of the gate or lock file at <path>
/// exclusively. Asks for a shared lock without waiting and lets it go at
/// once, so it takes no use and holds up no install.
fn gate_closed(path: &Path) -> Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => fail!("opening {}: {error}", path.display()),
    };
    Ok(!try_lock(&file, libc::LOCK_SH, path)?)
}

/// Whether an install that closed the gates ran since this process started;
/// false for a process that takes no use per connection.
pub fn install_ran() -> Result<bool> {
    match PER_CONNECTION.get() {
        Some(per_connection) => per_connection.install_ran(),
        None => Ok(false),
    }
}

fn install_lock(places: &Places) -> PathBuf {
    places.install_dir().join("install.lock")
}

fn installers(places: &Places) -> PathBuf {
    places.install_dir().join("installer")
}

fn install_waiters(places: &Places) -> PathBuf {
    places.install_dir().join("install-waiting")
}

/// The install lock, held by one install from before it reads the stores'
/// schemas until its program is on PATH.
pub struct Installing {
    places: Places,
    // Drop the owner record before the lock admits another installer.
    owner: Use,
    lock: File,
}

/// Takes the install lock, waiting while another install holds it, and gives
/// <say> the line that says it waits.
pub fn begin(places: &Places, say: &mut dyn FnMut(String)) -> Result<Installing> {
    let path = install_lock(places);
    let pid = std::process::id();
    let file = open(&path)?;
    pass_lock(&file, &path, libc::LOCK_EX, || {
        let waiter = locked_use(install_waiters(places), pid)?;
        say("another builds install runs: waiting for it".to_string());
        Ok(waiter)
    })?;
    let owner = locked_use(installers(places), pid)?;
    remove_ended_uses(&installers(places))?;
    remove_ended_uses(&install_waiters(places))?;
    Ok(Installing {
        places: places.clone(),
        owner,
        lock: file,
    })
}

impl Installing {
    /// Removes the file of each use whose process ended without removing it,
    /// with both gates open: for an install that waits for no use.
    pub fn remove_ended_uses(&self) -> Result<()> {
        for kind in [Kind::Command, Kind::Connection] {
            remove_ended_uses(&kind.uses(&self.places))?;
            remove_ended_uses(&kind.waiters(&self.places))?;
        }
        Ok(())
    }

    /// Closes the commands gate and waits for every command use, then closes
    /// the connections gate and waits for every connection use, telling <say>
    /// each use it waits on; then counts one more install.
    pub fn close(self, say: &mut dyn FnMut(String)) -> Result<Closed> {
        let commands = close_gate(&self.places, Kind::Command)?;
        wait_for_uses(&self.places, Kind::Command, say)?;
        let connections = close_gate(&self.places, Kind::Connection)?;
        wait_for_uses(&self.places, Kind::Connection, say)?;
        advance_generation(&self.places)?;
        Ok(Closed {
            _gates: [commands, connections],
            _owner: self.owner,
            _install: self.lock,
        })
    }
}

/// The closed gates and the install lock: no use starts and no other install
/// begins until install drops this value, which releases their locks.
pub struct Closed {
    _gates: [File; 2],
    _owner: Use,
    _install: File,
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
fn use_files(dir: &Path) -> Result<Vec<(File, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
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
    for (mut file, path) in use_files(&kind.uses(places))? {
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

/// Each use of <kind> that holds its lock, read by asking for a shared lock
/// without waiting and letting it go at once.
fn held_pids(dir: &Path) -> Result<Vec<i32>> {
    let mut held = Vec::new();
    for (file, path) in use_files(dir)? {
        if try_lock(&file, libc::LOCK_SH, &path)? {
            continue;
        }
        let pid = crate::checkout::file_name(&path)
            .parse()
            .context(|| format!("reading the process ID from {}", path.display()))?;
        held.push(pid);
    }
    Ok(held)
}

/// Removes the file of each use of <kind> whose lock is free: its process
/// ended without removing it, so nothing reads the file. Leaves the file of
/// each use that holds its lock.
fn remove_ended_uses(dir: &Path) -> Result<()> {
    for (file, path) in use_files(dir)? {
        if try_lock(&file, libc::LOCK_EX, &path)? {
            remove_use_file(&file, &path)?;
        }
    }
    Ok(())
}

/// Processes on either side of an install's closed gates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub installer: i32,
    pub held: Vec<i32>,
    pub blocked: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallRead {
    Idle,
    Handoff,
    Active(Waiting),
}

/// Reads the install that holds the lock and the processes that wait on
/// either side of it, without waiting on a lock.
pub fn waiting(places: &Places) -> Result<InstallRead> {
    if !gate_closed(&install_lock(places))? {
        return Ok(InstallRead::Idle);
    }
    let installer = match held_pids(&installers(places))?.as_slice() {
        [pid] => *pid,
        // Install is taking or releasing the lock.
        [] => return Ok(InstallRead::Handoff),
        pids => fail!("health found these processes holding the install lock: {pids:?}"),
    };
    let commands_closed = gate_closed(&Kind::Command.gate(places))?;
    let connections_closed = gate_closed(&Kind::Connection.gate(places))?;
    let mut held = Vec::new();
    let mut blocked = held_pids(&install_waiters(places))?;
    for (kind, closed) in [
        (Kind::Command, commands_closed),
        (Kind::Connection, connections_closed),
    ] {
        if closed {
            held.extend(held_pids(&kind.uses(places))?);
            blocked.extend(held_pids(&kind.waiters(places))?);
        }
    }
    Ok(InstallRead::Active(Waiting {
        installer,
        held,
        blocked,
    }))
}

#[cfg(test)]
#[path = "install_gate_tests.rs"]
mod tests;
