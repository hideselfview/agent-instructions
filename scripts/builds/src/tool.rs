//! What one command runs with: where things are, the rules, the moment it
//! started, and how it learns sizes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::checkout::{Root, all_roots};
use crate::daemon::client;
use crate::fail::Result;
use crate::places::{Places, Rules};
use crate::usage::{Measure, measure_many};

pub struct Tool {
    pub places: Places,
    pub rules: Rules,
    /// Seconds since 1970 when the command started.
    pub now: i64,
    /// Sizes measured ahead, for `status`, which reads every folder once.
    measured: HashMap<PathBuf, Measure>,
}

impl Tool {
    pub fn new(places: Places, rules: Rules) -> Tool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_secs() as i64;
        Tool { places, rules, now, measured: HashMap::new() }
    }

    /// <path>'s size and newest write, counting every write made before
    /// the call: measured ahead, else the daemon's answer, else measured now.
    pub fn measure(&self, path: &Path) -> Result<Measure> {
        Ok(self.measure_all(&[path.to_path_buf()])?.remove(0))
    }

    /// Each of <paths>' size and newest write, counting every write made
    /// before the call: measured ahead, else the daemon's answers, asked
    /// together, else measured now, several at a time. Deciding commands
    /// never measure ahead, so a size after their own deletions is fresh.
    pub fn measure_all(&self, paths: &[PathBuf]) -> Result<Vec<Measure>> {
        let wanted: Vec<PathBuf> = paths.iter().filter(|p| !self.measured.contains_key(*p)).cloned().collect();
        let learned = self.learn(&wanted, true)?;
        Ok(paths
            .iter()
            .map(|path| match self.measured.get(path) {
                Some(measured) => *measured,
                None => learned[wanted.iter().position(|w| w == path).expect("asked above")],
            })
            .collect())
    }

    /// <paths>' sizes from the daemon (<fresh>: see `Question`), the rest
    /// measured here.
    fn learn(&self, paths: &[PathBuf], fresh: bool) -> Result<Vec<Measure>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let answers =
            client::measure(&self.places, paths, fresh).unwrap_or_else(|| paths.iter().map(|_| None).collect());
        let unanswered: Vec<PathBuf> =
            paths.iter().zip(&answers).filter(|(_, a)| a.is_none()).map(|(p, _)| p.clone()).collect();
        let mut measured = measure_many(&unanswered)?.into_iter();
        Ok(answers
            .into_iter()
            .map(|answer| answer.unwrap_or_else(|| measured.next().expect("one measure per unanswered path")))
            .collect())
    }

    /// Learns every one of <paths> now, for `measure` to answer from: the
    /// daemon's answers without waiting for the last second's writes (status
    /// shows them; nothing decides on them), the rest measured.
    pub fn measure_ahead(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let learned = self.learn(&paths, false)?;
        self.measured.extend(paths.into_iter().zip(learned));
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
        crate::fail!("reading the free space of {}: {}", path.display(), std::io::Error::last_os_error());
    }
    let block = stats.f_bsize as u64;
    Ok(Space { free_kib: stats.f_bavail * block / 1024, size_kib: stats.f_blocks * block / 1024 })
}
