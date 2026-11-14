//! The rules a review reads: every `rules/*.md` and the project's
//! `projects/<project>-rules/*.md` of an agent-instructions checkout, with the
//! front matter `scripts/rules_review/matching_rules.py` reads (`paths`,
//! `blocking`, `review`) and the full body.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The file name without `.md`.
    pub slug: String,
    /// The globs the rule covers; None covers every path.
    pub paths: Option<Vec<String>>,
    /// Whether a finding of this rule refuses a landing.
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
            paths: parse_paths(front),
            blocking: value("blocking").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            reviewable: !value("review")
                .is_some_and(|v| ["false", "no", "off"].contains(&v.to_ascii_lowercase().as_str())),
            text: text.to_string(),
        }
    }

    /// The one-line `digest:` of the front matter.
    pub fn digest(&self) -> String {
        front_matter(&self.text)
            .and_then(|front| {
                front
                    .lines()
                    .find_map(|l| l.strip_prefix("digest:").map(|d| d.trim().to_string()))
            })
            .unwrap_or_default()
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

/// Gitignore-style match: `**/` is any number of directories (or none), `**`
/// anything, `*` and `?` stay inside one path segment.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    fn go(glob: &[u8], path: &[u8]) -> bool {
        match glob {
            [] => path.is_empty(),
            [b'*', b'*', b'/', rest @ ..] => {
                if go(rest, path) {
                    return true;
                }
                (0..path.len()).any(|i| path[i] == b'/' && go(rest, &path[i + 1..]))
            }
            [b'*', b'*', rest @ ..] => (0..=path.len()).any(|i| go(rest, &path[i..])),
            [b'*', rest @ ..] => {
                let segment = path.iter().position(|&b| b == b'/').unwrap_or(path.len());
                (0..=segment).any(|i| go(rest, &path[i..]))
            }
            [b'?', rest @ ..] => match path.split_first() {
                Some((&c, tail)) if c != b'/' => go(rest, tail),
                _ => false,
            },
            [c, rest @ ..] => match path.split_first() {
                Some((p, tail)) if p == c => go(rest, tail),
                _ => false,
            },
        }
    }
    go(glob.as_bytes(), path.as_bytes())
}

/// Every rule of a checkout: `rules/*.md`, then each `projects/*-rules/*.md`
/// when <project> is None, or only `projects/<project>-rules/*.md`.
/// Each in file-name order.
pub fn load(root: &Path, project: Option<&str>) -> Result<Vec<Rule>> {
    let mut files: Vec<PathBuf> = markdown_in(&root.join("rules"))?;
    match project {
        Some(project) => files.extend(markdown_in(
            &root.join("projects").join(format!("{project}-rules")),
        )?),
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
            for dir in dirs {
                files.extend(markdown_in(&dir)?);
            }
        }
    }
    let mut rules = Vec::new();
    for file in files {
        let text =
            std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
        let slug = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if rules.iter().any(|r: &Rule| r.slug == slug) {
            fail!("two rules are named {slug}; a slug names one rule");
        }
        rules.push(Rule::parse(&slug, &text));
    }
    Ok(rules)
}

fn markdown_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
