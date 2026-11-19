//! The variants of a design change: versions of one area of one board that
//! the change's review page shows side by side while the person and the
//! design session compare them. The page draws the board once per set of
//! props, at a commit the variants keep, and crops each drawing to the first
//! element a CSS selector finds. `builds design variants N` keeps a change's
//! variants, replacing the ones it had, or clears them; landing or dropping
//! the change deletes them (`design::landed`, `design::drop`). An amend of the
//! change leaves them as they are. They live in the store, never in the repo.
//! The tables are `design_variants.sql`; this is the only code that writes
//! them.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::Id;
use crate::lists::design::{Theme, open_design};
use crate::lists::store;

/// One set of props a board is drawn with: what its board's script reads
/// from `this.props`, as a board that mounts it with `dc-import` passes its
/// attributes.
pub type Props = serde_json::Map<String, Value>;

/// The variants of a design change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variants {
    /// The board file, `design/mocks/<name>.dc.html`.
    pub board: String,
    /// The CSS selector of the area: the page crops each drawing to the
    /// first element it finds that a person sees.
    pub selector: String,
    pub theme: Theme,
    /// The commit the board is drawn at, in the change's worktree.
    pub tip: String,
    /// Each set of props the board is drawn with, in the order the page
    /// shows them.
    pub props: Vec<Props>,
}

/// How the page labels the drawing of <props>: each prop as `name: value`,
/// a string without its quotes and any other value as JSON.
pub fn label(props: &Props) -> String {
    if props.is_empty() {
        return "No props".to_string();
    }
    props
        .iter()
        .map(|(name, value)| match value {
            Value::String(text) => format!("{name}: {text}"),
            other => format!("{name}: {other}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Keeps <variants> as the variants of open design change <id>, replacing the
/// ones it had. Refuses no props, a selector with no text, and a change that
/// is not open. (That the board is one of the repo's board places and a file
/// at the commit is the command's, which reads git.)
pub fn keep(db: &Connection, id: Id, variants: &Variants) -> Result<()> {
    open_design(&store::read(db)?, id)?;
    if variants.props.is_empty() {
        fail!("--props gives no props: give one JSON object of props for each variant");
    }
    if variants.selector.trim().is_empty() {
        fail!("--selector is empty: give the CSS selector of the area to crop each variant to");
    }
    db.execute("DELETE FROM design_variants WHERE design_id = ?1", [id])
        .sql()?;
    db.execute(
        "INSERT INTO design_variants (design_id, board, selector, theme, tip) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, variants.board, variants.selector, variants.theme.word(), variants.tip],
    )
    .sql()?;
    for (at, props) in variants.props.iter().enumerate() {
        db.execute(
            "INSERT INTO design_variant (design_id, position, props) VALUES (?1, ?2, ?3)",
            params![id, at as i64, Value::Object(props.clone()).to_string()],
        )
        .sql()?;
    }
    Ok(())
}

/// Deletes the variants of design change <id>; returns whether it had any.
pub fn clear(db: &Connection, id: Id) -> Result<bool> {
    let deleted = db
        .execute("DELETE FROM design_variants WHERE design_id = ?1", [id])
        .sql()?;
    Ok(deleted > 0)
}

/// Every design change's variants.
pub(in crate::lists) fn all(db: &Connection) -> Result<BTreeMap<Id, Variants>> {
    let mut props: BTreeMap<Id, Vec<Props>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, props FROM design_variant ORDER BY design_id, position")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (design, text) = row.sql()?;
        // design_variant_props_are_an_object holds, so anything else is a broken store.
        let Ok(Value::Object(one)) = serde_json::from_str(&text) else {
            fail!("design change {design} keeps props that are no JSON object; the store is broken")
        };
        props.entry(design).or_default().push(one);
    }
    let mut statement = db
        .prepare("SELECT design_id, board, selector, theme, tip FROM design_variants")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .sql()?;
    let mut all = BTreeMap::new();
    for row in rows {
        let (design, board, selector, theme, tip) = row.sql()?;
        // `keep` writes the variants with at least one set of props, so
        // variants with none are a broken store.
        let Some(props) = props.remove(&design) else {
            fail!("design change {design} keeps variants with no props; the store is broken")
        };
        all.insert(
            design,
            Variants {
                board,
                selector,
                theme: Theme::of_word(&theme)?,
                tip,
                props,
            },
        );
    }
    Ok(all)
}

/// The lines `design show` prints of <variants>.
pub fn describe(variants: &Variants) -> String {
    let mut out = format!(
        "  variants of {} ({}) at {}, cropped to {}:\n",
        variants.board,
        variants.theme.word(),
        variants.tip,
        variants.selector
    );
    for props in &variants.props {
        out.push_str(&format!("    {}\n", label(props)));
    }
    out
}

/// The tables design_variants.rs owns.
#[cfg(test)]
const TABLES: [&str; 2] = ["design_variants", "design_variant"];

/// Writes a row of one of design_variants.rs's tables as given, the
/// constraints still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "design_variants_tests.rs"]
mod tests;
