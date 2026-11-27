use super::*;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

use crate::command::{decision, item, question};

fn item_add() -> Command {
    Command::Item(item::Add)
}

#[test]
fn flags_values_switches_and_positionals_parse() {
    let args = Args::of(
        Command::Item(item::List),
        &words("--area a --running --tag t --project p"),
    )
    .unwrap();
    assert!(args.positional().is_empty());
    assert_eq!(args.value("area").unwrap(), Some("a"));
    assert!(args.switch("running") && !args.switch("ready"));
    assert_eq!(args.value("project").unwrap(), Some("p"));
    assert_eq!(args.value("for").unwrap(), None);
    let added = Args::of(
        item_add(),
        &words("--title T --area A --details-file F --wait a --wait b"),
    )
    .unwrap();
    assert_eq!(added.values("wait"), ["a", "b"]);
}

#[test]
fn a_flag_the_command_lacks_is_refused_by_name() {
    let error = Args::of(Command::Item(item::Show), &words("3 --nope 1"))
        .unwrap_err()
        .text();
    assert!(
        error.starts_with("builds item show has no flag --nope. Its flags are --project.\n"),
        "{error}"
    );
}

#[test]
fn a_missing_or_repeated_value_is_refused() {
    let missing = Args::of(item_add(), &words("--title")).unwrap_err().text();
    assert!(missing.starts_with("--title needs a value\n"), "{missing}");
    let twice = Args::of(item_add(), &words("--title a --title b"))
        .unwrap_err()
        .text();
    assert!(twice.starts_with("--title is given twice\n"), "{twice}");
    let none = Args::of(Command::Item(item::Stop), &words("3"))
        .unwrap_err()
        .text();
    assert!(none.starts_with("--why is required\n"), "{none}");
}

#[test]
fn double_dash_ends_the_flags_and_a_value_may_start_with_dashes() {
    let args = Args::of(
        Command::Decision(decision::Find),
        &words("--project --x -- --json"),
    )
    .unwrap();
    assert_eq!(args.value("project").unwrap(), Some("--x"));
    assert_eq!(args.positional(), ["--json"]);
}

#[test]
fn ids_come_as_bare_numbers() {
    let args = Args::of(
        Command::Question(question::Ask),
        &words("--title T --kind question --effect E --details-file F --blocks 3,5 --from-item 9"),
    )
    .unwrap();
    assert_eq!(args.ids("blocks").unwrap(), Some(vec![Id(3), Id(5)]));
    assert_eq!(args.id("from-item").unwrap(), Some(Id(9)));
    assert_eq!(args.id("origin").unwrap(), None);
}

#[test]
fn exactly_counts_positionals() {
    let args = Args::of(Command::Area(crate::command::area::Rename), &words("a b")).unwrap();
    assert!(args.exactly(2).is_ok());
    assert!(args.exactly(1).is_err());
}

#[test]
fn a_choice_is_a_variant_by_its_serde_spelling() {
    let args = Args::of(Command::Item(item::List), &words("--for design-session")).unwrap();
    assert_eq!(
        args.choice::<crate::lists::Audience>("for").unwrap(),
        Some(crate::lists::Audience::DesignSession)
    );
    assert_eq!(args.choice::<crate::lists::Audience>("area").unwrap(), None);
    let sideways = Args::of(Command::Item(item::List), &words("--for sideways")).unwrap();
    let crate::fail::Fail::Message(message) = sideways
        .choice::<crate::lists::Audience>("for")
        .unwrap_err()
    else {
        panic!()
    };
    assert!(
        message.starts_with("--for: unknown variant `sideways`"),
        "{message}"
    );
}

#[test]
fn every_usage_line_reads_as_a_form_and_its_example_fits_the_command() {
    for &command in Command::ALL {
        for line in command.usage() {
            let form = Form::of(after_words(command, line))
                .unwrap_or_else(|error| panic!("{line}: {}", error.text()));
            let example = form.example();
            if let Err(error) = Args::of(command, &example) {
                panic!("{line}: {example:?} does not fit: {}", error.text());
            }
        }
    }
}

#[test]
fn every_command_refuses_an_unknown_flag_the_same_way() {
    for &command in Command::ALL {
        let words = vec!["--no-such-flag".to_string()];
        let refusal = Args::of(command, &words).unwrap_err().text();
        let named = format!("builds {}", command.words()[0].join(" "));
        assert!(
            refusal.starts_with(&format!(
                "{named} has no flag --no-such-flag. Its flags are "
            )),
            "{refusal}"
        );
        assert!(refusal.ends_with(&command.usage_text()), "{refusal}");
    }
}
