use super::*;

const HEADER: &str = "version = 4\n";

fn package(name: &str, version: &str, source: bool, dependencies: &[&str]) -> String {
    let mut text = format!("\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n");
    if source {
        text.push_str(
            "source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\n",
        );
    }
    if !dependencies.is_empty() {
        text.push_str("dependencies = [\n");
        for dependency in dependencies {
            text.push_str(&format!(" \"{dependency}\",\n"));
        }
        text.push_str("]\n");
    }
    text
}

fn members() -> Vec<String> {
    ["app", "core", "tool"].map(String::from).to_vec()
}

/// app depends on core, core on the registry crate leaf through mid, and
/// tool on nothing.
fn lock(leaf: &str) -> String {
    [
        HEADER.to_string(),
        package("app", "0.1.0", false, &["core"]),
        package("core", "0.1.0", false, &["mid"]),
        package("mid", "2.0.0", true, &["leaf"]),
        package("leaf", leaf, true, &[]),
        package("tool", "0.1.0", false, &[]),
    ]
    .concat()
}

#[test]
fn a_changed_dependency_changes_every_crate_that_depends_on_it_and_no_other() {
    assert_eq!(
        with_changed_dependencies(&lock("1.0.0"), &lock("1.1.0"), &members()).unwrap(),
        ["app", "core"]
    );
    assert_eq!(
        with_changed_dependencies(&lock("1.0.0"), &lock("1.0.0"), &members()).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn a_crates_own_changed_dependencies_count_for_it() {
    let old = lock("1.0.0");
    let new = old.replace(
        "name = \"tool\"\nversion = \"0.1.0\"\n",
        "name = \"tool\"\nversion = \"0.1.0\"\ndependencies = [\n \"leaf\",\n]\n",
    );
    assert_ne!(old, new);
    assert_eq!(
        with_changed_dependencies(&old, &new, &members()).unwrap(),
        ["tool"]
    );
}

#[test]
fn two_versions_of_a_package_are_told_apart_by_version() {
    let old = [
        HEADER.to_string(),
        package("app", "0.1.0", false, &["leaf 1.0.0"]),
        package("tool", "0.1.0", false, &["leaf 2.0.0"]),
        package("leaf", "1.0.0", true, &[]),
        package("leaf", "2.0.0", true, &[]),
    ]
    .concat();
    let new = old.replace(
        "name = \"leaf\"\nversion = \"2.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"",
        "name = \"leaf\"\nversion = \"2.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"11\"",
    );
    assert_ne!(old, new);
    assert_eq!(
        with_changed_dependencies(&old, &new, &members()).unwrap(),
        ["tool"]
    );
}

#[test]
fn a_dependency_the_lock_does_not_have_is_an_error() {
    let broken = [
        HEADER.to_string(),
        package("app", "0.1.0", false, &["gone"]),
    ]
    .concat();
    assert!(with_changed_dependencies(&broken, &broken, &members()).is_err());
}

#[test]
fn an_entry_without_a_version_is_an_error() {
    let broken = [
        HEADER.to_string(),
        "\n[[package]]\nname = \"app\"\n".to_string(),
    ]
    .concat();
    let Err(error) = with_changed_dependencies(&broken, &broken, &members()) else {
        panic!("a lock entry without a version reads");
    };
    let text = error.text();
    assert!(text.contains("app without a version"), "{text}");
}
