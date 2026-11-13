//! The project: its name, the repos its lists cover, and each repo's globs
//! for design places, design-system sources and spec files. These are the
//! operations on the state a write holds; `command` is `lists init` and
//! `lists add-repo` around them.

use std::collections::BTreeMap;

use crate::fail;
use crate::fail::Result;
use crate::lists::{Id, Lists};

/// Per repo, the globs of one kind of design file.
pub type Globs = BTreeMap<String, Vec<String>>;

impl Lists {
    /// A new project's lists: nothing yet, version 0.
    pub fn empty(project: &str, repos: Vec<String>, design_places: Globs) -> Lists {
        Lists {
            project: project.to_string(),
            repos,
            design_places,
            design_sources: BTreeMap::new(),
            spec_files: BTreeMap::new(),
            next_id: Id(1),
            version: 0,
            areas: Vec::new(),
            questions: Vec::new(),
            items: Vec::new(),
            designs: Vec::new(),
            decisions: Vec::new(),
            closed: Vec::new(),
            log: Vec::new(),
        }
    }
}

/// The lists of a new project <project> over <repos>, with each repo's globs.
pub fn init(
    project: &str,
    repos: Vec<String>,
    places: Globs,
    sources: Globs,
    specs: Globs,
) -> Lists {
    let mut lists = Lists::empty(project, repos, places);
    lists.design_sources = sources;
    lists.spec_files = specs;
    lists
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
pub fn add_repo(lists: &mut Lists, repo: &str) -> Result<()> {
    if lists.repos.iter().any(|held| held == repo) {
        fail!("repo {repo} is already one of project {}'s", lists.project);
    }
    lists.repos.push(repo.to_string());
    Ok(())
}

/// Replaces the repos of the project, whatever they are called.
#[cfg(test)]
pub fn plant_repos(lists: &mut Lists, repos: Vec<String>) {
    lists.repos = repos;
}

/// Sets <repo>'s design place globs, whatever <repo> is.
#[cfg(test)]
pub fn plant_design_places(lists: &mut Lists, repo: &str, globs: Vec<String>) {
    lists.design_places.insert(repo.to_string(), globs);
}

/// Sets <repo>'s design source globs, whatever <repo> is.
#[cfg(test)]
pub fn plant_design_sources(lists: &mut Lists, repo: &str, globs: Vec<String>) {
    lists.design_sources.insert(repo.to_string(), globs);
}

/// Sets <repo>'s spec file globs, whatever <repo> is.
#[cfg(test)]
pub fn plant_spec_files(lists: &mut Lists, repo: &str, globs: Vec<String>) {
    lists.spec_files.insert(repo.to_string(), globs);
}
