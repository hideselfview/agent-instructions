use super::*;
use crate::lists::project::NewProject;
use crate::testing::{Scratch, git};

const MANIFEST: &str = r#"
date = "2026-04-16"

[pkg.rust]
version = "1.96.0 (3fc1c0e1b 2026-04-14)"
"#;

fn manifest(version: &str) -> String {
    MANIFEST.replace("1.96.0", version)
}

/// A clone at <home>/dev/<repo> whose origin/main holds <files>.
fn clone(home: &Path, repo: &str, files: &[(&str, &str)]) {
    let dir = home.join("dev").join(repo);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "test"]);
    std::fs::write(dir.join("README"), "x").unwrap();
    git(&dir, &["add", "README"]);
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
        git(&dir, &["add", name]);
    }
    git(&dir, &["commit", "-q", "-m", "Start"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
}

fn pin_file(channel: &str) -> String {
    format!("[toolchain]\nchannel = \"{channel}\"\ncomponents = [\"rustc\"]\n")
}

fn project(places: &Places, name: &str, repos: &[&str], areas: &[&str]) -> Store {
    let store = Store::named(places, name).unwrap();
    let stamp = Stamp {
        at: 1000,
        by: "test".into(),
    };
    store
        .create(
            &NewProject {
                name: name.into(),
                canvas: false,
                repos: repos.iter().map(|r| r.to_string()).collect(),
                places: Default::default(),
                sources: Default::default(),
                specs: Default::default(),
            },
            &stamp,
            "lists init",
        )
        .unwrap();
    store
        .write(&stamp, "area add", |db| {
            for area in areas {
                area::add_area(db, area, None)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn the_stable_version_is_the_first_word_of_the_manifest_s_rust_version() {
    assert_eq!(stable_version(MANIFEST).unwrap().to_string(), "1.96.0");
}

#[test]
fn a_manifest_that_names_no_version_is_an_error_that_says_so() {
    for (text, says) in [
        ("not toml [", "not TOML"),
        ("date = 1", "no pkg.rust.version"),
        (
            "[pkg.rust]\nversion = \"nightly\"",
            "starts with no x.y.z version",
        ),
    ] {
        let error = stable_version(text).expect_err("refused").text();
        assert!(error.contains(says), "{error}");
    }
}

#[test]
fn versions_compare_by_number_not_by_text() {
    let old = Version::parse("1.9.0").unwrap();
    assert!(old < Version::parse("1.10.0").unwrap());
    assert_eq!(Version::parse("1.95"), None);
    assert_eq!(Version::parse("1.95.0.1"), None);
    assert_eq!(Version::parse("stable"), None);
    assert_eq!(Version::parse("1.95.x"), None);
}

#[test]
fn a_pin_comes_from_origin_main_in_either_toolchain_file() {
    let scratch = Scratch::new("rust-pin");
    let home = scratch.places().home;
    clone(
        &home,
        "toml",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    clone(&home, "plain", &[("rust-toolchain", "1.94.1\n")]);
    clone(
        &home,
        "channel",
        &[("rust-toolchain.toml", &pin_file("stable"))],
    );
    clone(&home, "none", &[]);
    let at = |repo: &str| pin(&home.join("dev").join(repo)).unwrap();
    assert_eq!(at("toml").unwrap().version.to_string(), "1.95.0");
    assert_eq!(at("toml").unwrap().file, "rust-toolchain.toml");
    assert_eq!(at("plain").unwrap().version.to_string(), "1.94.1");
    assert_eq!(at("channel"), None);
    assert_eq!(at("none"), None);
}

#[test]
fn a_new_release_makes_one_item_per_project_that_pins_an_older_toolchain() {
    let scratch = Scratch::new("rust-queue");
    let places = scratch.places();
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    clone(
        &places.home,
        "lib",
        &[("rust-toolchain.toml", &pin_file("1.94.0"))],
    );
    clone(&places.home, "docs", &[]);
    let with = project(&places, "with", &["app", "lib"], &["Alpha", "tooling"]);
    let without = project(&places, "without", &["docs"], &["Alpha"]);

    let outcome = check(&places, &manifest("1.96.0"), 2000).unwrap();
    assert_eq!(outcome.faults, Vec::<String>::new());
    assert_eq!(outcome.queued.len(), 1, "{:?}", outcome.queued);

    let lists = with.load().unwrap();
    let [item] = lists.items.as_slice() else {
        panic!("one item: {:?}", lists.items)
    };
    assert_eq!(item.title, "Move the Rust toolchain pin to 1.96.0");
    assert_eq!(item.area, "tooling");
    assert!(
        item.details
            .contains("app pins 1.95.0 in rust-toolchain.toml")
    );
    assert!(
        item.details
            .contains("lib pins 1.94.0 in rust-toolchain.toml")
    );
    assert_eq!(without.load().unwrap().items.len(), 0);
}

#[test]
fn reading_the_same_release_again_makes_no_second_item() {
    let scratch = Scratch::new("rust-once");
    let places = scratch.places();
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    let store = project(&places, "demo", &["app"], &["Alpha"]);

    check(&places, &manifest("1.96.0"), 2000).unwrap();
    let again = check(&places, &manifest("1.96.0"), 3000).unwrap();
    assert_eq!(again, Outcome::default());
    assert_eq!(store.load().unwrap().items.len(), 1);
    // The project's first area stands in when it has neither tooling nor hosts.
    assert_eq!(store.load().unwrap().items[0].area, "Alpha");

    // The next release is a new item.
    let next = check(&places, &manifest("1.97.0"), 4000).unwrap();
    assert_eq!(next.queued.len(), 1);
    assert_eq!(store.load().unwrap().items.len(), 2);
}

#[test]
fn a_repo_already_on_the_release_or_newer_makes_no_item() {
    let scratch = Scratch::new("rust-current");
    let places = scratch.places();
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.96.0"))],
    );
    clone(
        &places.home,
        "lib",
        &[("rust-toolchain.toml", &pin_file("1.97.0"))],
    );
    let store = project(&places, "demo", &["app", "lib"], &["Alpha"]);
    let outcome = check(&places, &manifest("1.96.0"), 2000).unwrap();
    assert_eq!(outcome, Outcome::default());
    assert_eq!(store.load().unwrap().items.len(), 0);
}

#[test]
fn one_project_that_faults_does_not_stop_the_others() {
    let scratch = Scratch::new("rust-fault");
    let places = scratch.places();
    // A clone with no origin/main: its pin can't be read.
    let broken = places.home.join("dev/broken");
    std::fs::create_dir_all(&broken).unwrap();
    git(&broken, &["init", "-q", "-b", "main"]);
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    project(&places, "a-broken", &["broken"], &["Alpha"]);
    let fine = project(&places, "b-fine", &["app"], &["Alpha"]);
    let outcome = check(&places, &manifest("1.96.0"), 2000).unwrap();
    assert_eq!(outcome.faults.len(), 1, "{:?}", outcome.faults);
    assert!(outcome.faults[0].starts_with("a-broken: reading the Rust pin of broken"));
    assert_eq!(fine.load().unwrap().items.len(), 1);
}

#[test]
fn a_manifest_that_can_not_be_read_makes_an_error_and_no_item() {
    let scratch = Scratch::new("rust-bad-manifest");
    let places = scratch.places();
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    let store = project(&places, "demo", &["app"], &["Alpha"]);
    assert!(check(&places, "<html>502 Bad Gateway</html>", 2000).is_err());
    assert_eq!(store.load().unwrap().items.len(), 0);
}

#[test]
fn a_pin_is_read_as_rustup_reads_a_channel() {
    for channel in [
        "stable",
        "beta",
        "nightly",
        "nightly-2026-04-01",
        "beta-2026-04-01",
    ] {
        assert_eq!(pinned_version(channel), Ok(None), "{channel}");
    }
    assert_eq!(
        pinned_version("1.95.2").unwrap().unwrap().to_string(),
        "1.95.2"
    );
    assert_eq!(pinned_version("1.95").unwrap().unwrap().to_string(), "1.95");
    for bad in [
        "1",
        "1.95.0.1",
        "1.x",
        "latest",
        "nightly-soon",
        "1.95.0-beta",
        "",
    ] {
        assert!(pinned_version(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_pin_of_two_parts_compares_by_its_two_parts() {
    let stable = Version::parse("1.96.1").unwrap();
    assert!(pinned_version("1.95").unwrap().unwrap().older_than(stable));
    assert!(!pinned_version("1.96").unwrap().unwrap().older_than(stable));
    assert!(
        pinned_version("1.96.0")
            .unwrap()
            .unwrap()
            .older_than(stable)
    );
    assert!(
        !pinned_version("1.96.1")
            .unwrap()
            .unwrap()
            .older_than(stable)
    );
}

#[test]
fn a_toolchain_file_that_cannot_be_read_is_an_error_that_names_the_file() {
    let scratch = Scratch::new("rust-pin-faults");
    let home = scratch.places().home;
    clone(
        &home,
        "bad-toml",
        &[("rust-toolchain.toml", "[toolchain\nchannel = 1")],
    );
    clone(
        &home,
        "bad-legacy",
        &[("rust-toolchain", "[toolchain\nchannel = 1")],
    );
    clone(
        &home,
        "bad-channel",
        &[("rust-toolchain.toml", &pin_file("latest"))],
    );
    for (repo, file) in [
        ("bad-toml", "rust-toolchain.toml"),
        ("bad-legacy", "rust-toolchain"),
        ("bad-channel", "rust-toolchain.toml"),
    ] {
        let error = pin(&home.join("dev").join(repo))
            .expect_err("refused")
            .text();
        assert!(error.starts_with(&format!("{file}: ")), "{error}");
    }
}

#[test]
fn rust_toolchain_wins_over_rust_toolchain_toml_as_in_rustup() {
    let scratch = Scratch::new("rust-pin-order");
    let home = scratch.places().home;
    clone(
        &home,
        "both",
        &[
            ("rust-toolchain", "1.90.0\n"),
            ("rust-toolchain.toml", &pin_file("1.95.0")),
        ],
    );
    let found = pin(&home.join("dev/both")).unwrap().unwrap();
    assert_eq!(
        (found.file, found.version.to_string().as_str()),
        ("rust-toolchain", "1.90.0")
    );
}

#[test]
fn a_repo_with_no_clone_faults_its_project_and_records_no_release() {
    let scratch = Scratch::new("rust-no-clone");
    let places = scratch.places();
    clone(
        &places.home,
        "app",
        &[("rust-toolchain.toml", &pin_file("1.95.0"))],
    );
    let store = project(&places, "demo", &["app", "missing"], &["Alpha"]);
    let outcome = check(&places, &manifest("1.96.0"), 2000).unwrap();
    assert_eq!(outcome.faults.len(), 1, "{:?}", outcome.faults);
    assert!(
        outcome.faults[0].contains("missing has no clone"),
        "{:?}",
        outcome.faults
    );
    assert_eq!(outcome.queued, Vec::<String>::new());
    assert_eq!(store.load().unwrap().items.len(), 0);
    let db = store.open_existing().unwrap();
    assert_eq!(rust_release::queued(&db, "1.96.0").unwrap(), None);
}
