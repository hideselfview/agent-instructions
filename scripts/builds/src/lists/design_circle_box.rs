//! The circles of a review board that surround a box on the board rather
//! than the elements of a selector. `design::save` writes a change's review
//! page through `write`, and `design::read` reads every board's box circles
//! through `all` beside the selector circles it reads itself; a board's
//! circles share one order of positions across both tables. The table is
//! `design_circle_box.sql`; this is the only code that writes it. Its rows
//! leave with their review board (ON DELETE CASCADE), which `design::save`
//! deletes and writes again whole.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::Id;
use crate::lists::design::{Around, BoardBox, Circle};

/// Writes the circle around box <area> with <label> as the circle at
/// <position> of board <board> (its position on the review page) of design
/// change <design>.
pub(in crate::lists) fn write(
    db: &Connection,
    design: Id,
    board: i64,
    position: i64,
    area: &BoardBox,
    label: Option<&str>,
) -> Result<()> {
    db.execute(
        "INSERT INTO review_circle_box (design_id, board, position, x, y, width, height, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![design, board, position, area.x, area.y, area.width, area.height, label],
    )
    .sql()?;
    Ok(())
}

/// Circles by the review board they are on (its design change and its
/// position on the review page), each with its position among the board's
/// circles.
pub(in crate::lists) type BoardCircles = BTreeMap<(Id, i64), Vec<(i64, Circle)>>;

/// Every review board's box circles.
pub(in crate::lists) fn all(db: &Connection) -> Result<BoardCircles> {
    let mut circles = BoardCircles::new();
    let mut statement = db
        .prepare(
            "SELECT design_id, board, position, x, y, width, height, label FROM review_circle_box",
        )
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                BoardBox {
                    x: row.get(3)?,
                    y: row.get(4)?,
                    width: row.get(5)?,
                    height: row.get(6)?,
                },
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .sql()?
    {
        let (design, board, position, area, label) = row.sql()?;
        circles.entry((design, board)).or_default().push((
            position,
            Circle {
                around: Around::Box(area),
                label,
            },
        ));
    }
    Ok(circles)
}

/// Writes a row of review_circle_box as given, the constraints still holding:
/// for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &["review_circle_box"], "review_circle_box", columns)
}

#[cfg(test)]
#[path = "design_circle_box_tests.rs"]
mod tests;
