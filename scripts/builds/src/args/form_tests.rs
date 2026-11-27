use super::*;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn read(usage: &'static str, text: &str) -> std::result::Result<Read, Misfit> {
    Form::of(usage).unwrap().read(&words(text), &[])
}

fn flag(form: &Form, name: &str) -> (usize, bool) {
    let flag = form.flag(name).unwrap();
    (flag.values, flag.repeats)
}

#[test]
fn a_flag_takes_one_value_at_the_top_level_and_every_value_word_in_its_group() {
    let form = Form::of("<repo> --lane <app> <ref> [--test <lane> <Class>]... [--fresh]").unwrap();
    assert_eq!(flag(&form, "--lane"), (1, false));
    assert_eq!(flag(&form, "--test"), (2, true));
    assert_eq!(flag(&form, "--fresh"), (0, false));
    let read = read(
        "<repo> --lane <app> <ref> [--test <lane> <Class>]... [--fresh]",
        "bae --lane mac main --test mac A --fresh --test mac B",
    )
    .unwrap();
    assert_eq!(read.positional, ["bae", "main"]);
    assert_eq!(read.flags[1], ("--test", words("mac A")));
    assert_eq!(read.flags[3], ("--test", words("mac B")));
}

#[test]
fn a_flag_repeats_when_its_value_repeats_or_the_line_names_it_twice() {
    let form = Form::of("N [--worktree W --branch B [--worktree W2 --branch B2]] [--link URL...]")
        .unwrap();
    assert_eq!(flag(&form, "--worktree"), (1, true));
    assert_eq!(flag(&form, "--branch"), (1, true));
    assert_eq!(flag(&form, "--link"), (1, true));
    assert_eq!(
        read("N [--why WHY]", "3 --why a --why b").unwrap_err(),
        Misfit::Twice("--why")
    );
}

#[test]
fn positional_words_follow_the_pattern() {
    let usage = "N [M...] (first|last | --above M) --why WHY";
    assert!(read(usage, "3 --why w").is_ok());
    assert!(read(usage, "3 4 5 first --why w").is_ok());
    assert_eq!(read(usage, "").unwrap_err(), Misfit::Positional);
    assert!(read("<name> clear", "m1 clear").is_ok());
    assert_eq!(
        read("<name> clear", "m1 off").unwrap_err(),
        Misfit::Positional
    );
    assert!(read("N[,N...] DIR REV...", "3,4 d a b").is_ok());
    assert!(read("<days> <dry 0|1> <folder>...", "1 0 a b").is_ok());
}

#[test]
fn unknown_flags_and_missing_values_misfit() {
    assert_eq!(
        read("N [--why WHY]", "3 --nope").unwrap_err(),
        Misfit::Unknown
    );
    assert_eq!(
        read("N [--why WHY]", "3 --why").unwrap_err(),
        Misfit::NeedsValue("--why")
    );
    // A short flag is a flag; a lone dash and a negative number are words.
    assert!(read("[-b <branch>] [<path>]", "-b main").is_ok());
    assert!(read("[<path>]", "-").is_ok());
    assert!(read("[<path>]", "-1").is_ok());
}

#[test]
fn words_after_double_dash_or_from_args_on_are_positional() {
    let read_lock = read(
        "<repo> --lane <app> <command> [<args>...]",
        "bae --lane mac make --release -j",
    )
    .unwrap();
    assert_eq!(read_lock.positional, words("bae make --release -j"));
    let remote = read(
        "<repo> <ref> [--host <name>] -- <command> [<args>...]",
        "bae main --host m1 -- make --x",
    )
    .unwrap();
    assert_eq!(remote.positional, words("bae main -- make --x"));
    let lists = read("N [<text>...] [--title T]", "3 -- --title").unwrap();
    assert_eq!(lists.positional, words("3 --title"));
}

#[test]
fn a_value_may_start_with_dashes() {
    let read = read("[--title T]", "--title --x").unwrap();
    assert_eq!(read.flags, [("--title", words("--x"))]);
}
