//! `rules/groups.toml` of agent-instructions: the groups of rules that each
//! get one reviewer. Every rule is in exactly one group.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Result};
use crate::review::rules::Rule;

/// Where the file sits in a checkout.
pub const FILE: &str = "rules/groups.toml";

fn one() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub question: String,
    /// `opus`, `sonnet` or `haiku`.
    pub model: String,
    /// Rule slugs.
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
    /// How many times each call is made; the findings of all are kept. The
    /// later calls of one prompt read the first one's prompt from the cache.
    #[serde(default = "one")]
    pub passes: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct File {
    group: Vec<Group>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Groups {
    pub groups: Vec<Group>,
}

impl Groups {
    pub fn parse(text: &str) -> Result<Groups> {
        let file: File = toml::from_str(text)
            .map_err(|error| crate::fail::Fail::Message(format!("{FILE}: {error}")))?;
        Ok(Groups { groups: file.group })
    }

    /// The groups of the checkout at <root>.
    pub fn load(root: &Path) -> Result<Groups> {
        let path = root.join(FILE);
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        Groups::parse(&text)
    }

    /// What is wrong with the grouping against <rules>, which must be every
    /// rule there is: a rule in no group or in two, a slug naming no rule, a
    /// group with no rule, two groups of one name, a model that is not opus or
    /// sonnet.
    pub fn faults(&self, rules: &[Rule]) -> Vec<String> {
        let mut faults = Vec::new();
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
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
            if group.commit_text && !group.enumerate {
                faults.push(format!(
                    "group {} reads commit messages but does not list sites (`enumerate`)",
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
            for slug in &group.rules {
                if !rules.iter().any(|r| &r.slug == slug) {
                    faults.push(format!(
                        "group {} names {slug}, which is no rule",
                        group.name
                    ));
                }
                if let Some(first) = seen.insert(slug, &group.name) {
                    faults.push(format!(
                        "{slug} is in group {first} and group {}; a rule is in one group",
                        group.name
                    ));
                }
            }
        }
        for rule in rules {
            if !seen.contains_key(rule.slug.as_str()) {
                faults.push(format!("{} is in no group", rule.slug));
            }
        }
        faults
    }

    /// The groups a change to <changed> runs, each with its rules that exist
    /// in <rules> (the project's): a group runs when at least one of those
    /// rules is reviewable and covers a changed path.
    pub fn pick<'a>(
        &'a self,
        rules: &'a [Rule],
        changed: &[String],
    ) -> Vec<(&'a Group, Vec<&'a Rule>)> {
        let mut picked = Vec::new();
        for group in &self.groups {
            let members: Vec<&Rule> = group
                .rules
                .iter()
                .filter_map(|slug| rules.iter().find(|r| &r.slug == slug))
                .filter(|rule| rule.reviewable)
                .collect();
            if members.iter().any(|rule| rule.matches(changed)) {
                picked.push((group, members));
            }
        }
        picked
    }
}

/// Fails with every fault of the checkout's grouping, or says it is whole.
pub fn require_whole(groups: &Groups, rules: &[Rule]) -> Result<()> {
    let faults = groups.faults(rules);
    if faults.is_empty() {
        return Ok(());
    }
    fail!("{FILE} is not whole:\n  {}", faults.join("\n  "))
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
