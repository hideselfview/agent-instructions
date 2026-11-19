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

/// The stat of <pid>; None when the process has exited (its folder in /proc
/// is gone). Fails when the file cannot be read for another reason or does
/// not hold the fields `Stat` names.
fn read_stat(pid: i32) -> Result<Option<Stat>> {
    let path = format!("/proc/{pid}/stat");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // ESRCH: the process exited while the kernel wrote the file.
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return Ok(None);
        }
        Err(error) => fail!("reading {path}: {error}"),
    };
    let parsed = (|| {
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
    })();
    match parsed {
        Some(stat) => Ok(Some(stat)),
        None => fail!("{path} does not hold the fields builds reads: {text:?}"),
    }
}

/// Field <number> of a stat file, counted from 1, out of <rest>, the fields
/// after the name: `rest[0]` is the third field, the state.
fn field<T: std::str::FromStr>(rest: &[&str], number: usize) -> Option<T> {
    rest.get(number - 3)?.parse().ok()
}

/// The value of sysconf's <name>, which <what> names; fails when the system
/// does not give one.
fn sysconf(name: libc::c_int, what: &str) -> Result<u64> {
    // SAFETY: sysconf reads a system setting and touches no memory of ours.
    let value = unsafe { libc::sysconf(name) };
    match u64::try_from(value) {
        Ok(value) if value > 0 => Ok(value),
        _ => fail!(
            "sysconf gave no {what} ({value}): {}",
            std::io::Error::last_os_error()
        ),
    }
}

/// The kernel's clock ticks per second.
fn ticks_per_second() -> Result<f64> {
    Ok(sysconf(libc::_SC_CLK_TCK, "clock ticks per second")? as f64)
}

/// <result>'s value, for the readings that answer None when a process cannot
/// be read: a failure says why on stderr, so a reading that fails for a
/// reason other than the process's exit is not silent.
fn or_note<T>(result: Result<T>) -> Option<T> {
    result.map_err(|error| crate::text::note(error.text())).ok()
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
    let ticks = ticks_per_second()?;
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
        let Some(stat) = read_stat(pid)? else {
            continue;
        };
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("reading /proc/{pid}: {error}"),
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
    match or_note(read_stat(pid)).flatten() {
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

/// The environment <pid> started with (/proc/<pid>/environ), as `NAME=value`
/// strings; None when it exited or can't be read.
pub fn environment(pid: i32) -> Option<Vec<String>> {
    let bytes = match std::fs::read(format!("/proc/{pid}/environ")) {
        Ok(bytes) => bytes,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return None;
        }
        Err(error) => {
            crate::text::note(format!(
                "builds cannot read the environment of pid {pid}: {error}"
            ));
            return None;
        }
    };
    Some(
        bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}

/// CPU time used so far, in seconds, and the resident memory in bytes.
pub fn usage(pid: i32) -> Option<(f64, u64)> {
    let stat = or_note(read_stat(pid)).flatten()?;
    let page = or_note(sysconf(libc::_SC_PAGESIZE, "page size"))?;
    Some((
        (stat.utime + stat.stime) as f64 / or_note(ticks_per_second())?,
        stat.rss_pages * page,
    ))
}

/// CPU time <pid> and every child it has waited for have used, in seconds:
/// a finished child's time counts in its parent's, so the sum over a live
/// tree is all the CPU the tree has used.
pub fn tree_cpu(pid: i32) -> Option<f64> {
    let stat = or_note(read_stat(pid)).flatten()?;
    Some(
        (stat.utime + stat.stime + stat.cutime + stat.cstime) as f64 / or_note(ticks_per_second())?,
    )
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
