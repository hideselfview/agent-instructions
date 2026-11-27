use super::*;

/// The words of <line> as `builds web picture` reads them.
fn read(line: &str) -> Result<Args> {
    let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    Args::of(
        crate::command::Command::Web(crate::command::web::Picture),
        &words,
    )
}

#[test]
fn the_command_reads_a_url_a_width_and_full_page() {
    let plain = read("http://localhost:3000/").unwrap();
    assert_eq!(
        parse(&plain).unwrap(),
        WebPage {
            url: "http://localhost:3000/",
            width: 1440,
            full_page: false,
        }
    );
    let flagged = read("--full-page HTTPS://bae.fm --width 390").unwrap();
    assert_eq!(
        parse(&flagged).unwrap(),
        WebPage {
            url: "HTTPS://bae.fm",
            width: 390,
            full_page: true,
        }
    );
    for line in [
        "",
        "http://a http://b",
        "http://a --width",
        "http://a --width 0",
        "http://a --width wide",
        "http://a --height 3",
    ] {
        let refused = read(line).and_then(|args| parse(&args).map(|_| ()));
        assert!(refused.is_err(), "{line:?}");
    }
}

#[test]
fn a_url_that_is_not_http_or_https_is_refused() {
    for url in [
        "file:///etc/hosts",
        "data:text/html,x",
        "chrome://version",
        "localhost:3000",
        "bae.fm",
        "http://",
    ] {
        let args = read(url).unwrap();
        let error = parse(&args).unwrap_err().text();
        assert!(error.contains("http or https"), "{url}: {error}");
        assert!(error.contains(url), "{url}: {error}");
    }
}

#[test]
fn each_picture_is_kept_in_a_folder_of_its_own_on_the_volume() {
    let scratch = crate::testing::Scratch::new("web-picture-keep");
    let places = scratch.places();
    std::fs::create_dir_all(&places.root).unwrap();
    let first = keep(&places, b"one").unwrap();
    let second = keep(&places, b"two").unwrap();
    assert_ne!(first, second);
    assert!(first.starts_with(root(&places)), "{first:?}");
    assert_eq!(std::fs::read(first).unwrap(), b"one");
    assert_eq!(std::fs::read(second).unwrap(), b"two");
}
