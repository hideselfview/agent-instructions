use super::*;
use crate::testing::{Scratch, git};

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn a_glob_without_a_slash_matches_the_file_name_anywhere() {
    assert!(glob_match("*.rs", "lib.rs"));
    assert!(glob_match("*.rs", "crates/a/src/lib.rs"));
    assert!(!glob_match("*.rs", "crates/a/src/lib.rsx"));
    assert!(glob_match("Cargo.toml", "crates/a/Cargo.toml"));
    assert!(!glob_match("Cargo.toml", "crates/a/Cargo.toml.bak"));
}

#[test]
fn a_star_stays_inside_one_segment() {
    assert!(glob_match("scripts/*.sh", "scripts/a.sh"));
    assert!(!glob_match("scripts/*.sh", "scripts/hooks/a.sh"));
    assert!(glob_match("scripts/a*c.sh", "scripts/abbc.sh"));
    assert!(!glob_match("scripts/a*c.sh", "scripts/abbd.sh"));
    assert!(glob_match("scripts/a.sh", "scripts/a.sh"));
    assert!(!glob_match("scripts/a.sh", "other/scripts/a.sh"));
}

#[test]
fn a_double_star_is_any_number_of_segments() {
    assert!(glob_match("src/**/*.rs", "src/a.rs"));
    assert!(glob_match("src/**/*.rs", "src/x/y/a.rs"));
    assert!(!glob_match("src/**/*.rs", "lib/a.rs"));
    assert!(glob_match("**/gen.sh", "gen.sh"));
    assert!(glob_match("**/gen.sh", "a/b/gen.sh"));
    assert!(glob_match("design/**", "design/mocks/one.png"));
    assert!(!glob_match("design/**", "spec/one.png"));
}

#[test]
fn lines_parse_and_mistakes_name_the_line() {
    let text = "# comment\n\ncheck fmt *.rs,*.toml cargo fmt --check\nfix swift *.swift swift-format {files}\ntest t a/b.sh scripts/t.sh\n";
    let lines = parse(text).unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].paths, ["*.rs", "*.toml"]);
    assert_eq!(lines[0].command, ["cargo", "fmt", "--check"]);
    assert_eq!(lines[1].kind, Kind::Fix);
    assert_eq!(lines[2].kind, Kind::Test);
    for (bad, said) in [
        ("lint a *.rs x", "not check, fix or test"),
        ("check a *.rs", "names no command"),
        ("check a", "needs a kind"),
        ("check a *.rs x\ncheck a *.rs y", "named twice"),
    ] {
        let fail = parse(bad).unwrap_err().text();
        assert!(fail.contains(said), "{bad:?} said {fail:?}");
    }
}

fn workspace(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("ws");
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "-q", "-b", "main"]);
    std::fs::write(top.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n").unwrap();
    for name in ["a", "b"] {
        let dir = top.join("crates").join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
    }
    std::fs::write(top.join("Cargo.lock"), "").unwrap();
    top
}

fn crates_argv(top: &Path, staged: &[&str]) -> Vec<String> {
    let lines = parse("check c *.rs,Cargo.toml,Cargo.lock cargo clippy {crates} -- -D warnings\n").unwrap();
    let planned = plan(top, &lines, &[Kind::Check], &files(staged), false).unwrap();
    planned.into_iter().next().map(|p| p.argv).unwrap_or_default()
}

#[test]
fn crates_expand_to_the_crates_holding_a_staged_file() {
    let scratch = Scratch::new("checks-crates");
    let top = workspace(&scratch);
    assert_eq!(crates_argv(&top, &["crates/a/src/lib.rs"]), files(&["cargo", "clippy", "-p", "a", "--", "-D", "warnings"]));
    assert_eq!(
        crates_argv(&top, &["crates/b/src/lib.rs", "crates/a/src/lib.rs"]),
        files(&["cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings"])
    );
}

#[test]
fn a_root_cargo_toml_or_lock_counts_as_every_crate() {
    let scratch = Scratch::new("checks-root");
    let top = workspace(&scratch);
    let every = files(&["cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings"]);
    assert_eq!(crates_argv(&top, &["Cargo.toml"]), every);
    assert_eq!(crates_argv(&top, &["Cargo.lock"]), every);
    // A crate's own manifest counts as that crate only.
    assert_eq!(crates_argv(&top, &["crates/b/Cargo.toml"]), files(&["cargo", "clippy", "-p", "b", "--", "-D", "warnings"]));
}

#[test]
fn a_line_with_nothing_to_hand_its_placeholder_is_left_out() {
    let scratch = Scratch::new("checks-nothing");
    let top = workspace(&scratch);
    // Not in any crate.
    assert!(crates_argv(&top, &["tools/x.rs"]).is_empty());
    // Matches, but is not on disk.
    let lines = parse("check f *.rs tool {files}\n").unwrap();
    assert!(plan(&top, &lines, &[Kind::Check], &files(&["gone.rs"]), false).unwrap().is_empty());
}

#[test]
fn files_expand_to_the_matching_files_in_order_and_gates_run_whole() {
    let scratch = Scratch::new("checks-files");
    let top = scratch.dir.join("r");
    std::fs::create_dir_all(&top).unwrap();
    for f in ["b.sh", "a.sh", "c.md"] {
        std::fs::write(top.join(f), "").unwrap();
    }
    let lines = parse("check sh *.sh lint --x {files}\ncheck gate *.md ./gate.sh\n").unwrap();
    let planned = plan(&top, &lines, &[Kind::Check], &files(&["b.sh", "a.sh", "c.md"]), false).unwrap();
    assert_eq!(planned[0].argv, files(&["lint", "--x", "b.sh", "a.sh"]));
    assert_eq!(planned[1].argv, files(&["./gate.sh"]));
    let only_md = plan(&top, &lines, &[Kind::Check], &files(&["c.md"]), false).unwrap();
    assert_eq!(only_md.len(), 1);
    assert_eq!(only_md[0].line.name, "gate");
}
