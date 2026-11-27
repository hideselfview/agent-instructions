//! The words of a builds command after the words that name it, read one way
//! for every command: by the command's usage lines (command.rs), each of
//! which is a form (form.rs) that names the flags the command takes and the
//! positional words it takes in order. A flag no form names is refused by
//! name, the same way for every command, never ignored.

mod form;

use std::io::Read;

use serde::de::DeserializeOwned;

use crate::command::Command;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::Id;

pub use form::is_flag;
use form::{Flag, Form, Misfit};

/// The flag every command takes, whether its usage lines name it or not
/// (project_flag.rs).
const GLOBAL: [Flag; 1] = [Flag {
    name: crate::project_flag::PROJECT,
    values: 1,
    repeats: false,
    required: false,
}];

#[derive(Clone, Debug)]
pub struct Args {
    command: Command,
    /// The words as given.
    words: Vec<String>,
    positional: Vec<String>,
    /// Each flag given, in order, with its values.
    flags: Vec<(&'static str, Vec<String>)>,
}

impl Args {
    /// <words>, the words after those that name <command>, read by the first
    /// of the command's forms they fit. Fails naming a flag that no form
    /// takes, else a flag that a form whose other words fit requires, else
    /// with the first form's reason, and the command's usage.
    pub fn of(command: Command, words: &[String]) -> Result<Args> {
        let forms = forms(command)?;
        let mut first_misfit = None;
        for form in &forms {
            match form.read(words, &GLOBAL) {
                Ok(read) => {
                    return Ok(Args {
                        command,
                        words: words.to_vec(),
                        positional: read.positional,
                        flags: read.flags,
                    });
                }
                // A form whose words fit but for a flag they lack says more
                // than a form whose words or flags do not fit.
                Err(misfit @ Misfit::Missing(_)) => {
                    if matches!(
                        first_misfit,
                        None | Some(Misfit::Positional | Misfit::Unknown)
                    ) {
                        first_misfit = Some(misfit);
                    }
                }
                Err(misfit) => {
                    first_misfit.get_or_insert(misfit);
                }
            }
        }
        let named = format!("builds {}", command.words()[0].join(" "));
        let usage = command.usage_text();
        let unknown = words.iter().find(|word| {
            is_flag(word)
                && forms.iter().all(|form| {
                    form.flag(word).is_none() && GLOBAL.iter().all(|flag| flag.name != *word)
                })
        });
        if let Some(word) = unknown {
            let mut known: Vec<&str> = forms
                .iter()
                .flat_map(|form| form.flags.iter().map(|flag| flag.name))
                .chain(GLOBAL.iter().map(|flag| flag.name))
                .collect();
            known.sort_unstable();
            known.dedup();
            fail!(
                "{named} has no flag {word}. Its flags are {}.\n{usage}",
                crate::command::and_list(&known)
            );
        }
        match first_misfit {
            Some(Misfit::NeedsValue(flag)) => fail!("{flag} needs a value\n{usage}"),
            Some(Misfit::Twice(flag)) => fail!("{flag} is given twice\n{usage}"),
            Some(Misfit::Missing(flag)) => fail!("{flag} is required\n{usage}"),
            _ => fail!("{usage}"),
        }
    }

    /// The arguments of <argv>, the command line of a running builds
    /// process, when the process runs <command>, or why its words do not fit
    /// the command, as a builds of another version may have written them;
    /// None for any other command.
    pub fn of_process(command: Command, argv: &[String]) -> Option<Result<Args>> {
        let words = crate::ack::agent::without_flag(argv);
        let parsed = Command::of(words.get(1..)?)?;
        if parsed.command != command {
            return None;
        }
        let rest: Vec<String> = parsed.rest.iter().map(|word| word.to_string()).collect();
        Some(Args::of(command, &rest))
    }

    /// The command whose words these are.
    pub fn command(&self) -> Command {
        self.command
    }

    /// The words as given, which a record of the command names it by.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    pub fn positional(&self) -> &[String] {
        &self.positional
    }

    /// The positional arguments, exactly <count> of them, else a refusal that
    /// gives the command's usage.
    pub fn exactly(&self, count: usize) -> Result<&[String]> {
        if self.positional.len() != count {
            fail!("{}", self.command.usage_text());
        }
        Ok(&self.positional)
    }

    /// The values of each flag <name> given, in order: <name> is a long
    /// flag's name without its dashes (`why`), or a short flag with its dash
    /// (`-b`).
    fn given(&self, name: &str) -> impl Iterator<Item = &[String]> {
        debug_assert!(
            declares(self.command, name),
            "{:?} reads flag {name}, which none of its usage lines names",
            self.command
        );
        self.flags
            .iter()
            .filter(|(flag, _)| spelled(flag, name))
            .map(|(_, values)| values.as_slice())
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The value of <name>, None when it was not given; refused when given
    /// twice.
    pub fn value(&self, name: &str) -> Result<Option<&str>> {
        let mut given = self.given(name);
        let first = given.next();
        if given.next().is_some() {
            fail!("--{name} is given twice");
        }
        Ok(first.and_then(|values| values.first()).map(String::as_str))
    }

    /// The value of <name>, which the command requires.
    pub fn require(&self, name: &str) -> Result<&str> {
        match self.value(name)? {
            Some(value) => Ok(value),
            None => fail!("--{name} is required"),
        }
    }

    /// The `--why`, which the command requires and which says something.
    pub fn why(&self) -> Result<&str> {
        let why = self.require("why")?;
        if why.trim().is_empty() {
            fail!("--why is empty");
        }
        Ok(why)
    }

    /// Every value of a flag that may repeat, in order.
    pub fn values(&self, name: &str) -> Vec<&str> {
        self.given(name)
            .filter_map(|values| values.first())
            .map(String::as_str)
            .collect()
    }

    /// Each flag given, in the order given, with its values.
    pub fn given_flags(&self) -> impl Iterator<Item = (&'static str, &[String])> {
        self.flags
            .iter()
            .map(|(flag, values)| (*flag, values.as_slice()))
    }

    /// Whether the switch <name> was given.
    pub fn switch(&self, name: &str) -> bool {
        self.given(name).next().is_some()
    }

    /// The value of <name> as a variant of <T> (its serde spelling), if given.
    pub fn choice<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let Some(text) = self.value(name)? else {
            return Ok(None);
        };
        serde_json::from_value(serde_json::Value::String(text.to_string()))
            .map(Some)
            .map_err(|error| crate::fail::Fail::Message(format!("--{name}: {error}")))
    }

    /// The id given as flag <name>, if any.
    pub fn id(&self, name: &str) -> Result<Option<Id>> {
        self.value(name)?.map(Id::parse).transpose()
    }

    /// The positional arguments as ids: at least one, each named once, else
    /// a refusal that gives the command's usage.
    pub fn positional_ids(&self) -> Result<Vec<Id>> {
        if self.positional.is_empty() {
            fail!("{}", self.command.usage_text());
        }
        let ids = self
            .positional
            .iter()
            .map(|id| Id::parse(id))
            .collect::<Result<Vec<_>>>()?;
        for (at, id) in ids.iter().enumerate() {
            if ids[..at].contains(id) {
                fail!("the command names {id} twice");
            }
        }
        Ok(ids)
    }

    /// The comma-separated ids given as flag <name>, if any.
    pub fn ids(&self, name: &str) -> Result<Option<Vec<Id>>> {
        self.value(name)?.map(Id::parse_list).transpose()
    }
}

/// Whether <flag>, a flag's spelling, is the flag an `Args` method names
/// <name>.
fn spelled(flag: &str, name: &str) -> bool {
    match name.strip_prefix('-') {
        Some(_) => flag == name,
        None => flag.strip_prefix("--") == Some(name),
    }
}

/// Whether a usage line of <command>, or the global flags, names the flag an
/// `Args` method names <name>.
pub fn declares(command: Command, name: &str) -> bool {
    GLOBAL.iter().any(|flag| spelled(flag.name, name))
        || forms(command).is_ok_and(|forms| {
            forms
                .iter()
                .any(|form| form.flags.iter().any(|flag| spelled(flag.name, name)))
        })
}

/// The forms of <command>'s usage lines, each read without the words that
/// name the command.
fn forms(command: Command) -> Result<Vec<Form>> {
    command
        .usage()
        .iter()
        .map(|line| Form::of(after_words(command, line)))
        .collect()
}

/// The arguments of words that fit <command>'s first usage line, for a test
/// that asks a question of every command.
#[cfg(test)]
pub fn example(command: Command) -> Args {
    let form = Form::of(after_words(command, command.usage()[0])).unwrap();
    Args::of(command, &form.example()).unwrap()
}

/// <line>, a usage line of <command>, after `builds` and the longest of the
/// command's names that it starts with.
fn after_words(command: Command, line: &'static str) -> &'static str {
    let line = line.strip_prefix("builds ").unwrap_or(line);
    command
        .words()
        .iter()
        .filter_map(|named| {
            let named = named.join(" ");
            let rest = line.strip_prefix(named.as_str())?;
            (rest.is_empty() || rest.starts_with(' ')).then(|| (named.len(), rest.trim_start()))
        })
        .max_by_key(|(length, _)| *length)
        .map_or(line, |(_, rest)| rest)
}

/// A variant's name as the request files spell it: through the type's own
/// serde names, so the flag and the JSON never disagree.
pub fn parse_named<T: DeserializeOwned>(flag: &str, text: &str, names: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|_| crate::fail::Fail::Message(format!("{flag} is {names}, not '{text}'")))
}

/// What <spec> names: standard input for `-`, else a file.
pub fn read_input(spec: &str) -> Result<String> {
    if spec == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context(|| "reading standard input".to_string())?;
        return Ok(text);
    }
    std::fs::read_to_string(spec).context(|| format!("reading {spec}"))
}

/// A JSON request from <spec> (a file, or `-` for standard input).
pub fn read_request<T: DeserializeOwned>(spec: &str) -> Result<T> {
    let text = read_input(spec)?;
    serde_json::from_str(&text).map_err(|error| {
        crate::fail::Fail::Message(format!("the request in {spec} is not valid: {error}"))
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

/// The directory the command runs in.
pub fn current_dir() -> Result<std::path::PathBuf> {
    std::env::current_dir().context(|| "reading the current directory".to_string())
}
