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
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
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
