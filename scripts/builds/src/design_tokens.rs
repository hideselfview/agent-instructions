//! A design system's `tokens.css`, written from its `design/system/tokens.json`:
//! one custom property per token and one class per text style, laid out as the
//! claude.ai Design System page compiles it. builds draws boards with it, and
//! `builds design tokens-css DIR` writes the file into a worktree for other
//! tools that load it there (a repo's screenshot checks, the design system's
//! preview pages); both read the tokens through `design_render::Source`.
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

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::Value;

use crate::fail;
use crate::fail::{Context, Result};

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
}

/// The stylesheet of the tokens in <json>, the text of a tokens.json.
pub fn tokens_css(json: &str) -> Result<String> {
    let tokens: Tokens = serde_json::from_str(json).context(|| "reading the tokens".to_string())?;
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
            let value = match &token.value {
                ColorValue::Every(value) => value,
                ColorValue::PerTheme(values) => match values.get(id) {
                    Some(value) => value,
                    None => fail!("color {} has no value for theme {id}", token.name),
                },
            };
            block.push(format!("  --{}: {value};", token.name));
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
            let mut declarations = vec![
                format!("font-family: var(--font-{family})"),
                format!("font-size: {}", style.font_size),
                format!("font-weight: {}", style.font_weight),
            ];
            if let Some(height) = &style.line_height {
                declarations.push(format!("line-height: {height}"));
            }
            if let Some(spacing) = &style.letter_spacing {
                declarations.push(format!("letter-spacing: {spacing}"));
            }
            out.push(format!(
                ".{} {{ {}; }}",
                style.name,
                declarations.join("; ")
            ));
        }
    }
    Ok(out.join("\n") + "\n")
}

#[cfg(test)]
#[path = "design_tokens_tests.rs"]
mod tests;
