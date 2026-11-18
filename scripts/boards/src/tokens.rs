//! A design system's `tokens.css`, written from its `design/system/tokens.json`:
//! one custom property per token and one class per text style, laid out as the
//! claude.ai Design System page compiles it. The boards load it, and
//! `builds design tokens-css DIR` writes the file into a worktree for other
//! tools that load it there; both read the tokens through `Source`.
//!
//! The stylesheet, in order:
//!
//! - one block of color properties per theme in `color.themes`, the first
//!   theme's block on `:root` too. A color token's value is one value for
//!   every theme, or an object of one value per theme id;
//! - one `:root` block holding every token of the other families (`spacing`,
//!   `radius` and the like, in the file's order) and a `--font-<key>`
//!   property per font stack in `type.families`;
//! - one class per text style of `type.groups`, in the font family its group
//!   names unless the style names its own.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Context, Result, fail};

/// Where a worktree's tokens are, and where their stylesheet goes.
pub const TOKENS_JSON: &str = "design/system/tokens.json";
pub const TOKENS_CSS: &str = "design/system/tokens.css";

/// An object's entries in the order the file has them.
struct Ordered<V>(Vec<(String, V)>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Entries<V>(PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for Entries<V> {
            type Value = Ordered<V>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Entries(PhantomData))
    }
}

/// A value written into the stylesheet as it is: text, or a number.
#[derive(Deserialize)]
#[serde(untagged)]
enum Scalar {
    Text(String),
    Number(serde_json::Number),
}

impl fmt::Display for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Scalar::Text(text) => f.write_str(text),
            Scalar::Number(number) => write!(f, "{number}"),
        }
    }
}

#[derive(Deserialize)]
struct Tokens {
    name: String,
    color: Colors,
    #[serde(rename = "type")]
    text: Text,
    /// Every other entry: `version` and `meta`, and the other families.
    #[serde(flatten)]
    rest: Ordered<Value>,
}

/// Entries of tokens.json that hold no tokens.
const NOT_FAMILIES: [&str; 2] = ["version", "meta"];

#[derive(Deserialize)]
struct Colors {
    themes: Vec<ThemeId>,
    tokens: Vec<ColorToken>,
}

#[derive(Deserialize)]
struct ThemeId {
    id: String,
}

#[derive(Deserialize)]
struct ColorToken {
    name: String,
    value: ColorValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ColorValue {
    Every(Scalar),
    PerTheme(std::collections::BTreeMap<String, Scalar>),
}

#[derive(Deserialize)]
struct Family {
    tokens: Vec<Token>,
}

#[derive(Deserialize)]
struct Token {
    name: String,
    value: Scalar,
}

#[derive(Deserialize)]
struct Text {
    families: Ordered<String>,
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    family: String,
    styles: Vec<Style>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Style {
    name: String,
    family: Option<String>,
    font_size: Scalar,
    font_weight: Scalar,
    line_height: Option<Scalar>,
    letter_spacing: Option<Scalar>,
    /// The words the design system shows the style in.
    sample: Option<String>,
    usage: Option<String>,
}

impl Style {
    /// The style's CSS declarations, with <font_family> as its font-family's
    /// value.
    fn declarations(&self, font_family: String) -> Vec<String> {
        let mut declarations = vec![
            format!("font-family: {font_family}"),
            format!("font-size: {}", self.font_size),
            format!("font-weight: {}", self.font_weight),
        ];
        if let Some(height) = &self.line_height {
            declarations.push(format!("line-height: {height}"));
        }
        if let Some(spacing) = &self.letter_spacing {
            declarations.push(format!("letter-spacing: {spacing}"));
        }
        declarations
    }
}

impl ColorToken {
    /// The token's value in theme <id>.
    fn value_in(&self, id: &str) -> Result<&Scalar> {
        match &self.value {
            ColorValue::Every(value) => Ok(value),
            ColorValue::PerTheme(values) => match values.get(id) {
                Some(value) => Ok(value),
                None => fail!("color {} has no value for theme {id}", self.name),
            },
        }
    }
}

fn parse(json: &str) -> Result<Tokens> {
    serde_json::from_str(json).context(|| "reading the tokens".to_string())
}

/// The stylesheet of the tokens in <json>, the text of a tokens.json.
pub fn tokens_css(json: &str) -> Result<String> {
    let tokens = parse(json)?;
    let mut out = vec![format!(
        "/* {}: builds design tokens-css writes this file from tokens.json */",
        tokens.name
    )];
    for (at, theme) in tokens.color.themes.iter().enumerate() {
        let id = &theme.id;
        let mut block = vec![if at == 0 {
            format!(":root, [data-theme=\"{id}\"] {{")
        } else {
            format!("[data-theme=\"{id}\"] {{")
        }];
        for token in &tokens.color.tokens {
            block.push(format!("  --{}: {};", token.name, token.value_in(id)?));
        }
        block.push("}".to_string());
        out.push(block.join("\n"));
    }
    let mut root = vec![":root {".to_string()];
    for (name, body) in &tokens.rest.0 {
        if NOT_FAMILIES.contains(&name.as_str()) {
            continue;
        }
        let family =
            Family::deserialize(body).context(|| format!("reading the tokens of family {name}"))?;
        for token in &family.tokens {
            root.push(format!("  --{}: {};", token.name, token.value));
        }
    }
    for (key, stack) in &tokens.text.families.0 {
        root.push(format!("  --font-{key}: {stack};"));
    }
    root.push("}".to_string());
    out.push(root.join("\n"));
    for group in &tokens.text.groups {
        for style in &group.styles {
            let family = style.family.as_ref().unwrap_or(&group.family);
            out.push(format!(
                ".{} {{ {}; }}",
                style.name,
                style
                    .declarations(format!("var(--font-{family})"))
                    .join("; ")
            ));
        }
    }
    Ok(out.join("\n") + "\n")
}

/// What a page shows of a design system's tokens without its stylesheet:
/// each color in the first theme, and each text style as CSS declarations
/// that name its font stack, with the words to draw it in.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Specimen {
    /// Each color's value in the first theme.
    pub colors: Vec<Swatch>,
    pub styles: Vec<TextSpecimen>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Swatch {
    pub name: String,
    pub value: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct TextSpecimen {
    pub name: String,
    /// The style's declarations, as one `style` attribute's text.
    pub css: String,
    /// The style's sample, else its usage, else its name.
    pub words: String,
}

/// The specimen of the tokens in <json>, the text of a tokens.json. Fails
/// where `tokens_css` does, and on a text style whose family names no font
/// stack.
pub fn specimen(json: &str) -> Result<Specimen> {
    let tokens = parse(json)?;
    let Some(first) = tokens.color.themes.first() else {
        fail!("the tokens name no color theme");
    };
    let colors = tokens
        .color
        .tokens
        .iter()
        .map(|token| {
            Ok(Swatch {
                name: token.name.clone(),
                value: token.value_in(&first.id)?.to_string(),
            })
        })
        .collect::<Result<_>>()?;
    let mut styles = Vec::new();
    for group in &tokens.text.groups {
        for style in &group.styles {
            let family = style.family.as_ref().unwrap_or(&group.family);
            let Some((_, stack)) = tokens.text.families.0.iter().find(|(key, _)| key == family)
            else {
                fail!(
                    "text style {} is in the family {family}, which the tokens give no font stack",
                    style.name
                );
            };
            styles.push(TextSpecimen {
                name: style.name.clone(),
                css: style.declarations(stack.clone()).join("; "),
                words: style
                    .sample
                    .clone()
                    .or_else(|| style.usage.clone())
                    .unwrap_or_else(|| style.name.clone()),
            });
        }
    }
    Ok(Specimen { colors, styles })
}

#[cfg(test)]
#[path = "tokens_tests.rs"]
mod tests;
