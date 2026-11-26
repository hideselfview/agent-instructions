use super::*;

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn split_line(command: &str, line: &str) -> (Option<String>, String) {
    let split = split(command, &words(line)).unwrap();
    (split.project, split.args.join(" "))
}

fn of(project: &str, args: &str) -> (Option<String>, String) {
    (Some(project.to_string()), args.to_string())
}

#[test]
fn the_flag_leaves_the_arguments_of_a_command_that_does_not_read_it() {
    assert_eq!(
        split_line("queue", "demo --project two --lane mac"),
        of("two", "demo --lane mac")
    );
    assert_eq!(
        split_line("app-build", "demo --list --project two"),
        of("two", "demo --list")
    );
}

#[test]
fn a_command_that_reads_the_flag_keeps_it() {
    for (command, line) in [
        ("item", "show 3 --project two"),
        ("app-build", "demo main --project two"),
        ("shots", "picture demo main mac.a.one shot --project two"),
        ("checks", "stats --project two"),
    ] {
        assert_eq!(split_line(command, line), of("two", line));
    }
}

#[test]
fn the_words_of_a_command_that_builds_runs_stay_as_they_are() {
    assert_eq!(
        split_line("lock", "demo --lane mac --project two run --project theirs"),
        of("two", "demo --lane mac run --project theirs")
    );
    assert_eq!(
        split_line("lock", "demo --lane mac run --project theirs"),
        (None, "demo --lane mac run --project theirs".to_string())
    );
    assert_eq!(
        split_line("remote", "run demo main -- run --project theirs"),
        (None, "run demo main -- run --project theirs".to_string())
    );
}

#[test]
fn the_check_reads_no_flag_after_the_end_of_a_commands_own_words() {
    assert_eq!(
        split_line("item", "show --project p -- --project x").0,
        Some("p".to_string())
    );
}

#[test]
fn a_flag_with_no_name_or_given_twice_is_refused() {
    for line in ["demo --project", "demo --project a --project b"] {
        assert!(split("queue", &words(line)).is_err(), "{line}");
    }
}
