use super::*;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;

/// Writes a cargo-nextest program into <dir> that reports <version>.
pub fn fake_program(dir: &Path, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("cargo-nextest");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho 'cargo-nextest {version}'\necho 'release: {version}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_probe_answer_names_the_version_or_none() {
    assert_eq!(
        parse_version("cores=10\nnextest=0.9.143\n").as_deref(),
        Some("0.9.143")
    );
    assert_eq!(parse_version("cores=10\n"), None);
    assert_eq!(parse_version("nextest=\n"), None);
}

#[test]
fn install_downloads_when_the_host_has_none_or_another_version_and_then_changes_nothing() {
    let scratch = Scratch::new("nextest-install");
    let home = scratch.dir.join("home");
    let bin = home.join(".cargo/bin");
    assert_eq!(installed_version(&home), None);

    let downloads = std::cell::Cell::new(0);
    let fetch = |dir: &Path| -> Result<()> {
        downloads.set(downloads.get() + 1);
        fake_program(dir, VERSION);
        Ok(())
    };
    assert!(install_with(&home, fetch).unwrap());
    assert_eq!(installed_version(&home).as_deref(), Some(VERSION));

    // The pinned version is there: nothing downloads.
    assert!(!install_with(&home, fetch).unwrap());
    assert_eq!(downloads.get(), 1);

    // Another version is replaced.
    fake_program(&bin, "0.9.1");
    assert_eq!(installed_version(&home).as_deref(), Some("0.9.1"));
    assert!(install_with(&home, fetch).unwrap());
    assert_eq!(installed_version(&home).as_deref(), Some(VERSION));
    assert_eq!(downloads.get(), 2);

    // No download folder is left behind.
    let left: Vec<_> = std::fs::read_dir(&bin)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["cargo-nextest"]);
}

#[test]
fn a_download_that_fails_or_holds_the_wrong_version_fails_install_and_keeps_the_old_program() {
    let scratch = Scratch::new("nextest-install-fails");
    let home = scratch.dir.join("home");
    let bin = home.join(".cargo/bin");
    fake_program(&bin, "0.9.1");

    let text = install_with(&home, |_| fail!("no network"))
        .unwrap_err()
        .text();
    assert!(text.contains("no network"), "{text}");
    assert_eq!(installed_version(&home).as_deref(), Some("0.9.1"));

    let text = install_with(&home, |_| Ok(())).unwrap_err().text();
    assert!(text.contains("held no cargo-nextest program"), "{text}");

    let text = install_with(&home, |dir| {
        fake_program(dir, "0.9.2");
        Ok(())
    })
    .unwrap_err()
    .text();
    assert!(text.contains("0.9.2"), "{text}");
    assert!(std::fs::read_dir(&bin).unwrap().count() == 1);
}
