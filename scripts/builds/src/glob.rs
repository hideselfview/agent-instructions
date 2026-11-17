//! Matching `/`-separated globs against repo-relative paths, for the commit
//! checks' paths (checks.rs), the design globs (lists/design_paths.rs) and the
//! rules' paths (review/rules.rs). A
//! segment that is `**` alone matches any number of path segments, none
//! included; `*` matches any characters inside one segment. Whether `?`
//! matches one character or only itself is the caller's: the design globs
//! and the rules' paths read it as a wildcard, and the commit checks' paths
//! as a plain character.

/// What `?` means in a glob's segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    /// `?` matches any one character.
    Wildcard,
    /// `?` matches only a `?`.
    Plain,
}

/// Does <path>, a path's segments, match <pattern>, a glob's segments, where
/// a `**` segment matches any number of segments?
pub fn segments_match<P: AsRef<str>>(pattern: &[P], path: &[&str], question: Question) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first.as_ref() == "**" => {
            (0..=path.len()).any(|skip| segments_match(rest, &path[skip..], question))
        }
        Some((first, rest)) => path.split_first().is_some_and(|(name, others)| {
            segment_matches(first.as_ref(), name, question)
                && segments_match(rest, others, question)
        }),
    }
}

/// Does the path segment <name> match the glob segment <pattern>, where `*`
/// matches any characters and <question> says what `?` matches?
pub fn segment_matches(pattern: &str, name: &str, question: Question) -> bool {
    fn go(pattern: &[char], name: &[char], question: Question) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len()).any(|skip| go(rest, &name[skip..], question)),
            Some(('?', rest)) if question == Question::Wildcard => {
                !name.is_empty() && go(rest, &name[1..], question)
            }
            Some((c, rest)) => name.first() == Some(c) && go(rest, &name[1..], question),
        }
    }
    go(
        &pattern.chars().collect::<Vec<_>>(),
        &name.chars().collect::<Vec<_>>(),
        question,
    )
}

#[cfg(test)]
#[path = "glob_tests.rs"]
mod tests;
