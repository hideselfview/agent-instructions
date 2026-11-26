//! The `shots-declared <platform> <folder>` lines of a repo's `.app-build`,
//! which builds and its shots checker both read: the folder below which a
//! platform's shots are declared.

/// A platform a `shots-` line can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Platform {
    Mac,
    Android,
    Web,
}

impl Platform {
    /// Every platform.
    pub const ALL: [Platform; 3] = [Self::Mac, Self::Android, Self::Web];

    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "mac" => Some(Self::Mac),
            "android" => Some(Self::Android),
            "web" => Some(Self::Web),
            _ => None,
        }
    }

    /// The first segment of every name on this platform.
    pub fn word(self) -> &'static str {
        match self {
            Self::Mac => "mac",
            Self::Android => "android",
            Self::Web => "web",
        }
    }

    /// The prefix of this platform's component names in the design system
    /// (`MacRow`, `AndroidCastRow`).
    pub fn component_prefix(self) -> &'static str {
        match self {
            Self::Mac => "Mac",
            Self::Android => "Android",
            Self::Web => "Web",
        }
    }
}

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
/// `Platform`, no folder or more than one, a folder outside the repo, or a
/// platform that an earlier line declared.
pub fn declared(text: &str) -> Result<Vec<(Platform, String)>, LineError> {
    read_declarations(text, None)
}

/// The declare folder for <platform>, if the file names one. Every
/// declaration must name a known platform. Only the selected platform's
/// folders are read and checked.
pub fn declared_folder(text: &str, platform: Platform) -> Result<Option<String>, LineError> {
    Ok(read_declarations(text, Some(platform))?
        .into_iter()
        .next()
        .map(|(_, folder)| folder))
}

fn read_declarations(
    text: &str,
    only: Option<Platform>,
) -> Result<Vec<(Platform, String)>, LineError> {
    let mut declared: Vec<(Platform, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        let mut words = line.split_whitespace();
        if words.next() != Some("shots-declared") {
            continue;
        }
        let bad = |why| LineError {
            line: line.to_string(),
            why,
        };
        let Some(word) = words.next() else {
            return Err(bad("names no platform"));
        };
        let platform = Platform::parse(word)
            .ok_or_else(|| bad("names a platform other than mac, android or web"))?;
        if let Some(only) = only
            && platform != only
        {
            continue;
        }
        let value: Vec<&str> = words.collect();
        let folder = match value.as_slice() {
            [] => return Err(bad("names its platform and nothing else")),
            [folder] => *folder,
            _ => return Err(bad("gives more than one declare folder")),
        };
        if folder.starts_with('/') || folder.split('/').any(|part| part == "..") {
            return Err(bad("names a folder outside the repo"));
        }
        if declared.iter().any(|(p, _)| *p == platform) {
            return Err(bad("gives a platform a second declare folder"));
        }
        declared.push((platform, folder.trim_end_matches('/').to_string()));
    }
    Ok(declared)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
