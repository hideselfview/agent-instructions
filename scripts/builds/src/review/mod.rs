//! `builds review`: reviews a change against the rules of agent-instructions,
//! one reviewer per group of rules (`review/groups.toml`).
//!
//! `builds review groups [--rules-root DIR] [--project P] [--changed PATH]...`
//! checks the grouping against the rules and lists it; with `--changed` it
//! lists instead the groups a change to those paths runs, for project P's
//! rules.

mod groups;
mod rules;

use std::path::PathBuf;

use crate::fail;
use crate::fail::Result;
use crate::lists::args::Args;

const USAGE: &str =
    "usage: builds review groups [--rules-root DIR] [--project P] [--changed PATH]...";

/// The checkout the rules and groups are read from: `--rules-root`, else the
/// one `install.sh` links at ~/.codex/agent-instructions.
fn rules_root(args: &Args) -> Result<PathBuf> {
    if let Some(root) = args.value("rules-root")? {
        return Ok(PathBuf::from(root));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".codex/agent-instructions")),
        None => fail!("HOME is not set; name the checkout with --rules-root"),
    }
}

pub fn cmd_review(args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "groups" => list_groups(rest),
        other => fail!("unknown review command {other}\n{USAGE}"),
    }
}

/// Prints each group, and fails when the grouping is not whole. With
/// `--changed`, prints only the groups those paths run.
fn list_groups(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "changed"], &[])?;
    args.exactly(0, USAGE)?;
    let root = rules_root(&args)?;
    let groups = groups::Groups::load(&root)?;
    let every = rules::load(&root, None)?;
    groups::require_whole(&groups, &every)?;
    let changed: Vec<String> = args
        .values("changed")
        .into_iter()
        .map(str::to_string)
        .collect();
    if changed.is_empty() {
        for group in &groups.groups {
            print_group(group, group.rules.iter().map(String::as_str));
        }
        return Ok(());
    }
    let project_rules = rules::load(&root, args.value("project")?)?;
    for (group, members) in groups.pick(&project_rules, &changed) {
        print_group(group, members.iter().map(|rule| rule.slug.as_str()));
    }
    Ok(())
}

fn print_group<'a>(group: &groups::Group, slugs: impl Iterator<Item = &'a str>) {
    println!("{} ({}): {}", group.name, group.model, group.question);
    for slug in slugs {
        println!("  {slug}");
    }
}
