//! `builds area add | rename | move | remove`: the work queue's areas, in the
//! order the sidebar shows them.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir};
use crate::lists::check::title_fault;
use crate::lists::store::Store;
use crate::lists::{Id, Stamp, item};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds area add NAME [--after AREA]\n       builds area rename OLD NEW\n       builds area move NAME --after AREA\n       builds area remove NAME";

pub fn cmd_area(tool: &Tool, args: &[String]) -> Result<()> {
    let cwd = current_dir()?;
    let Some((command, rest)) = args.split_first() else { fail!("{USAGE}") };
    match command.as_str() {
        "add" => add(tool, rest, &cwd),
        "rename" => rename(tool, rest, &cwd),
        "move" => move_after(tool, rest, &cwd),
        "remove" => remove(tool, rest, &cwd),
        other => fail!("unknown area command {other}\n{USAGE}"),
    }
}

/// The areas in the order the sidebar shows them.
pub fn areas(db: &Connection) -> Result<Vec<String>> {
    let mut statement = db.prepare("SELECT name FROM area ORDER BY position").sql()?;
    let names = statement.query_map([], |row| row.get(0)).sql()?;
    names.map(|name| name.sql()).collect()
}

fn position(areas: &[String], area: &str) -> Result<usize> {
    match areas.iter().position(|known| known == area) {
        Some(at) => Ok(at),
        None => fail!("area '{area}' does not exist (the areas are: {})", areas.join("; ")),
    }
}

fn require_new_name(areas: &[String], name: &str) -> Result<()> {
    if let Some(fault) = title_fault(name) {
        fail!("{fault}");
    }
    if areas.iter().any(|known| known == name) {
        fail!("area '{name}' exists already");
    }
    Ok(())
}

/// Gives the areas the positions of <order>. The unique positions are never
/// two at once: every area moves out of the range first.
fn renumber(db: &Connection, order: &[String]) -> Result<()> {
    db.execute("UPDATE area SET position = position + 1000000", []).sql()?;
    for (at, name) in order.iter().enumerate() {
        db.execute("UPDATE area SET position = ?1 WHERE name = ?2", params![at as i64, name]).sql()?;
    }
    Ok(())
}

/// Adds area <name> after <after>, or at the end.
pub fn add_area(db: &Connection, name: &str, after: Option<&str>) -> Result<()> {
    let mut order = areas(db)?;
    require_new_name(&order, name)?;
    let at = match after {
        Some(after) => position(&order, after)? + 1,
        None => order.len(),
    };
    db.execute("INSERT INTO area (name, position) VALUES (?1, ?2)", params![name, 2_000_000 + order.len() as i64]).sql()?;
    order.insert(at, name.to_string());
    renumber(db, &order)
}

/// Renames area <old> to <new>, on every item in it, open or closed: the
/// items' foreign key carries the new name.
pub fn rename_area(db: &Connection, old: &str, new: &str) -> Result<()> {
    let order = areas(db)?;
    position(&order, old)?;
    require_new_name(&order, new)?;
    db.execute("UPDATE area SET name = ?1 WHERE name = ?2", params![new, old]).sql()?;
    Ok(())
}

/// Moves area <name> to just after <after>.
pub fn move_area(db: &Connection, name: &str, after: &str) -> Result<()> {
    let mut order = areas(db)?;
    let from = position(&order, name)?;
    position(&order, after)?;
    let area = order.remove(from);
    let to = position(&order, after)? + 1;
    order.insert(to, area);
    renumber(db, &order)
}

/// Removes area <name>, which no item, open or closed, is in.
pub fn remove_area(db: &Connection, name: &str) -> Result<()> {
    position(&areas(db)?, name)?;
    let inside: Vec<String> = item::items_in_area(db, name)?.iter().map(Id::to_string).collect();
    if !inside.is_empty() {
        fail!("area '{name}' still holds {}; move each with `builds item edit N --area AREA`", inside.join(", "));
    }
    db.execute("DELETE FROM area WHERE name = ?1", [name]).sql()?;
    Ok(())
}

/// Puts <areas> in as they were written elsewhere, in their order.
pub fn move_in(db: &Connection, areas: &[String]) -> Result<()> {
    for (at, name) in areas.iter().enumerate() {
        db.execute("INSERT INTO area (name, position) VALUES (?1, ?2)", params![name, at as i64]).sql()?;
    }
    Ok(())
}

/// The tables area.rs owns.
#[cfg(test)]
const TABLES: [&str; 1] = ["area"];

/// Writes a row of area.rs's table as given, the constraints still holding:
/// for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(db: &Connection, columns: &[(&str, rusqlite::types::Value)]) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, "area", columns)
}

fn add(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["after"], &[])?;
    let [name] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("area add {name}"), |db| {
        add_area(db, name, args.value("after")?)
    })?;
    note(format!("added area {name}"));
    Ok(())
}

fn rename(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [old, new] = args.exactly(2, USAGE)? else { unreachable!("exactly two") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("area rename {old}"), |db| {
        rename_area(db, old, new)
    })?;
    note(format!("renamed area {old} to {new}"));
    Ok(())
}

fn move_after(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["after"], &[])?;
    let [name] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let after = args.require("after")?;
    if after == name {
        fail!("area '{name}' cannot move after itself");
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("area move {name}"), |db| {
        move_area(db, name, after)
    })?;
    note(format!("moved area {name} after {after}"));
    Ok(())
}

fn remove(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [name] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("area remove {name}"), |db| {
        remove_area(db, name)
    })?;
    note(format!("removed area {name}"));
    Ok(())
}

#[cfg(test)]
#[path = "area_tests.rs"]
mod tests;
