//! Linux's side of procs.rs: the machine's processes, read from /proc.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::Process;
use crate::fail;
use crate::fail::{Context, Result};

/// The fields of /proc/<pid>/stat that builds reads. The kernel writes the
/// command's name in parentheses as the second field, and the name may hold
/// spaces and parentheses, so the fields after it are read from after the
/// last `)`.
struct Stat {
    name: String,
    /// One letter: `Z` for a zombie.
    state: String,
    ppid: i32,
    /// CPU time in clock ticks: the process's own, user and system, and that
    /// of the children it has waited for.
    utime: u64,
    stime: u64,
    cutime: u64,
    cstime: u64,
    /// When the process started, in clock ticks since the machine booted.
    start_ticks: u64,
    /// Resident pages.
    rss_pages: u64,
}

fn read_stat(pid: i32) -> Option<Stat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    Some(Stat {
        name,
        state: rest.first()?.to_string(),
        ppid: field(&rest, 4)?,
        utime: field(&rest, 14)?,
        stime: field(&rest, 15)?,
        cutime: field(&rest, 16)?,
        cstime: field(&rest, 17)?,
        start_ticks: field(&rest, 22)?,
        rss_pages: field(&rest, 24)?,
    })
}

/// Field <number> of a stat file, counted from 1, out of <rest>, the fields
/// after the name: `rest[0]` is the third field, the state.
fn field<T: std::str::FromStr>(rest: &[&str], number: usize) -> Option<T> {
    rest.get(number - 3)?.parse().ok()
}

/// The kernel's clock ticks per second.
fn ticks_per_second() -> f64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as f64 } else { 100.0 }
}

/// When the machine booted, seconds since 1970 (`btime` in /proc/stat).
fn boot_time() -> Result<i64> {
    let text =
        std::fs::read_to_string("/proc/stat").context(|| "reading /proc/stat".to_string())?;
    match text
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|value| value.trim().parse().ok())
    {
        Some(at) => Ok(at),
        None => fail!("/proc/stat names no boot time (btime)"),
    }
}

/// Every process this user can read. Fails when /proc cannot be listed, so
/// no caller takes a short list for the whole machine.
pub fn list() -> Result<Vec<Process>> {
    let booted = boot_time()?;
    let ticks = ticks_per_second();
    let entries = std::fs::read_dir("/proc").context(|| "listing /proc".to_string())?;
    let mut list = Vec::new();
    for entry in entries {
        let entry = entry.context(|| "listing /proc".to_string())?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        // A process that exited since it was listed has nothing to say.
        let (Some(stat), Ok(meta)) = (read_stat(pid), entry.metadata()) else {
            continue;
        };
        list.push(Process {
            pid,
            ppid: stat.ppid,
            uid: meta.uid(),
            name: stat.name,
            started: booted + (stat.start_ticks as f64 / ticks) as i64,
        });
    }
    Ok(list)
}

/// Has <pid> exited, as a zombie waiting for its parent or gone altogether?
/// The kernel keeps a zombie's pid, so `runs` says it runs, but it has no
/// arguments or working directory left to read.
pub fn ended(pid: i32) -> bool {
    match read_stat(pid) {
        Some(stat) => stat.state == "Z",
        None => !super::runs(pid),
    }
}

/// The working directory of <pid>; None when it exited or can't be read.
pub fn cwd(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The arguments <pid> was started with (/proc/<pid>/cmdline); None when it
/// exited or can't be read. A zombie's command line is empty.
pub fn args(pid: i32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    Some(
        bytes
            .split(|b| *b == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}

/// CPU time used so far, in seconds, and the resident memory in bytes.
pub fn usage(pid: i32) -> Option<(f64, u64)> {
    let stat = read_stat(pid)?;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    Some((
        (stat.utime + stat.stime) as f64 / ticks_per_second(),
        stat.rss_pages * page,
    ))
}

/// CPU time <pid> and every child it has waited for have used, in seconds:
/// a finished child's time counts in its parent's, so the sum over a live
/// tree is all the CPU the tree has used.
pub fn tree_cpu(pid: i32) -> Option<f64> {
    let stat = read_stat(pid)?;
    Some((stat.utime + stat.stime + stat.cutime + stat.cstime) as f64 / ticks_per_second())
}

/// The paths of the files <pid> has open: each descriptor in
/// /proc/<pid>/fd that leads to a path, not to a socket, pipe or other
/// kernel object.
pub fn open_files(pid: i32) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|path| path.is_absolute() && Path::new(path).exists())
        .collect()
}
