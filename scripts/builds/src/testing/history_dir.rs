//! The folder of a test that replays FSEvents history. A test run's temporary
//! folder is on the build volume, which keeps none (`builds help`, HOSTS), so
//! such a test makes its folder in `~/Library/Caches/builds-tests/<run id>`,
//! the one place outside the run's temporary folder where the tests' sandbox
//! lets a test make a folder (tests/no-machine-temp.sh).
//!
//! A test keeps a shared lock on its run's folder while it uses a folder in
//! it, so the processes that used a run's folder with no lock are gone. A lock
//! on `builds-tests` keeps one test's sweep off the folder another is making.

use std::fs::File;
use std::path::{Path, PathBuf};

pub struct HistoryDir {
    pub dir: PathBuf,
    runs: PathBuf,
    /// The lock on the run's folder; `drop` takes it to release it.
    running: Option<File>,
}

impl HistoryDir {
    /// A new folder named <name> for this test's process. First deletes the
    /// folder of each run whose tests are gone.
    pub fn new(name: &str) -> HistoryDir {
        let home = std::env::var_os("HOME").expect("a test has a home");
        // The run's temporary folder is named for the run's id (run_tmp.rs).
        let temp = std::env::temp_dir();
        let run = temp.file_name().expect("the temp dir has a name");
        let runs = Path::new(&home).join("Library/Caches/builds-tests");
        HistoryDir::in_runs(&runs, Path::new(run), name)
    }

    fn in_runs(runs: &Path, run: &Path, name: &str) -> HistoryDir {
        std::fs::create_dir_all(runs).expect("the folder of the runs is creatable");
        let _sweeping = locked(runs);
        delete_ended_runs(runs);
        let run = runs.join(run);
        std::fs::create_dir_all(&run).expect("the run's folder is creatable");
        let running = open(&run);
        crate::flock::lock(&running, libc::LOCK_SH, &run).expect("the run's folder locks");
        let dir = run.join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).expect("the test's folder is creatable");
        HistoryDir {
            dir,
            runs: runs.to_path_buf(),
            running: Some(running),
        }
    }
}

/// Deletes the test's folder, and the run's folder when no other test uses it.
impl Drop for HistoryDir {
    fn drop(&mut self) {
        let _sweeping = locked(&self.runs);
        std::fs::remove_dir_all(&self.dir).expect("the test's folder is removable");
        drop(self.running.take());
        delete_if_ended(self.dir.parent().expect("the run's folder"));
    }
}

fn open(folder: &Path) -> File {
    File::open(folder).expect("the folder opens")
}

/// <folder> with an exclusive lock, which lasts until the file is dropped.
fn locked(folder: &Path) -> File {
    let file = open(folder);
    crate::flock::lock(&file, libc::LOCK_EX, folder).expect("the folder locks");
    file
}

fn delete_ended_runs(runs: &Path) {
    for entry in std::fs::read_dir(runs).expect("the runs are listable") {
        let entry = entry.expect("the runs are listable");
        // Finder leaves files of its own in a folder it shows.
        if entry.file_type().expect("an entry has a type").is_dir() {
            delete_if_ended(&entry.path());
        }
    }
}

fn delete_if_ended(run: &Path) {
    let file = open(run);
    if crate::flock::try_lock(&file, libc::LOCK_EX, run).expect("the run's folder locks") {
        std::fs::remove_dir_all(run).expect("an ended run's folder is removable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn taking_a_folder_deletes_the_folders_of_ended_runs_only() {
        let scratch = Scratch::new("history-sweep");
        let runs = scratch.dir.join("runs");
        std::fs::create_dir_all(runs.join("ended/left")).unwrap();
        let first = HistoryDir::in_runs(&runs, Path::new("running"), "first");
        assert!(!runs.join("ended").exists());
        let _second = HistoryDir::in_runs(&runs, Path::new("another"), "second");
        assert!(first.dir.is_dir());
    }

    #[test]
    fn the_last_test_to_use_a_runs_folder_deletes_it() {
        let scratch = Scratch::new("history-end");
        let runs = scratch.dir.join("runs");
        let run = Path::new("running");
        let first = HistoryDir::in_runs(&runs, run, "first");
        let second = HistoryDir::in_runs(&runs, run, "second");
        drop(first);
        assert!(second.dir.is_dir());
        drop(second);
        assert!(!runs.join(run).exists());
    }
}
