//! builds' own check of a finding's two quotes: the clause must be in the
//! rule's text, and the code must be in the diff, in the file the finding
//! names, on a line the diff adds or removes. A finding that fails either is
//! dropped and recorded as dropped.
//! The check also tells whether the code sits only on comment lines, for the
//! groups whose rules judge no code comment (`groups::Group::skips_comments`).

/// <text> with every run of whitespace as one space, and without the markdown
/// marks (`*`, backtick) a quote may have lost or kept.
pub fn squash(text: &str) -> String {
    let plain: String = text.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether <clause> is in <text> (a rule's text, or the project facts).
pub fn clause_in(clause: &str, text: &str) -> bool {
    let clause = squash(clause);
    !clause.is_empty() && squash(text).contains(&clause)
}

/// One file of a diff: its path in the new tree (the old one for a deletion)
/// and its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub lines: Vec<DiffLine>,
}

/// One line of a file's diff: its text without the diff's `+`, `-` or space
/// mark, and what the mark said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub text: String,
    pub kind: LineKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// A `+` line.
    Added,
    /// A `-` line.
    Removed,
    /// A line around the changes, which the diff neither adds nor removes.
    Context,
}

impl DiffLine {
    /// Whether the diff adds or removes the line.
    pub fn changed(&self) -> bool {
        self.kind != LineKind::Context
    }
}

/// Where a quote sits in a file's diff: the first and the last of a run of
/// lines (indexes into `DiffFile::lines`) whose text holds the quote, which
/// starts on the line where the quote starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub first: usize,
    pub last: usize,
}

impl DiffFile {
    /// Each place <code> sits in the file's lines, matched through
    /// whitespace and markdown marks (`squash`); none for an empty quote.
    pub fn places(&self, code: &str) -> Vec<Place> {
        let wanted = squash(code);
        if wanted.is_empty() {
            return Vec::new();
        }
        let lines: Vec<String> = self.lines.iter().map(|line| squash(&line.text)).collect();
        let joined = |lines: &[String]| -> String {
            let words: Vec<&str> = lines
                .iter()
                .map(String::as_str)
                .filter(|line| !line.is_empty())
                .collect();
            words.join(" ")
        };
        let mut places = Vec::new();
        for first in 0..lines.len() {
            for last in first..lines.len() {
                let text = joined(&lines[first..=last]);
                if text.contains(&wanted) {
                    // The run starts where the quote starts when the lines
                    // after its first one do not hold the quote alone.
                    if !joined(&lines[first + 1..=last]).contains(&wanted) {
                        places.push(Place { first, last });
                    }
                    break;
                }
                // A run that holds the quote past this length holds it
                // without its first line.
                if text.len() > wanted.len() + lines[first].len() + 1 {
                    break;
                }
            }
        }
        places
    }

    /// The lines of <place>.
    pub fn lines_of(&self, place: Place) -> &[DiffLine] {
        &self.lines[place.first..=place.last]
    }
}

/// The new path a `diff --git a/<old> b/<new>` line names: the text after its
/// last ` b/`; None for any other line.
pub fn new_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("diff --git ")?.trim_end();
    Some(rest.rsplit_once(" b/").map(|(_, new)| new).unwrap_or(rest))
}

pub fn parse_diff(diff: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if !in_hunk || line.starts_with("diff --git ") {
            if let Some(path) = new_path(line) {
                files.push(DiffFile {
                    path: path.to_string(),
                    lines: Vec::new(),
                });
                in_hunk = false;
            } else if line.starts_with("@@") {
                in_hunk = true;
            }
            continue;
        }
        if line.starts_with("@@") {
            continue;
        }
        let kind = match line.chars().next() {
            Some('+') => LineKind::Added,
            Some('-') => LineKind::Removed,
            Some(' ') => LineKind::Context,
            _ => continue,
        };
        if let Some(file) = files.last_mut() {
            file.lines.push(DiffLine {
                text: line[1..].to_string(),
                kind,
            });
        }
    }
    files
}

/// How many lines the hunks of <diff> add and remove, leaving out each file's
/// header lines.
pub fn changed_lines(diff: &str) -> i64 {
    let mut in_hunk = false;
    let mut changed = 0;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk && (line.starts_with('+') || line.starts_with('-')) {
            changed += 1;
        }
    }
    changed
}

/// Whether the path <path> is the file <file> a finding names: the whole
/// path, or its end when the reviewer named a shorter one.
pub fn names_file(path: &str, file: &str) -> bool {
    path == file || path.ends_with(&format!("/{file}"))
}

/// Whether <code> is in the diff's lines of <file>, matched by `names_file`,
/// on a line the diff adds or removes. A quote that sits only on the lines
/// around the changes quotes code that the diff does not change: in a later
/// run of a review, an earlier run read those lines.
pub fn code_in(files: &[DiffFile], file: &str, code: &str) -> Result<(), String> {
    if squash(code).is_empty() {
        return Err("it quotes no code".to_string());
    }
    let named: Vec<&DiffFile> = files.iter().filter(|f| names_file(&f.path, file)).collect();
    if named.is_empty() {
        return Err(format!("the diff has no file {file}"));
    }
    let mut found = false;
    for named in named {
        for place in named.places(code) {
            if named.lines_of(place).iter().any(DiffLine::changed) {
                return Ok(());
            }
            found = true;
        }
    }
    if found {
        Err(format!(
            "its code sits only on lines of {file} around the diff's changes, which the diff does not add or remove"
        ))
    } else {
        Err(format!("its code is not in the diff of {file}"))
    }
}

/// Whether <line>, a line of the file at <path>, is a comment line: its
/// first characters past its indent are the mark of a line comment, or the
/// start, middle or end of a block comment, in the file's language. builds
/// knows the comments of Rust, Swift, Kotlin and C# (`//`, `/*`, `*/`, and
/// for all but Rust a block comment's middle line `*`), of Python, shell and
/// TOML (`#`) and of SQL (`--`), by the file's extension. A line of any other
/// file, such as Markdown, is no comment line. In Rust a line that starts
/// with `* ` multiplies or dereferences, so a Rust line is a comment line
/// only by its slashes.
pub fn is_comment_line(path: &str, line: &str) -> bool {
    let text = line.trim_start();
    let slashes = text.starts_with("//") || text.starts_with("/*") || text.starts_with("*/");
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    match extension {
        "rs" => slashes,
        "swift" | "kt" | "kts" | "cs" => slashes || text == "*" || text.starts_with("* "),
        "py" | "sh" | "bash" | "zsh" | "toml" => text.starts_with('#'),
        "sql" => text.starts_with("--"),
        _ => false,
    }
}

/// Whether <code> sits only on comment lines of the diff's lines of <file>
/// (`is_comment_line`), matched by `names_file`: every line of every place
/// the quote sits (`DiffFile::places`) that is not blank is a comment line. A
/// quote builds finds nowhere sits on no comment line.
pub fn only_on_comment_lines(files: &[DiffFile], file: &str, code: &str) -> bool {
    let mut found = false;
    for named in files.iter().filter(|f| names_file(&f.path, file)) {
        for place in named.places(code) {
            found = true;
            let comments = named.lines_of(place).iter().all(|line| {
                line.text.trim().is_empty() || is_comment_line(&named.path, &line.text)
            });
            if !comments {
                return false;
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
