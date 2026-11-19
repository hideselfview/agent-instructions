//! Board and shot names: `<platform>.<area>.<screen>[.<state>]` for a screen
//! and `<platform>.component.<thing>.<state>` for a design-system component's
//! state. A component both platforms share (`Artwork`) has the same form, with
//! the platform of the shot it draws. Each segment is lower-case words joined by `-`.

use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum NameError {
    #[error("'{0}' is not a board name: {1}")]
    Invalid(String, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Platform {
    Mac,
    Android,
    Web,
}

impl Platform {
    pub(crate) fn parse(word: &str) -> Option<Self> {
        match word {
            "mac" => Some(Self::Mac),
            "android" => Some(Self::Android),
            "web" => Some(Self::Web),
            _ => None,
        }
    }

    /// The first segment of every name on this platform.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Mac => "mac",
            Self::Android => "android",
            Self::Web => "web",
        }
    }

    /// The platform's word in a shot's file name (`<name>@macos.png`).
    pub(crate) fn file_word(self) -> &'static str {
        match self {
            Self::Mac => "macos",
            Self::Android => "android",
            Self::Web => "web",
        }
    }

    /// The platform whose word in a shot's file name is `word`.
    pub(crate) fn from_file_word(word: &str) -> Option<Self> {
        [Self::Mac, Self::Android, Self::Web]
            .into_iter()
            .find(|platform| platform.file_word() == word)
    }

    /// The prefix of this platform's component names in the design system
    /// (`MacRow`, `AndroidCastRow`).
    fn component_prefix(self) -> &'static str {
        match self {
            Self::Mac => "Mac",
            Self::Android => "Android",
            Self::Web => "Web",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Name {
    Screen(ScreenName),
    Component(ComponentName),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ScreenName {
    platform: Platform,
    area: String,
    screen: String,
    state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ComponentName {
    platform: Platform,
    thing: String,
    state: String,
}

impl Name {
    pub(crate) fn parse(text: &str) -> Result<Self, NameError> {
        let invalid = |why| NameError::Invalid(text.to_string(), why);
        let segments: Vec<&str> = text.split('.').collect();
        if !segments.iter().all(|s| is_segment(s)) {
            return Err(invalid("every segment is lower-case words joined by '-'"));
        }
        let platform = Platform::parse(segments[0])
            .ok_or_else(|| invalid("it starts with no platform (mac, android, web)"))?;
        match segments[1..] {
            ["component", thing, state] => Ok(Self::Component(ComponentName {
                platform,
                thing: thing.to_string(),
                state: state.to_string(),
            })),
            ["component", ..] => Err(invalid("a component is component.<thing>.<state>")),
            [area, screen] => Ok(Self::Screen(ScreenName {
                platform,
                area: area.to_string(),
                screen: screen.to_string(),
                state: None,
            })),
            [area, screen, state] => Ok(Self::Screen(ScreenName {
                platform,
                area: area.to_string(),
                screen: screen.to_string(),
                state: Some(state.to_string()),
            })),
            _ => Err(invalid("a screen is <area>.<screen>[.<state>]")),
        }
    }
}

impl Name {
    pub(crate) fn platform(&self) -> Platform {
        match self {
            Self::Screen(s) => s.platform,
            Self::Component(c) => c.platform,
        }
    }

    /// The file a shot writes with `extension` (`png` or `json`):
    /// `<name>@<platform>.<extension>`.
    pub(crate) fn file_name(&self, extension: &str) -> String {
        format!("{self}@{}.{extension}", self.platform().file_word())
    }

    /// The shot a file in a capture directory belongs to, when its name has the
    /// form of a shot file of `platform`.
    pub(crate) fn from_file_name(
        platform: Platform,
        file_name: &str,
    ) -> Option<Result<Self, NameError>> {
        let suffix_png = format!("@{}.png", platform.file_word());
        let suffix_json = format!("@{}.json", platform.file_word());
        let stem = file_name
            .strip_suffix(&suffix_png)
            .or_else(|| file_name.strip_suffix(&suffix_json))?;
        Some(Self::parse(stem))
    }
}

/// The start of board names a run is limited to: `mac.welcome.` is the Mac
/// welcome boards, `mac.component.row.` one component's states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prefix {
    platform: Platform,
    text: String,
}

impl Prefix {
    pub(crate) fn parse(text: &str) -> Result<Self, NameError> {
        let invalid = |why| NameError::Invalid(text.to_string(), why);
        let (platform, rest) = text
            .split_once('.')
            .ok_or_else(|| invalid("a prefix is a platform and a dot at least"))?;
        let platform = Platform::parse(platform)
            .ok_or_else(|| invalid("it starts with no platform (mac, android, web)"))?;
        if !rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        {
            return Err(invalid(
                "a prefix is lower-case words joined by '-' and '.'",
            ));
        }
        Ok(Self {
            platform,
            text: text.to_string(),
        })
    }

    pub(crate) fn platform(&self) -> Platform {
        self.platform
    }

    pub(crate) fn matches(&self, name: &Name) -> bool {
        name.to_string().starts_with(&self.text)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }
}

impl ComponentName {
    /// The design-system component the shot draws, as a folder of the repo at
    /// <root>. `cast-row` on Android is `AndroidCastRow`. A thing with no
    /// preview under its platform's name but one under the bare name, such as
    /// `artwork` (`Artwork`), is a component both platforms share; the shot's
    /// platform stays in its `@<platform>` file suffix. A thing with neither
    /// preview names the platform's component, so a missing preview is
    /// reported under that name.
    pub(crate) fn component(&self, root: &std::path::Path) -> Result<String, String> {
        let shared = self.shared_component();
        let platform = format!("{}{shared}", self.platform.component_prefix());
        let has_preview = |name: &str| {
            let path = root.join(boards::preview::preview_path(name));
            match std::fs::metadata(&path) {
                Ok(found) => Ok(found.is_file()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(format!("cannot read {}: {e}", path.display())),
            }
        };
        Ok(if !has_preview(&platform)? && has_preview(&shared)? {
            shared
        } else {
            platform
        })
    }

    /// The thing in PascalCase, the name of a component both platforms share.
    fn shared_component(&self) -> String {
        self.thing
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                    .unwrap_or_default()
            })
            .collect()
    }

    pub(crate) fn state(&self) -> &str {
        &self.state
    }
}

fn is_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Screen(s) => {
                write!(f, "{}.{}.{}", s.platform.word(), s.area, s.screen)?;
                match &s.state {
                    Some(state) => write!(f, ".{state}"),
                    None => Ok(()),
                }
            }
            Self::Component(c) => {
                write!(f, "{}.component.{}.{}", c.platform.word(), c.thing, c.state)
            }
        }
    }
}

/// The id of a preview row's state: its name in lower-case words joined by
/// `-` (`Nested, off` is `nested-off`).
pub(crate) fn state_id(row_name: &str) -> String {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in row_name.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c.to_ascii_lowercase());
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words.join("-")
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
