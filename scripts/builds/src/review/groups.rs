//! The groups of rules of agent-instructions that each get one reviewer. A
//! group is a folder `rules/<dir>/` holding its `group.toml`; its rules are the
//! `.md` files in that folder and in `projects/<project>-rules/<dir>/`. Every
//! rule is in exactly one group.

use std::path::Path;

use serde::Deserialize;

use super::provider::{Selection, Tier};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::design_paths::Globs;
use crate::review::rules::Rule;

/// The file of a group's settings in its folder.
pub const FILE: &str = "group.toml";

fn high() -> Selection {
    Selection::Tier(Tier::High)
}

fn tier<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Selection, D::Error> {
    Tier::deserialize(deserializer).map(Selection::Tier)
}

fn one() -> usize {
    1
}

/// What a group's rules judge of a code comment (a doc or line comment in a
/// source file).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Comments {
    /// The rules judge code, and a comment as part of it: the group reads
    /// the comments of a file whose code changed, and no file that a change
    /// changes only in comments (`comments::only_in_comments`).
    Code,
    /// The rules judge what a comment says, so the group also reads a file
    /// that a change changes only in comments.
    Judged,
    /// The rules judge no comment, so builds drops a finding of one of them
    /// whose code sits only on comment lines (`check::only_on_comment_lines`).
    Skipped,
}

/// A group whose group.toml says nothing of comments judges code.
fn comments_code() -> Comments {
    Comments::Code
}

/// The paths a change touches, and those of them it changes only in
/// comments, which only the groups whose rules judge what a comment says
/// read (`Comments::Judged`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub paths: Vec<String>,
    pub only_comments: Vec<String>,
}

impl Changed {
    /// <paths>, each changed in code, as builds counts the paths of a change
    /// whose files it does not read.
    pub fn in_code(paths: Vec<String>) -> Changed {
        Changed {
            paths,
            only_comments: Vec::new(),
        }
    }

    /// The change of <path> alone.
    pub fn only(&self, path: &str) -> Changed {
        let one = |paths: &[String]| paths.iter().filter(|p| *p == path).cloned().collect();
        Changed {
            paths: one(&self.paths),
            only_comments: one(&self.only_comments),
        }
    }

    /// The paths of the change that a group whose rules judge comments as
    /// <comments> reads.
    fn read_by(&self, comments: Comments) -> Vec<String> {
        self.paths
            .iter()
            .filter(|path| comments == Comments::Judged || !self.only_comments.contains(path))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub question: String,
    /// The capability tier, or a named model when a command overrides the tier.
    #[serde(rename = "tier", default = "high", deserialize_with = "tier")]
    pub model: Selection,
    /// The group's folder under `rules/`, set by the loader.
    #[serde(skip)]
    pub dir: String,
    /// Rule slugs, set by the loader from the folders.
    #[serde(skip)]
    pub rules: Vec<String>,
    /// Extra words for the reviewer.
    #[serde(default)]
    pub instructions: Option<String>,
    /// Whether the group also answers for the project's always-loaded facts.
    #[serde(default)]
    pub project_facts: bool,
    /// Whether the reviewer first lists every site of the diff that `sites`
    /// describes and judges each against every rule of the group, one file per
    /// call.
    #[serde(default)]
    pub enumerate: bool,
    /// What a site is, for a group that lists sites: the end of the sentence
    /// "every site in the diff that ...".
    #[serde(default)]
    pub sites: Option<String>,
    /// Whether the commit messages of the change are read as one more file.
    #[serde(default)]
    pub commit_text: bool,
    /// What the group's rules judge of a code comment.
    #[serde(default = "comments_code")]
    pub comments: Comments,
    /// Whether the group reviews copy: its rules judge word choice, grammar
    /// and style. Only the copy review (`builds review copy`) asks such a
    /// group; `builds review`, its audits and its replays never do.
    #[serde(default)]
    pub copy: bool,
    /// How many times each call is made; the findings of all are kept. The
    /// later calls of one prompt read the first one's prompt from the cache.
    #[serde(default = "one")]
    pub passes: usize,
}

impl Group {
    /// The group of folder <dir> from the text of its `group.toml`.
    pub fn parse(dir: &str, text: &str, rules: Vec<String>) -> Result<Group> {
        let mut group: Group = toml::from_str(text)
            .map_err(|error| crate::fail::Fail::Message(format!("rules/{dir}/{FILE}: {error}")))?;
        group.dir = dir.to_string();
        group.rules = rules;
        Ok(group)
    }
}

#[derive(Clone, Debug)]
pub struct Groups {
    pub groups: Vec<Group>,
}

impl Groups {
    /// The groups of the checkout at <root>: each folder of `rules/` that holds
    /// a `group.toml`, in folder-name order, with the rules found in it and in
    /// the same-named folder of each `projects/*-rules/`.
    pub fn load(root: &Path) -> Result<Groups> {
        let rules = crate::review::rules::load(root, None)?;
        let tree = root.join("rules");
        let entries = std::fs::read_dir(&tree).context(|| format!("listing {}", tree.display()))?;
        let mut dirs: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().join(FILE).is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        dirs.sort();
        let mut groups = Vec::new();
        for dir in dirs {
            let path = tree.join(&dir).join(FILE);
            let text =
                std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
            let slugs: Vec<String> = rules
                .iter()
                .filter(|rule| rule.group_dir.as_deref() == Some(dir.as_str()))
                .map(|rule| rule.slug.clone())
                .collect();
            groups.push(Group::parse(&dir, &text, slugs)?);
        }
        Ok(Groups { groups })
    }

    /// What is wrong with the grouping against <rules>, which must be every
    /// rule there is: a rule outside a group folder (a folder with no
    /// `group.toml`, or none), a group with no rule, or two groups with one name.
    pub fn faults(&self, rules: &[Rule]) -> Vec<String> {
        let mut faults = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        for group in &self.groups {
            if names.contains(&group.name.as_str()) {
                faults.push(format!("two groups are named {}", group.name));
            }
            names.push(&group.name);
            if group.rules.is_empty() {
                faults.push(format!("group {} has no rule", group.name));
            }
            if group.enumerate && (group.sites.is_none() || group.project_facts) {
                faults.push(format!(
                    "group {} lists sites, so it says what a site is (`sites`) and has no project facts",
                    group.name
                ));
            }
            if !group.enumerate && group.sites.is_some() {
                faults.push(format!(
                    "group {} says what a site is but does not list sites (`enumerate`)",
                    group.name
                ));
            }
            if group.passes == 0 {
                faults.push(format!(
                    "group {} makes no call (`passes` is 0)",
                    group.name
                ));
            }
        }
        for rule in rules {
            let in_group = rule
                .group_dir
                .as_ref()
                .is_some_and(|dir| self.groups.iter().any(|g| &g.dir == dir));
            if !in_group {
                faults.push(format!(
                    "{} ({}) is in no group: put it in a folder of rules/ that has a {FILE}",
                    rule.slug, rule.path
                ));
            }
        }
        faults
    }

    /// The groups a review of a change to <changed>, in a repo whose design and capture
    /// files <files> name, runs, each with its rules that exist in <rules>
    /// (the project's): a group that reviews no copy runs when at least one of
    /// those rules is reviewable and covers a changed path that the group
    /// reads (`Rule::matches`, `Changed::read_by`).
    pub fn pick<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &Changed,
        files: &Globs,
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.covering(rules, changed, files, false)
    }

    /// The copy groups (`Group::copy`) a copy review of a change to
    /// <changed> runs, picked as `pick` picks the others.
    pub fn pick_copy<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &Changed,
        files: &Globs,
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.covering(rules, changed, files, true)
    }

    /// The groups whose `copy` is <copy> and whose reviewable rules of
    /// <rules> cover one of the paths of <changed> that the group reads, each
    /// with those rules.
    fn covering<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &Changed,
        files: &Globs,
        copy: bool,
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.with_members(rules)
            .filter(|(group, members)| {
                let read = changed.read_by(group.comments);
                group.copy == copy && members.iter().any(|rule| rule.matches(&read, files))
            })
            .collect()
    }

    /// The groups that review no copy whose names are in <names>, each with
    /// its reviewable rules that exist in <rules> (the project's).
    pub fn named<'a>(
        &'a self,
        rules: &'a [Rule],
        names: &[String],
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.reviewing(rules)
            .filter(|(group, _)| names.contains(&group.name))
            .collect()
    }

    /// The groups that review no copy, each with its reviewable rules that
    /// exist in <rules> (the project's).
    pub fn reviewing<'a>(
        &'a self,
        rules: &'a [Rule],
    ) -> impl Iterator<Item = (&'a Group, Vec<&'a Rule>)> {
        self.with_members(rules).filter(|(group, _)| !group.copy)
    }

    /// Every group with its reviewable rules that exist in <rules>.
    fn with_members<'a>(
        &'a self,
        rules: &'a [Rule],
    ) -> impl Iterator<Item = (&'a Group, Vec<&'a Rule>)> {
        self.groups.iter().map(|group| {
            let members: Vec<&Rule> = group
                .rules
                .iter()
                .filter_map(|slug| rules.iter().find(|r| &r.slug == slug))
                .filter(|rule| rule.reviewable)
                .collect();
            (group, members)
        })
    }
}

/// Fails with every fault of the checkout's grouping, or says it is whole.
pub fn require_whole(groups: &Groups, rules: &[Rule]) -> Result<()> {
    let faults = groups.faults(rules);
    if faults.is_empty() {
        return Ok(());
    }
    fail!("the rule groups are not whole:\n  {}", faults.join("\n  "))
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
