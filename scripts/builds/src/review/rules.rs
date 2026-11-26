//! The rules a review reads: every `.md` under `rules/` and under the
//! project's `projects/<project>-rules/` of an agent-instructions checkout
//! (each in its group's folder), with the
//! front matter (`paths`, `blocking`, `review`), whether the rule judges
//! design files (`design`) or capture files
//! (`capture`),
//! and the full body.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::glob::{Question, segments_match};
use crate::lists::design_paths::{FileKind, Globs};

/// The agent-instructions checkout linked by the installer.
pub fn installed_root(home: &Path) -> PathBuf {
    home.join(".codex/agent-instructions")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The file name without `.md`.
    pub slug: String,
    /// The file's path in the checkout.
    pub path: String,
    /// The group folder the file is in, None when it sits outside one.
    pub group_dir: Option<String>,
    /// The globs the rule covers; None covers every path. A glob that starts
    /// with `!` leaves out the paths it matches, whatever other globs match
    /// them.
    pub paths: Option<Vec<String>>,
    /// Whether the reviewer's prompt and the report mark its findings blocking.
    pub blocking: bool,
    /// False for a rule marked `review: false`, which the reviewer can't check.
    pub reviewable: bool,
    /// True for a rule marked `design: true`, which judges how a screen, board
    /// or component looks: it judges the design files its globs match. Every
    /// other rule judges no design file, whatever its globs match.
    pub design: bool,
    /// True for a rule marked `capture: true`, which judges screenshot capture
    /// files: it judges the capture files its globs match. Every other rule
    /// judges no capture file, whatever its globs match.
    pub capture: bool,
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
            design: value("design").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            capture: value("capture").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            text: text.to_string(),
        }
    }

    /// Does the rule judge files of <kind>? Only a rule marked for the kind.
    pub fn judges(&self, kind: FileKind) -> bool {
        match kind {
            FileKind::Design => self.design,
            FileKind::Capture => self.capture,
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

    /// The rule as an agent reads it: a heading with the rule's path, then its
    /// text.
    pub fn printed(&self) -> String {
        format!(
            "### `{}`\n\n{}\n",
            self.path,
            self.body().trim_matches('\n')
        )
    }

    /// Whether the rule covers at least one of <changed>, in a repo whose
    /// design and capture files <files> names: a path its globs match, which
    /// is neither unless the rule judges files of that kind.
    pub fn matches(&self, changed: &[String], files: &Globs) -> bool {
        changed.iter().any(|path| self.covers(path, files))
    }

    /// Whether the rule applies when authoring any of these paths.
    pub fn matches_paths(&self, paths: &[String]) -> bool {
        paths.iter().any(|path| self.matches_path(path))
    }

    /// Whether the rule covers <path>, in a repo whose design and capture
    /// files <files> names.
    fn covers(&self, path: &str, files: &Globs) -> bool {
        self.matches_path(path) && files.kind_of(path).is_none_or(|kind| self.judges(kind))
    }

    fn matches_path(&self, path: &str) -> bool {
        match &self.paths {
            None => true,
            Some(globs) => {
                let mut covered = false;
                for glob in globs {
                    match glob.strip_prefix('!') {
                        Some(left_out) if glob_matches(left_out, path) => return false,
                        Some(_) => {}
                        None => covered |= glob_matches(glob, path),
                    }
                }
                covered
            }
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
    if let Some(project) = project {
        let dir = root.join("projects").join(format!("{project}-rules"));
        return load_trees(root, [(dir, true)]);
    }
    let projects = root.join("projects");
    let entries =
        std::fs::read_dir(&projects).context(|| format!("listing {}", projects.display()))?;
    let mut trees = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", projects.display()))?;
        if entry.file_name().to_string_lossy().ends_with("-rules") {
            trees.push(entry.path());
        }
    }
    trees.sort();
    load_trees(root, trees.into_iter().map(|tree| (tree, false)))
}

/// The rules shared by all projects.
pub fn load_shared(root: &Path) -> Result<Vec<Rule>> {
    load_trees(root, [])
}

/// The shared rules and a project rule directory that must exist.
pub fn load_project(root: &Path, project: &str) -> Result<Vec<Rule>> {
    load_trees(
        root,
        [(
            root.join("projects").join(format!("{project}-rules")),
            false,
        )],
    )
}

fn load_trees(root: &Path, trees: impl IntoIterator<Item = (PathBuf, bool)>) -> Result<Vec<Rule>> {
    let mut rules: Vec<Rule> = Vec::new();
    for (tree, optional) in std::iter::once((root.join("rules"), false)).chain(trees) {
        let mut files = Vec::new();
        markdown_under(&tree, &mut files, optional)?;
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

/// Every `.md` file under <dir>, at any depth.
fn markdown_under(dir: &Path, files: &mut Vec<PathBuf>, optional: bool) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    for entry in entries {
        let path = entry
            .context(|| format!("listing {}", dir.display()))?
            .path();
        if path.is_dir() {
            markdown_under(&path, files, false)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
