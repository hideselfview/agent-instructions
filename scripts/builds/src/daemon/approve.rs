//! The dashboard's Approve button: records the person's approval of an open
//! design change as `builds design approve N --by-person` does, and tells the
//! session that recorded it to land it (lists/design_session.rs).
//!
//! The button is the person's, from this Mac only: the route that calls this
//! (http.rs) refuses a request that did not come from a loopback address on
//! the page served at 127.0.0.1 or localhost.

use crate::fail;
use crate::fail::Result;
use crate::lists::design_session::{Approval, approve_and_tell};
use crate::lists::store::Store;
use crate::lists::{Id, Stamp};
use crate::places::Places;

/// Who the log names for a write the person makes on the dashboard: an
/// approval, or the effect the person gives a question.
pub const BY: &str = "person (dashboard)";

/// Approves open design change <id> of project <project>, at <now>.
pub fn approve_design(places: &Places, now: i64, project: &str, id: u32) -> Result<Approval> {
    let id = Id(id);
    let store = Store::named(places, project)?;
    if store.load()?.design(id).is_none() {
        fail!("project {project} has no design change {id}");
    }
    let stamp = Stamp {
        at: now,
        by: BY.to_string(),
    };
    approve_and_tell(&store, &places.sessions(), id, &stamp)
}

#[cfg(test)]
#[path = "approve_tests.rs"]
mod tests;
