use super::*;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

#[test]
fn every_form_of_the_package_flag_names_its_crate_once() {
    assert_eq!(
        named(&args(&[
            "test",
            "-p",
            "builds",
            "--package",
            "boards",
            "--package=shots",
            "-pbuilds",
            "--features",
            "x"
        ])),
        ["builds", "boards", "shots"]
    );
}

#[test]
fn a_run_naming_no_crate_and_an_argument_after_the_double_dash_name_none() {
    assert!(named(&args(&["check", "--workspace", "--all-targets"])).is_empty());
    assert!(named(&args(&["nextest", "run", "--", "-p", "x"])).is_empty());
    assert!(named(&args(&["build", "--profile", "dev", "-p"])).is_empty());
}
