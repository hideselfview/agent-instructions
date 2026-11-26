//! The dashboard's Start button: moves a backlog item to the queue, as
//! `builds item start N --said WORDS` does.
//!
//! The button is the person's, from this Mac only: the route that calls this
//! (http.rs) refuses a request that did not come from a loopback address on
//! the page served at 127.0.0.1 or localhost.

use crate::fail::Result;
use crate::lists::store::Store;
use crate::lists::{Id, Stamp};
use crate::places::Places;

/// The words the dashboard gives as the person's start.
pub const SAID: &str = "started on the dashboard";

/// Starts backlog item <id> of project <project> at <now>, by the person on
/// the dashboard.
pub fn start_item(places: &Places, now: i64, project: &str, id: u32) -> Result<()> {
    let store = Store::named(places, project)?;
    let stamp = Stamp {
        at: now,
        by: super::approve::BY.to_string(),
    };
    crate::lists::start_items(&store, &[Id(id)], SAID, &stamp).map(|_| ())
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
