//! What the files of readings that builds keeps between runs share
//! (sccache_readings.rs, process_readings.rs): a list of readings in one JSON
//! file in builds' caches folder, which health and the dashboard both write,
//! each through a staged file of its own that it renames onto the record, so
//! a reader sees one writer's whole record. A write that loses to another's
//! loses at most one reading.

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::fail::{Context, Fail, Result};

/// The usual spacing of readings: one is added when the newest of its series
/// is this old or older.
pub const KEEP_SECONDS: i64 = 60;

/// The readings in <file>; none when there is no file yet.
pub fn read<T: DeserializeOwned>(file: &Path) -> Result<Vec<T>> {
    read_value(file)
}

/// The value in <file>; the default value when there is no file yet.
pub fn read_value<V: DeserializeOwned + Default>(file: &Path) -> Result<V> {
    match std::fs::read(file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).context(|| format!("reading {}", file.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(V::default()),
        Err(error) => Err(Fail::Message(format!(
            "reading {}: {error}",
            file.display()
        ))),
    }
}

/// Writes <readings> as the record in <file>, through a staged file of this
/// write's own.
pub fn write<T: Serialize>(file: &Path, readings: &[T]) -> Result<()> {
    write_value(file, readings)
}

/// Writes <value> as the record in <file>, through a staged file of this
/// write's own.
pub fn write_value<V: Serialize + ?Sized>(file: &Path, value: &V) -> Result<()> {
    let dir = file.parent().expect("the file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let staged = file.with_extension(crate::text::staged_suffix());
    std::fs::write(
        &staged,
        serde_json::to_vec(value).expect("the value serializes"),
    )
    .context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, file).context(|| format!("writing {}", file.display()))
}

/// Adds <reading>, taken at <now>, to <readings> unless a reading of its
/// series (<in_series>) is under <every> seconds old, and drops those more than
/// <max_age> seconds old. <at> is when a reading was taken. Returns whether
/// it added it.
pub fn record<T>(
    readings: &mut Vec<T>,
    reading: T,
    now: i64,
    at: impl Fn(&T) -> i64,
    in_series: impl Fn(&T) -> bool,
    every: i64,
    max_age: i64,
) -> bool {
    if readings.iter().any(|r| in_series(r) && now - at(r) < every) {
        return false;
    }
    readings.push(reading);
    readings.retain(|r| now - at(r) <= max_age);
    true
}

#[cfg(test)]
#[path = "readings_file_tests.rs"]
mod tests;
