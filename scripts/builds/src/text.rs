//! The tool's ways of saying things: notes on stderr, sizes, local times,
//! words of a command written for a shell, and directory listings in the
//! order a shell glob gives them.

use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::fail::{Context, Result};

/// A line on stderr, `builds: <message>`, which agents and hooks read.
pub fn note(message: impl AsRef<str>) {
    eprintln!("builds: {}", message.as_ref());
}

/// <arg> as a shell would need it written, for a command the person reruns.
pub fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// The local time as `19:20:05`, for a log line that must say when.
pub fn clock() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut parts) };
    format!(
        "{:02}:{:02}:{:02}",
        parts.tm_hour, parts.tm_min, parts.tm_sec
    )
}

const GIB_IN_KIB: u64 = 1024 * 1024;

/// <kib> as `9.3G` from a GiB up, else whole MiB rounded down (`769M`).
pub fn human(kib: u64) -> String {
    if kib >= GIB_IN_KIB {
        format!("{:.1}G", kib as f64 / GIB_IN_KIB as f64)
    } else {
        format!("{}M", kib / 1024)
    }
}

/// <bytes> the way `human` says KiB.
pub fn human_bytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1}G", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{}M", bytes >> 20)
    }
}

/// The whole minutes from <then> to <now>, both in seconds since 1970; zero
/// when <then> is later.
pub fn minutes_since(now: i64, then: i64) -> i64 {
    (now - then).max(0) / 60
}

/// <seconds> as `6d2h`, `3h05m` or `12m`.
pub fn duration(seconds: i64) -> String {
    let (days, hours, mins) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins:02}m")
    } else {
        format!("{mins}m")
    }
}

/// <text> on one line: each run of whitespace, line breaks included, as one
/// space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `1 build copy`, `3 build copies`.
pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// <epoch> as local `%Y-%m-%d %H:%M`, the form claims and status use.
pub fn local_minute(epoch: i64) -> String {
    strftime(epoch, c"%Y-%m-%d %H:%M")
}

/// The epoch of a local `%Y-%m-%d %H:%M` form (`local_minute`'s), or None
/// when <text> is not one.
pub fn local_minute_epoch(text: &str) -> Option<i64> {
    let text = std::ffi::CString::new(text).ok()?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let rest = unsafe { libc::strptime(text.as_ptr(), c"%Y-%m-%d %H:%M".as_ptr(), &mut tm) };
    if rest.is_null() || unsafe { *rest } != 0 {
        return None;
    }
    tm.tm_isdst = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    (epoch != -1).then_some(epoch)
}

/// <epoch> as local `%Y-%m-%d`.
pub fn local_day(epoch: i64) -> String {
    strftime(epoch, c"%Y-%m-%d")
}

/// Local midnight at the start of the day that a `--since` flag names
/// (<day>, as `YYYY-MM-DD`); None when the command got no `--since`. A day
/// that is not a real one fails, naming the flag.
pub fn since_start(day: Option<&str>) -> Result<Option<i64>> {
    match day {
        None => Ok(None),
        Some(day) => match local_start_of(day) {
            Some(start) => Ok(Some(start)),
            None => crate::fail!("--since takes a day as YYYY-MM-DD, not {day:?}"),
        },
    }
}

/// Local midnight at the start of <day> (`YYYY-MM-DD`) as seconds since 1970;
/// None unless it is a real day.
pub fn local_start_of(day: &str) -> Option<i64> {
    let (year, month, date) = crate::lists::day::parse(day)?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (year - 1900) as libc::c_int;
    tm.tm_mon = (month - 1) as libc::c_int;
    tm.tm_mday = date as libc::c_int;
    tm.tm_isdst = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    (epoch != -1).then_some(epoch)
}

/// <epoch> in UTC as ISO 8601, `2026-11-18T19:20:05Z`; fails when the C
/// library cannot break it into a date, as for a year it cannot hold.
pub fn utc_second(epoch: i64) -> Result<String> {
    let time: libc::time_t = epoch;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::gmtime_r(&time, &mut tm) }.is_null() {
        crate::fail!("the time {epoch} has no date in UTC");
    }
    Ok(format_tm(&tm, c"%Y-%m-%dT%H:%M:%SZ"))
}

fn strftime(epoch: i64, format: &std::ffi::CStr) -> String {
    let time: libc::time_t = epoch;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time, &mut tm) };
    format_tm(&tm, format)
}

fn format_tm(tm: &libc::tm, format: &std::ffi::CStr) -> String {
    let mut buffer = [0u8; 64];
    unsafe {
        let written = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            format.as_ptr(),
            tm,
        );
        String::from_utf8_lossy(&buffer[..written]).into_owned()
    }
}

/// Status's `last write` column: the time, or `-` for a folder with nothing
/// written in it (an empty one).
pub fn last_write(epoch: Option<i64>) -> String {
    match epoch {
        Some(when) if when != 0 => local_minute(when),
        _ => "-".to_string(),
    }
}

/// Makes `strcoll` compare the way the caller's shell sorts a glob.
pub fn use_callers_collation() {
    unsafe {
        libc::setlocale(libc::LC_COLLATE, c"".as_ptr());
        libc::setlocale(libc::LC_CTYPE, c"".as_ptr());
    }
}

fn collate(a: &OsStr, b: &OsStr) -> std::cmp::Ordering {
    let a = CString::new(a.as_bytes()).expect("a file name holds no NUL");
    let b = CString::new(b.as_bytes()).expect("a file name holds no NUL");
    unsafe { libc::strcoll(a.as_ptr(), b.as_ptr()) }.cmp(&0)
}

/// The names in <dir> in the order bash lists `<dir>/* <dir>/.[!.]*`: the
/// visible ones sorted by the locale's collation, then the hidden ones the
/// same way. A missing <dir> lists nothing.
pub fn glob_names(dir: &Path) -> Result<Vec<OsString>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context(|| format!("listing {}", dir.display())),
    };
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for entry in entries {
        let name = entry
            .context(|| format!("listing {}", dir.display()))?
            .file_name();
        let bytes = name.as_bytes();
        if bytes.first() == Some(&b'.') {
            if bytes.get(1) != Some(&b'.') && bytes.len() > 1 {
                hidden.push(name);
            }
        } else {
            visible.push(name);
        }
    }
    visible.sort_by(|a, b| collate(a, b));
    hidden.sort_by(|a, b| collate(a, b));
    visible.extend(hidden);
    Ok(visible)
}

/// <paths> in the order bash sorts a glob's matches: whole paths by the
/// locale's collation.
pub fn sort_collated(paths: &mut [std::path::PathBuf]) {
    paths.sort_by(|a, b| collate(a.as_os_str(), b.as_os_str()));
}

/// <path> with $HOME written as `~`, as the tool shows links in a home.
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A suffix for a staged file's name that no other write has used: the
/// process id (another process's writes) and a count (this process's).
pub fn staged_suffix() -> String {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The end of the file at <path>: its last <cap> bytes as text, and whether
/// the file holds more before them. When the file holds more, the text starts
/// after the first line break, since the cut falls inside a line.
pub fn file_end(path: &Path, cap: u64) -> std::io::Result<(String, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let cut = length > cap;
    if cut {
        file.seek(SeekFrom::Start(length - cap))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let text = match (cut, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_string(),
        _ => text,
    };
    Ok((text, cut))
}

/// The 32-bit FNV-1a hash of <text>: the stamp of a schema (db.rs) and the
/// version of the text agents acknowledge (ack/text.rs).
pub fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_since_flag_names_a_real_day_or_fails() {
        assert_eq!(since_start(None).unwrap(), None);
        assert_eq!(
            since_start(Some("2026-11-13")).unwrap(),
            local_start_of("2026-11-13")
        );
        let refused = since_start(Some("yesterday")).unwrap_err().text();
        assert!(refused.contains("--since"), "{refused}");
    }

    #[test]
    fn a_utc_time_is_iso_8601_or_fails() {
        assert_eq!(utc_second(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(utc_second(1_795_032_000).unwrap(), "2026-11-18T20:00:00Z");
        assert!(utc_second(i64::MAX).is_err());
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0M");
        assert_eq!(human(787_456), "769M");
        assert_eq!(human(1_048_576), "1.0G");
        assert_eq!(human(31_457_280), "30.0G");
        assert_eq!(human_bytes(3 << 29), "1.5G");
        assert_eq!(human_bytes(5 << 20), "5M");
    }
}
