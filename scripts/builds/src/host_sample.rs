//! The hosts' load and memory over time (host_sample.sql). The dashboard
//! daemon asks every host about every `EVERY_SECONDS` seconds and writes each
//! answer to every project's store, so `builds item times` can show a step's
//! host load beside the step. Each write also deletes the rows older than
//! `KEEP_DAYS` days.

use std::time::Duration;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::{Context, Result};
use crate::host::{Host, Probe, Ssh};
use crate::host_memory::Pressure;
use crate::places::Places;

/// How often the daemon asks the hosts.
pub const EVERY_SECONDS: u64 = 30;

/// How many days a store keeps a sample.
pub const KEEP_DAYS: i64 = 30;

/// One host's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub host: String,
    pub cores: u32,
    /// The 1-minute load average.
    pub load_1: f64,
    pub memory: Pressure,
}

/// The samples of the hosts that answered in <answers>. A host that did
/// not answer is said, with its error, and has none.
pub fn samples_of(answers: &[(Host, std::result::Result<Probe, String>)]) -> Vec<Sample> {
    answers
        .iter()
        .filter_map(|(host, probe)| match probe {
            Ok(probe) => Some(Sample {
                host: host.name.clone(),
                cores: probe.cores,
                load_1: probe.load[0],
                memory: probe.memory.pressure,
            }),
            Err(error) => {
                crate::output::note(format!("host {} gave no sample: {error}", host.name));
                None
            }
        })
        .collect()
}

/// Writes <samples>, taken at <at>, and deletes the samples older than
/// `KEEP_DAYS` days before <at>, in one transaction.
pub fn record(db: &mut Connection, at: i64, samples: &[Sample]) -> Result<()> {
    let tx = db.transaction().sql()?;
    for sample in samples {
        tx.execute(
            "INSERT INTO host_sample (host, at, cores, load_1, memory) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sample.host,
                at,
                sample.cores,
                sample.load_1,
                sample.memory.word()
            ],
        )
        .sql()?;
    }
    tx.execute(
        "DELETE FROM host_sample WHERE at < ?1",
        [at - KEEP_DAYS * 86400],
    )
    .sql()?;
    tx.commit().sql()
}

/// Asks every host about itself and writes the answers, as of <at>, to every
/// project's store. A store that refuses the write is said and the others
/// still get theirs.
pub fn sample_once(places: &Places, ssh: &Ssh, at: i64) -> Result<()> {
    let stores = crate::lists::store::stores(places)?;
    if stores.is_empty() {
        return Ok(());
    }
    let hosts = crate::host::all(places)?;
    let samples = samples_of(&crate::host::probe_all(places, ssh, hosts));
    for store in stores {
        let written = crate::db::open(&store.database(), crate::db::Make::Never)
            .and_then(|mut db| record(&mut db, at, &samples));
        if let Err(error) = written {
            crate::output::note(format!(
                "recording the hosts' load in project {}: {}",
                store.project,
                error.text()
            ));
        }
    }
    Ok(())
}

/// Starts the daemon's thread that samples the hosts every `EVERY_SECONDS`
/// seconds, the first sample at once.
pub fn start(places: Places) -> Result<()> {
    std::thread::Builder::new()
        .name("builds-host-samples".into())
        .spawn(move || {
            let ssh = Ssh::from_env();
            loop {
                if let Err(error) = sample_once(&places, &ssh, crate::app_timing::now()) {
                    crate::output::note(format!("sampling the hosts: {}", error.text()));
                }
                std::thread::sleep(Duration::from_secs(EVERY_SECONDS));
            }
        })
        .context(|| "starting a thread".to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "host_sample_tests.rs"]
mod tests;
