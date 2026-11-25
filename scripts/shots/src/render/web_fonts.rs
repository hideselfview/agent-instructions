//! The web font the boards load from Google Fonts: Roboto, which the Android
//! boards and previews link as `https://fonts.googleapis.com/css2?family=Roboto…`.
//!
//! Google answers that link with whichever copy of the font it picks for the
//! request, and two runs of one page got different files (a variable `woff2`
//! in one, a static font under `fonts.gstatic.com/l/font?kit=` in another),
//! which draw text edges a level apart. So the checker never lets a page reach
//! Google: the server that serves the boards points each link at its own
//! pinned copy of Roboto v51, kept in `canvas/`, and answers it with the
//! stylesheet and files below.

/// Where the server answers what a Google Fonts link asked for.
pub(crate) const URL_PREFIX: &str = "/web-fonts/";

const GOOGLE_STYLESHEET: &str = "https://fonts.googleapis.com/css2?";

/// The stylesheet of the pinned Roboto: one face for each subset, each holding
/// every weight.
const STYLESHEET: &str = include_str!("../../canvas/roboto.css");

/// The pinned files, by subset.
const FILES: [(&str, &[u8]); 9] = [
    (
        "cyrillic-ext",
        include_bytes!("../../canvas/roboto/cyrillic-ext.woff2"),
    ),
    (
        "cyrillic",
        include_bytes!("../../canvas/roboto/cyrillic.woff2"),
    ),
    (
        "greek-ext",
        include_bytes!("../../canvas/roboto/greek-ext.woff2"),
    ),
    ("greek", include_bytes!("../../canvas/roboto/greek.woff2")),
    ("math", include_bytes!("../../canvas/roboto/math.woff2")),
    (
        "symbols",
        include_bytes!("../../canvas/roboto/symbols.woff2"),
    ),
    (
        "vietnamese",
        include_bytes!("../../canvas/roboto/vietnamese.woff2"),
    ),
    (
        "latin-ext",
        include_bytes!("../../canvas/roboto/latin-ext.woff2"),
    ),
    ("latin", include_bytes!("../../canvas/roboto/latin.woff2")),
];

/// <html> with each link to Google's stylesheet pointing at the server's own.
pub(crate) fn relink(html: &str) -> String {
    html.replace(GOOGLE_STYLESHEET, &format!("{URL_PREFIX}css2?"))
}

/// What <url_path> names under `URL_PREFIX`: the stylesheet and its type, or a
/// font file and its type. None for a name the server does not hold; an error
/// for a stylesheet of a family it holds no copy of.
pub(crate) fn answer(url_path: &str) -> Result<Option<(&'static str, Vec<u8>)>, String> {
    let Some(rest) = url_path.strip_prefix(URL_PREFIX) else {
        return Ok(None);
    };
    if let Some(query) = rest.strip_prefix("css2?") {
        let query = query.split('#').next().unwrap_or_default();
        for family in query.split('&').filter_map(|p| p.strip_prefix("family=")) {
            let name = family.split(':').next().unwrap_or_default();
            if name != "Roboto" {
                return Err(format!(
                    "the page links the web font {name}, and the checker holds a copy of Roboto only"
                ));
            }
        }
        return Ok(Some((
            "text/css; charset=utf-8",
            STYLESHEET.as_bytes().to_vec(),
        )));
    }
    let subset = rest
        .strip_prefix("roboto/")
        .and_then(|file| file.strip_suffix(".woff2"));
    Ok(FILES
        .iter()
        .find(|(name, _)| Some(*name) == subset)
        .map(|(_, bytes)| ("font/woff2", bytes.to_vec())))
}

#[cfg(test)]
#[path = "web_fonts_tests.rs"]
mod tests;
