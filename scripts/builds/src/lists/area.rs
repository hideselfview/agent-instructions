//! `builds area add | rename | move | remove`: the work queue's areas, in the
//! order the sidebar shows them.

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::args::{Args, current_dir};
use crate::lists::check::title_fault;
use crate::lists::store::Store;
use crate::lists::{Id, Lists, Stamp, item};
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

fn position(lists: &Lists, area: &str) -> Result<usize> {
    match lists.areas.iter().position(|known| known == area) {
        Some(at) => Ok(at),
        None => fail!("area '{area}' does not exist (the areas are: {})", lists.areas.join("; ")),
    }
}

fn require_new_name(lists: &Lists, name: &str) -> Result<()> {
    if let Some(fault) = title_fault(name) {
        fail!("{fault}");
    }
    if lists.areas.iter().any(|known| known == name) {
        fail!("area '{name}' exists already");
    }
    Ok(())
}

/// Adds area <name> after <after>, or at the end.
pub fn add_area(lists: &mut Lists, name: &str, after: Option<&str>) -> Result<()> {
    require_new_name(lists, name)?;
    let at = match after {
        Some(after) => position(lists, after)? + 1,
        None => lists.areas.len(),
    };
    lists.areas.insert(at, name.to_string());
    Ok(())
}

/// Renames area <old> to <new>, on every item in it, open or closed.
pub fn rename_area(lists: &mut Lists, old: &str, new: &str) -> Result<()> {
    let at = position(lists, old)?;
    require_new_name(lists, new)?;
    lists.areas[at] = new.to_string();
    item::rename_area(lists, old, new);
    Ok(())
}

/// Moves area <name> to just after <after>.
pub fn move_area(lists: &mut Lists, name: &str, after: &str) -> Result<()> {
    let from = position(lists, name)?;
    position(lists, after)?;
    let area = lists.areas.remove(from);
    let to = position(lists, after)? + 1;
    lists.areas.insert(to, area);
    Ok(())
}

/// Removes area <name>, which no item, open or closed, is in.
pub fn remove_area(lists: &mut Lists, name: &str) -> Result<()> {
    let at = position(lists, name)?;
    let inside: Vec<String> = item::items_in_area(lists, name).iter().map(Id::to_string).collect();
    if !inside.is_empty() {
        fail!("area '{name}' still holds {}; move each with `builds item edit N --area AREA`", inside.join(", "));
    }
    lists.areas.remove(at);
    Ok(())
}

/// Lists area <name> again at the end, whether or not it is there already.
#[cfg(test)]
pub fn plant_area_listed_again(lists: &mut Lists, name: &str) {
    lists.areas.push(name.to_string());
}

fn add(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &["after"], &[])?;
    let [name] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("area add {name}"), |lists| {
        add_area(lists, name, args.value("after")?)
    })?;
    note(format!("added area {name}"));
    Ok(())
}

fn rename(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [old, new] = args.exactly(2, USAGE)? else { unreachable!("exactly two") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("area rename {old}"), |lists| {
        rename_area(lists, old, new)
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
    store.with_write(&stamp, &format!("area move {name}"), |lists| {
        move_area(lists, name, after)
    })?;
    note(format!("moved area {name} after {after}"));
    Ok(())
}

fn remove(tool: &Tool, args: &[String], cwd: &Path) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [name] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.with_write(&stamp, &format!("area remove {name}"), |lists| {
        remove_area(lists, name)
    })?;
    note(format!("removed area {name}"));
    Ok(())
}

#[cfg(test)]
#[path = "area_tests.rs"]
mod tests;
