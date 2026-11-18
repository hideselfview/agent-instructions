//! `builds design listen` and `builds design publish take | done | failed`: the
//! design session's side of a publish the person asks for on the dashboard
//! (publish.rs).

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::Stamp;
use crate::lists::args::Args;
use crate::lists::design_session::Messaging;
use crate::lists::publish;
use crate::lists::store::Store;
use crate::text::local_minute;
use crate::tool::Tool;

const USAGE: &str = "usage: builds design listen --project P\n       builds design publish take N [--project P] [--by WHO]\n       builds design publish done N --link TITLE=URL... [--project P] [--by WHO]\n       builds design publish failed N --why WHY [--project P] [--by WHO]";

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
pub fn listen(
    tool: &Tool,
    args: &[String],
    cwd: &Path,
    session: Option<&Messaging>,
) -> Result<String> {
    let args = Args::parse(args, &["project", "by"], &[])?;
    args.exactly(0, USAGE)?;
    let Some(project) = args.value("project")? else {
        fail!("say which project's publish requests the session listens for with --project");
    };
    let Some(session) = session else {
        fail!(
            "this command runs outside a Claude Code session with an inbox (CLAUDE_CODE_MESSAGING_SOCKET is not set), so the dashboard could not wake it; run it from the design session"
        );
    };
    let store = Store::named(&tool.places, project)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, "design listen", |db| {
        publish::listen(db, session, &stamp)
    })?;
    Ok(format!(
        "this session listens for project {project}'s publish requests; the dashboard sends it each one\n"
    ))
}

/// `publish take N`, `publish done N` and `publish failed N --why WHY`.
pub fn publish(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let Some((verb, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    let args = Args::parse(rest, &["project", "by", "why", "link"], &[])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = request_id(id)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    match verb.as_str() {
        "take" => {
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
        "done" => {
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
        "failed" => {
            let Some(why) = args.value("why")? else {
                fail!("say what stopped the publish with --why");
            };
            store.write(&stamp, &format!("design publish failed {id}"), |db| {
                publish::failed(db, id, why, stamp.at)
            })?;
            Ok(format!("publish request {id} ended failed: {why}\n"))
        }
        other => fail!("unknown publish command {other}\n{USAGE}"),
    }
}
