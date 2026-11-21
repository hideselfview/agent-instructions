//! The `shots-declared <platform> <folder>` lines of a repo's `.app-build`,
//! which builds and builds-shots both read: the folder below which a
//! platform's shots are declared.

use crate::Error;

/// The folder a `shots-declared` line gives in <value>, the words after its
/// platform, or why the line is wrong.
pub fn declared_folder(value: &[String]) -> Result<&str, &'static str> {
    let [folder] = value else {
        return Err("gives more than one declare folder");
    };
    if folder.starts_with('/') || folder.split('/').any(|part| part == "..") {
        return Err("names a folder outside the repo");
    }
    Ok(folder)
}

/// The folder of each `shots-declared` line of <text>, an `.app-build` file,
/// in the order written, each without a trailing `/`. A line that
/// `declared_folder` refuses, or that names no platform or no folder, is an
/// error.
pub fn declared_folders(text: &str) -> Result<Vec<String>, Error> {
    let mut folders = Vec::new();
    for line in text.lines().map(str::trim) {
        let mut words = line.split_whitespace();
        if words.next() != Some("shots-declared") {
            continue;
        }
        let bad = |why: &str| Error::new(format!(".app-build: the line {line:?} {why}"));
        let rest: Vec<String> = words.map(str::to_string).collect();
        let [_platform, value @ ..] = rest.as_slice() else {
            return Err(bad("names no platform"));
        };
        if value.is_empty() {
            return Err(bad("names its platform and nothing else"));
        }
        let folder = declared_folder(value).map_err(bad)?;
        folders.push(folder.trim_end_matches('/').to_string());
    }
    Ok(folders)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
