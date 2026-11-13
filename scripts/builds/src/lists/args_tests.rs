use super::*;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn flags_values_switches_and_positionals_parse() {
    let args = Args::parse(&words("12 --title T --json --wait a --wait b --by me"), &["title", "wait"], &["json"]).unwrap();
    assert_eq!(args.positional(), ["12"]);
    assert_eq!(args.value("title").unwrap(), Some("T"));
    assert_eq!(args.values("wait"), ["a", "b"]);
    assert!(args.switch("json") && !args.switch("title"));
    assert_eq!(args.value("by").unwrap(), Some("me"));
    assert_eq!(args.value("project").unwrap(), None);
}

#[test]
fn a_flag_the_command_lacks_is_refused_by_name() {
    let error = format!("{:?}", Args::parse(&words("--nope 1"), &["title"], &[]).unwrap_err());
    assert!(error.contains("--nope") && error.contains("--title"), "{error}");
}

#[test]
fn a_missing_or_repeated_value_is_refused() {
    assert!(Args::parse(&words("--title"), &["title"], &[]).is_err());
    let args = Args::parse(&words("--title a --title b"), &["title"], &[]).unwrap();
    assert!(args.value("title").is_err());
    assert!(args.require("why").is_err());
}

#[test]
fn double_dash_ends_the_flags_and_a_value_may_start_with_dashes() {
    let args = Args::parse(&words("--title --x -- --json"), &["title"], &["json"]).unwrap();
    assert_eq!(args.value("title").unwrap(), Some("--x"));
    assert_eq!(args.positional(), ["--json"]);
}

#[test]
fn ids_come_as_bare_numbers() {
    let args = Args::parse(&words("--blocks 3,5 --from-item 9"), &["blocks", "from-item"], &[]).unwrap();
    assert_eq!(args.ids("blocks").unwrap(), Some(vec![Id(3), Id(5)]));
    assert_eq!(args.id("from-item").unwrap(), Some(Id(9)));
    assert_eq!(args.id("missing").unwrap(), None);
}

#[test]
fn exactly_counts_positionals() {
    let args = Args::parse(&words("a b"), &[], &[]).unwrap();
    assert!(args.exactly(2, "u").is_ok());
    assert!(args.exactly(1, "u").is_err());
}
