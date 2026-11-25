//! The web fonts the boards load from Google Fonts: Roboto for the Android
//! boards and previews, and Inter and JetBrains Mono for the dashboard boards.
//! They link `https://fonts.googleapis.com/css2?family=…`, either in a page or
//! in an `@import` of a stylesheet.
//!
//! Google's answer to one link varies from request to request, as a variable
//! `woff2` in one and a static font under `fonts.gstatic.com/l/font?kit=` in
//! another, and a different file draws text edges a level apart. So the
//! server that serves the boards points each of those links at a pinned copy
//! of the family's variable font, kept in `canvas/web-fonts/`, and answers it
//! with the stylesheet and files below. A link to a family or an axis it holds
//! no copy of fails the page that serves it.

/// Where the server answers what a Google Fonts link asked for.
pub(crate) const URL_PREFIX: &str = "/web-fonts/";

const GOOGLE_STYLESHEET: &str = "https://fonts.googleapis.com/css2?";

/// The families, by the name a link gives (`JetBrains+Mono` is `JetBrains Mono`),
/// each with its stylesheet: one face for each subset, holding every weight.
const FAMILIES: [(&str, &str); 3] = [
    ("Roboto", include_str!("../../canvas/web-fonts/roboto.css")),
    ("Inter", include_str!("../../canvas/web-fonts/inter.css")),
    (
        "JetBrains Mono",
        include_str!("../../canvas/web-fonts/jetbrains-mono.css"),
    ),
];

/// The pinned files, by `<family>/<subset>`.
const FILES: [(&str, &[u8]); 22] = [
    (
        "roboto/cyrillic-ext",
        include_bytes!("../../canvas/web-fonts/roboto/cyrillic-ext.woff2"),
    ),
    (
        "roboto/cyrillic",
        include_bytes!("../../canvas/web-fonts/roboto/cyrillic.woff2"),
    ),
    (
        "roboto/greek-ext",
        include_bytes!("../../canvas/web-fonts/roboto/greek-ext.woff2"),
    ),
    (
        "roboto/greek",
        include_bytes!("../../canvas/web-fonts/roboto/greek.woff2"),
    ),
    (
        "roboto/math",
        include_bytes!("../../canvas/web-fonts/roboto/math.woff2"),
    ),
    (
        "roboto/symbols",
        include_bytes!("../../canvas/web-fonts/roboto/symbols.woff2"),
    ),
    (
        "roboto/vietnamese",
        include_bytes!("../../canvas/web-fonts/roboto/vietnamese.woff2"),
    ),
    (
        "roboto/latin-ext",
        include_bytes!("../../canvas/web-fonts/roboto/latin-ext.woff2"),
    ),
    (
        "roboto/latin",
        include_bytes!("../../canvas/web-fonts/roboto/latin.woff2"),
    ),
    (
        "inter/cyrillic-ext",
        include_bytes!("../../canvas/web-fonts/inter/cyrillic-ext.woff2"),
    ),
    (
        "inter/cyrillic",
        include_bytes!("../../canvas/web-fonts/inter/cyrillic.woff2"),
    ),
    (
        "inter/greek-ext",
        include_bytes!("../../canvas/web-fonts/inter/greek-ext.woff2"),
    ),
    (
        "inter/greek",
        include_bytes!("../../canvas/web-fonts/inter/greek.woff2"),
    ),
    (
        "inter/vietnamese",
        include_bytes!("../../canvas/web-fonts/inter/vietnamese.woff2"),
    ),
    (
        "inter/latin-ext",
        include_bytes!("../../canvas/web-fonts/inter/latin-ext.woff2"),
    ),
    (
        "inter/latin",
        include_bytes!("../../canvas/web-fonts/inter/latin.woff2"),
    ),
    (
        "jetbrains-mono/cyrillic-ext",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/cyrillic-ext.woff2"),
    ),
    (
        "jetbrains-mono/cyrillic",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/cyrillic.woff2"),
    ),
    (
        "jetbrains-mono/greek",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/greek.woff2"),
    ),
    (
        "jetbrains-mono/vietnamese",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/vietnamese.woff2"),
    ),
    (
        "jetbrains-mono/latin-ext",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/latin-ext.woff2"),
    ),
    (
        "jetbrains-mono/latin",
        include_bytes!("../../canvas/web-fonts/jetbrains-mono/latin.woff2"),
    ),
];

/// The stylesheet of the families a link asks for, or why the checker holds
/// no copy of what it asks for: <query> is what follows `css2?` in the link.
fn stylesheet(query: &str) -> Result<String, String> {
    let mut sheets = Vec::new();
    for family in query.split('&').filter_map(|p| p.strip_prefix("family=")) {
        let (name, axes) = match family.split_once(':') {
            Some((name, axes)) => (name, Some(axes)),
            None => (family, None),
        };
        let name = name.replace('+', " ");
        let Some((_, sheet)) = FAMILIES.iter().find(|(known, _)| *known == name) else {
            return Err(format!(
                "a page links the web font {name}, and the checker holds a copy of Roboto, Inter and JetBrains Mono only"
            ));
        };
        if let Some(axes) = axes
            && !axes
                .strip_prefix("wght@")
                .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit() || c == ';'))
        {
            return Err(format!(
                "a page links {name} with the axes {axes}, and the checker holds its weights only"
            ));
        }
        sheets.push(*sheet);
    }
    if sheets.is_empty() {
        return Err(format!("a page links Google Fonts with no family: {query}"));
    }
    Ok(sheets.concat())
}

/// <text>, a page or a stylesheet, with each Google Fonts link pointing at the
/// server's own copy; an error when a link asks for what the checker holds no
/// copy of.
pub(crate) fn relink(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(GOOGLE_STYLESHEET) {
        let (before, link) = rest.split_at(at);
        let link = &link[GOOGLE_STYLESHEET.len()..];
        let end = link
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '<' | '>'))
            .unwrap_or(link.len());
        stylesheet(&link[..end].replace("&amp;", "&"))?;
        out.push_str(before);
        out.push_str(URL_PREFIX);
        out.push_str("css2?");
        rest = link;
    }
    out.push_str(rest);
    Ok(out)
}

/// What <rest>, the path after `URL_PREFIX` with its query, names: the
/// stylesheet or a font file, with its type. None for a name the server does
/// not hold; an error for a stylesheet of what the checker holds no copy of.
pub(crate) fn answer(rest: &str) -> Result<Option<(&'static str, Vec<u8>)>, String> {
    if let Some(query) = rest.strip_prefix("css2?") {
        return Ok(Some((
            "text/css; charset=utf-8",
            stylesheet(query)?.into_bytes(),
        )));
    }
    let name = rest.strip_suffix(".woff2");
    Ok(FILES
        .iter()
        .find(|(file, _)| Some(*file) == name)
        .map(|(_, bytes)| ("font/woff2", bytes.to_vec())))
}

#[cfg(test)]
#[path = "web_fonts_tests.rs"]
mod tests;
