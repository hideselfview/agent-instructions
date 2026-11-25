//! The machine's processes: names, parents, start times, working
//! directories, arguments, CPU time, memory and open files. One `Processes`
//! is one pass over every process; what costs more than the name is read
//! only for the processes a caller asks about. Each OS reads them its own
//! way: macOS through libproc (procs_macos.rs), Linux through /proc
//! (procs_linux.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::fail::Result;

#[cfg(target_os = "macos")]
#[path = "procs_macos.rs"]
mod os;
#[cfg(target_os = "linux")]
#[path = "procs_linux.rs"]
pub(crate) mod os;

#[cfg(all(test, target_os = "macos"))]
pub use os::exit_after;
#[cfg(target_os = "macos")]
pub use os::terminate_and_wait;
pub use os::{args, cwd, ended, environment, open_files, output_paths, tree_cpu, usage};

/// The bytes of one value in the process's NUL-separated environment.
pub fn environment_value<'a>(
    environment: &'a [std::ffi::OsString],
    key: &str,
) -> Option<&'a std::ffi::OsStr> {
    use std::os::unix::ffi::OsStrExt;
    environment.iter().find_map(|entry| {
        let value = entry.as_bytes().strip_prefix(key.as_bytes())?;
        Some(std::ffi::OsStr::from_bytes(value.strip_prefix(b"=")?))
    })
}

/// Names of the processes that build: compilers, linkers and build tools.
pub const BUILD_NAMES: [&str; 17] = [
    "cargo",
    "rustc",
    "clippy-driver",
    "rustdoc",
    "sccache",
    "cc",
    "clang",
    "ld",
    "xcodebuild",
    "swift-frontend",
    "swiftc",
    "swift-build",
    "java",
    "lake",
    "lean",
    "dotnet",
    "cargo-nextest",
];

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// The short name the kernel keeps (up to 32 bytes).
    pub name: String,
    /// Seconds since 1970.
    pub started: i64,
}

#[derive(Debug)]
pub struct Processes {
    pub list: Vec<Process>,
    by_pid: HashMap<i32, usize>,
}

impl Processes {
    /// One pass over every process this user can read. Fails when the
    /// kernel would not list the processes, so no caller takes a short list
    /// for the whole machine.
    pub fn read() -> Result<Processes> {
        Ok(Processes::of(os::list()?))
    }

    /// A listing of <list>.
    pub fn of(list: Vec<Process>) -> Processes {
        let by_pid = list.iter().enumerate().map(|(at, p)| (p.pid, at)).collect();
        Processes { list, by_pid }
    }

    /// A listing of no process, for tests of what a missing process means.
    #[cfg(test)]
    pub fn empty() -> Processes {
        Processes::of(Vec::new())
    }

    /// Whether <pid> is a `builds` process: a signal goes only to one, since
    /// a pid recorded earlier may be a stranger's by now.
    pub fn is_builds(&self, pid: i32) -> bool {
        self.get(pid)
            .is_some_and(|process| process.name == "builds")
    }

    pub fn get(&self, pid: i32) -> Option<&Process> {
        self.by_pid.get(&pid).map(|at| &self.list[*at])
    }

    /// The direct children of <pid>.
    pub fn children(&self, pid: i32) -> impl Iterator<Item = &Process> {
        self.list.iter().filter(move |p| p.ppid == pid)
    }

    /// <pid> and its ancestors, nearest first.
    pub fn ancestry(&self, pid: i32) -> Vec<&Process> {
        let mut chain = Vec::new();
        let mut at = self.get(pid);
        while let Some(process) = at {
            if chain.iter().any(|p: &&Process| p.pid == process.pid) {
                break;
            }
            chain.push(process);
            at = if process.ppid > 0 {
                self.get(process.ppid)
            } else {
                None
            };
        }
        chain
    }
}

/// Does the process <pid> run? A process of another user runs too: signal 0
/// to it fails with EPERM.
pub fn runs(pid: i32) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) };
    signalled == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Sends SIGTERM to a process. A process that has exited needs no signal.
pub fn terminate(pid: i32) -> std::io::Result<()> {
    // SAFETY: kill touches no caller memory.
    if unsafe { libc::kill(pid, libc::SIGTERM) } < 0 {
        return accept_gone(std::io::Error::last_os_error());
    }
    Ok(())
}

fn accept_gone(error: std::io::Error) -> std::io::Result<()> {
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

/// The arguments of <pid>: None when it has exited, and an error that names
/// it when it still runs and builds cannot read its arguments.
pub fn live_args(pid: i32) -> std::result::Result<Option<Vec<String>>, String> {
    match args(pid) {
        Some(argv) => Ok(Some(argv)),
        None if !runs(pid) => Ok(None),
        None => Err(format!(
            "builds cannot read the arguments of process {pid}, which still runs"
        )),
    }
}

/// A running `git commit`. Its hooks can run for minutes, and until the
/// commit ends, the files it commits are in no commit yet, so the worktree
/// is not free.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitCommit {
    pub pid: i32,
    /// Its working directory: the worktree, or a folder in it.
    pub dir: PathBuf,
}

/// The `git commit`s that run, and the git processes whose arguments builds
/// could not read although they still run: each of those may be a `git
/// commit` in its working directory.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct GitCommits {
    pub found: Vec<GitCommit>,
    pub unread: Vec<GitCommit>,
}

/// The `git commit`s among <processes> that this user runs. builds reads a
/// git process's working directory first: the kernel drops it when the
/// process exits, so a git process with none has ended or is ending.
pub fn git_commits(processes: &Processes) -> GitCommits {
    let uid = unsafe { libc::getuid() };
    let mut commits = GitCommits::default();
    for process in processes
        .list
        .iter()
        .filter(|p| p.name == "git" && p.uid == uid)
    {
        let pid = process.pid;
        let Some(dir) = cwd(pid) else { continue };
        match args(pid) {
            Some(argv) if git_subcommand(&argv) == Some("commit") => {
                commits.found.push(GitCommit { pid, dir });
            }
            Some(_) => {}
            None if ended(pid) => {}
            None => commits.unread.push(GitCommit { pid, dir }),
        }
    }
    commits
}

/// The subcommand of the git command line <argv>: its first word that is
/// neither an option nor the value of `-C`, `-c` or another global option
/// that takes its value as the next word.
pub fn git_subcommand(argv: &[String]) -> Option<&str> {
    let mut rest = argv.iter().skip(1);
    while let Some(word) = rest.next() {
        match word.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
            | "--exec-path" | "--super-prefix" | "--list-cmds" => {
                rest.next();
            }
            option if option.starts_with('-') => {}
            subcommand => return Some(subcommand),
        }
    }
    None
}

/// The pids of the commits of <commits> that run in <top> or a folder in it.
/// Fails when a git process there has arguments builds could not read,
/// since it may be a `git commit`, or when builds cannot resolve <top>. The
/// kernel gives each working directory with its links resolved, so builds
/// compares it with <top> resolved the same way.
pub fn commits_in(commits: &GitCommits, top: &Path) -> std::result::Result<Vec<i32>, String> {
    let real = std::fs::canonicalize(top)
        .map_err(|error| format!("resolving {}: {error}", top.display()))?;
    let here = |commit: &&GitCommit| commit.dir.starts_with(&real);
    if let Some(unread) = commits.unread.iter().find(here) {
        return Err(format!(
            "builds cannot read the arguments of git process {}, which runs in {}, so it cannot tell whether a git commit runs there",
            unread.pid,
            top.display()
        ));
    }
    Ok(commits
        .found
        .iter()
        .filter(here)
        .map(|commit| commit.pid)
        .collect())
}

/// The 1-, 5- and 15-minute load averages.
pub fn load_average() -> [f64; 3] {
    let mut loads = [0f64; 3];
    unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    loads
}

/// The number of CPU cores.
pub fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// A checkout processes are placed in: its working tree and its build root.
#[derive(Clone, Debug)]
pub struct Place {
    /// `bae/side-3`.
    pub label: String,
    /// The repo the checkout is of: `bae`.
    pub repo: String,
    pub top: PathBuf,
    pub root: PathBuf,
}

/// The place <dir> is in, the deepest when several hold it.
pub fn place_of(places: &[Place], dir: &Path) -> Option<usize> {
    deepest_containing(places.iter().map(|place| place.top.as_path()), dir)
}

pub fn deepest_containing<'a>(tops: impl Iterator<Item = &'a Path>, dir: &Path) -> Option<usize> {
    tops.enumerate()
        .filter(|(_, top)| dir.starts_with(top))
        .max_by_key(|(_, top)| top.as_os_str().len())
        .map(|(at, _)| at)
}

/// The place the working directory of process <pid> is in; None when it is
/// in none, or the process has exited.
pub fn place_of_process(places: &[Place], pid: i32) -> Option<usize> {
    cwd(pid).and_then(|dir| place_of(places, &dir))
}

/// A build process at work in a checkout.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BuildProcess {
    pub pid: i32,
    pub name: String,
    /// Index into the places it was placed among.
    #[serde(skip)]
    pub place: usize,
    pub started: i64,
    /// CPU time so far, seconds.
    pub cpu_seconds: f64,
    pub footprint: u64,
}

/// Does the text <text> name <path> or something inside it?
fn mentions(text: &str, path: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(path) {
        let end = from + at + path.len();
        if matches!(text.as_bytes().get(end), None | Some(b'/')) {
            return true;
        }
        from = from + at + 1;
    }
    false
}

/// The processes of <processes> named one of <names> (BUILD_NAMES) placed in
/// <places>: by working
/// directory, else by an argument naming a checkout or its build root
/// (sccache's compilers run in the registry's source folder with
/// `--out-dir <checkout>/target/...`), else by the nearest ancestor placed
/// either way. sccache's own server belongs to no checkout; its compilers do.
pub fn build_processes(
    processes: &Processes,
    places: &[Place],
    names: &[&str],
) -> Vec<BuildProcess> {
    let mut order: Vec<usize> = (0..places.len()).collect();
    // The longest path first: ~/dev/bae-worktrees/side-1 before ~/dev/bae.
    order.sort_by_key(|at| std::cmp::Reverse(places[*at].top.as_os_str().len()));
    let tops: Vec<(String, String)> = places
        .iter()
        .map(|place| {
            (
                place.top.display().to_string(),
                place.root.display().to_string(),
            )
        })
        .collect();
    let mut cache: HashMap<i32, Option<usize>> = HashMap::new();
    let mut place_of = |pid: i32| -> Option<usize> {
        if let Some(known) = cache.get(&pid) {
            return *known;
        }
        let dir = cwd(pid).map(|dir| dir.display().to_string());
        let argv = args(pid).unwrap_or_default();
        let found = order.iter().copied().find(|at| {
            let (top, root) = &tops[*at];
            dir.as_deref()
                .is_some_and(|dir| mentions(dir, top) || mentions(dir, root))
                || argv
                    .iter()
                    .skip(1)
                    .any(|arg| mentions(arg, top) || mentions(arg, root))
        });
        cache.insert(pid, found);
        found
    };
    let mut found = Vec::new();
    for process in &processes.list {
        if !names.contains(&process.name.as_str()) {
            continue;
        }
        let Some(place) = processes
            .ancestry(process.pid)
            .iter()
            .take(12)
            .find_map(|p| place_of(p.pid))
        else {
            continue;
        };
        let (cpu_seconds, footprint) = usage(process.pid).unwrap_or((0.0, 0));
        found.push(BuildProcess {
            pid: process.pid,
            name: process.name.clone(),
            place,
            started: process.started,
            cpu_seconds,
            footprint,
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worktree has the commits that run in it or a folder in it, and
    /// commits_in fails for one where a git process runs whose arguments
    /// builds could not read.
    #[test]
    fn commits_in_names_the_commits_of_a_worktree_and_fails_on_an_unread_git_there() {
        let top = std::env::temp_dir().canonicalize().unwrap();
        let other = top.join("elsewhere");
        let commit = |pid: i32, dir: PathBuf| GitCommit { pid, dir };
        let mut commits = GitCommits {
            found: vec![
                commit(1, top.join("sub")),
                commit(2, PathBuf::from("/nowhere")),
            ],
            unread: vec![commit(3, PathBuf::from("/nowhere"))],
        };
        assert_eq!(commits_in(&commits, &top), Ok(vec![1]));
        commits.unread.push(commit(4, top.clone()));
        let said = commits_in(&commits, &top).unwrap_err();
        assert!(said.contains("git process 4"), "{said}");
        assert!(
            commits_in(&commits, &other).is_err(),
            "{other:?} does not exist"
        );
    }

    /// The subcommand is the first word past git's global options and
    /// their values.
    #[test]
    fn finds_the_subcommand_past_the_global_options() {
        let words = |line: &str| -> Vec<String> { line.split(' ').map(str::to_string).collect() };
        assert_eq!(git_subcommand(&words("git commit -m x")), Some("commit"));
        assert_eq!(
            git_subcommand(&words("git -C /w -c core.hooksPath=h --no-pager commit -q")),
            Some("commit")
        );
        assert_eq!(git_subcommand(&words("git -C commit log")), Some("log"));
        assert_eq!(git_subcommand(&words("git --version")), None);
    }

    #[test]
    fn reads_a_child_process() {
        let dir = std::env::temp_dir();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let processes = Processes::read().unwrap();
        let found = processes.get(pid).map(|p| (p.name.clone(), p.ppid));
        let place = cwd(pid);
        let argv = args(pid);
        let used = usage(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            found,
            Some(("sleep".to_string(), std::process::id() as i32))
        );
        assert_eq!(
            place.map(|p| std::fs::canonicalize(p).unwrap()),
            Some(std::fs::canonicalize(&dir).unwrap())
        );
        assert_eq!(argv, Some(vec!["sleep".to_string(), "30".to_string()]));
        assert!(used.is_some_and(|(_, footprint)| footprint > 0));
        assert!(
            processes
                .ancestry(pid)
                .iter()
                .any(|p| p.pid == std::process::id() as i32)
        );
    }

    #[test]
    fn places_a_build_process_by_its_folder_or_its_arguments() {
        let base = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("builds-place-{}", std::process::id()));
        let top = base.join("demo-worktrees/side-1");
        let other = base.join("demo");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let places = vec![
            Place {
                label: "demo/main".into(),
                repo: "demo".into(),
                top: other.clone(),
                root: base.join("volume/demo/main"),
            },
            Place {
                label: "demo/side-1".into(),
                repo: "demo".into(),
                top: top.clone(),
                root: base.join("volume/demo/side-1"),
            },
        ];
        // A `sleep` in side-1's folder; a `bash` elsewhere naming main's
        // build root, as sccache's rustc names its --out-dir. bash, not sh:
        // macOS's /bin/sh execs the shell /private/var/select/sh names a
        // moment after it starts, which changes its name, and while it execs,
        // the kernel fails a read of its arguments.
        let mut in_folder = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&top)
            .spawn()
            .unwrap();
        let out_dir = format!("{}/target/debug", places[0].root.display());
        let mut by_argument = std::process::Command::new("/bin/bash")
            .args(["-c", "read -r line", "rustc", &out_dir])
            .stdin(std::process::Stdio::piped())
            .current_dir("/")
            .spawn()
            .unwrap();
        let found = build_processes(&Processes::read().unwrap(), &places, &["sleep", "bash"]);
        for child in [&mut in_folder, &mut by_argument] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        std::fs::remove_dir_all(&base).unwrap();
        let placed = |pid: u32| {
            found
                .iter()
                .find(|p| p.pid == pid as i32)
                .map(|p| places[p.place].label.clone())
        };
        assert_eq!(placed(in_folder.id()), Some("demo/side-1".to_string()));
        assert_eq!(placed(by_argument.id()), Some("demo/main".to_string()));
    }

    /// A process that runs has arguments to read; one that has exited has
    /// none, and that is no error.
    #[test]
    fn the_arguments_of_a_process_that_exited_are_none() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        assert_eq!(
            live_args(pid),
            Ok(Some(vec!["sleep".to_string(), "30".to_string()]))
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(live_args(pid), Ok(None));
    }

    #[test]
    fn lists_open_files() {
        let path = std::env::temp_dir().join(format!("builds-open-{}", std::process::id()));
        std::fs::write(&path, "x").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let open = open_files(std::process::id() as i32);
        drop(file);
        std::fs::remove_file(&path).unwrap();
        let wanted = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(path.file_name().unwrap());
        assert!(open.contains(&wanted), "{open:?}");
    }
}
