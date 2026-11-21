//! What a capture wrote for one shot: the PNG and the element list beside it,
//! `<board>@<platform>.json`, in the one shape both harnesses write. Sizes and
//! boxes are in points (Mac) or dp (Android) from the shot's top-left, and
//! `scale` is PNG pixels per point or dp. A key the platform's tree has no
//! answer for is `null` or, for `text`, `placeholder` and Android's `focused`,
//! left out; the types keep it absent.

use crate::names::{Name, Platform};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ShotError {
    #[error("cannot read {0}: {1}")]
    Read(String, std::io::Error),
    #[error("{0}: {1}")]
    List(String, serde_json::Error),
    #[error("{0}: {1}")]
    Picture(String, image::ImageError),
}

/// One shot: its element list and its picture.
#[derive(Debug)]
pub(crate) struct Shot {
    pub(crate) list: ElementList,
    pub(crate) picture: Picture,
}

impl Shot {
    /// Reads the files `name` wrote into `dir`.
    pub(crate) fn load(dir: &Path, name: &Name) -> Result<Self, ShotError> {
        let json = name.file_name("json");
        let png = name.file_name("png");
        let text = std::fs::read_to_string(dir.join(&json))
            .map_err(|e| ShotError::Read(json.clone(), e))?;
        let bytes = std::fs::read(dir.join(&png)).map_err(|e| ShotError::Read(png.clone(), e))?;
        let list = ElementList::parse(&text).map_err(|e| ShotError::List(json, e))?;
        let picture = Picture::decode(&bytes).map_err(|e| ShotError::Picture(png, e))?;
        Ok(Self { list, picture })
    }

    /// What is inconsistent in the shot named `name`: the list names another
    /// board or platform, the picture is not the list's size at its scale, an
    /// element's parent does not come before it.
    pub(crate) fn problems(&self, name: &Name) -> Vec<String> {
        let mut problems = Vec::new();
        let list = &self.list;
        if list.board != name.to_string() {
            problems.push(format!("its element list names the board {}", list.board));
        }
        if list.platform != name.platform() {
            problems.push("its element list names another platform".to_string());
        }
        let wide = (list.size.width * list.scale).round();
        let high = (list.size.height * list.scale).round();
        if (wide - f64::from(self.picture.width)).abs() > 1.0
            || (high - f64::from(self.picture.height)).abs() > 1.0
        {
            problems.push(format!(
                "its picture is {}x{} pixels, not the list's {wide}x{high}",
                self.picture.width, self.picture.height
            ));
        }
        if self.picture.pixels.len()
            != self.picture.width as usize * self.picture.height as usize * 4
        {
            problems.push("its picture holds the wrong number of pixels".to_string());
        }
        for (index, element) in list.elements.iter().enumerate() {
            if element.parent.is_some_and(|parent| parent >= index) {
                problems.push(format!(
                    "element {index} has a parent that does not come before it"
                ));
            }
        }
        problems
    }
}

/// A decoded PNG: 8-bit RGBA, row by row from the top-left.
#[derive(Debug)]
pub(crate) struct Picture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

impl Picture {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, image::ImageError> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.to_rgba8();
        Ok(Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        })
    }
}

/// The element list a shot or a board writes beside its picture; the checker
/// writes a board's and reads both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ElementList {
    pub(crate) board: String,
    #[serde(serialize_with = "platform_word", deserialize_with = "platform")]
    pub(crate) platform: Platform,
    pub(crate) size: Size,
    pub(crate) scale: f64,
    pub(crate) elements: Vec<Element>,
    /// Areas of the picture the other platform's picture has no counterpart
    /// for: a board's on-screen keyboard. Only boards write it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unshot: Vec<Bounds>,
}

fn platform_word<S: serde::Serializer>(
    platform: &Platform,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(platform.file_word())
}

fn platform<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Platform, D::Error> {
    let word = String::deserialize(deserializer)?;
    Platform::from_file_word(&word).ok_or_else(|| {
        serde::de::Error::custom(format!("'{word}' is no platform (macos, android, web)"))
    })
}

impl ElementList {
    /// Reads the element list file at `path`.
    pub(crate) fn load(path: &Path) -> Result<Self, ShotError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ShotError::Read(path.display().to_string(), e))?;
        Self::parse(&text).map_err(|e| ShotError::List(path.display().to_string(), e))
    }

    pub(crate) fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Size {
    pub(crate) width: f64,
    pub(crate) height: f64,
}

// A shot holds every key the harnesses write, whether or not a check reads it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bounds {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Text,
    Button,
    Toggle,
    Radio,
    Field,
    Popup,
    Tab,
    Link,
    Image,
    Slider,
    Progress,
    Other,
}

impl Role {
    /// Roles whose name is their words, which the text inside them repeats.
    pub(crate) fn is_control(self) -> bool {
        !matches!(self, Self::Text | Self::Image | Self::Other)
    }

    /// The word a sentence uses for the role.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Button => "button",
            Self::Toggle => "toggle",
            Self::Radio => "radio button",
            Self::Field => "field",
            Self::Popup => "pop-up",
            Self::Tab => "tab",
            Self::Link => "link",
            Self::Image => "image",
            Self::Slider => "slider",
            Self::Progress => "progress bar",
            Self::Other => "element",
        }
    }
}

/// The look of a text element's words, each part only where the platform gave
/// it; `color` is sRGB `#RRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size: Option<f64>,
    /// 100 to 900.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) weight: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
}

/// One element of a shot's accessibility tree, in the tree's order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Element {
    pub(crate) role: Role,
    /// The platform's own role string, as read.
    pub(crate) native: String,
    pub(crate) label: Option<String>,
    /// A field's text, `on` or `off` for a toggle, a pop-up's choice.
    pub(crate) value: Option<String>,
    /// The words a field shows while empty; absent when the platform gives none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) placeholder: Option<String>,
    pub(crate) bounds: Bounds,
    pub(crate) enabled: bool,
    pub(crate) selected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<TextStyle>,
    /// The index in the list of the element that holds this one.
    pub(crate) parent: Option<usize>,
    /// On a board, the design-system component that draws the element: the
    /// nearest component above it in React's tree, by its name in the design
    /// system's global (`MacIconButton` of `Bae.MacIconButton`). Absent for
    /// an element no component draws, and in every shot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) component: Option<String>,
}

/// Words: composed Unicode, whitespace runs collapsed to one space, trimmed.
/// Both sides of every comparison read an element's label through this.
pub(crate) fn words(text: &str) -> String {
    text.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "shots_tests.rs"]
pub(crate) mod tests;
