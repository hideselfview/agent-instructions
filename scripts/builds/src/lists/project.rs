//! The project: its name, the repos its lists cover, and each repo's globs
//! for design places, design-system sources and spec files. These are the
//! writes of the tables in project.sql; `command` is `lists init`,
//! `lists add-repo` and `lists design-globs` around them.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;

/// Per repo, the globs of one kind of design file.
pub type Globs = BTreeMap<String, Vec<String>>;

/// What a project is made of when it is made.
pub struct NewProject {
    pub name: String,
    pub repos: Vec<String>,
    pub places: Globs,
    pub sources: Globs,
    pub specs: Globs,
}

impl NewProject {
    /// What is wrong with the project as given, before anything is made.
    pub fn refusal(&self) -> Option<String> {
        if let Some(fault) = crate::lists::check::project_name_fault(&self.name) {
            return Some(fault);
        }
        if self.repos.is_empty() {
            return Some("the project has no repos".to_string());
        }
        for repo in &self.repos {
            if let Some(fault) = crate::lists::check::repo_name_fault(repo) {
                return Some(fault);
            }
            if self.repos.iter().filter(|other| *other == repo).count() > 1 {
                return Some(format!("repo {repo} is listed twice"));
            }
        }
        [
            (GlobKind::Place, &self.places),
            (GlobKind::Source, &self.sources),
            (GlobKind::Spec, &self.specs),
        ]
        .into_iter()
        .find_map(|(kind, globs)| globs_fault(&self.repos, kind, globs))
    }
}

/// One kind of design file a repo's globs name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobKind {
    Place,
    Source,
    Spec,
}

impl GlobKind {
    /// The kind as the design_glob table writes it.
    fn column(self) -> &'static str {
        match self {
            GlobKind::Place => "place",
            GlobKind::Source => "source",
            GlobKind::Spec => "spec",
        }
    }

    fn what(self) -> &'static str {
        match self {
            GlobKind::Place => "design places",
            GlobKind::Source => "design sources",
            GlobKind::Spec => "spec files",
        }
    }
}

/// What is wrong with <globs> of <kind> for a project of <repos>: a repo not
/// among them, or an empty glob.
fn globs_fault(repos: &[String], kind: GlobKind, globs: &Globs) -> Option<String> {
    let what = kind.what();
    for (repo, globs) in globs {
        if !repos.contains(repo) {
            return Some(format!(
                "{what} name repo {repo}, which is not one of the project's"
            ));
        }
        if globs.iter().any(|glob| glob.trim().is_empty()) {
            return Some(format!("repo {repo} has an empty glob among its {what}"));
        }
    }
    None
}

/// The project's row, its repos and every repo's globs.
pub fn init(db: &Connection, new: &NewProject) -> Result<()> {
    let held: i64 = db
        .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
        .sql()?;
    if held > 0 {
        fail!(
            "the lists of project {} already exist; init makes them once",
            new.name
        );
    }
    db.execute(
        "INSERT INTO project (singleton, name) VALUES (1, ?1)",
        [&new.name],
    )
    .sql()?;
    for repo in &new.repos {
        db.execute("INSERT INTO repo (name) VALUES (?1)", [repo])
            .sql()?;
    }
    for (kind, globs) in [
        (GlobKind::Place, &new.places),
        (GlobKind::Source, &new.sources),
        (GlobKind::Spec, &new.specs),
    ] {
        insert_globs(db, kind, globs)?;
    }
    Ok(())
}

fn insert_globs(db: &Connection, kind: GlobKind, globs: &Globs) -> Result<()> {
    for (repo, globs) in globs {
        for glob in globs {
            db.execute(
                "INSERT OR IGNORE INTO design_glob (repo, kind, glob) VALUES (?1, ?2, ?3)",
                params![repo, kind.column(), glob],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// Sets the globs of <kind> of each repo <globs> names to the ones it gives,
/// in place of what the repo had; other repos and kinds keep theirs.
pub fn set_design_globs(db: &Connection, kind: GlobKind, globs: &Globs) -> Result<()> {
    let project = read(db)?;
    if let Some(fault) = globs_fault(&project.repos, kind, globs) {
        fail!("project {}: {fault}", project.name);
    }
    for repo in globs.keys() {
        db.execute(
            "DELETE FROM design_glob WHERE repo = ?1 AND kind = ?2",
            params![repo, kind.column()],
        )
        .sql()?;
    }
    insert_globs(db, kind, globs)
}

/// The health finding for each repo of a loaded store whose main checkout
/// (`<home>/dev/<repo>`) has a design folder and that has no design place
/// globs: lists cannot record a design change in it, and `land` never asks
/// for one.
pub fn missing_globs_findings(
    home: &std::path::Path,
    loaded: &std::result::Result<Vec<crate::lists::store::Loaded>, String>,
) -> Vec<crate::health::Advice> {
    let Ok(loaded) = loaded else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for lists in loaded.iter().filter_map(|store| store.lists.as_ref().ok()) {
        for repo in &lists.repos {
            let design = home.join(crate::host::clone_path(repo)).join("design");
            if design.is_dir() && !lists.design_places.contains_key(repo) {
                found.push(crate::health::Advice::run(
                    format!(
                        "lists: repo {repo} has a design folder ({}), and project {}'s store names none of its design places, so lists cannot record a design change there and landing never asks for one; set its globs with",
                        design.display(),
                        lists.project
                    ),
                    format!(
                        "builds lists design-globs {} --design-places {repo}:GLOB,... [--design-sources {repo}:GLOB,...] [--spec-files {repo}:GLOB,...]",
                        lists.project
                    ),
                ));
            }
        }
    }
    found
}

/// `R:GLOB,R:GLOB` as each repo's globs, for the flag <flag>.
pub fn parse_globs(flag: &str, text: &str) -> Result<Globs> {
    let mut places = Globs::new();
    for part in text.split(',') {
        let Some((repo, glob)) = part.split_once(':') else {
            fail!("--{flag} takes REPO:GLOB entries, got '{part}'");
        };
        places
            .entry(repo.to_string())
            .or_default()
            .push(glob.to_string());
    }
    Ok(places)
}

/// The project carries the work of one more repo, <repo>.
pub fn add_repo(db: &Connection, repo: &str) -> Result<()> {
    let project = name(db)?;
    let held: i64 = db
        .query_row("SELECT COUNT(*) FROM repo WHERE name = ?1", [repo], |row| {
            row.get(0)
        })
        .sql()?;
    if held > 0 {
        fail!("repo {repo} is already one of project {project}'s");
    }
    db.execute("INSERT INTO repo (name) VALUES (?1)", [repo])
        .sql()?;
    Ok(())
}

/// The project's name, or the refusal when no project row exists.
pub fn name(db: &Connection) -> Result<String> {
    match db.query_row("SELECT name FROM project", [], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(name) => Ok(name),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            fail!("builds.sqlite holds no project; `builds lists init` makes the lists")
        }
        Err(error) => Err(error).sql(),
    }
}

/// What the project's tables hold.
pub struct Read {
    pub name: String,
    pub repos: Vec<String>,
    pub places: Globs,
    pub sources: Globs,
    pub specs: Globs,
}

pub fn read(db: &Connection) -> Result<Read> {
    let name = name(db)?;
    let mut statement = db.prepare("SELECT name FROM repo ORDER BY name").sql()?;
    let repos: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .sql()?
        .map(|row| row.sql())
        .collect::<Result<_>>()?;
    let mut globs: BTreeMap<&str, Globs> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT kind, repo, glob FROM design_glob ORDER BY kind, repo, glob")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?;
    for row in rows {
        let (kind, repo, glob) = row.sql()?;
        let kind = match kind.as_str() {
            "place" => "place",
            "source" => "source",
            _ => "spec",
        };
        globs
            .entry(kind)
            .or_default()
            .entry(repo)
            .or_default()
            .push(glob);
    }
    Ok(Read {
        name,
        repos,
        places: globs.remove("place").unwrap_or_default(),
        sources: globs.remove("source").unwrap_or_default(),
        specs: globs.remove("spec").unwrap_or_default(),
    })
}

/// The tables project.rs owns.
#[cfg(test)]
const TABLES: [&str; 3] = ["project", "repo", "design_glob"];

/// Writes a row of one of project.rs's tables as given, the constraints
/// still holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
