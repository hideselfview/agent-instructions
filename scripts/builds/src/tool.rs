//! What one command runs with: where things are, the rules, the moment it
//! started, and how it learns sizes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ack::agent::Session;
use crate::checkout::{Root, all_roots};
use crate::fail::Result;
use crate::places::{DiskSpace, Places, Rules};
use crate::review::Reviewer;
use crate::usage::{Measure, measure_many};

pub struct Tool {
    pub places: Places,
    pub rules: Rules,
    /// Seconds since 1970 when the command started.
    pub now: i64,
    /// The reviewer the commands that check text before they write it ask.
    pub reviewer: Reviewer,
    pub session: Session,
    /// The directory the command runs in: what it reads a claim or a
    /// repo from when its arguments name none.
    pub dir: PathBuf,
    /// This builds program (`crate::this_program`), which `builds install`
    /// copies, and which `builds design record` and `amend` run as `builds
    /// shots states` to find the component states a design change adds
    /// (`shots_program::require_states_shown`).
    pub program: PathBuf,
    /// Sizes measured ahead, for `status`, which reads every folder once.
    measured: HashMap<PathBuf, Measure>,
}

impl Tool {
    /// A tool whose builds program is <program>.
    pub fn new(places: Places, rules: Rules, program: PathBuf, session: Session) -> Result<Tool> {
        let dir = crate::lists::args::current_dir()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_secs() as i64;
        Ok(Tool {
            reviewer: Reviewer::installed(&places, &session, std::env::var_os("BUILDS_REVIEWER")),
            session,
            dir,
            program,
            places,
            rules,
            now,
            measured: HashMap::new(),
        })
    }

    /// <path>'s size and newest write, counting every write made before
    /// the call: measured ahead, else measured now.
    pub fn measure(&self, path: &Path) -> Result<Measure> {
        Ok(self.measure_all(&[path.to_path_buf()])?.remove(0))
    }

    /// Each of <paths>' size and newest write, counting every write made
    /// before the call: measured ahead, else measured now, several at a time. Deciding commands
    /// never measure ahead, so a size after their own deletions is fresh.
    pub fn measure_all(&self, paths: &[PathBuf]) -> Result<Vec<Measure>> {
        let wanted: Vec<PathBuf> = paths
            .iter()
            .filter(|p| !self.measured.contains_key(*p))
            .cloned()
            .collect();
        let learned = self.learn(&wanted)?;
        Ok(paths
            .iter()
            .map(|path| match self.measured.get(path) {
                Some(measured) => *measured,
                None => learned[wanted.iter().position(|w| w == path).expect("asked above")],
            })
            .collect())
    }

    /// <paths>' sizes measured now.
    fn learn(&self, paths: &[PathBuf]) -> Result<Vec<Measure>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        measure_many(paths)
    }

    /// Learns every one of <paths> now, for `measure` to answer from: the
    /// sizes the last command that wrote each folder recorded (sizes.rs;
    /// status shows them, nothing decides on them), the rest measured.
    pub fn measure_ahead(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let recorded = crate::sizes::recorded(&self.places, &paths)?;
        let unrecorded: Vec<PathBuf> = paths
            .iter()
            .zip(&recorded)
            .filter(|(_, size)| size.is_none())
            .map(|(path, _)| path.clone())
            .collect();
        let mut measured = self.learn(&unrecorded)?.into_iter();
        for (path, size) in paths.into_iter().zip(recorded) {
            let size = match size {
                Some(size) => size,
                None => measured.next().expect("one measure per unrecorded path"),
            };
            self.measured.insert(path, size);
        }
        Ok(())
    }

    /// Did anything write inside what <measured> measured in the last
    /// <minutes>?
    pub fn written_within(&self, measured: &Measure, minutes: u64) -> bool {
        measured.written_since(self.now - (minutes * 60) as i64)
    }

    pub fn roots(&self) -> Result<Vec<Root>> {
        all_roots(&self.places)
    }

    /// The volume's free space in KiB, as `df -k` gives Available.
    pub fn free_kib(&self) -> Result<u64> {
        Ok(volume_space(&self.places.root)?.free_kib)
    }

    /// The volume's free space against the rules' floors.
    pub fn disk_space(&self) -> Result<DiskSpace> {
        Ok(self.rules.disk_space(self.free_kib()?))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub free_kib: u64,
    pub size_kib: u64,
}

/// The free space and size of the volume holding <path> (statfs, what `df -k`
/// prints as Available and 1024-blocks).
pub fn volume_space(path: &Path) -> Result<Space> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("a path holds no NUL");
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut stats) } != 0 {
        crate::fail!(
            "reading the free space of {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    let block = stats.f_bsize as u64;
    Ok(Space {
        free_kib: stats.f_bavail * block / 1024,
        size_kib: stats.f_blocks * block / 1024,
    })
}
