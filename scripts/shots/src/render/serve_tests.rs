use super::*;
use crate::design::tests::{system, write};

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    write(r, "design/mocks/mac.a.b.dc.html", "<x-dc>board</x-dc>");
    write(
        r,
        "design/system/components/MacRow/preview.html",
        "<head></head><script>row</script>",
    );
    write(r, "secret.txt", "secret");
    root
}

async fn get(server: &Server, path: &str) -> String {
    let mut stream = TcpStream::connect(server.address).await.unwrap();
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
async fn serves_the_canvas_tree_and_refuses_the_rest() {
    let root = tree();
    let design = system("tokens", "css", "js");
    let tree = Tree {
        root: root.path(),
        design: &design,
        runtime: "runtime",
    };
    with_server(tree, async |server| {
        let board = get(server, "/mac.a.b.dc.html?x=1").await;
        assert!(board.starts_with("HTTP/1.1 200 OK"), "{board}");
        assert!(board.contains("text/html"), "{board}");
        assert!(board.ends_with("<x-dc>board</x-dc>"), "{board}");
        assert!(get(server, "/support.js").await.ends_with("runtime"));
        assert!(
            get(server, "/storage/support.js")
                .await
                .ends_with("runtime")
        );
        assert!(get(server, "/ds/demo/tokens.css").await.ends_with("tokens"));
        assert!(
            get(server, "/storage/ds/demo/components/bundle.js")
                .await
                .ends_with("js")
        );
        assert!(
            get(server, "/ds/other/tokens.css")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(get(server, "/react.js").await.contains("React"));
        assert!(
            get(server, "/missing.html")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(
            get(server, "/../secret.txt")
                .await
                .starts_with("HTTP/1.1 404")
        );
        let preview = get(server, "/components/MacRow.html").await;
        assert!(
            preview.contains("<script src=\"/react.js\"></script>"),
            "{preview}"
        );
        assert!(
            preview.contains("/ds/demo/components/bundle.js"),
            "{preview}"
        );
        assert!(preview.ends_with("<script>row</script>"), "{preview}");
        assert!(
            get(server, "/components/Nothing.html")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(server.board_url("mac.a.b").ends_with("/mac.a.b.dc.html"));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_file_it_cannot_give_answers_500_with_the_cause() {
    let root = tree();
    write(
        root.path(),
        "design/system/components/NoHead/preview.html",
        "<body></body>",
    );
    write(
        root.path(),
        "design/system/components/Binary/preview.html",
        "",
    );
    std::fs::write(
        root.path()
            .join("design/system/components/Binary/preview.html"),
        [0xff, 0xfe],
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("design/mocks/folder.dc.html")).unwrap();
    let design = system("tokens", "css", "js");
    let tree = Tree {
        root: root.path(),
        design: &design,
        runtime: "runtime",
    };
    with_server(tree, async |server| {
        let no_head = get(server, "/components/NoHead.html").await;
        assert!(no_head.starts_with("HTTP/1.1 500"), "{no_head}");
        assert!(no_head.contains("has no <head>"), "{no_head}");
        let binary = get(server, "/components/Binary.html").await;
        assert!(
            binary.starts_with("HTTP/1.1 500") && binary.contains("is not text"),
            "{binary}"
        );
        let folder = get(server, "/folder.dc.html").await;
        assert!(
            folder.starts_with("HTTP/1.1 500") && folder.contains("cannot read"),
            "{folder}"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn serves_the_sf_mono_files_of_its_fonts_folder_and_nothing_else_under_fonts() {
    let root = tree();
    let fonts = tempfile::tempdir().unwrap();
    std::fs::write(fonts.path().join("SF-Mono-Regular.otf"), "font bytes").unwrap();
    std::fs::write(fonts.path().join("secret.txt"), "secret").unwrap();
    let mut design = system("tokens", "css", "js");
    design.fonts = fonts.path().to_path_buf();
    let tree = Tree {
        root: root.path(),
        design: &design,
        runtime: "runtime",
    };
    with_server(tree, async |server| {
        let font = get(server, "/fonts/sf-mono/SF-Mono-Regular.otf").await;
        assert!(font.starts_with("HTTP/1.1 200 OK"), "{font}");
        assert!(font.contains("Content-Type: font/otf"), "{font}");
        assert!(font.ends_with("font bytes"), "{font}");
        // A file of the family the folder lacks is not found.
        let lacking = get(server, "/fonts/sf-mono/SF-Mono-Bold.otf").await;
        assert!(lacking.starts_with("HTTP/1.1 404"), "{lacking}");
        for refused in ["secret.txt", "..%2Fsecret.txt", "SF-Mono-Unknown.otf"] {
            let answer = get(server, &format!("/fonts/sf-mono/{refused}")).await;
            assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
        }
    })
    .await
    .unwrap();
}
