//! The groups of rules of agent-instructions that each get one reviewer. A
//! group is a folder `rules/<dir>/` holding its `group.toml`; its rules are the
//! `.md` files in that folder and in `projects/<project>-rules/<dir>/`. Every
//! rule is in exactly one group.

use std::path::Path;

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Result};
use crate::review::rules::Rule;

/// The file of a group's settings in its folder.
pub const FILE: &str = "group.toml";

/// The model a reviewer runs unless its group says otherwise.
const DEFAULT_MODEL: &str = "opus";

fn opus() -> String {
    DEFAULT_MODEL.to_string()
}

fn one() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub question: String,
    /// `opus`, `sonnet` or `haiku`; opus unless the group overrides it.
    #[serde(default = "opus")]
    pub model: String,
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
    /// Whether the group's rules judge no code comment, so builds drops a
    /// finding of one of them whose code sits only on comment lines
    /// (`check::only_on_comment_lines`).
    #[serde(default)]
    pub skips_comments: bool,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Groups {
    pub groups: Vec<Group>,
    /// A hash of every group's settings and rules, recorded with every run.
    pub digest: String,
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
        let mut fingerprint = String::new();
        for dir in dirs {
            let path = tree.join(&dir).join(FILE);
            let text =
                std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
            let slugs: Vec<String> = rules
                .iter()
                .filter(|rule| rule.group_dir.as_deref() == Some(dir.as_str()))
                .map(|rule| rule.slug.clone())
                .collect();
            fingerprint.push_str(&format!("{dir}\n{text}\n{}\n", slugs.join(",")));
            groups.push(Group::parse(&dir, &text, slugs)?);
        }
        Ok(Groups {
            groups,
            digest: crate::review::run::hash(&fingerprint),
        })
    }

    /// What is wrong with the grouping against <rules>, which must be every
    /// rule there is: a rule outside a group folder (a folder with no
    /// `group.toml`, or none), a group with no rule, two groups of one name, a
    /// model that is not opus, sonnet or haiku.
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
            if !["opus", "sonnet", "haiku"].contains(&group.model.as_str()) {
                faults.push(format!(
                    "group {} names model {}; it is opus, sonnet or haiku",
                    group.name, group.model
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

    /// The groups a review of a change to <changed> runs, each with its rules
    /// that exist in <rules> (the project's): a group that reviews no copy
    /// runs when at least one of those rules is reviewable and covers a
    /// changed path.
    pub fn pick<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &[String],
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.covering(rules, changed, false)
    }

    /// The copy groups (`Group::copy`) a copy review of a change to
    /// <changed> runs, picked as `pick` picks the others.
    pub fn pick_copy<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &[String],
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.covering(rules, changed, true)
    }

    /// The groups whose `copy` is <copy> and whose reviewable rules of
    /// <rules> cover one of <changed>, each with those rules.
    fn covering<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &[String],
        copy: bool,
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        self.with_members(rules)
            .filter(|(group, members)| {
                group.copy == copy && members.iter().any(|rule| rule.matches(changed))
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
