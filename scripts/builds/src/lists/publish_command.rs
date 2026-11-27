//! `builds design listen` and `builds design publish take | done | failed`: the
//! design session's side of a publish the person asks for on the dashboard
//! (publish.rs).

use std::path::Path;

use crate::args::Args;
use crate::fail;
use crate::fail::Result;
use crate::inbox::Messaging;
use crate::lists::Stamp;
use crate::lists::publish;
use crate::lists::store::Store;
use crate::text::local_minute;
use crate::tool::Tool;

/// A publish request's id, as a command names it.
fn request_id(text: &str) -> Result<i64> {
    match text.parse::<i64>() {
        Ok(id) if id > 0 => Ok(id),
        _ => fail!("{text:?} is no publish request's number"),
    }
}

/// `listen --project P`: keeps the inbox of the session running the command,
/// <session>, as the one the dashboard wakes for project P's publish
/// requests, in place of any earlier one.
pub fn listen(tool: &Tool, args: &Args, cwd: &Path, session: Option<&Messaging>) -> Result<String> {
    args.exactly(0)?;
    let Some(project) = args.value("project")? else {
        fail!("say which project's publish requests the session listens for with --project");
    };
    let Some(session) = session else {
        fail!(
            "Run this command from the Codex or Claude Code design session so the dashboard can notify it."
        );
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, args, cwd)?;
    store.write(&stamp, "design listen", |db| {
        publish::listen(db, session, &stamp)
    })?;
    Ok(format!(
        "this session listens for project {project}'s publish requests; the dashboard sends it each one\n"
    ))
}

/// The request <args> name, with the store and the stamp of its write: the
/// arguments of `publish take N`, `publish done N` and `publish failed N`.
fn request(tool: &Tool, args: &Args, cwd: &Path) -> Result<(i64, Store, Stamp)> {
    let [id] = args.exactly(1)? else {
        unreachable!("exactly one")
    };
    let id = request_id(id)?;
    let store = Store::resolve(&tool.places, args, cwd)?;
    let stamp = Stamp::resolve(tool.now, args, cwd)?;
    Ok((id, store, stamp))
}

/// `publish take N`.
pub fn take(tool: &Tool, args: &Args, cwd: &Path) -> Result<String> {
    let (id, store, stamp) = request(tool, args, cwd)?;
    let taken = store.write(&stamp, &format!("design publish take {id}"), |db| {
        publish::take(db, id, &stamp)
    })?;
    Ok(format!(
        "you took publish request {id} at {}: publish commit {} with the publish-designs skill (`builds design publish-files DIR OUT --index FILE --at {}`), and end the request with `builds design publish done {id} --link TITLE=URL` for each artifact (the urls publish-find printed) once the design system's last calls and the runtime check are done\n",
        local_minute(stamp.at),
        taken.commit,
        taken.commit
    ))
}

/// `publish done N`.
pub fn done(tool: &Tool, args: &Args, cwd: &Path) -> Result<String> {
    let (id, store, stamp) = request(tool, args, cwd)?;
    let links = args
        .values("link")
        .into_iter()
        .map(publish::Link::parse)
        .collect::<Result<Vec<_>>>()?;
    let ended = store.write(&stamp, &format!("design publish done {id}"), |db| {
        publish::published(db, id, &links, stamp.at)
    })?;
    Ok(format!(
        "publish request {id} ended: it published commit {}\n",
        ended.commit
    ))
}

/// `publish failed N --why WHY`.
pub fn failed(tool: &Tool, args: &Args, cwd: &Path) -> Result<String> {
    let (id, store, stamp) = request(tool, args, cwd)?;
    let Some(why) = args.value("why")? else {
        fail!("say what stopped the publish with --why");
    };
    store.write(&stamp, &format!("design publish failed {id}"), |db| {
        publish::failed(db, id, why, stamp.at)
    })?;
    Ok(format!("publish request {id} ended failed: {why}\n"))
}
