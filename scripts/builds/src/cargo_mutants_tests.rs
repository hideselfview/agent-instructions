use super::*;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn band(low: f64, high: f64) -> Band {
    Band { low, high }
}

#[test]
fn the_cap_is_the_room_under_the_bands_high_end_and_at_least_one() {
    assert_eq!(jobs_cap(10.0, band(20.0, 30.0)), 20);
    assert_eq!(jobs_cap(10.5, band(20.0, 30.0)), 19);
    assert_eq!(jobs_cap(29.5, band(20.0, 30.0)), 1);
    assert_eq!(
        jobs_cap(45.0, band(20.0, 30.0)),
        1,
        "a host over its band still runs one job"
    );
}

#[test]
fn a_run_asking_for_more_jobs_than_the_cap_gets_the_cap_and_a_line_that_says_so() {
    for (given, want) in [
        (
            &["--jobs", "8", "--file", "src/a.rs"][..],
            &["--jobs", "3", "--file", "src/a.rs"][..],
        ),
        (&["-j", "8"][..], &["-j", "3"][..]),
        (&["--jobs=8"][..], &["--jobs=3"][..]),
        (&["-j8", "--list"][..], &["-j3", "--list"][..]),
    ] {
        let (out, note) = capped(&args(given), 3);
        assert_eq!(out, args(want), "{given:?}");
        assert!(
            note.unwrap().contains("runs 3 jobs, not the 8"),
            "{given:?}"
        );
    }
}

#[test]
fn jobs_within_the_cap_and_a_run_with_none_stay_as_given() {
    for given in [
        &["--jobs", "2"][..],
        &["-j2"][..],
        &["--file", "src/a.rs"][..],
        &[][..],
    ] {
        let (out, note) = capped(&args(given), 3);
        assert_eq!(out, args(given));
        assert_eq!(note, None);
    }
}

#[test]
fn a_jobs_word_after_the_double_dash_belongs_to_cargo_test() {
    let given = args(&["--file", "src/a.rs", "--", "-j", "9"]);
    let (out, note) = capped(&given, 3);
    assert_eq!((out, note), (given, None));
}

#[test]
fn a_jobs_value_that_is_no_number_goes_on_for_cargo_mutants_to_refuse() {
    let given = args(&["--jobs", "many"]);
    assert_eq!(capped(&given, 3), (given, None));
}

#[test]
fn cargo_settings_become_cargo_arg_words() {
    assert_eq!(
        cargo_args(["--locked", "--config", "build.incremental=false"]),
        args(&[
            "--cargo-arg=--locked",
            "--cargo-arg=--config",
            "--cargo-arg=build.incremental=false"
        ])
    );
}
