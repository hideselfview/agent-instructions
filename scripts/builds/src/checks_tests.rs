use super::*;
use crate::testing::{Scratch, git};

/// The scratch folder's machine, which the checks run on.
macro_rules! machine {
    ($scratch:expr) => {
        Machine {
            places: &$scratch.places(),
            system: &$scratch.system(),
        }
    };
}

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
    assert_eq!(
        lines[0]
            .paths
            .iter()
            .map(|p| (p.glob.as_str(), p.every_crate))
            .collect::<Vec<_>>(),
        [("*.rs", false), ("*.toml", false)]
    );
    assert_eq!(lines[0].command, ["cargo", "fmt", "--check"]);
    assert_eq!(lines[1].kind, Kind::Fix);
    assert_eq!(lines[2].kind, Kind::Test);
    for (bad, said) in [
        ("lint a *.rs x", "not check, whole, fix or test"),
        ("whole a *.rs x {files}", "reads the whole tree"),
        ("whole a *.rs cargo c {crates}", "reads the whole tree"),
        ("check a *.rs", "names no command"),
        ("check a", "needs a kind"),
        ("check a *.rs x\ncheck a *.rs y", "named twice"),
        ("check a *.rs,every: cargo c {crates}", "no glob after it"),
        ("check a every:tools/** x {files}", "names no {crates}"),
    ] {
        let fail = parse(bad).unwrap_err().text();
        assert!(fail.contains(said), "{bad:?} said {fail:?}");
    }
}

#[test]
fn a_whole_line_is_a_check_the_landing_marks() {
    let lines = parse("whole a *.py x\ncheck b *.py y\n").unwrap();
    assert_eq!((lines[0].kind, lines[0].whole), (Kind::Check, true));
    assert_eq!((lines[1].kind, lines[1].whole), (Kind::Check, false));
}

fn workspace(scratch: &Scratch) -> std::path::PathBuf {
    let top = scratch.dir.join("ws");
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "-q", "-b", "main"]);
    std::fs::write(
        top.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
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

fn crates_argv(scratch: &Scratch, top: &Path, staged: &[&str]) -> Vec<String> {
    let lines =
        parse("check c *.rs,Cargo.toml,clippy.toml,.clippy.toml,Cargo.lock,rust-toolchain.toml,rust-toolchain,.cargo/** cargo clippy {crates} -- -D warnings\n").unwrap();
    let planned = plan(
        machine!(scratch),
        top,
        &lines,
        &[Kind::Check],
        &files(staged),
        false,
        Some("HEAD"),
    )
    .unwrap();
    planned
        .into_iter()
        .next()
        .map(|p| p.argv)
        .unwrap_or_default()
}

#[test]
fn crates_expand_to_the_crates_holding_a_staged_file() {
    let scratch = Scratch::new("checks-crates");
    let top = workspace(&scratch);
    assert_eq!(
        crates_argv(&scratch, &top, &["crates/a/src/lib.rs"]),
        files(&["cargo", "clippy", "-p", "a", "--", "-D", "warnings"])
    );
    assert_eq!(
        crates_argv(
            &scratch,
            &top,
            &["crates/b/src/lib.rs", "crates/a/src/lib.rs"]
        ),
        files(&[
            "cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings"
        ])
    );
}

/// A lock of the workspace where crate a depends on the registry crate dep
/// at <version>, and b on nothing.
fn lock_with_dep(version: &str) -> String {
    format!(
        r#"version = 4

[[package]]
name = "a"
version = "0.1.0"
dependencies = [
 "dep",
]

[[package]]
name = "b"
version = "0.1.0"

[[package]]
name = "dep"
version = "{version}"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{version}0"
"#
    )
}

/// A staged lock hands `{crates}` only the crates whose resolved
/// dependencies changed since HEAD.
#[test]
fn a_staged_lock_counts_as_the_crates_whose_dependencies_changed() {
    let scratch = Scratch::new("checks-lock");
    let top = workspace(&scratch);
    std::fs::write(top.join("Cargo.lock"), lock_with_dep("1.0.0")).unwrap();
    git(&top, &["add", "-A"]);
    git(
        &top,
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "Start",
        ],
    );
    std::fs::write(top.join("Cargo.lock"), lock_with_dep("1.0.1")).unwrap();
    assert_eq!(
        crates_argv(&scratch, &top, &["Cargo.lock"]),
        files(&["cargo", "clippy", "-p", "a", "--", "-D", "warnings"])
    );
    // When the lock matches HEAD, no crate's dependencies change, so plan
    // leaves the line out.
    std::fs::write(top.join("Cargo.lock"), lock_with_dep("1.0.0")).unwrap();
    assert!(crates_argv(&scratch, &top, &["Cargo.lock"]).is_empty());
    // The root manifest still counts as every crate.
    assert_eq!(
        crates_argv(&scratch, &top, &["Cargo.toml"]),
        files(&[
            "cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings"
        ])
    );
}

#[test]
fn a_root_cargo_toml_clippy_toml_or_lock_counts_as_every_crate() {
    let scratch = Scratch::new("checks-root");
    let top = workspace(&scratch);
    let every = files(&[
        "cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings",
    ]);
    assert_eq!(crates_argv(&scratch, &top, &["Cargo.toml"]), every);
    assert_eq!(crates_argv(&scratch, &top, &["Cargo.lock"]), every);
    std::fs::write(top.join("clippy.toml"), "").unwrap();
    std::fs::write(top.join(".clippy.toml"), "").unwrap();
    assert_eq!(crates_argv(&scratch, &top, &["clippy.toml"]), every);
    assert_eq!(crates_argv(&scratch, &top, &[".clippy.toml"]), every);
    // A crate's own manifest counts as that crate only.
    assert_eq!(
        crates_argv(&scratch, &top, &["crates/b/Cargo.toml"]),
        files(&["cargo", "clippy", "-p", "b", "--", "-D", "warnings"])
    );
}

#[test]
fn the_root_toolchain_file_or_cargo_config_counts_as_every_crate() {
    let scratch = Scratch::new("checks-toolchain");
    let top = workspace(&scratch);
    std::fs::create_dir_all(top.join(".cargo")).unwrap();
    let every = files(&[
        "cargo", "clippy", "-p", "a", "-p", "b", "--", "-D", "warnings",
    ]);
    for config in [
        "rust-toolchain.toml",
        "rust-toolchain",
        ".cargo/config.toml",
        ".cargo/config",
    ] {
        // A toolchain file that names no channel, and a config that sets
        // nothing.
        let text = match config.starts_with("rust-toolchain") {
            true => "[toolchain]\n",
            false => "",
        };
        std::fs::write(top.join(config), text).unwrap();
        assert_eq!(crates_argv(&scratch, &top, &[config]), every, "{config}");
    }
}

#[test]
fn planning_installs_the_pinned_toolchain_on_the_machine_it_is_given() {
    let scratch = Scratch::new("checks-install");
    let top = workspace(&scratch);
    std::fs::write(top.join("rust-toolchain"), "1.100.1\n").unwrap();
    scratch.stand_in("rustup", "echo 'no network' >&2; exit 1");
    let lines = parse("check c *.md true\n").unwrap();
    let error = plan(
        machine!(scratch),
        &top,
        &lines,
        &[Kind::Check],
        &files(&["a.md"]),
        false,
        None,
    )
    .unwrap_err()
    .text();
    assert!(error.contains("no network"), "{error}");
}

#[test]
fn a_file_an_every_crate_glob_matches_counts_as_every_crate() {
    let scratch = Scratch::new("checks-every-crate");
    let top = workspace(&scratch);
    let driver = top.join("tools/lints/src/lib.rs");
    std::fs::create_dir_all(driver.parent().unwrap()).unwrap();
    std::fs::write(driver, "").unwrap();
    let lines = parse(
        "check c *.rs,every:tools/lints/** lint {crates}\ncheck d *.rs,every:crates/a/** doc {crates}\n",
    )
    .unwrap();
    let argv = |staged: &[&str]| -> Vec<Vec<String>> {
        plan(
            machine!(scratch),
            &top,
            &lines,
            &[Kind::Check],
            &files(staged),
            false,
            None,
        )
        .unwrap()
        .into_iter()
        .map(|p| p.argv)
        .collect()
    };
    // The lint driver's source lies in no crate, and hands line c every
    // crate; line d reads it as a plain `*.rs` that no crate holds.
    assert_eq!(
        argv(&["tools/lints/src/lib.rs"]),
        [files(&["lint", "-p", "a", "-p", "b"])]
    );
    // A crate's file that a line marks counts as every crate for that line
    // alone.
    assert_eq!(
        argv(&["crates/a/src/lib.rs"]),
        [
            files(&["lint", "-p", "a"]),
            files(&["doc", "-p", "a", "-p", "b"])
        ]
    );
}

#[test]
fn a_line_with_nothing_to_hand_its_placeholder_is_left_out() {
    let scratch = Scratch::new("checks-nothing");
    let top = workspace(&scratch);
    // Not in any crate.
    assert!(crates_argv(&scratch, &top, &["tools/x.rs"]).is_empty());
    // Matches, but is not on disk.
    let lines = parse("check f *.rs tool {files}\n").unwrap();
    assert!(
        plan(
            machine!(scratch),
            &top,
            &lines,
            &[Kind::Check],
            &files(&["gone.rs"]),
            false,
            None
        )
        .unwrap()
        .is_empty()
    );
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
    let planned = plan(
        machine!(scratch),
        &top,
        &lines,
        &[Kind::Check],
        &files(&["b.sh", "a.sh", "c.md"]),
        false,
        None,
    )
    .unwrap();
    assert_eq!(planned[0].argv, files(&["lint", "--x", "b.sh", "a.sh"]));
    assert_eq!(planned[1].argv, files(&["./gate.sh"]));
    let only_md = plan(
        machine!(scratch),
        &top,
        &lines,
        &[Kind::Check],
        &files(&["c.md"]),
        false,
        None,
    )
    .unwrap();
    assert_eq!(only_md.len(), 1);
    assert_eq!(only_md[0].line.name, "gate");
}
