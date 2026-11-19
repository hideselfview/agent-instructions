//! Each host's sccache server as its scan shows it (host_scan.rs): the
//! compiles it runs (its child processes), the clients that wait for one
//! (the other processes named sccache that name a compiler), how full its
//! cache is, and the share of Rust compiles its cache answered since a
//! reading at least READING_MINUTES old. `builds health` prints a line for
//! each host, and lists under "needs action" a host whose waiting clients
//! number more than WAITING_PER_CORE times its cores: it has more compile
//! work than it can run, so new agents only wait longer.
//!
//! Health keeps a reading of each server's counts at most once a minute, for
//! an hour, in the file `sccache-host-readings.json` in builds' caches
//! folder, so the share covers the last few minutes, not the server's whole
//! life. A server that started again since a reading (another pid, or counts
//! that went down) is compared with nothing, and its share covers its life.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fail::{Context, Fail, Result};
use crate::health::Advice;
use crate::host::{Host, place};
use crate::host_scan::{CacheStats, Scan, arguments, program};
use crate::text::{human_bytes, local_minute};

/// Readings this old or older say what the cache answered since.
const READING_MINUTES: i64 = 5;
/// More waiting clients than this many per core is more compile work than
/// the host can run.
const WAITING_PER_CORE: usize = 2;
/// A cache this share of its most or more is full: each new entry pushes an
/// old one out.
const FULL_SHARE: f64 = 0.99;

/// One server's counts at one time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub host: String,
    pub server: i32,
    /// Seconds since 1970.
    pub at: i64,
    pub hits: u64,
    pub misses: u64,
}

/// The Rust compiles a server's cache answered (hits) and those it ran
/// (misses), since <since> (seconds since 1970), or since the server started
/// when <since> is None.
#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    pub since: Option<i64>,
    pub hits: u64,
    pub misses: u64,
}

impl Rate {
    /// `50% of 824 Rust compiles since 01:40`.
    fn text(&self) -> String {
        let since = match self.since {
            Some(at) => format!("since {}", local_minute(at)),
            None => "since the server started".to_string(),
        };
        match self.hits + self.misses {
            0 => format!("no Rust compile {since}"),
            all => format!(
                "{:.0}% of {all} Rust compiles {since}",
                self.hits as f64 * 100.0 / all as f64
            ),
        }
    }
}

/// A host's server, as its scan shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub pid: i32,
    /// The compiles it runs: its child processes.
    pub running: usize,
    /// The clients: processes named sccache that name a compiler.
    pub clients: usize,
    pub stats: CacheStats,
    pub rate: Rate,
}

impl Server {
    /// The clients that wait for a compile: those beyond the compiles the
    /// server runs.
    pub fn waiting(&self) -> usize {
        self.clients.saturating_sub(self.running)
    }

    fn full(&self) -> bool {
        self.stats.cache_size as f64 >= self.stats.max_cache_size as f64 * FULL_SHARE
    }
}

/// A host and what its scan showed of its server: None when no server
/// listens, Err when the scan could not tell.
#[derive(Clone, Debug, PartialEq)]
pub struct Cache {
    pub host: Host,
    pub server: std::result::Result<Option<Server>, String>,
}

/// The rate of the server <pid> on <host> with <stats>, against the newest
/// of <readings> of that server at least READING_MINUTES before <now>.
fn rate(readings: &[Reading], host: &str, pid: i32, stats: &CacheStats, now: i64) -> Rate {
    let since_start = Rate {
        since: None,
        hits: stats.rust_hits,
        misses: stats.rust_misses,
    };
    let Some(before) = readings
        .iter()
        .filter(|r| r.host == host && r.server == pid && now - r.at >= READING_MINUTES * 60)
        .max_by_key(|r| r.at)
    else {
        return since_start;
    };
    match (
        stats.rust_hits.checked_sub(before.hits),
        stats.rust_misses.checked_sub(before.misses),
    ) {
        (Some(hits), Some(misses)) => Rate {
            since: Some(before.at),
            hits,
            misses,
        },
        _ => since_start,
    }
}

/// Adds <reading> to <readings> when the newest of its host is a minute old
/// or more, and keeps the last hour. Returns whether it added it.
fn record(readings: &mut Vec<Reading>, reading: Reading) -> bool {
    let now = reading.at;
    if readings
        .iter()
        .any(|r| r.host == reading.host && now - r.at < 60)
    {
        return false;
    }
    readings.push(reading);
    readings.retain(|r| now - r.at <= 3600);
    true
}

/// The server each of <scans> shows, with its rate against <readings>, which
/// gain a reading of each server.
fn judge_with(
    scans: &[(Host, std::result::Result<Scan, String>)],
    readings: &mut Vec<Reading>,
    now: i64,
) -> (Vec<Cache>, bool) {
    let mut changed = false;
    let caches = scans
        .iter()
        .filter_map(|(host, scan)| {
            // stuck.rs lists a host whose scan failed.
            let scan = scan.as_ref().ok()?;
            let server = scan.sccache.clone().map(|server| {
                server.map(|server| {
                    let rate = rate(readings, &host.name, server.pid, &server.stats, now);
                    changed |= record(
                        readings,
                        Reading {
                            host: host.name.clone(),
                            server: server.pid,
                            at: now,
                            hits: server.stats.rust_hits,
                            misses: server.stats.rust_misses,
                        },
                    );
                    Server {
                        pid: server.pid,
                        running: scan
                            .rows
                            .iter()
                            .filter(|row| row.ppid == server.pid)
                            .count(),
                        clients: scan
                            .rows
                            .iter()
                            .filter(|row| {
                                row.pid != server.pid
                                    && program(row) == "sccache"
                                    && !arguments(row).is_empty()
                            })
                            .count(),
                        stats: server.stats,
                        rate,
                    }
                })
            });
            Some(Cache {
                host: host.clone(),
                server,
            })
        })
        .collect();
    (caches, changed)
}

/// The file of readings in builds' caches folder under <home>.
fn readings_file(home: &Path) -> PathBuf {
    home.join("Library/Caches/builds/sccache-host-readings.json")
}

/// Each host's server in <scans>, with the share of Rust compiles its cache
/// answered since a reading kept in builds' caches folder under <home>, to
/// which it adds a reading of each server.
pub fn judge(
    home: &Path,
    scans: &[(Host, std::result::Result<Scan, String>)],
    now: i64,
) -> Result<Vec<Cache>> {
    let file = readings_file(home);
    let mut readings: Vec<Reading> = match std::fs::read(&file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).context(|| format!("reading {}", file.display()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(Fail::Message(format!(
                "reading {}: {error}",
                file.display()
            )));
        }
    };
    let (caches, changed) = judge_with(scans, &mut readings, now);
    if changed {
        let dir = file.parent().expect("the file has a folder");
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
        crate::whole_file::write(
            &file,
            serde_json::to_string(&readings).expect("readings serialize"),
        )?;
    }
    Ok(caches)
}

/// The line `builds health` prints for each of <caches>.
pub fn lines(caches: &[Cache]) -> Vec<String> {
    caches
        .iter()
        .map(|cache| {
            let place = place(&cache.host.name);
            match &cache.server {
                Err(error) => format!("compile cache on {place}: not read: {error}"),
                Ok(None) => format!("compile cache on {place}: no sccache server listens"),
                Ok(Some(server)) => format!(
                    "compile cache on {place} (sccache pid {}): {} compiles run, {} clients wait; it answered {}; cache {} of {}",
                    server.pid,
                    server.running,
                    server.waiting(),
                    server.rate.text(),
                    human_bytes(server.stats.cache_size),
                    human_bytes(server.stats.max_cache_size),
                ),
            }
        })
        .collect()
}

/// The lines under "needs action": each host whose server has more than
/// WAITING_PER_CORE waiting clients per core.
pub fn findings(caches: &[Cache]) -> Vec<Advice> {
    caches
        .iter()
        .filter_map(|cache| {
            let server = cache.server.as_ref().ok()?.as_ref()?;
            let cores = cache.host.cores.max(1) as usize;
            if server.waiting() <= WAITING_PER_CORE * cores {
                return None;
            }
            let place = place(&cache.host.name);
            let full = if server.full() {
                format!(
                    "; its cache is full ({} of {}), so each new entry pushes an old one out",
                    human_bytes(server.stats.cache_size),
                    human_bytes(server.stats.max_cache_size)
                )
            } else {
                String::new()
            };
            Some(Advice::said(format!(
                "sccache: {place}'s server has {} clients waiting while it runs {} compiles, more than {WAITING_PER_CORE} for each of the host's {cores} cores, so the host has more compile work than it can run; its cache answered {}{full}; start nothing new for {place}",
                server.waiting(),
                server.running,
                server.rate.text(),
            )))
        })
        .collect()
}

#[cfg(test)]
#[path = "sccache_queue_tests.rs"]
mod tests;
