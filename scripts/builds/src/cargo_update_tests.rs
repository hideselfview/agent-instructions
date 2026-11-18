use super::*;
use crate::testing::Scratch;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn an_update_that_names_a_package_or_the_workspace_runs() {
    for words in [
        &["update", "-p", "coven"][..],
        &["update", "-pcoven"],
        &["update", "-p=coven"],
        &["update", "--package", "coven"],
        &["update", "--package=coven"],
        &["update", "coven"],
        &["update", "coven", "uniffi"],
        &["update", "--precise", "21abf28c6", "coven"],
        &["update", "--manifest-path", "x/Cargo.toml", "-p", "coven"],
        &["update", "-w"],
        &["update", "--workspace"],
        &["update", "--", "coven"],
    ] {
        assert_eq!(refusal(&args(words)), None, "{words:?}");
    }
}

#[test]
fn an_update_that_names_no_package_is_refused() {
    for words in [
        &["update"][..],
        &["update", "--dry-run"],
        &["update", "--recursive", "-v"],
        &["update", "--precise", "21abf28c6"],
        &["update", "--manifest-path", "x/Cargo.toml"],
        &["update", "--config", "net.git-fetch-with-cli=true"],
        &["update", "--config=net.git-fetch-with-cli=true"],
        &["update", "--color", "never", "-Z", "unstable-options"],
        &["update", "--lockfile-path", "x/Cargo.lock"],
    ] {
        let said = refusal(&args(words)).unwrap_or_else(|| panic!("{words:?} runs"));
        assert!(said.contains("every dependency"), "{said}");
    }
}

/// Writes a workspace whose crate `app` depends on the path crate `dep` at
/// <version>, with the given requirement.
fn workspace(top: &Path, version: &str) {
    std::fs::create_dir_all(top.join("app/src")).unwrap();
    std::fs::create_dir_all(top.join("dep/src")).unwrap();
    std::fs::write(
        top.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"dep\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        top.join("app/Cargo.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = {{ path = \"../dep\", version = \"{version}\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(top.join("app/src/lib.rs"), "").unwrap();
    std::fs::write(
        top.join("dep/Cargo.toml"),
        format!("[package]\nname = \"dep\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    std::fs::write(top.join("dep/src/lib.rs"), "").unwrap();
}

#[test]
fn an_update_of_a_named_package_rewrites_its_entry_in_the_lock() {
    let scratch = Scratch::new("cargo-update");
    let top = scratch.dir.join("ws");
    workspace(&top, "0.1.0");
    let status = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--quiet"])
        .current_dir(&top)
        .status()
        .unwrap();
    assert!(status.success());
    workspace(&top, "0.2.0");
    run_in(&top, &args(&["update", "-p", "dep"]), None).unwrap();
    let lock = std::fs::read_to_string(top.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("name = \"dep\"\nversion = \"0.2.0\""),
        "the lock moved to the new version without --locked: {lock}"
    );
}
