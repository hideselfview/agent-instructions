//! The whole-state check: what must hold of a store at all times and the
//! database's own keys, foreign keys and CHECKs cannot say. It is a short list
//! of queries, run inside every write before it commits and on every read. A
//! store that fails it makes every command fail naming each fault, and nothing
//! repairs it.
//!
//! The rest of what must hold is in the tables themselves (`lists/*.sql`):
//! one id per record, a reference naming a record of the right kind, an item's
//! area existing, a title that is one line, a worktree that is a side worktree
//! of a project repo, a closing that fits its kind, and so on. What needs
//! counting words, git or other databases is checked at the write that makes
//! it.

use rusqlite::Connection;

use crate::db::Sql;
use crate::fail::Result;
use crate::lists::trail::EARLIER;

/// The most words an item's title has, and the fewest.
pub const TITLE_WORDS: std::ops::RangeInclusive<usize> = 5..=7;

/// Is <name> usable as a project's folder name? Letters, digits, `-`, `_`
/// and inner dots; so no name leaves the store's parent folder.
pub fn project_name_fault(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        None
    } else {
        Some(format!(
            "'{name}' is not a project name: letters, digits, - _ and . (not first) only"
        ))
    }
}

/// Why <title> is not a one-line title, or None.
pub fn title_fault(title: &str) -> Option<String> {
    if title.trim().is_empty() {
        Some("a title is empty".to_string())
    } else if title.contains(['\n', '\r', '\t']) {
        Some(format!(
            "title '{}' is not one line",
            title.replace(['\n', '\r', '\t'], " ")
        ))
    } else if title != title.trim() {
        Some(format!("title '{title}' has spaces at its ends"))
    } else {
        None
    }
}

/// Why <title> is not an item's title (one line, five to seven words).
pub fn item_title_fault(title: &str) -> Option<String> {
    if let Some(fault) = title_fault(title) {
        return Some(fault);
    }
    let words = title.split_whitespace().count();
    if TITLE_WORDS.contains(&words) {
        None
    } else {
        Some(format!(
            "item title '{title}' has {words} words; item titles are {} to {}",
            TITLE_WORDS.start(),
            TITLE_WORDS.end()
        ))
    }
}

/// Why <repo> is no repo directory name, if it isn't.
pub fn repo_name_fault(repo: &str) -> Option<String> {
    (repo.is_empty() || repo.contains('/'))
        .then(|| format!("repo '{repo}' is not a repo directory name"))
}

/// A closing's `how` that meets a wait on the record it closed.
const MEETS_WAIT: &str = "((c.kind = 'question' AND c.how IN ('answered', 'settled', 'done')) \
                           OR (c.kind IN ('item', 'design') AND c.how = 'landed') \
                           OR (c.kind = 'item' AND c.how = 'finished'))";

/// Every fault of the store behind <db>, one line each. <project> is the
/// name of the folder the store is in.
pub fn faults(db: &Connection, project: &str) -> Result<Vec<String>> {
    let mut faults = Vec::new();
    // The project row names the folder's project.
    let held: Option<String> = db
        .query_row("SELECT name FROM project", [], |row| row.get(0))
        .ok();
    match held {
        Some(held) if held != project => faults.push(format!(
            "the project row names {held}, and the folder is {project}'s"
        )),
        _ => {}
    }
    let queries: [(&str, String); 11] = [
        // 1. A project has at least one repo: a parent that needs a child.
        ("repos", "SELECT 'the project has no repos' WHERE NOT EXISTS (SELECT 1 FROM repo)".to_string()),
        // 2. An open question has an open item waiting on it or gives the
        // reason it stays on the stack anyway, never both.
        (
            "questions",
            "SELECT CASE WHEN q.blocks_nothing IS NULL \
                 THEN 'question #' || q.id || ' blocks no open item and gives no reason it stays on the stack: \
                     answer or drop it, or give the reason in the write that removes the last open item''s wait on it \
                     (`builds item wait ITEM --off question:' || q.id || ' --blocks-nothing WHY`)' \
                 ELSE 'question #' || q.id || ' gives a reason it blocks nothing (' || q.blocks_nothing || ') while open item #' || \
                     (SELECT MIN(w.item_id) FROM item_wait_question w WHERE w.on_question = q.id \
                      AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = w.item_id)) || ' waits on it: \
                     a question gives that reason only while no open item waits on it, and \
                     `builds item wait ITEM --off question:' || q.id || ' --blocks-nothing WHY` gives the reason in the write that removes the last open item''s wait on it' \
                 END FROM question q \
             WHERE NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = q.id) \
             AND (q.blocks_nothing IS NULL) = (NOT EXISTS (SELECT 1 FROM item_wait_question w WHERE w.on_question = q.id \
                             AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = w.item_id))) \
             ORDER BY q.id"
                .to_string(),
        ),
        // 3. An open item's wait on a record names one that is open, or closed in a way that meets waits.
        (
            "waits",
            format!(
                "SELECT 'item #' || w.item_id || ' waits on #' || w.target || ', which can no longer be met (closed as ' || c.how || ')' \
                 FROM (SELECT item_id, on_item AS target FROM item_wait_item \
                       UNION ALL SELECT item_id, on_question FROM item_wait_question \
                       UNION ALL SELECT item_id, on_design FROM item_wait_design) w \
                 JOIN closing c ON c.record_id = w.target \
                 WHERE NOT EXISTS (SELECT 1 FROM closing ci WHERE ci.record_id = w.item_id) AND NOT {MEETS_WAIT} \
                 ORDER BY w.item_id, w.target"
            ),
        ),
        // 4. No circle of waits through open items.
        (
            "circle",
            "WITH RECURSIVE edge (a, b) AS ( \
                 SELECT item_id, on_item FROM item_wait_item \
                 WHERE NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = item_id) \
                   AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = on_item)), \
             walk (start, at, path) AS ( \
                 SELECT a, b, '#' || a || ' -> #' || b FROM edge \
                 UNION ALL \
                 SELECT walk.start, edge.b, walk.path || ' -> #' || edge.b FROM walk JOIN edge ON edge.a = walk.at \
                 WHERE walk.at <> walk.start \
                   AND (edge.b = walk.start OR instr(' ' || walk.path || ' ', ' #' || edge.b || ' ') = 0)) \
             SELECT 'items wait on each other in a circle: ' || path FROM walk WHERE at = start \
             ORDER BY start, length(path) LIMIT 1"
                .to_string(),
        ),
        // 5. A closed item runs nowhere.
        (
            "running",
            "SELECT 'item #' || r.item_id || ' is closed and still runs somewhere' FROM running r \
             JOIN closing c ON c.record_id = r.item_id ORDER BY r.item_id"
                .to_string(),
        ),
        // 6. A worktree holds one branch of work, running or parked.
        (
            "worktrees",
            "SELECT 'worktree ' || worktree || ' holds two branches of work: ' || group_concat(DISTINCT branch) \
             FROM (SELECT worktree, branch FROM running_place UNION ALL SELECT worktree, branch FROM parked_place) \
             GROUP BY worktree HAVING COUNT(DISTINCT branch) > 1 ORDER BY worktree"
                .to_string(),
        ),
        // 6a. A closed item is parked nowhere, an item is never both running
        // and parked, and a worktree with a parked item holds no other item.
        (
            "parked",
            "SELECT 'item #' || k.item_id || ' is closed and still parked somewhere' FROM parked k \
             JOIN closing c ON c.record_id = k.item_id \
             UNION ALL \
             SELECT 'item #' || k.item_id || ' is both running and parked' FROM parked k \
             JOIN running r ON r.item_id = k.item_id \
             UNION ALL \
             SELECT 'worktree ' || p.worktree || ' holds parked item #' || p.item_id || ' and item #' || o.item_id \
             FROM parked_place p JOIN (SELECT item_id, worktree FROM running_place \
                 UNION ALL SELECT item_id, worktree FROM parked_place) o \
             ON o.worktree = p.worktree AND o.item_id <> p.item_id"
                .to_string(),
        ),
        // 6b. Only an item closed as dropped records the work it held.
        (
            "dropped",
            "SELECT DISTINCT 'item #' || d.item_id || ' records dropped work but is not closed as dropped' \
             FROM dropped_place d WHERE NOT EXISTS (SELECT 1 FROM closing c \
             WHERE c.record_id = d.item_id AND c.how = 'dropped') ORDER BY d.item_id"
                .to_string(),
        ),
        // 7. Every landed closing and every landed part has a commit: a parent that needs a child.
        (
            "commits",
            "SELECT 'closed ' || kind || ' #' || record_id || ' landed with no commits' FROM closing c \
             WHERE how = 'landed' AND NOT EXISTS (SELECT 1 FROM closing_commit k WHERE k.record_id = c.record_id) \
             UNION ALL \
             SELECT 'item #' || item_id || ' has a landed part of repo ' || repo || ' with no commits' FROM item_landing l \
             WHERE NOT EXISTS (SELECT 1 FROM item_landing_commit k WHERE k.landing_id = l.id)"
                .to_string(),
        ),
        // 8. No chain of origins loops.
        (
            "origins",
            format!(
                "{EARLIER} SELECT 'record #' || record || ' comes from itself through its chain of origins' \
                 FROM earlier WHERE record = earlier ORDER BY record"
            ),
        ),
        // 9. A record carries no tag that an earlier record of its trail carries.
        (
            "tags",
            format!(
                "{EARLIER} SELECT 'record #' || t.record_id || ' carries the tag ' || t.tag || \
                 ', which #' || e.earlier || ' earlier in its trail carries' \
                 FROM earlier e JOIN record_tag t ON t.record_id = e.record \
                 JOIN record_tag s ON s.record_id = e.earlier AND s.tag = t.tag \
                 ORDER BY t.record_id, t.tag, e.earlier"
            ),
        ),
    ];
    for (_, sql) in &queries {
        let mut statement = db.prepare(sql).sql()?;
        for fault in statement
            .query_map([], |row| row.get::<_, String>(0))
            .sql()?
        {
            faults.push(fault.sql()?);
        }
    }
    Ok(faults)
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
