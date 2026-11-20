//! The sleeps a Mac records in its power log, which say why a connection to
//! a host ended.
//!
//! A Mac that sleeps answers nothing on its connections, and the host at the
//! other end ends a connection that gets no answer (awake.rs). ssh then
//! exits 255 with "Connection reset by peer" or "Broken pipe", which names
//! neither Mac. So when ssh exits 255 under a command builds runs on a host
//! (remote.rs), builds reads the power log of this Mac and of the host
//! (`pmset -g log`), and says each sleep either one recorded while the
//! command ran: when it began, how long it lasted, the reason macOS gave and
//! whether the Mac ran on battery. A line of the log that records a sleep
//! reads:
//!
//! ```text
//! 2026-11-20 12:51:20 -0500 Sleep  Entering Sleep state due to 'Clamshell Sleep':TCPKeepAlive=active Using Batt (Charge:69%) 58 secs
//! ```
//!
//! where the last number is how long the Mac slept.

use crate::host::{self, Accept, Host, Ssh, Terminal};
use crate::text::local_second;

/// Prints the lines of the power log that record a sleep. The log of a Mac
/// that has run for weeks holds tens of megabytes, so the host picks the
/// lines out itself; holding the log first keeps a failing pmset's status.
const SCRIPT: &str = r#"log=$(pmset -g log) || exit 1
printf '%s\n' "$log" | awk '$4 == "Sleep"'"#;

/// One sleep a Mac's power log records.
#[derive(Debug, PartialEq)]
pub struct Sleep {
    /// When the sleep began, in seconds since 1970.
    pub at: i64,
    /// How long the Mac slept; None when the log does not say yet.
    pub seconds: Option<i64>,
    /// The reason macOS gave, such as `Clamshell Sleep` or `Idle Sleep`.
    pub reason: String,
    pub on_battery: bool,
}

impl Sleep {
    /// `at 2026-11-20 12:51:20 for 58 seconds (Clamshell Sleep, on battery)`.
    fn text(&self) -> String {
        let length = match self.seconds {
            Some(seconds) => format!(" for {seconds} seconds"),
            None => String::new(),
        };
        let power = if self.on_battery {
            "on battery"
        } else {
            "on power"
        };
        format!(
            "at {}{length} ({}, {power})",
            local_second(self.at),
            self.reason
        )
    }

    /// Whether the sleep began or lasted into <from>..=<to>.
    fn overlaps(&self, from: i64, to: i64) -> bool {
        self.at <= to && self.at + self.seconds.unwrap_or(0) >= from
    }
}

/// The sleeps the lines `SCRIPT` printed record. Fails, naming the line, on
/// a line that is not a sleep entry of `pmset -g log`.
pub fn parse(text: &str) -> std::result::Result<Vec<Sleep>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_line(line).ok_or_else(|| format!("not a sleep of pmset's log: {line:?}")))
        .collect()
}

fn parse_line(line: &str) -> Option<Sleep> {
    let mut words = line.split_whitespace();
    let (date, time, zone, kind) = (words.next()?, words.next()?, words.next()?, words.next()?);
    if kind != "Sleep" {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    let message = rest.join(" ");
    let reason = message
        .split_once("due to '")
        .and_then(|(_, after)| after.split_once('\''))
        .map(|(reason, _)| reason.to_string())?;
    let seconds = match rest[..] {
        [.., count, "secs"] => Some(count.parse().ok()?),
        _ => None,
    };
    Some(Sleep {
        at: epoch_of(date, time, zone)?,
        seconds,
        reason,
        on_battery: message.to_ascii_lowercase().contains("using batt"),
    })
}

/// The epoch of a local time `2026-11-20 12:51:20` at the offset `-0500`.
fn epoch_of(date: &str, time: &str, zone: &str) -> Option<i64> {
    let text = std::ffi::CString::new(format!("{date} {time}")).ok()?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let rest = unsafe { libc::strptime(text.as_ptr(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &mut tm) };
    if rest.is_null() || unsafe { *rest } != 0 {
        return None;
    }
    let as_utc = unsafe { libc::timegm(&mut tm) };
    let (sign, digits) = match zone.as_bytes().first()? {
        b'+' => (1, &zone[1..]),
        b'-' => (-1, &zone[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(as_utc - sign * (hours * 3600 + minutes * 60))
}

/// The sleeps <host>'s power log records from <from> to <to> (this Mac's
/// for host local), or what stopped builds reading the log.
fn read(ssh: &Ssh, host: &Host, from: i64, to: i64) -> std::result::Result<Vec<Sleep>, String> {
    let printed = host::output_of(
        host::command_at(ssh, host, SCRIPT, Terminal::None),
        "reading the power log",
        Accept::Zero,
    )?;
    Ok(parse(&printed)?
        .into_iter()
        .filter(|sleep| sleep.overlaps(from, to))
        .collect())
}

/// Why ssh to <host> exited 255 under a command that ran from <from> to
/// <to>: the sleeps this Mac's power log and the host's record in that time.
pub fn why_dropped(ssh: &Ssh, host: &Host, from: i64, to: i64) -> String {
    explain(
        &host.name,
        read(ssh, &Host::local(), from, to),
        read(ssh, host, from, to),
    )
}

/// What `why_dropped` says, from the sleeps this Mac's log records (<here>)
/// and host <name>'s (<there>), or what stopped builds reading each.
fn explain(
    name: &str,
    here: std::result::Result<Vec<Sleep>, String>,
    there: std::result::Result<Vec<Sleep>, String>,
) -> String {
    let mut slept = false;
    let mut said = Vec::new();
    for (whose, read) in [
        ("this Mac's".to_string(), here),
        (format!("host {name}'s"), there),
    ] {
        said.push(match read {
            Err(error) => format!("builds could not read {whose} power log: {error}"),
            Ok(sleeps) if sleeps.is_empty() => {
                format!("{whose} power log shows no sleep while the command ran")
            }
            Ok(sleeps) => {
                slept = true;
                let each: Vec<String> = sleeps.iter().map(Sleep::text).collect();
                format!(
                    "{whose} power log shows that it slept {} while the command ran",
                    each.join(" and ")
                )
            }
        });
    }
    let then = if slept {
        ". A Mac that sleeps answers nothing on its connections, so the connection ended"
    } else {
        ""
    };
    format!(
        "ssh to host {name} exited 255, as it does when its connection fails or the command exits 255: {}{then}",
        said.join("; ")
    )
}

#[cfg(test)]
#[path = "sleeps_tests.rs"]
mod tests;
