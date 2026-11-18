use super::*;

#[test]
fn each_published_platform_has_a_url_and_a_folder() {
    let mac = Platform::of("macos", "aarch64").unwrap();
    assert_eq!(
        mac.url("157.0.8103.78"),
        "https://storage.googleapis.com/chrome-for-testing-public/157.0.8103.78/mac-arm64/chrome-mac-arm64.zip"
    );
    assert_eq!(
        mac.folder(Path::new("/cache"), "157.0.8103.78"),
        Path::new("/cache/157.0.8103.78-mac-arm64")
    );
    assert!(mac.executable.starts_with("chrome-mac-arm64/"));
    let linux = Platform::of("linux", "x86_64").unwrap();
    assert_eq!(linux.executable, "chrome-linux64/chrome");
    assert!(
        linux
            .url("1.2.3")
            .ends_with("/1.2.3/linux64/chrome-linux64.zip")
    );
}

#[test]
fn an_unpublished_platform_has_none() {
    assert_eq!(Platform::of("windows", "x86_64"), None);
    assert_eq!(Platform::of("linux", "aarch64"), None);
}

#[test]
fn an_archive_with_the_pinned_hash_passes() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("chrome.zip");
    std::fs::write(&archive, b"abc").unwrap();
    verify(
        &archive,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    )
    .unwrap();
    assert!(archive.exists());
}

#[test]
fn an_archive_with_another_hash_fails_naming_both() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("chrome.zip");
    std::fs::write(&archive, b"abc").unwrap();
    let wrong = "0".repeat(64);
    let message = verify(&archive, &wrong).unwrap_err().to_string();
    assert!(message.contains(&wrong), "{message}");
    assert!(
        message.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "{message}"
    );
}

#[test]
fn every_platform_pins_a_sha256() {
    for (os, arch) in [
        ("macos", "aarch64"),
        ("macos", "x86_64"),
        ("linux", "x86_64"),
    ] {
        let sha256 = Platform::of(os, arch).unwrap().sha256;
        assert_eq!(sha256.len(), 64, "{os} {arch}");
        assert!(sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{os} {arch}");
    }
}
