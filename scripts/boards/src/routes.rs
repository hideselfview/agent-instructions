//! The URLs of a served board tree, as the canvas serves them: a board and
//! the parts it mounts as siblings (`<name>.dc.html`), the canvas runtime as
//! `/support.js`, and the design system under `/ds/<namespace>/`, each also
//! under `/storage/`. The canvas runtime fetches the page's own URL and each
//! part as `./<name>.dc.html`, so a board is drawn from a server, never from a
//! file. Every server of boards answers with these routes.

use std::path::{Component, Path};

/// A design-system file a board loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignFile {
    TokensCss,
    BundleCss,
    BundleJs,
}

/// What a URL path names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The canvas runtime.
    Runtime,
    /// A file of the design system whose namespace is the first field.
    Design(String, DesignFile),
    /// A file under `design/mocks`, by its path there.
    Mocks(String),
}

/// What <url_path> names, if anything. A path under `design/mocks` stays
/// inside it.
pub fn route(url_path: &str) -> Option<Route> {
    let path = url_path.split(['?', '#']).next()?;
    let path = path
        .strip_prefix("/storage")
        .filter(|p| p.starts_with('/'))
        .unwrap_or(path);
    if path == "/support.js" {
        return Some(Route::Runtime);
    }
    if let Some(rest) = path.strip_prefix("/ds/") {
        let (namespace, file) = rest.split_once('/')?;
        let file = match file {
            "tokens.css" => DesignFile::TokensCss,
            "components/bundle.css" => DesignFile::BundleCss,
            "components/bundle.js" => DesignFile::BundleJs,
            _ => return None,
        };
        return Some(Route::Design(namespace.to_string(), file));
    }
    let relative = Path::new(path.strip_prefix('/')?);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(Route::Mocks(relative.to_string_lossy().into_owned()))
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;
