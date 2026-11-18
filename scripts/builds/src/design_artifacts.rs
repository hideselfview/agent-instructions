//! Which claude.ai artifacts a repo's designs publish to, on whichever account
//! Claude Code is signed in to. `design/mocks/canvases.toml` names each canvas
//! and the design system by its title, never by a link, because a link
//! belongs to one account. The session lists the account's artifacts of the
//! Design and Design System types with the Artifact tool, and builds finds
//! each title among the artifacts of its type that the person owns.

use std::collections::BTreeMap;

use crate::design_canvas::Published;
use crate::fail;
use crate::fail::Result;

/// How every artifact link starts; the artifact's id follows.
const LINK: &str = "https://claude.ai/artifact/";
/// The Artifact type that each canvas is made from.
pub const CANVAS_TYPE: &str = "Design";
/// The Artifact type that the design system is made from.
pub const SYSTEM_TYPE: &str = "Design System";
/// What the header of one type's listing says before the type's name.
const HEADER: &str = " made from the type \"";
/// How a listing row of an artifact the person owns starts.
const MINE: &str = "- (mine) ";
/// What separates a row's title from its link.
const SEPARATOR: &str = " \u{2014} ";

/// The id of the artifact that <url> links to, the end of its link; None
/// when <url> is no artifact link.
pub fn artifact_id(url: &str) -> Option<&str> {
    url.strip_prefix(LINK)
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The artifacts of one type that a listing shows.
#[derive(Debug, Default)]
struct TypeListing {
    /// The type's own link, which its header gives.
    link: Option<String>,
    /// Each artifact the person owns, as its title and link.
    mine: Vec<(String, String)>,
}

/// The listings in <listing>, by type name: each header line that names a
/// type starts that type's rows. Rows of artifacts shared with the person
/// are not theirs to publish to, and the listing's other lines name no
/// artifact. Fails on a row of the person's own before any header, or one
/// with no artifact link after its title.
fn parse(listing: &str) -> Result<BTreeMap<String, TypeListing>> {
    let mut types: BTreeMap<String, TypeListing> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in listing.lines() {
        if let Some((_, rest)) = line.split_once(HEADER) {
            let Some((name, after)) = rest.split_once('"') else {
                fail!("the listing line {line:?} opens a type's name and never closes it");
            };
            let link = after
                .trim_start()
                .strip_prefix('(')
                .and_then(|rest| rest.split_once(')'))
                .map(|(link, _)| link.to_string())
                .filter(|link| artifact_id(link).is_some());
            let entry = types.entry(name.to_string()).or_default();
            if entry.link.is_none() {
                entry.link = link;
            }
            current = Some(name.to_string());
            continue;
        }
        let Some(row) = line.trim_start().strip_prefix(MINE) else {
            continue;
        };
        let Some(name) = &current else {
            fail!("the listing row {line:?} comes before any header that names its Artifact type");
        };
        let Some((title, url)) = row.rsplit_once(SEPARATOR) else {
            fail!("the listing row {line:?} gives no artifact link after its title");
        };
        let url = url.trim();
        if artifact_id(url).is_none() {
            fail!("the listing row {line:?} ends in {url:?}, which is no artifact link");
        }
        types
            .get_mut(name)
            .expect("a header added its type")
            .mine
            .push((title.trim().to_string(), url.to_string()));
    }
    Ok(types)
}

/// The links of the artifacts that `CANVASES` names, on the listed account.
#[derive(Debug, PartialEq)]
pub struct Found {
    /// Each canvas's link, in the order `CANVASES` names the canvases.
    pub canvases: Vec<String>,
    pub design_system: String,
}

/// The link of each artifact that <published> names by title, from
/// <listing>, the Artifact tool's listings of the account's artifacts of the
/// types `CANVAS_TYPE` and `SYSTEM_TYPE`. Fails, naming every title it could
/// not settle, when the listing holds no listing of one of the types, the
/// person owns no artifact of a title's type with that title, or owns more
/// than one.
pub fn find(published: &Published, listing: &str) -> Result<Found> {
    let types = parse(listing)?;
    let mut problems = Vec::new();
    let mut find_one = |type_name: &str, title: &str| -> Option<String> {
        let Some(listed) = types.get(type_name) else {
            problems.push(format!(
                "the listing holds no listing of the {type_name} type: list the account's artifacts with type \"{type_name}\", through every page"
            ));
            return None;
        };
        let urls: Vec<&str> = listed
            .mine
            .iter()
            .filter(|(listed_title, _)| listed_title == title)
            .map(|(_, url)| url.as_str())
            .collect();
        match urls.as_slice() {
            [url] => Some(url.to_string()),
            [] => {
                let from = match &listed.link {
                    Some(link) => format!("the {type_name} type ({link})"),
                    None => format!("the {type_name} type"),
                };
                problems.push(format!(
                    "the account has no {type_name} artifact titled {title:?}: create it from {from} with that title and no files"
                ));
                None
            }
            many => {
                problems.push(format!(
                    "the account has {} {type_name} artifacts titled {title:?} ({}): the person keeps one and renames or deletes the others on claude.ai",
                    many.len(),
                    many.join(", ")
                ));
                None
            }
        }
    };
    let canvases: Vec<Option<String>> = published
        .canvases
        .iter()
        .map(|canvas| find_one(CANVAS_TYPE, &canvas.title))
        .collect();
    let design_system = find_one(SYSTEM_TYPE, &published.design_system);
    problems.dedup();
    if !problems.is_empty() {
        fail!(
            "builds could not find every artifact on the account:\n  {}",
            problems.join("\n  ")
        );
    }
    Ok(Found {
        canvases: canvases
            .into_iter()
            .map(|url| url.expect("no problem means every canvas"))
            .collect(),
        design_system: design_system.expect("no problem means a design system"),
    })
}

#[cfg(test)]
#[path = "design_artifacts_tests.rs"]
pub(crate) mod tests;
