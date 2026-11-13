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

/// The most words an item's title has, and the fewest.
pub const TITLE_WORDS: std::ops::RangeInclusive<usize> = 5..=7;

/// Is <name> usable as a project's folder name? Letters, digits, `-`, `_`
/// and inner dots; so no name leaves the store's parent folder.
pub fn project_name_fault(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok { None } else { Some(format!("'{name}' is not a project name: letters, digits, - _ and . (not first) only")) }
}

/// Why <title> is not a one-line title, or None.
pub fn title_fault(title: &str) -> Option<String> {
    if title.trim().is_empty() {
        Some("a title is empty".to_string())
    } else if title.contains(['\n', '\r', '\t']) {
        Some(format!("title '{}' is not one line", title.replace(['\n', '\r', '\t'], " ")))
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
    (repo.is_empty() || repo.contains('/')).then(|| format!("repo '{repo}' is not a repo directory name"))
}


/// A closing's `how` that meets a wait on the record it closed.
const MEETS_WAIT: &str = "((c.kind = 'question' AND c.how IN ('answered', 'settled', 'done')) \
                           OR (c.kind IN ('item', 'design') AND c.how = 'landed'))";

/// Every fault of the store behind <db>, one line each. <project> is the
/// name of the folder the store is in.
pub fn faults(db: &Connection, project: &str) -> Result<Vec<String>> {
    let mut faults = Vec::new();
    // The project row names the folder's project.
    let held: Option<String> = db.query_row("SELECT name FROM project", [], |row| row.get(0)).ok();
    match held {
        Some(held) if held != project => faults.push(format!("the project row names {held}, and the folder is {project}'s")),
        _ => {}
    }
    let queries: [(&str, String); 7] = [
        // 1. A project has at least one repo: a parent that needs a child.
        ("repos", "SELECT 'the project has no repos' WHERE NOT EXISTS (SELECT 1 FROM repo)".to_string()),
        // 2. A question that says nothing of why it blocks nothing has an open item waiting on it.
        (
            "questions",
            "SELECT 'question #' || q.id || ' blocks no open item and says nothing about why it is asked' FROM question q \
             WHERE q.blocks_nothing IS NULL \
             AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = q.id) \
             AND NOT EXISTS (SELECT 1 FROM item_wait_question w WHERE w.on_question = q.id \
                             AND NOT EXISTS (SELECT 1 FROM closing c WHERE c.record_id = w.item_id)) \
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
        // 6. A worktree holds one branch of work.
        (
            "worktrees",
            "SELECT 'worktree ' || worktree || ' holds two branches of work: ' || group_concat(DISTINCT branch) \
             FROM running_place GROUP BY worktree HAVING COUNT(DISTINCT branch) > 1 ORDER BY worktree"
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
    ];
    for (_, sql) in &queries {
        let mut statement = db.prepare(sql).sql()?;
        for fault in statement.query_map([], |row| row.get::<_, String>(0)).sql()? {
            faults.push(fault.sql()?);
        }
    }
    Ok(faults)
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
