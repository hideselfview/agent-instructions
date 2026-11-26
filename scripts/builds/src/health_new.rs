//! The needs-action lines of `builds health` that are new since the same
//! caller's last run. builds keeps, for each caller (an agent by its id, the
//! person's terminal as `person`), the keys of the lines its last text run
//! printed, in `health-seen/<caller>.json` in builds' caches folder. A line's
//! key is the line with what changes between runs while the line says the
//! same thing replaced: times, process ids, loads, CPU shares, sizes.

use std::path::{Path, PathBuf};

use crate::fail::Result;
use crate::health::Finding;

/// The file of the keys that <caller>'s last run printed, under <home>.
fn file(home: &Path, caller: &str) -> PathBuf {
    home.join("Library/Caches/builds/health-seen")
        .join(format!("{caller}.json"))
}

/// The name of the caller of `builds health`: its agent id, or `person` for
/// the person's terminal.
pub fn caller_name(session: &crate::ack::agent::Session) -> Result<String> {
    Ok(session
        .caller()?
        .map_or_else(|| "person".to_string(), |caller| caller.id))
}

/// The keys of the lines <caller>'s last run printed, or None when it ran no
/// text run before.
pub fn read(home: &Path, caller: &str) -> Result<Option<Vec<String>>> {
    let path = file(home, caller);
    if !path.exists() {
        return Ok(None);
    }
    crate::readings_file::read_value(&path).map(Some)
}

/// Records <findings>' keys as what <caller>'s last run printed.
pub fn write(home: &Path, caller: &str, findings: &[Finding]) -> Result<()> {
    let keys: Vec<String> = findings
        .iter()
        .map(|finding| key(&finding.advice.line()))
        .collect();
    crate::readings_file::write_value(&file(home, caller), &keys)
}

/// Marks in <out>, the lines of the report, each line of <findings> whose key
/// is not among <earlier>, the keys of the caller's last run. With no earlier
/// run, marks none and says so in the header.
pub fn mark(out: &mut [String], findings: &[Finding], earlier: Option<&[String]>) {
    let Some(header) = out.iter().position(|line| line == "needs action:") else {
        return;
    };
    let Some(earlier) = earlier else {
        out[header] = "needs action (no earlier run of yours to compare with):".to_string();
        return;
    };
    for (finding, line) in findings.iter().zip(&mut out[header + 1..]) {
        if !earlier.contains(&key(&finding.advice.line())) {
            *line = format!("    new: {}", line.trim_start());
        }
    }
}

/// <line> with each word that changes between runs replaced: times and
/// dates, process ids, decimals (loads, CPU seconds), percentages and sizes.
fn key(line: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let mut before = String::new();
    for c in line.chars().chain(std::iter::once('\n')) {
        if c.is_alphanumeric() || matches!(c, '.' | ':' | '%' | '-' | '_') {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out.push_str(&replaced(&word, &before));
            before = std::mem::take(&mut word);
        }
        out.push(c);
    }
    out.pop();
    out
}

/// The word <word>, or what stands for it when it changes between runs;
/// <before> is the word before it.
fn replaced(word: &str, before: &str) -> String {
    let core = word.trim_end_matches(['.', ':', '-']);
    let tail = &word[core.len()..];
    let stand = if is_time(core) {
        "<time>"
    } else if core.starts_with("snapshot-") {
        "<snapshot>"
    } else if matches!(before, "pid" | "process") && is_digits(core) {
        "<pid>"
    } else if is_decimal(core) || is_percent(core) {
        "<n>"
    } else if is_size(core) {
        "<size>"
    } else {
        return word.to_string();
    };
    format!("{stand}{tail}")
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `6d4h`, `1h23m`, `0m`, `12:30`, `12:30:05` or `2026-11-26`.
fn is_time(text: &str) -> bool {
    let units = |text: &str| {
        let mut rest = text;
        let mut pairs = 0;
        while !rest.is_empty() {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let unit = rest.as_bytes().get(digits).copied();
            if digits == 0 || !matches!(unit, Some(b'd' | b'h' | b'm' | b's')) {
                return false;
            }
            rest = &rest[digits + 1..];
            pairs += 1;
        }
        pairs > 0
    };
    let parts = |text: &str, sep: char, lens: &[usize]| {
        let found: Vec<&str> = text.split(sep).collect();
        found.len() == lens.len()
            && found
                .iter()
                .zip(lens)
                .all(|(part, len)| part.len() == *len && is_digits(part))
    };
    units(text)
        || parts(text, ':', &[2, 2])
        || parts(text, ':', &[2, 2, 2])
        || parts(text, '-', &[4, 2, 2])
}

/// `10.6`, or `1.2s`: digits, a point and digits, then letters.
fn is_decimal(text: &str) -> bool {
    let Some((whole, rest)) = text.split_once('.') else {
        return false;
    };
    let fraction = rest.bytes().take_while(u8::is_ascii_digit).count();
    is_digits(whole) && fraction > 0 && rest[fraction..].bytes().all(|b| b.is_ascii_alphabetic())
}

/// `29%`.
fn is_percent(text: &str) -> bool {
    text.strip_suffix('%').is_some_and(is_digits)
}

/// `14M`, `906M` or `3G`.
fn is_size(text: &str) -> bool {
    text.strip_suffix(['K', 'M', 'G', 'T'])
        .is_some_and(is_digits)
}

#[cfg(test)]
#[path = "health_new_tests.rs"]
mod tests;
