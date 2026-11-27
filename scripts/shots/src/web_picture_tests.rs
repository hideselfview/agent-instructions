use super::*;
use crate::render::fixture;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Serves the files of <folder> over HTTP on 127.0.0.1 from threads of the
/// test, and returns the URL of the folder.
fn serve(folder: PathBuf) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let folder = folder.clone();
            std::thread::spawn(move || {
                let mut request = [0u8; 4096];
                let Ok(length) = stream.read(&mut request) else {
                    return;
                };
                let head = String::from_utf8_lossy(&request[..length]);
                let path = head.split(' ').nth(1).unwrap_or("/");
                let (status, body) = match std::fs::read(folder.join(&path[1..])) {
                    Ok(body) => ("200 OK", body),
                    Err(_) => ("404 Not Found", Vec::new()),
                };
                let kind = if path.ends_with(".css") {
                    "text/css"
                } else {
                    "text/html; charset=utf-8"
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // Chrome may close a connection it no longer wants.
                let _ = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(&body));
            });
        }
    });
    url
}

fn pictured(url: &str, width: u32, full_page: bool) -> image::RgbaImage {
    let page = WebPage {
        url,
        width,
        full_page,
    };
    let png = picture(&fixture::folders(), &page).unwrap();
    image::load_from_memory(&png).unwrap().to_rgba8()
}

/// A page whose stylesheet makes it a red box as tall as the window, then a
/// blue box that ends <height> points down.
fn site(height: u32) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("index.html"),
        r#"<!doctype html><link rel="stylesheet" href="page.css"><body><header></header><main></main></body>"#,
    )
    .unwrap();
    std::fs::write(
        folder.path().join("page.css"),
        format!(
            "body {{ margin: 0 }} header {{ height: 100vh; background: #f00 }} \
             main {{ height: calc({height}px - 100vh); background: #00f }}"
        ),
    )
    .unwrap();
    folder
}

const RED: image::Rgba<u8> = image::Rgba([255, 0, 0, 255]);
const BLUE: image::Rgba<u8> = image::Rgba([0, 0, 255, 255]);

#[test]
fn a_served_page_is_pictured_as_one_window_at_two_pixels_per_point() {
    let site = site(2000);
    let url = format!("{}/index.html", serve(site.path().to_path_buf()));
    let picture = pictured(&url, 1440, false);
    assert_eq!(picture.dimensions(), (2880, 1800));
    assert_eq!(*picture.get_pixel(2879, 1799), RED);
}

#[test]
fn a_full_page_picture_takes_the_whole_height_and_keeps_the_window_900_high() {
    let site = site(12000);
    let url = format!("{}/index.html", serve(site.path().to_path_buf()));
    let picture = pictured(&url, 390, true);
    assert_eq!(picture.dimensions(), (780, 24000));
    // The box as tall as the window ends 900 points down.
    assert_eq!(*picture.get_pixel(0, 1799), RED);
    assert_eq!(*picture.get_pixel(0, 1800), BLUE);
    assert_eq!(*picture.get_pixel(779, 23999), BLUE);
}

#[test]
fn a_page_no_server_answers_fails_naming_the_url() {
    // A port that was free a moment ago, which nothing listens on.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let url = format!("http://{address}/");
    let page = WebPage {
        url: &url,
        width: 1440,
        full_page: false,
    };
    match picture(&fixture::folders(), &page) {
        Err(WebPictureError::Page(why)) => {
            assert!(why.contains(&url), "{why}");
            assert!(why.contains("ERR_CONNECTION_REFUSED"), "{why}");
        }
        other => panic!("{:?}", other.map(|png| png.len())),
    }
}
