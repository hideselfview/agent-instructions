use super::*;
use crate::install_gate::Stores;
use crate::orphans::Orphaned;

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// The command that <line> names.
fn command(line: &str) -> Command {
    Command::parse(&words(line)).unwrap().command
}

/// Each command's words name it, and each of its usage lines starts with
/// them, so the parser and help agree on every command.
#[test]
fn every_command_is_named_by_its_words_and_its_usage_lines_start_with_them() {
    for command in Command::ALL {
        assert!(!command.usage().is_empty(), "{command:?} has no usage line");
        for named in command.words() {
            let parsed = Command::of(named).unwrap();
            assert_eq!(parsed.command, *command, "{named:?}");
            assert!(parsed.rest.is_empty());
        }
        for line in command.usage() {
            let usage = words(line);
            let rest = usage.get(1..).unwrap_or_default();
            assert!(
                usage.first().map(String::as_str) == Some("builds")
                    && command.words().iter().any(|named| {
                        named
                            .iter()
                            .zip(rest)
                            .all(|(name, word)| word == name || word == &format!("[{name}]"))
                    }),
                "{command:?}: {line}"
            );
        }
    }
}

/// Every command answers each question builds asks about a command, and the
/// answers agree: a command that makes a run folder keeps the Mac awake, a
/// command that takes no use of the stores needs no acknowledgement, and the
/// commands that only print are one way.
/// The arguments of `builds <words>`.
fn args_of(words: &[String]) -> crate::args::Args {
    let parsed = Command::parse(words).unwrap();
    crate::args::Args::of(parsed.command, parsed.rest).unwrap()
}

#[test]
fn every_command_answers_each_property_and_the_answers_agree() {
    for command in Command::ALL {
        let parsed = crate::args::example(*command);
        let stores = command.stores();
        if command.makes_run_folder() {
            assert!(command.keeps_awake(), "{command:?}");
        }
        if stores == Stores::Storeless {
            assert!(!parsed.needs_acknowledgement().unwrap(), "{command:?}");
        }
        let _ = (
            command.guards_claimed_branch(),
            command.runs_steps(),
            command.orphaned(),
            parsed.reads_project(),
        );
    }
}

#[test]
fn the_longest_words_that_start_a_line_name_its_command() {
    for (line, expected, rest) in [
        (
            "review /w --kind code",
            Command::Review(review::Run),
            "/w --kind code",
        ),
        ("review run /w", Command::Review(review::Run), "/w"),
        (
            "review case add 3 4",
            Command::Review(review::CaseAdd),
            "3 4",
        ),
        ("shots run mac", Command::Shots(shots::Checker), "run mac"),
        (
            "shots findings bae main",
            Command::Shots(shots::Findings),
            "bae main",
        ),
        (
            "dashboard --port 0",
            Command::Dashboard(dashboard::Serve),
            "--port 0",
        ),
        (
            "dashboard stage --fixtures f s",
            Command::Dashboard(dashboard::Stage),
            "--fixtures f s",
        ),
        ("-h", Command::Help, ""),
        ("help workflow", Command::Help, "workflow"),
        (
            "design publish take 3",
            Command::Design(design::PublishTake),
            "3",
        ),
    ] {
        let words = words(line);
        let parsed = Command::parse(&words).unwrap();
        assert_eq!(parsed.command, expected, "{line}");
        assert_eq!(parsed.rest.join(" "), rest, "{line}");
    }
}

#[test]
fn a_line_that_names_no_command_is_refused_with_the_commands_or_the_forms() {
    let said = Command::parse(&words("frobnicate")).unwrap_err().text();
    assert!(
        said.starts_with(
            "builds has no command frobnicate. Its commands are help, ack, rules-on-edit,"
        ),
        "{said}"
    );
    assert!(said.ends_with(" and lists."), "{said}");
    let said = Command::parse(&words("item frobnicate 3"))
        .unwrap_err()
        .text();
    assert!(
        said.starts_with(
            "builds item has no command frobnicate. Its forms are:\n  builds item add "
        ),
        "{said}"
    );
    let said = Command::parse(&words("host")).unwrap_err().text();
    assert!(said.starts_with("builds host needs a command."), "{said}");
    assert!(Command::of(&["frobnicate"]).is_none());
    assert!(Command::of::<&str>(&[]).is_none());
}

/// The commands that write a store after long work hold a use for their
/// life. `review key` only prints a key, so it takes a use per connection.
#[test]
fn only_the_hook_run_and_the_reviews_that_ask_reviewers_hold_a_use_for_their_life() {
    let for_life: Vec<Command> = Command::ALL
        .iter()
        .copied()
        .filter(|command| command.stores() == Stores::ForLife)
        .collect();
    assert_eq!(
        for_life,
        vec![
            Command::Checks(checks::Run),
            Command::Review(review::Run),
            Command::Review(review::Copy),
            Command::Review(review::Replay),
            Command::Review(review::Audit),
        ]
    );
    assert_eq!(
        command("review key /w --project p").stores(),
        Stores::PerConnection
    );
    assert_eq!(command("agent start").stores(), Stores::Storeless);
    assert_eq!(command("agent list").stores(), Stores::PerConnection);
    assert_eq!(command("shots run").stores(), Stores::Storeless);
    assert_eq!(
        command("shots findings a b c d").stores(),
        Stores::PerConnection
    );
}

#[test]
fn the_commands_that_run_work_keep_this_mac_awake_and_the_rest_do_not() {
    for line in [
        "agent run --role implementing 1",
        "agent resume abc",
        "cargo test",
        "lake build",
        "app-build bae work",
        "build-app bae",
        "lock bae --lane mac true",
        "remote run bae main -- true",
        "remote step m1 /w -- true",
        "review /w --kind code",
        "review run /w",
        "review copy /w",
        "land /w",
        "health",
        "checks run",
        "item take 1 --repo bae --branch b",
        "new",
        "setup",
        "app-checkout mac /w",
        "app-clone mac /a /b",
        "prune",
        "install",
        "host setup-here",
        "shots run",
    ] {
        assert!(command(line).keeps_awake(), "{line}");
    }
    for line in [
        "agent list",
        "agent start",
        "review wait 3",
        "review settle 4 confirmed",
        "review show 3",
        "review key /w",
        "judge listen",
        "dashboard",
        "status",
        "item show 1",
        "next",
        "queue demo",
        "checks list",
        "shots findings",
        "ack",
    ] {
        assert!(!command(line).keeps_awake(), "{line}");
    }
}

#[test]
fn the_commands_that_start_build_work_make_a_run_folder() {
    for line in [
        "cargo",
        "lake build",
        "app-build",
        "build-app",
        "lock",
        "remote run",
        "remote step",
        "shots run",
    ] {
        assert!(command(line).makes_run_folder(), "{line}");
    }
    for line in [
        "health",
        "item list",
        "shots findings",
        "shots check-program",
        "shots picture",
    ] {
        assert!(!command(line).makes_run_folder(), "{line}");
    }
}

#[test]
fn a_command_reads_the_project_flag_by_its_words() {
    for (line, reads) in [
        ("item show 3", true),
        ("app-build demo main", true),
        ("app-build demo --list", false),
        ("app-build --landing-coverage /w b r", false),
        ("app-build --record-handed demo s", false),
        ("shots picture demo main s shot", true),
        ("shots run", false),
        ("checks stats", true),
        ("checks run", false),
        ("queue demo", false),
    ] {
        let words = words(line);
        assert_eq!(args_of(&words).reads_project(), reads, "{line}");
    }
}

/// The acknowledgement check passes `ack`, the rules hook, the storeless
/// commands and `status --json --no-folders`, and covers the rest.
#[test]
fn the_acknowledgement_check_covers_every_command_but_those_that_need_none() {
    for (line, needs) in [
        ("ack", false),
        ("rules-on-edit", false),
        ("brief src/a.rs", false),
        ("agent start", false),
        ("install", false),
        ("status --json --no-folders", false),
        ("status --json", true),
        ("status", true),
        ("item list", true),
        ("checks run --since main", true),
        ("du /x", true),
    ] {
        let words = words(line);
        assert_eq!(
            args_of(&words).needs_acknowledgement().unwrap(),
            needs,
            "{line}"
        );
    }
}

/// The orphan check stops the commands that only print, and lets the rest
/// work to their end.
#[test]
fn an_orphaned_command_only_prints_or_works() {
    for (line, orphaned) in [
        ("dashboard", Orphaned::Works),
        ("build-app bae mac", Orphaned::Works),
        ("cargo test", Orphaned::OnlyPrints),
        ("remote run bae main -- true", Orphaned::OnlyPrints),
        ("checks run", Orphaned::OnlyPrints),
        ("checks whole", Orphaned::Works),
        ("land /w", Orphaned::Works),
    ] {
        assert_eq!(command(line).orphaned(), orphaned, "{line}");
    }
}
