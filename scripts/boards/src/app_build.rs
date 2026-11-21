//! The `shots-declared <platform> <folder>` lines of a repo's `.app-build`,
//! which builds and builds-shots both read: the folder below which a
//! platform's shots are declared.

/// The platforms a `shots-` line can name.
pub const PLATFORMS: [&str; 3] = ["mac", "android", "web"];

/// A `shots-declared` line that is wrong, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    pub line: String,
    pub why: &'static str,
}

impl LineError {
    /// The sentence that names the file, the line and what is wrong.
    pub fn text(&self) -> String {
        format!(".app-build: the line {:?} {}", self.line, self.why)
    }
}

/// The platform and folder of each `shots-declared` line of <text>, an
/// `.app-build` file, in the order written, each folder without a trailing
/// `/`. A line is wrong when it names no platform, a platform other than
/// `PLATFORMS`, no folder or more than one, a folder outside the repo, or a
/// platform that an earlier line declared.
pub fn declared(text: &str) -> Result<Vec<(String, String)>, LineError> {
    let mut declared: Vec<(String, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        let mut words = line.split_whitespace();
        if words.next() != Some("shots-declared") {
            continue;
        }
        let bad = |why| LineError {
            line: line.to_string(),
            why,
        };
        let rest: Vec<&str> = words.collect();
        let [platform, value @ ..] = rest.as_slice() else {
            return Err(bad("names no platform"));
        };
        if !PLATFORMS.contains(platform) {
            return Err(bad("names a platform other than mac, android or web"));
        }
        let folder = match value {
            [] => return Err(bad("names its platform and nothing else")),
            [folder] => *folder,
            _ => return Err(bad("gives more than one declare folder")),
        };
        if folder.starts_with('/') || folder.split('/').any(|part| part == "..") {
            return Err(bad("names a folder outside the repo"));
        }
        if declared.iter().any(|(p, _)| p == platform) {
            return Err(bad("gives a platform a second declare folder"));
        }
        declared.push((
            platform.to_string(),
            folder.trim_end_matches('/').to_string(),
        ));
    }
    Ok(declared)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
