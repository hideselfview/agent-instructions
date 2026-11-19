//! The crates a `builds cargo` run named (cargo_run_crate.sql). cargo_run.rs
//! writes them in the transaction that writes the run.

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;

/// The crates that cargo's arguments <args> name with `-p` or `--package`,
/// in order, each once. An argument after `--` is the program's, so it names
/// none.
pub fn named(args: &[String]) -> Vec<String> {
    let mut crates: Vec<String> = Vec::new();
    let mut words = args.iter().take_while(|arg| *arg != "--");
    while let Some(word) = words.next() {
        let name = match word.as_str() {
            "-p" | "--package" => words.next().map(String::as_str),
            other => other
                .strip_prefix("--package=")
                .or_else(|| other.strip_prefix("-p")),
        };
        if let Some(name) = name.filter(|name| !name.is_empty())
            && !crates.iter().any(|known| known == name)
        {
            crates.push(name.to_string());
        }
    }
    crates
}

/// Records that run <run> named <crates>.
pub fn record(db: &Connection, run: i64, crates: &[String]) -> Result<()> {
    for name in crates {
        db.execute(
            "INSERT INTO builds_cargo_run_crate (run_id, crate) VALUES (?1, ?2)",
            params![run, name],
        )
        .sql()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "cargo_run_crate_tests.rs"]
mod tests;
