//! The dashboard's effect control: gives an open question the effect the
//! person chose, as `builds question edit N --effect E` does.
//!
//! The control is the person's, from this Mac only: the route that calls this
//! (http.rs) refuses a request that did not come from a loopback address on
//! the page served at 127.0.0.1 or localhost.

use crate::fail::Result;
use crate::lists::question_command::set_effect;
use crate::lists::store::Store;
use crate::lists::{Effect, Id, Stamp};
use crate::places::Places;

/// Gives open question <id> of project <project> the effect that <word>
/// names, at <now>.
pub fn set_question_effect(
    places: &Places,
    now: i64,
    project: &str,
    id: u32,
    word: &str,
) -> Result<()> {
    let effect = Effect::parse("the effect", word)?;
    let store = Store::named(places, project)?;
    let stamp = Stamp {
        at: now,
        by: super::approve::BY.to_string(),
    };
    set_effect(&store, Id(id), effect, &stamp)
}

#[cfg(test)]
#[path = "effect_tests.rs"]
mod tests;
