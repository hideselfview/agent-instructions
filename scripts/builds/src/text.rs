//! The tool's ways of saying things: notes on stderr, sizes, local times,
//! and directory listings in the order a shell glob gives them.

use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::fail::{Context, Result};

/// A line on stderr, `builds: <message>`, which agents and hooks read.
pub fn note(message: impl AsRef<str>) {
    eprintln!("builds: {}", message.as_ref());
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

/// `1 build copy`, `3 build copies`.
pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// <epoch> as local `%Y-%m-%d %H:%M`, the form claims and status use.
pub fn local_minute(epoch: i64) -> String {
    strftime(epoch, c"%Y-%m-%d %H:%M")
}

/// <epoch> as local `%Y-%m-%d`.
pub fn local_day(epoch: i64) -> String {
    strftime(epoch, c"%Y-%m-%d")
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

fn strftime(epoch: i64, format: &std::ffi::CStr) -> String {
    let time: libc::time_t = epoch;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let mut buffer = [0u8; 64];
    unsafe {
        libc::localtime_r(&time, &mut tm);
        let written = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            format.as_ptr(),
            &tm,
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

#[cfg(test)]
mod tests {
    use super::*;

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
