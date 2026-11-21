//! What a design-system component's name says about the shots of its
//! preview rows. A component's shot is named
//! `<platform>.component.<thing>.<state>`: a component whose name starts with
//! a platform's word in PascalCase (`MacRow`) is that platform's, and one
//! whose name starts with none (`Artwork`) is every platform's. The thing is
//! the rest of the name in lower-case words joined by `-`, a run of capitals
//! being one word: `MacQRScanner` is mac's `qr-scanner`, `StatusChip` the
//! shared `status-chip`. The shots checker names each row's shot from it, and
//! builds finds the shots of a component by it.

use crate::app_build::Platform;
use crate::preview::is_component_name;

/// The platform that draws a component, none for a component every platform
/// shares, and the `<thing>` of its shots' names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentKey {
    /// The platform's word (`mac`), as `Platform::word` gives it.
    pub platform: Option<&'static str>,
    pub thing: String,
}

impl ComponentKey {
    /// The key of the component <name>; none when <name> is no component's
    /// name (a capital, then letters and digits).
    pub fn of(name: &str) -> Option<Self> {
        if !is_component_name(name) || !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return None;
        }
        let platform = Platform::ALL.into_iter().find(|platform| {
            name.strip_prefix(platform.component_prefix())
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
        });
        let rest = match platform {
            Some(platform) => &name[platform.component_prefix().len()..],
            None => name,
        };
        Some(Self {
            platform: platform.map(Platform::word),
            thing: words(rest).join("-"),
        })
    }

    /// The start of the names of this component's shots on the platform
    /// whose word is <platform>: `mac.component.row.` for `MacRow` on mac.
    pub fn prefix(&self, platform: &str) -> String {
        format!("{platform}.component.{}.", self.thing)
    }
}

/// The words of a PascalCase name, in lower case. A capital starts a word
/// after a small letter or a digit, and after a capital when a small letter
/// follows it, so a run of capitals is one word: `QRScanner` is `qr`,
/// `scanner`; `S3KeyFields` is `s3`, `key`, `fields`.
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let starts = i > 0
            && c.is_ascii_uppercase()
            && (chars[i - 1].is_ascii_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (chars[i - 1].is_ascii_uppercase()
                    && chars.get(i + 1).is_some_and(char::is_ascii_lowercase)));
        if starts {
            words.push(std::mem::take(&mut word));
        }
        word.push(c.to_ascii_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
#[path = "component_key_tests.rs"]
mod tests;
