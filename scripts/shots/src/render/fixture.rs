//! A served design tree for the tests that load boards in Chrome: the newest
//! runtime copy, a one-component design system, and the boards a test gives.

use crate::design::DesignSystem;
use crate::render::chrome::{self, Chrome};
use crate::render::serve::{self, Server, Tree};
use std::path::{Path, PathBuf};

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

/// The design system the fixture's boards load: `Demo.Box`; `Demo.Card`,
/// which draws a heading, a `Demo.Box` with props of its own and its
/// children; `Demo.Frame`, which draws its children copied with a `place`
/// prop added; `Demo.Nothing`, which draws nothing; and `Demo.Word`, which
/// returns its `value` as a bare string.
pub(crate) fn design() -> DesignSystem {
    DesignSystem {
        namespace: "demo".to_string(),
        tokens_css: ":root{--text-primary:#fff}".to_string(),
        bundle_css: String::new(),
        bundle_js: "window.Demo={Box:function(){return React.createElement('div',{id:'inner'},'hello')},\
Card:function(p){return React.createElement('section',null,React.createElement('h3',null,'Card title'),React.createElement(window.Demo.Box,{kind:'inner'}),p.children)},\
Frame:function(p){return React.createElement('div',null,React.Children.map(p.children,function(c){return React.cloneElement(c,{place:'mid'})}))},\
Nothing:function(){return null},\
Word:function(p){return p.value}};"
            .to_string(),
        fonts: None,
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

/// The cache folder the tests keep pinned Chrome in, `~/.cache/builds-shots`:
/// builds gives the checker one on its build volume, which a test does not
/// have.
pub(crate) fn cache() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".cache/builds-shots")
}

/// Runs `work` with the tree of `root` and the fixture's design system
/// served, and a browser started.
pub(crate) async fn with_browser<T>(
    root: &Path,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    with_design(root, &design(), work).await
}

/// `with_browser` with <design> in place of the fixture's.
pub(crate) async fn with_design<T>(
    root: &Path,
    design: &DesignSystem,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    let tree = Tree {
        root,
        design,
        runtime: boards::runtime::newest().text,
    };
    serve::with_server(tree, async |server| {
        chrome::with_chrome(&cache(), async |chrome| work(server, chrome).await)
            .await
            .unwrap()
    })
    .await
    .unwrap()
}
