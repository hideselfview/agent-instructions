//! Board and shot names: `<platform>.<area>.<screen>[.<state>]` for a screen
//! and `<platform>.component.<thing>.<state>` for a design-system component's
//! state. A component both platforms share (`Artwork`) has the same form, with
//! the platform of the shot it draws. Each segment is lower-case words joined by `-`.

use boards::component_key::ComponentKey;
use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("'{0}' is not a board name: {1}")]
    Invalid(String, &'static str),
}

pub(crate) use boards::app_build::Platform;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Name {
    Screen(ScreenName),
    Component(ComponentName),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenName {
    platform: Platform,
    area: String,
    screen: String,
    state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComponentName {
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
pub struct Prefix {
    platform: Platform,
    text: String,
}

impl Prefix {
    pub fn parse(text: &str) -> Result<Self, NameError> {
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

    pub(crate) fn all_of(platform: Platform) -> Self {
        Self {
            platform,
            text: format!("{}.", platform.word()),
        }
    }

    pub(crate) fn platform(&self) -> Platform {
        self.platform
    }

    pub(crate) fn matches(&self, name: &Name) -> bool {
        name.to_string().starts_with(&self.text)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl ComponentName {
    /// The design-system component the shot draws, among <previews>: the one
    /// whose key (`ComponentKey::of`) is the shot's platform and thing, so
    /// `cast-row` on Android is `AndroidCastRow` and `qr-scanner` on mac is
    /// `MacQRScanner`. A thing with no preview under its platform but one with
    /// no platform, such as `artwork` (`Artwork`), is a component both
    /// platforms share; the shot's platform stays in its `@<platform>` file
    /// suffix. A thing with neither preview names the platform's component, so
    /// a missing preview is reported under that name.
    pub(crate) fn component(&self, previews: &Previews) -> String {
        let find = |platform: Option<Platform>| {
            previews
                .components
                .iter()
                .find(|(_, key)| {
                    key.platform == platform.map(Platform::word) && key.thing == self.thing
                })
                .map(|(name, _)| name.clone())
        };
        find(Some(self.platform))
            .or_else(|| find(None))
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    self.platform.component_prefix(),
                    pascal_case(&self.thing)
                )
            })
    }

    pub(crate) fn state(&self) -> &str {
        &self.state
    }
}

/// The name of the shot of the preview row <state> of the component <key>
/// names (`boards::component_key`) on <platform>.
pub(crate) fn component_shot(
    key: &ComponentKey,
    platform: Platform,
    state: &str,
) -> Result<Name, NameError> {
    Name::parse(&format!("{}{state}", key.prefix(platform.word())))
}

/// The design-system components of a repo that have a preview: each folder
/// `boards::preview::preview_folders` names.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Previews {
    /// The folders whose name is a component's name, by name, each with its
    /// key.
    pub(crate) components: Vec<(String, ComponentKey)>,
    /// The folders whose name is no component's name, by name, which no shot
    /// can draw.
    pub(crate) unnamed: Vec<String>,
}

impl Previews {
    /// The previews of the repo at <root>; none when it has no components
    /// folder.
    pub(crate) fn of(root: &std::path::Path) -> Result<Self, String> {
        let mut previews = Self::default();
        for name in boards::preview::preview_folders(root).map_err(|e| e.to_string())? {
            match ComponentKey::of(&name) {
                Some(key) => previews.components.push((name, key)),
                None => previews.unnamed.push(name),
            }
        }
        Ok(previews)
    }
}

/// <thing>, lower-case words joined by `-`, in PascalCase: the name a
/// component of that thing has when no word of it is a run of capitals.
fn pascal_case(thing: &str) -> String {
    thing
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
