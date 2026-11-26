//! The `app <name> <platform>` and `shots-declared <app> <folder>` lines of a
//! repo's `.app-build`, which builds and its shots checker both read: the
//! apps and the platform of each, and the folder below which an app's shots
//! are declared.
//!
//! An app is what one lane builds. Its name is the first segment of every
//! shot and board name of the app, and its platform picks how the shots
//! checker declares, takes and reads its shots. An app whose name is a
//! platform's word (`mac`, `android`, `web`) runs on that platform and needs
//! no `app` line.

/// A platform an app runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// An app: the platform it runs on and its name. Apps order by platform, then
/// by name, so that the apps named for platforms keep the platforms' order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct App {
    pub platform: Platform,
    pub name: String,
}

impl App {
    /// The app that is named for <platform> and runs on it.
    pub fn named_for(platform: Platform) -> Self {
        Self {
            name: platform.word().to_string(),
            platform,
        }
    }
}

/// Whether <name> is an app's name: lower-case words joined by `-`.
pub fn is_app_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// The apps a repo's `.app-build` names with `app` lines. An app named for a
/// platform needs no line, so `get` finds it with none.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Apps {
    named: Vec<App>,
}

impl Apps {
    /// The apps of the `app` lines of <text>, an `.app-build` file. A line is
    /// wrong when it names no app or no platform, a platform other than
    /// `Platform`, more words, an app that an earlier line named, or a
    /// platform's word as the name of an app on another platform.
    pub fn read(text: &str) -> Result<Self, LineError> {
        let mut apps = Self::default();
        for line in text.lines().map(str::trim) {
            let mut words = line.split_whitespace();
            if words.next() != Some("app") {
                continue;
            }
            let bad = |why| LineError {
                line: line.to_string(),
                why,
            };
            let Some(name) = words.next() else {
                return Err(bad("names no app"));
            };
            if !is_app_name(name) {
                return Err(bad(
                    "names an app that is not lower-case words joined by '-'",
                ));
            }
            let Some(platform) = words.next() else {
                return Err(bad("names its app and nothing else"));
            };
            let platform = Platform::parse(platform)
                .ok_or_else(|| bad("names a platform other than mac, android or web"))?;
            if words.next().is_some() {
                return Err(bad("gives more than an app and its platform"));
            }
            if Platform::parse(name).is_some_and(|word| word != platform) {
                return Err(bad(
                    "names a platform's word as the name of an app on another platform",
                ));
            }
            if apps.named.iter().any(|app| app.name == name) {
                return Err(bad("names an app that an earlier line named"));
            }
            apps.named.push(App {
                name: name.to_string(),
                platform,
            });
        }
        Ok(apps)
    }

    /// The apps that `app` lines name, in the order written.
    pub fn named(&self) -> impl Iterator<Item = App> + '_ {
        self.named.iter().cloned()
    }

    /// The app called <name>: the one an `app` line names, else the one
    /// named for the platform whose word <name> is.
    pub fn get(&self, name: &str) -> Option<App> {
        self.named
            .iter()
            .find(|app| app.name == name)
            .cloned()
            .or_else(|| Platform::parse(name).map(App::named_for))
    }
}

/// A line that is wrong, and why.
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

/// The app and folder of each `shots-declared` line of <text>, an
/// `.app-build` file, in the order written, each folder without a trailing
/// `/`. A line is wrong when it names no app, an app that no `app` line and
/// no platform's word names, no folder or more than one, a folder outside the
/// repo, or an app that an earlier line declared.
pub fn declared(text: &str) -> Result<Vec<(App, String)>, LineError> {
    declared_in(text, &Apps::read(text)?)
}

/// `declared`, for a caller that has read the apps of <text> already.
pub fn declared_in(text: &str, apps: &Apps) -> Result<Vec<(App, String)>, LineError> {
    read_declarations(text, apps, None)
}

/// The declare folder for the app called <app>, if the file names one. Every
/// declaration must name a known app. Only the selected app's folders are
/// read and checked.
pub fn declared_folder(text: &str, app: &str) -> Result<Option<String>, LineError> {
    Ok(read_declarations(text, &Apps::read(text)?, Some(app))?
        .into_iter()
        .next()
        .map(|(_, folder)| folder))
}

fn read_declarations(
    text: &str,
    apps: &Apps,
    only: Option<&str>,
) -> Result<Vec<(App, String)>, LineError> {
    let mut declared: Vec<(App, String)> = Vec::new();
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
            return Err(bad("names no app"));
        };
        let app = apps
            .get(word)
            .ok_or_else(|| bad("names no app (an app line or a platform's word gives one)"))?;
        if let Some(only) = only
            && app.name != only
        {
            continue;
        }
        let value: Vec<&str> = words.collect();
        let folder = match value.as_slice() {
            [] => return Err(bad("names its app and nothing else")),
            [folder] => *folder,
            _ => return Err(bad("gives more than one declare folder")),
        };
        if folder.starts_with('/') || folder.split('/').any(|part| part == "..") {
            return Err(bad("names a folder outside the repo"));
        }
        if declared.iter().any(|(a, _)| *a == app) {
            return Err(bad("gives an app a second declare folder"));
        }
        declared.push((app, folder.trim_end_matches('/').to_string()));
    }
    Ok(declared)
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
