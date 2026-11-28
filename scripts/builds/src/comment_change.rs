//! Which changes to a source file are only in its comments. A change is
//! comment-only when the file's text before and after differ, and are the
//! same once the lexer of its language removes every comment (comments.rs),
//! and the change neither adds nor removes the file. A file of a language
//! with no lexer changes code.

use std::path::Path;

use crate::comments::{Language, without_comments};
use crate::fail::Result;

/// Whether the change of a file of <language> from <before> to <after>, each
/// None when the file is absent, is only in comments. Two equal texts are no
/// change at all.
pub fn only_in_comments(language: Language, before: Option<&str>, after: Option<&str>) -> bool {
    let (Some(before), Some(after)) = (before, after) else {
        return false;
    };
    before != after && without_comments(language, before) == without_comments(language, after)
}

/// The paths of <paths> whose change from the commit or tree <from> to <to>
/// in the repository at <top> is only in comments (`only_in_comments`). A
/// path of a language with no lexer changes code.
pub fn paths_only_in_comments(
    top: &Path,
    from: &str,
    to: &str,
    paths: &[String],
) -> Result<Vec<String>> {
    let mut only = Vec::new();
    for path in paths {
        let Some(language) = Language::of(path) else {
            continue;
        };
        let before = crate::git::file_text(top, from, path)?;
        let after = crate::git::file_text(top, to, path)?;
        if only_in_comments(language, before.as_deref(), after.as_deref()) {
            only.push(path.clone());
        }
    }
    Ok(only)
}

#[cfg(test)]
#[path = "comment_change_tests.rs"]
mod tests;
