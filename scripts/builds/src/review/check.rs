//! builds' own check of a finding's two quotes: the clause must be in the
//! rule's text, and the code must be in the diff, in the file the finding
//! names. A finding that fails either is dropped and recorded as dropped.

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
/// and its lines, without the diff's `+`, `-` and space marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub lines: Vec<String>,
}

pub fn parse_diff(diff: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if !in_hunk || line.starts_with("diff --git ") {
            if let Some(rest) = line.strip_prefix("diff --git ") {
                // `a/<old> b/<new>`: the new path follows the last ` b/`.
                let path = rest.rsplit_once(" b/").map(|(_, new)| new).unwrap_or(rest);
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
        if let Some(file) = files.last_mut()
            && let Some('+' | '-' | ' ') = line.chars().next()
        {
            file.lines.push(line[1..].to_string());
        }
    }
    files
}

/// Whether the path <path> is the file <file> a finding names: the whole
/// path, or its end when the reviewer named a shorter one.
pub fn names_file(path: &str, file: &str) -> bool {
    path == file || path.ends_with(&format!("/{file}"))
}

/// Whether <code> is in the diff's lines of <file>, matched by `names_file`.
pub fn code_in(files: &[DiffFile], file: &str, code: &str) -> Result<(), String> {
    let wanted = squash(code);
    if wanted.is_empty() {
        return Err("it quotes no code".to_string());
    }
    let named: Vec<&DiffFile> = files.iter().filter(|f| names_file(&f.path, file)).collect();
    if named.is_empty() {
        return Err(format!("the diff has no file {file}"));
    }
    if named
        .iter()
        .any(|f| squash(&f.lines.join("\n")).contains(&wanted))
    {
        Ok(())
    } else {
        Err(format!("its code is not in the diff of {file}"))
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
