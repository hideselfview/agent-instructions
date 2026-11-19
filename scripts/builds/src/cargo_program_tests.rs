use super::*;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;

/// Writes <program> into <dir> as a program that reports <version>.
pub fn fake_program(of: &Program, dir: &Path, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join(of.name);
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho '{} {version}'\necho 'release: {version}'\n",
            of.name
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_probe_answer_names_what_the_host_has() {
    assert_eq!(
        NEXTEST.parse_has("cores=10\nnextest=0.9.143\n"),
        Has::Version("0.9.143".to_string())
    );
    assert_eq!(NEXTEST.parse_has("cores=10\n"), Has::Nothing);
    assert_eq!(
        NEXTEST.parse_has("nextest_fails=bad CPU type \n"),
        Has::Broken("bad CPU type".to_string())
    );
    assert!(matches!(NEXTEST.parse_has("nextest=\n"), Has::Broken(_)));
}

/// A program that exists and fails to start is broken, never missing.
#[test]
fn a_program_that_fails_is_broken_and_install_stops() {
    let scratch = Scratch::new("nextest-broken");
    let home = scratch.dir.join("home");
    let bin = home.join(".cargo/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("cargo-nextest");
    std::fs::write(&program, "#!/bin/sh\necho dyld missing >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let Has::Broken(said) = NEXTEST.has(&home) else {
        panic!("expected a broken program");
    };
    assert!(said.contains("dyld missing"), "{said}");
    let text = NEXTEST
        .install_with(&home, |_| panic!("no download"))
        .unwrap_err()
        .text();
    assert!(
        text.contains("does not start") && text.contains("dyld missing"),
        "{text}"
    );
}

#[test]
fn install_downloads_when_the_host_has_none_or_another_version_and_then_changes_nothing() {
    let scratch = Scratch::new("nextest-install");
    let home = scratch.dir.join("home");
    let bin = home.join(".cargo/bin");
    assert_eq!(NEXTEST.has(&home), Has::Nothing);

    let downloads = std::cell::Cell::new(0);
    let fetch = |dir: &Path| -> Result<()> {
        downloads.set(downloads.get() + 1);
        fake_program(&NEXTEST, dir, NEXTEST.version);
        Ok(())
    };
    assert!(NEXTEST.install_with(&home, fetch).unwrap());
    assert_eq!(
        NEXTEST.has(&home),
        Has::Version(NEXTEST.version.to_string())
    );

    // The pinned version is there: nothing downloads.
    assert!(!NEXTEST.install_with(&home, fetch).unwrap());
    assert_eq!(downloads.get(), 1);

    // Another version is replaced.
    fake_program(&NEXTEST, &bin, "0.9.1");
    assert_eq!(NEXTEST.has(&home), Has::Version("0.9.1".to_string()));
    assert!(NEXTEST.install_with(&home, fetch).unwrap());
    assert_eq!(
        NEXTEST.has(&home),
        Has::Version(NEXTEST.version.to_string())
    );
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
    fake_program(&NEXTEST, &bin, "0.9.1");

    let text = NEXTEST
        .install_with(&home, |_| fail!("no network"))
        .unwrap_err()
        .text();
    assert!(text.contains("no network"), "{text}");
    assert_eq!(NEXTEST.has(&home), Has::Version("0.9.1".to_string()));

    let text = NEXTEST.install_with(&home, |_| Ok(())).unwrap_err().text();
    assert!(text.contains("held no cargo-nextest program"), "{text}");

    let text = NEXTEST
        .install_with(&home, |dir| {
            fake_program(&NEXTEST, dir, "0.9.2");
            Ok(())
        })
        .unwrap_err()
        .text();
    assert!(text.contains("0.9.2"), "{text}");
    assert!(std::fs::read_dir(&bin).unwrap().count() == 1);
}

#[test]
fn each_program_reads_its_own_probe_lines_and_installs_its_own_version() {
    assert_eq!(
        MUTANTS.parse_has("mutants=27.1.0\nnextest=0.9.143\n"),
        Has::Version("27.1.0".to_string())
    );
    assert_eq!(MUTANTS.parse_has("nextest=0.9.143\n"), Has::Nothing);
    assert!(MUTANTS.probe_lines().contains("cargo-mutants"));
    assert!(MUTANTS.probe_lines().contains("mutants --version"));

    let scratch = Scratch::new("mutants-install");
    let home = scratch.dir.join("home");
    let bin = home.join(".cargo/bin");
    fake_program(&NEXTEST, &bin, NEXTEST.version);
    assert_eq!(MUTANTS.has(&home), Has::Nothing);
    assert!(
        MUTANTS
            .install_with(&home, |dir| {
                fake_program(&MUTANTS, dir, MUTANTS.version);
                Ok(())
            })
            .unwrap()
    );
    assert_eq!(
        MUTANTS.has(&home),
        Has::Version(MUTANTS.version.to_string())
    );
    assert_eq!(
        NEXTEST.has(&home),
        Has::Version(NEXTEST.version.to_string())
    );
    let text = MUTANTS.install_with(&home, |_| Ok(())).map(|_| ());
    assert!(
        text.is_ok(),
        "the pinned version is there: nothing is fetched"
    );
}
