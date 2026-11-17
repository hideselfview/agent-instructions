//! The rules a review reads: every `.md` under `rules/` and under the
//! project's `projects/<project>-rules/` of an agent-instructions checkout
//! (each in its group's folder), with the
//! front matter `scripts/rules_review/matching_rules.py` reads (`paths`,
//! `blocking`, `review`) and the full body.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::glob::{Question, segments_match};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The file name without `.md`.
    pub slug: String,
    /// The file's path in the checkout.
    pub path: String,
    /// The group folder the file is in, None when it sits outside one.
    pub group_dir: Option<String>,
    /// The globs the rule covers; None covers every path.
    pub paths: Option<Vec<String>>,
    /// Whether the reviewer's prompt and the report mark its findings blocking.
    pub blocking: bool,
    /// False for a rule marked `review: false`, which the reviewer can't check.
    pub reviewable: bool,
    /// The whole file, front matter included, which a reviewer reads.
    pub text: String,
}

impl Rule {
    pub fn parse(slug: &str, text: &str) -> Rule {
        let front = front_matter(text).unwrap_or("");
        let value = |key: &str| {
            front.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .map(|rest| rest.trim().to_string())
            })
        };
        Rule {
            slug: slug.to_string(),
            path: String::new(),
            group_dir: None,
            paths: parse_paths(front),
            blocking: value("blocking").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            reviewable: !value("review")
                .is_some_and(|v| ["false", "no", "off"].contains(&v.to_ascii_lowercase().as_str())),
            text: text.to_string(),
        }
    }

    /// The text after the front matter, which a person reads.
    pub fn body(&self) -> &str {
        match front_matter(&self.text) {
            Some(front) => {
                let after = &self.text["---\n".len() + front.len() + "\n---".len()..];
                after.split_once('\n').map_or("", |(_, body)| body)
            }
            None => &self.text,
        }
    }

    /// Whether the rule covers at least one of <changed>.
    pub fn matches(&self, changed: &[String]) -> bool {
        match &self.paths {
            None => true,
            Some(globs) => changed
                .iter()
                .any(|path| globs.iter().any(|glob| glob_matches(glob, path))),
        }
    }
}

/// The front matter block between the first two `---` lines.
fn front_matter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// The `paths:` globs of a front matter block, block or inline list.
fn parse_paths(front: &str) -> Option<Vec<String>> {
    let unquote = |s: &str| s.trim().trim_matches(|c| c == '\'' || c == '"').to_string();
    let mut lines = front.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("paths:") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inline) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            return Some(
                inline
                    .split(',')
                    .filter(|p| !p.trim().is_empty())
                    .map(unquote)
                    .collect(),
            );
        }
        let mut globs = Vec::new();
        for next in lines.by_ref() {
            match next.trim_start().strip_prefix("- ") {
                Some(item) if next.starts_with(' ') || next.starts_with('-') => {
                    globs.push(unquote(item))
                }
                _ => break,
            }
        }
        return Some(globs);
    }
    None
}

/// Gitignore-style match: a `**` segment is any number of directories (or
/// none), and `*` and `?` stay inside one path segment (glob.rs).
pub fn glob_matches(glob: &str, path: &str) -> bool {
    let pattern: Vec<&str> = glob.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments_match(&pattern, &path, Question::Wildcard)
}

/// Every rule of a checkout: the `.md` files under `rules/`, then those under
/// each `projects/*-rules/` when <project> is None, or only under
/// `projects/<project>-rules/`. Each tree in path order.
pub fn load(root: &Path, project: Option<&str>) -> Result<Vec<Rule>> {
    let mut trees: Vec<PathBuf> = vec![root.join("rules")];
    match project {
        Some(project) => trees.push(root.join("projects").join(format!("{project}-rules"))),
        None => {
            let projects = root.join("projects");
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(&projects)
                .context(|| format!("listing {}", projects.display()))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().ends_with("-rules"))
                })
                .collect();
            dirs.sort();
            trees.extend(dirs);
        }
    }
    let mut rules: Vec<Rule> = Vec::new();
    for tree in trees {
        let mut files = Vec::new();
        markdown_under(&tree, &mut files)?;
        files.sort();
        for file in files {
            let text =
                std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
            let slug = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(first) = rules.iter().find(|r| r.slug == slug) {
                fail!(
                    "two rules are named {slug} ({} and {}); a slug names one rule",
                    first.path,
                    file.display()
                );
            }
            let relative = file.strip_prefix(&tree).unwrap_or(&file);
            let mut parts = relative.components();
            let group_dir = match (parts.next(), parts.next()) {
                (Some(dir), Some(_)) => Some(dir.as_os_str().to_string_lossy().into_owned()),
                _ => None,
            };
            let mut rule = Rule::parse(&slug, &text);
            rule.path = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .display()
                .to_string();
            rule.group_dir = group_dir;
            rules.push(rule);
        }
    }
    Ok(rules)
}

/// Every `.md` file under <dir>, at any depth; none when <dir> is missing.
fn markdown_under(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", dir.display()))?
            .path();
        if path.is_dir() {
            markdown_under(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
