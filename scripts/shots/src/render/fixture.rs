//! A served design tree for the tests that load boards in Chrome: the newest
//! runtime copy, a one-component design system, and the boards a test gives.

use crate::design::DesignSystem;
use crate::render::chrome::{self, Chrome};
use crate::render::serve::{self, Server, Tree};
use std::path::Path;

pub(crate) const HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<script src="./support.js"></script>
<link rel="stylesheet" href="ds/demo/tokens.css">
<link rel="stylesheet" href="ds/demo/components/bundle.css">
<script src="ds/demo/components/bundle.js"></script></head><body>"#;

pub(crate) fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The design system the fixture's boards load: one component, `Demo.Box`.
pub(crate) fn design() -> DesignSystem {
    DesignSystem {
        namespace: "demo".to_string(),
        tokens_css: ":root{--text-primary:#fff}".to_string(),
        bundle_css: String::new(),
        bundle_js:
            "window.Demo={Box:function(){return React.createElement('div',{id:'inner'},'hello')}};"
                .to_string(),
    }
}

/// A tree holding each of `boards` (name, the page after the shared head).
pub(crate) fn tree(boards: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (name, page) in boards {
        write(
            root.path(),
            &format!("design/mocks/{name}.dc.html"),
            &format!("{HEAD}{page}</body></html>"),
        );
    }
    root
}

/// Runs `work` with the tree of `root` and the fixture's design system
/// served, and a browser started.
pub(crate) async fn with_browser<T>(
    root: &Path,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    let design = design();
    let tree = Tree {
        root,
        design: &design,
        runtime: boards::runtime::newest().text,
    };
    serve::with_server(tree, async |server| {
        chrome::with_chrome(&crate::cache_dir().unwrap(), async |chrome| {
            work(server, chrome).await
        })
        .await
        .unwrap()
    })
    .await
    .unwrap()
}
