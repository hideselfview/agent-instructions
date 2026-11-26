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

/// The build volume the tests keep the Chrome slots in, so that the tests and
/// the builds runs on the host share them: the volume BUILDS_ROOT names, else
/// the default one.
pub(crate) fn volume() -> PathBuf {
    std::env::var_os("BUILDS_ROOT").map_or_else(|| PathBuf::from("/Volumes/Builds"), PathBuf::from)
}

/// The folders the tests start Chrome with.
pub(crate) fn folders() -> boards::Folders {
    boards::Folders {
        cache: cache(),
        fonts: None,
        volume: volume(),
    }
}

/// Runs `work` with the tree of `root` and the fixture's design system
/// served, and a browser started at a device scale of 1.
pub(crate) async fn with_browser<T>(
    root: &Path,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    with_browser_at(root, 1.0, work).await
}

/// `with_browser` with a browser at a device scale of <scale>.
pub(crate) async fn with_browser_at<T>(
    root: &Path,
    scale: f64,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    with_design_at(root, &design(), scale, work).await
}

/// `with_browser` with <design> in place of the fixture's.
pub(crate) async fn with_design<T>(
    root: &Path,
    design: &DesignSystem,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    with_design_at(root, design, 1.0, work).await
}

/// `with_design` with a browser at a device scale of <scale>.
pub(crate) async fn with_design_at<T>(
    root: &Path,
    design: &DesignSystem,
    scale: f64,
    work: impl AsyncFnOnce(&Server, &Chrome) -> T,
) -> T {
    let tree = Tree {
        root,
        design,
        runtime: boards::runtime::newest().text,
    };
    serve::with_server(tree, async |server| {
        chrome::with_chrome(&folders(), scale, async |chrome| work(server, chrome).await)
            .await
            .unwrap()
    })
    .await
    .unwrap()
}

/// Serves <html> at every path on 127.0.0.1 from threads of the test, and
/// returns the page's URL.
pub(crate) fn serve_page(html: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut request = [0u8; 4096];
                if stream.read(&mut request).is_ok() {
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                            html.len()
                        )
                        .as_bytes(),
                    );
                }
            });
        }
    });
    url
}
