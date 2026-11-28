//! builds' own check of a finding's two quotes: the clause must be in the
//! rule's text, and the code must be in the diff, in the file the finding
//! names, on a line the diff adds or removes. A finding that fails either is
//! dropped and recorded as dropped.
//! The check also tells whether the code sits only on comment lines, for the
//! groups whose rules judge no code comment (`groups::Comments::Skipped`).

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
    pub renamed_from: Option<String>,
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
    Added(i64),
    /// A `-` line.
    Removed(i64),
    /// A line around the changes, which the diff neither adds nor removes.
    Context { before: i64, after: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

impl DiffLine {
    /// Whether the diff adds or removes the line.
    pub fn changed(&self) -> bool {
        !matches!(self.kind, LineKind::Context { .. })
    }

    fn number(&self, side: Side) -> Option<i64> {
        match (side, self.kind) {
            (Side::Before, LineKind::Removed(line) | LineKind::Context { before: line, .. })
            | (Side::After, LineKind::Added(line) | LineKind::Context { after: line, .. }) => {
                Some(line)
            }
            _ => None,
        }
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
    pub(super) fn from_file(file: &super::files::FileDiff) -> Self {
        Self {
            path: file.path.clone(),
            renamed_from: file.renamed_from.clone(),
            lines: parse_lines(&file.text),
        }
    }

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

#[cfg(test)]
fn parse_diff(diff: &str) -> crate::fail::Result<Vec<DiffFile>> {
    let patch = super::files::Patch::parse(diff.to_string())?;
    Ok(patch.files().map(DiffFile::from_file).collect())
}

fn parse_lines(diff: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut numbers = None;
    for line in diff.lines() {
        if let Some((old, new)) = super::files::hunk_ranges(line) {
            numbers = Some((old.start, new.start));
            continue;
        }
        let Some((before, after)) = &mut numbers else {
            continue;
        };
        let kind = match line.chars().next() {
            Some('+') => LineKind::Added(*after),
            Some('-') => LineKind::Removed(*before),
            Some(' ') => LineKind::Context {
                before: *before,
                after: *after,
            },
            _ => continue,
        };
        if !matches!(kind, LineKind::Added(_)) {
            *before += 1;
        }
        if !matches!(kind, LineKind::Removed(_)) {
            *after += 1;
        }
        lines.push(DiffLine {
            text: line[1..].to_string(),
            kind,
        });
    }
    lines
}

/// The most reviewer processes a run starts at once.
pub const REVIEWERS_AT_ONCE: usize = 3;

/// <f> over <items>, one reviewer process each, at most `REVIEWERS_AT_ONCE`
/// at a time, results in order.
pub fn reviewers_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    crate::checkout::parallel_map_at_most(REVIEWERS_AT_ONCE, items, f)
}

/// How many lines the hunks of <diff> add.
pub fn added_lines(diff: &str) -> i64 {
    hunk_lines(diff, |line| line.starts_with('+'))
}

/// How many lines the hunks of <diff> add and remove, leaving out each file's
/// header lines.
pub fn changed_lines(diff: &str) -> i64 {
    hunk_lines(diff, |line| line.starts_with('+') || line.starts_with('-'))
}

/// How many lines in the hunks of <diff> satisfy <counts>.
fn hunk_lines(diff: &str, counts: impl Fn(&str) -> bool) -> i64 {
    let mut in_hunk = false;
    let mut found = 0;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk && counts(line) {
            found += 1;
        }
    }
    found
}

/// Whether the path <path> is the file <file> a finding names: the whole
/// path, or its end when the reviewer named a shorter one.
pub fn names_file(path: &str, file: &str) -> bool {
    path == file || path.ends_with(&format!("/{file}"))
}

/// The patches of the exact path, or the patches of the one path with the
/// given bare name when the diff contains no exact path.
fn named_patches<'a>(files: &'a [DiffFile], file: &str) -> Result<Vec<&'a DiffFile>, String> {
    let exact: Vec<_> = files.iter().filter(|patch| patch.path == file).collect();
    if !exact.is_empty() {
        return Ok(exact);
    }
    if file.contains('/') {
        return Err(format!("the diff has no file {file}"));
    }
    let named: Vec<_> = files
        .iter()
        .filter(|patch| names_file(&patch.path, file))
        .collect();
    let Some(first) = named.first() else {
        return Err(format!("the diff has no file {file}"));
    };
    if named.iter().any(|patch| patch.path != first.path) {
        return Err(format!("the diff has more than one file named {file}"));
    }
    Ok(named)
}

fn unchanged_quote(file: &str) -> String {
    format!(
        "its code sits only on lines of {file} around the diff's changes, which the diff does not add or remove"
    )
}

/// The starting lines of contiguous occurrences of the quote on one side
/// of the diff, with whether each occurrence touches a changed line.
fn quote_sites(patch: &DiffFile, side: Side, code: &str) -> Vec<(i64, bool)> {
    let view = DiffFile {
        path: patch.path.clone(),
        renamed_from: patch.renamed_from.clone(),
        lines: patch
            .lines
            .iter()
            .filter(|line| line.number(side).is_some())
            .cloned()
            .collect(),
    };
    view.places(code)
        .into_iter()
        .filter_map(|place| {
            let lines = view.lines_of(place);
            let first = lines[0].number(side).expect("line on this side");
            lines
                .iter()
                .zip(first..)
                .all(|(part, number)| part.number(side) == Some(number))
                .then(|| (first, lines.iter().any(DiffLine::changed)))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub side: Side,
    pub line: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuoteSite<'a> {
    pub file: &'a str,
    pub source: FilePosition<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePosition<Path> {
    pub path: Path,
    pub position: Position,
}

impl FilePosition<&str> {
    pub fn owned(self) -> FilePosition<String> {
        FilePosition {
            path: self.path.to_string(),
            position: self.position,
        }
    }
}

/// The places where one file's diff contains the quote.
struct QuoteMatches<'a> {
    file: &'a str,
    changed: Vec<FilePosition<&'a str>>,
    context: Vec<i64>,
}

impl QuoteMatches<'_> {
    fn no_changed_quote(&self, file: &str) -> String {
        if self.context.is_empty() {
            format!("its code is not in the diff of {file}")
        } else {
            unchanged_quote(file)
        }
    }
}

fn quote_matches<'a>(
    files: &'a [DiffFile],
    file: &str,
    code: &str,
) -> Result<QuoteMatches<'a>, String> {
    if squash(code).is_empty() {
        return Err("it quotes no code".to_string());
    }
    let named = named_patches(files, file)?;
    let mut found = QuoteMatches {
        file: named[0].path.as_str(),
        changed: Vec::new(),
        context: Vec::new(),
    };
    for patch in named {
        let old_path = patch.renamed_from.as_deref().unwrap_or(found.file);
        for (side, path) in [(Side::After, found.file), (Side::Before, old_path)] {
            for (line, changed) in quote_sites(patch, side, code) {
                if changed {
                    let site = FilePosition {
                        path,
                        position: Position { side, line },
                    };
                    if !found.changed.contains(&site) {
                        found.changed.push(site);
                    }
                } else {
                    found.context.push(line);
                }
            }
        }
    }
    Ok(found)
}

/// The start of the quote on the side where the diff changes it. The
/// reported line disambiguates occurrences within the named file.
pub fn code_site<'a>(
    files: &'a [DiffFile],
    file: &str,
    line: i64,
    code: &str,
) -> Result<QuoteSite<'a>, String> {
    let found = quote_matches(files, file, code)?;
    let exact: Vec<_> = found
        .changed
        .iter()
        .filter(|site| site.position.line == line)
        .collect();
    let site = |source| QuoteSite {
        file: found.file,
        source,
    };
    let ambiguous = || {
        format!(
            "its quote occurs more than once in {file} and its line does not identify one occurrence"
        )
    };
    match exact.as_slice() {
        [exact] => return Ok(site(**exact)),
        [first, further @ ..] => {
            return Err(
                if further
                    .iter()
                    .any(|other| other.position.side != first.position.side)
                {
                    format!("its quote starts at line {line} on both sides of the diff of {file}")
                } else {
                    ambiguous()
                },
            );
        }
        [] => {}
    }
    if found.context.contains(&line) {
        return Err(unchanged_quote(file));
    }
    match found.changed.as_slice() {
        [] => Err(found.no_changed_quote(file)),
        [only] => Ok(site(*only)),
        _ => Err(ambiguous()),
    }
}

/// The file whose diff holds the quote at its stored site, or anywhere on a
/// changed line when the store has no verified position.
pub fn quote_file<'a>(
    files: &'a [DiffFile],
    file: &str,
    code: &str,
    expected: Option<&FilePosition<String>>,
) -> Result<&'a str, String> {
    let found = quote_matches(files, file, code)?;
    if found.changed.is_empty() {
        return Err(found.no_changed_quote(file));
    }
    if let Some(expected) = expected {
        let site = FilePosition {
            path: expected.path.as_str(),
            position: expected.position,
        };
        if !found.changed.contains(&site) {
            return Err(format!(
                "its code is not at the verified line {} in {}",
                site.position.line, site.path
            ));
        }
    }
    Ok(found.file)
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
    let Ok(patches) = named_patches(files, file) else {
        return false;
    };
    for named in patches {
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
