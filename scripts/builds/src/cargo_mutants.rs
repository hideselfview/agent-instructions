//! `builds cargo mutants`: cargo-mutants through builds. It runs on the host
//! with the most free cores like other cargo work (cargo_place.rs), and
//! builds caps its `--jobs` by what the host has room for under its load
//! band, since each job builds and tests a whole copy of the workspace and an
//! uncapped run holds the host's cores for hours.
//!
//! cargo-mutants has no `--message-format`, so builds reads none of its
//! output and records no build copies for it. It takes cargo's own settings
//! (`--locked`, `--config`) as `--cargo-arg` words, which `cargo_args` makes.

use crate::host_band::Band;

/// How many jobs a run may use on a host whose 5-minute load average is
/// <load> and whose band is <band>: the room under the band's high end,
/// rounded down, and at least one.
pub fn jobs_cap(load: f64, band: Band) -> u32 {
    let room = (band.high - load).floor();
    if room < 1.0 { 1 } else { room as u32 }
}

/// <args> with the run's jobs at most <cap>, and a line that says so when it
/// lowered them. A run that names no jobs keeps cargo-mutants' one. The
/// arguments after `--` are cargo test's own.
pub fn capped(args: &[String], cap: u32) -> (Vec<String>, Option<String>) {
    let mut out = Vec::new();
    let mut note = None;
    let mut words = args.iter().enumerate();
    let mut own = true;
    while let Some((at, word)) = words.next() {
        if !own {
            out.push(word.clone());
            continue;
        }
        if word == "--" {
            own = false;
            out.push(word.clone());
            continue;
        }
        let (joined, requested) = match word.as_str() {
            "--jobs" | "-j" => match args.get(at + 1) {
                Some(value) => {
                    words.next();
                    (Joined::Apart(word.clone()), value.parse::<u32>().ok())
                }
                None => (Joined::Apart(word.clone()), None),
            },
            other => match other
                .strip_prefix("--jobs=")
                .map(|value| ("--jobs=", value))
                .or_else(|| other.strip_prefix("-j").map(|value| ("-j", value)))
            {
                Some((prefix, value)) => (Joined::Together(prefix), value.parse::<u32>().ok()),
                None => {
                    out.push(word.clone());
                    continue;
                }
            },
        };
        let Some(requested) = requested else {
            // Not a number: cargo-mutants refuses it.
            match joined {
                Joined::Apart(flag) => {
                    out.push(flag);
                    if let Some(value) = args.get(at + 1) {
                        out.push(value.clone());
                    }
                }
                Joined::Together(_) => out.push(word.clone()),
            }
            continue;
        };
        let jobs = requested.min(cap);
        if jobs < requested {
            note = Some(format!(
                "builds cargo mutants runs {jobs} jobs, not the {requested} asked for: the host has room for {cap} under its load band"
            ));
        }
        match joined {
            Joined::Apart(flag) => {
                out.push(flag);
                out.push(jobs.to_string());
            }
            Joined::Together(prefix) => out.push(format!("{prefix}{jobs}")),
        }
    }
    (out, note)
}

/// How a jobs argument was written.
enum Joined {
    /// `--jobs 4`: the flag, then its value.
    Apart(String),
    /// `--jobs=4` or `-j4`: the flag's own prefix.
    Together(&'static str),
}

/// cargo-mutants' words for cargo's settings <words> (`--locked`, `--config`
/// and its value): each is one `--cargo-arg=<word>`, which cargo-mutants adds
/// to every cargo command it runs.
pub fn cargo_args<'a>(words: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    words
        .into_iter()
        .map(|word| format!("--cargo-arg={word}"))
        .collect()
}

#[cfg(test)]
#[path = "cargo_mutants_tests.rs"]
mod tests;
