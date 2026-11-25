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

#[tokio::test]
async fn a_page_that_never_finishes_fails_its_draw_at_the_deadline_naming_the_page() {
    use crate::design::Frame;
    use crate::render::{board, fixture};
    let root = fixture::tree(&[(
        "fixture.stuck",
        "<x-dc><div>stuck</div></x-dc><script>while (true) {}</script>",
    )]);
    let started = std::time::Instant::now();
    let error = fixture::with_browser(root.path(), async |server, chrome| {
        let url = server.board_url("fixture.stuck");
        chrome
            .with_page_within(
                "board fixture.stuck",
                Duration::from_secs(3),
                async |page| {
                    board::render(page, "fixture.stuck", &url, Frame { w: 300, h: 200 }, 1.0)
                        .await
                        .map_err(|e| e.to_string())
                },
            )
            .await
            .err()
    })
    .await
    .expect("the draw fails");
    assert!(error.contains("did not finish within 3 seconds"), "{error}");
    assert!(error.starts_with("board fixture.stuck: "), "{error}");
    assert!(started.elapsed() < Duration::from_secs(40), "{error}");
}

#[test]
fn chrome_rasterizes_in_software_on_one_thread_so_a_picture_does_not_depend_on_the_machines_load() {
    use chromiumoxide::cdp::browser_protocol::system_info::GetInfoParams;
    let root = tempfile::tempdir().unwrap();
    let info = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(crate::render::fixture::with_browser(
            root.path(),
            async |_server, chrome| chrome.browser.execute(GetInfoParams::default()).await,
        ))
        .unwrap();
    let status = info.result.gpu.feature_status.unwrap();
    assert_eq!(status["gpu_compositing"], "disabled_software", "{status}");
    assert_eq!(status["rasterization"], "disabled_software", "{status}");
    assert_eq!(
        status["multiple_raster_threads"], "disabled_off",
        "{status}"
    );
}

#[test]
fn a_browser_that_takes_longer_than_chromiumoxides_default_to_start_is_waited_for() {
    use std::os::unix::fs::PermissionsExt;
    // macOS holds Chrome's main thread in its registration with the window
    // server for 20 to 45 seconds on a busy host before Chrome prints the
    // address of its debugging socket. This stand-in prints one after 25.
    let dir = tempfile::tempdir().unwrap();
    let browser = dir.path().join("browser");
    std::fs::write(
        &browser,
        "#!/bin/sh\nsleep 25\necho 'DevTools listening on ws://127.0.0.1:9/devtools/browser/x'\nsleep 5\n",
    )
    .unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(with_executable(
            browser,
            Duration::from_secs(40),
            Slots::of_host().unwrap(),
            async |_chrome| (),
        ))
        .unwrap_err()
        .to_string();
    assert!(
        !error.contains("Timeout while resolving websocket URL"),
        "{error}"
    );
}

#[test]
fn a_browser_that_prints_no_address_fails_naming_the_window_server() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let browser = dir.path().join("browser");
    std::fs::write(&browser, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(with_executable(
            browser,
            Duration::from_secs(2),
            Slots::of_host().unwrap(),
            async |_chrome| (),
        ))
        .unwrap_err()
        .to_string();
    assert!(error.contains("in 2 seconds"), "{error}");
    assert!(error.contains("window server"), "{error}");
}

#[test]
fn starting_chrome_names_how_it_came_out() {
    let said = |started| Start::of(started).line();
    assert_eq!(
        said(Ok("HeadlessChrome/157.0.8103.78".to_string())),
        "chrome=started HeadlessChrome/157.0.8103.78"
    );
    let hung =
        ChromeError::NoAddress("Chrome printed no debugging address\nin 120 seconds".to_string());
    assert_eq!(
        said(Err(hung)),
        "chrome=no-address cannot start Chrome: Chrome printed no debugging address in 120 seconds"
    );
    let broken = ChromeError::Launch("no such file".to_string());
    assert_eq!(
        said(Err(broken)),
        "chrome=failed cannot start Chrome: no such file"
    );
}

#[test]
fn the_pinned_chrome_starts_from_the_cache() {
    let started = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(start(&crate::render::fixture::cache()));
    assert!(
        matches!(started, Start::Started(ref version) if version.contains(PINNED)),
        "{started:?}"
    );
}

#[tokio::test]
async fn processes_of_a_chrome_end_when_the_program_that_started_it_ends() {
    // A stand-in browser: any process whose command line names the profile.
    let profile = tempfile::tempdir().unwrap();
    let mut browser = tokio::process::Command::new("sh")
        .args(["-c", "sleep 300; true", "--"])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut watchdog = Watchdog::over(profile.path()).unwrap();
    // The program ends: the kernel closes its end of the watchdog's stdin.
    drop(watchdog.child.stdin.take());
    let status = tokio::time::timeout(Duration::from_secs(20), browser.wait())
        .await
        .expect("the watchdog ended the browser")
        .unwrap();
    assert!(!status.success(), "{status}");
    watchdog.fire().await;
}

#[tokio::test]
async fn firing_the_watchdog_leaves_other_browsers_alone() {
    let (mine, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let start = |profile: &Path| {
        tokio::process::Command::new("sh")
            .args(["-c", "sleep 300; true", "--"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    let (mut mine_browser, mut other_browser) = (start(mine.path()), start(other.path()));
    Watchdog::over(mine.path()).unwrap().fire().await;
    mine_browser.wait().await.unwrap();
    assert!(other_browser.try_wait().unwrap().is_none());
    other_browser.kill().await.unwrap();
}

#[tokio::test]
async fn a_watchdog_says_when_chrome_left_processes_and_when_its_kill_failed() {
    let profile = tempfile::tempdir().unwrap();
    let nothing = Watchdog::over(profile.path()).unwrap().end().await;
    assert!(nothing.is_ok(), "{nothing:?}");
    let mut left = tokio::process::Command::new("sh")
        .args(["-c", "sleep 300; true", "--"])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let killed = Watchdog::over(profile.path()).unwrap().end().await;
    left.wait().await.unwrap();
    assert!(
        killed.unwrap_err().to_string().contains("left processes"),
        "named"
    );
    // A pattern that is no valid expression makes pkill exit 2.
    let failed = Watchdog::over(Path::new("/x[")).unwrap().end().await;
    assert!(
        failed.unwrap_err().to_string().contains("pkill failed"),
        "named"
    );
}
