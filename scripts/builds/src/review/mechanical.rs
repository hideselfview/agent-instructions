//! Whether a change over the review's size cap (`check::MAX_ADDED_LINES`)
//! is mechanical work that the compiler and the hook check, so that `builds
//! land` takes it with no review: commits that change design files only, or
//! a rename that changes identifiers and nothing else.

use std::collections::HashMap;
/// Why a change over the cap needs no review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanical {
    /// Every changed file is a design file (`builds design only`).
    Design,
    /// Once git pairs the renamed files, the lines that differ differ by
    /// renamed identifiers only.
    Rename,
}

impl Mechanical {
    pub fn word(self) -> &'static str {
        match self {
            Mechanical::Design => "design",
            Mechanical::Rename => "rename",
        }
    }

    pub fn parse(word: &str) -> Option<Mechanical> {
        match word {
            "design" => Some(Mechanical::Design),
            "rename" => Some(Mechanical::Rename),
            _ => None,
        }
    }

    /// What `builds review` and `builds land` say of the change.
    pub fn says(self) -> &'static str {
        match self {
            Mechanical::Design => "changes design files only",
            Mechanical::Rename => "changes only identifiers it renames",
        }
    }
}

/// Does <diff>, a net change that git wrote with rename detection (-M), change nothing but file names and
/// identifiers? Each run of changed lines must hold as many removed as added
/// lines, and each removed line must equal its added line once its
/// identifiers are replaced, with one new name for each old name across the
/// whole diff and one old name for each new name. A file added, deleted or
/// given another mode, and a binary file, are no rename.
pub fn only_renames(diff: &str) -> bool {
    let mut names = Names::default();
    let (mut removed, mut added): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let mut in_hunk = false;
    for line in diff.lines() {
        if in_hunk && !line.starts_with("diff --git ") {
            match line.chars().next() {
                Some('-') => {
                    if !added.is_empty() && !pair_off(&mut names, &mut removed, &mut added) {
                        return false;
                    }
                    removed.push(&line[1..]);
                }
                Some('+') => added.push(&line[1..]),
                Some('\\') => {}
                _ => {
                    if !pair_off(&mut names, &mut removed, &mut added) {
                        return false;
                    }
                    // The hunk's lines stop at the next header.
                    in_hunk = line.starts_with(' ') || line.is_empty();
                    if line.starts_with("@@") {
                        in_hunk = true;
                    }
                }
            }
            continue;
        }
        if !pair_off(&mut names, &mut removed, &mut added) {
            return false;
        }
        in_hunk = line.starts_with("@@");
        let breaks = [
            "new file mode",
            "deleted file mode",
            "old mode",
            "new mode",
            "Binary files",
            "GIT binary patch",
        ];
        if breaks.iter().any(|start| line.starts_with(start)) {
            return false;
        }
    }
    pair_off(&mut names, &mut removed, &mut added)
}

/// Checks the removed and added lines of one run against each other, and
/// empties both.
fn pair_off(names: &mut Names, removed: &mut Vec<&str>, added: &mut Vec<&str>) -> bool {
    let paired = removed.len() == added.len()
        && removed
            .iter()
            .zip(added.iter())
            .all(|(old, new)| names.rename(old, new));
    removed.clear();
    added.clear();
    paired
}

/// The identifiers a diff renames: the new name of each old one, and the
/// reverse.
#[derive(Default)]
struct Names {
    new_of: HashMap<String, String>,
    old_of: HashMap<String, String>,
}

impl Names {
    /// Is <new> <old> with identifiers renamed, each as the diff renamed it
    /// before?
    fn rename(&mut self, old: &str, new: &str) -> bool {
        let (old, new) = (tokens(old), tokens(new));
        if old.len() != new.len() {
            return false;
        }
        old.iter()
            .zip(&new)
            .all(|((was_word, was), (is_word, is))| {
                if was_word != is_word {
                    return false;
                }
                if was == is {
                    return true;
                }
                if !was_word {
                    return false;
                }
                let forward = self.new_of.entry(was.to_string()).or_insert(is.to_string());
                let backward = self.old_of.entry(is.to_string()).or_insert(was.to_string());
                forward == is && backward == was
            })
    }
}

/// <line> cut into runs of word characters and runs of the others, each with
/// whether it is an identifier: a word that starts with no digit. A number is
/// no identifier, so no rename changes it.
fn tokens(line: &str) -> Vec<(bool, &str)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = Vec::new();
    let mut start = 0;
    let mut kind = None;
    for (at, c) in line.char_indices() {
        let is_word = word(c);
        if kind != Some(is_word) {
            if let Some(was) = kind {
                found.push(token(was, &line[start..at]));
            }
            start = at;
            kind = Some(is_word);
        }
    }
    if let Some(was) = kind {
        found.push(token(was, &line[start..]));
    }
    found
}

fn token(word: bool, text: &str) -> (bool, &str) {
    let digit = text.starts_with(|c: char| c.is_ascii_digit());
    (word && !digit, text)
}

#[cfg(test)]
#[path = "mechanical_tests.rs"]
mod tests;
