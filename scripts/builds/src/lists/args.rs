//! The flags every lists command takes, parsed one way: `--name value`,
//! `--switch`, the rest positional. A flag the command doesn't list is
//! refused by name, never ignored. `--project` and `--by` are every
//! command's.

use std::io::Read;

use serde::de::DeserializeOwned;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::Id;

/// The flags every lists command takes.
pub const COMMON: [&str; 2] = ["project", "by"];

#[derive(Debug)]
pub struct Args {
    positional: Vec<String>,
    /// Each flag given, in order; a switch has no value.
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    /// <args> with <valued> flags (and `--project`, `--by`) taking the next
    /// argument as their value and <switches> taking none. `--` ends the
    /// flags.
    pub fn parse(args: &[String], valued: &[&str], switches: &[&str]) -> Result<Args> {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if arg == "--" {
                positional.extend(rest.by_ref().cloned());
                break;
            }
            let Some(name) = arg.strip_prefix("--") else {
                positional.push(arg.clone());
                continue;
            };
            if valued.contains(&name) || COMMON.contains(&name) {
                let Some(value) = rest.next() else { fail!("--{name} needs a value") };
                flags.push((name.to_string(), Some(value.clone())));
            } else if switches.contains(&name) {
                flags.push((name.to_string(), None));
            } else {
                let mut known: Vec<String> =
                    valued.iter().chain(&COMMON).chain(switches).map(|n| format!("--{n}")).collect();
                known.dedup();
                fail!("unknown flag --{name}; this command takes {}", known.join(", "));
            }
        }
        Ok(Args { positional, flags })
    }

    pub fn positional(&self) -> &[String] {
        &self.positional
    }

    /// The positional arguments, exactly <count> of them, else <usage>.
    pub fn exactly(&self, count: usize, usage: &str) -> Result<&[String]> {
        if self.positional.len() != count {
            fail!("usage: {usage}");
        }
        Ok(&self.positional)
    }

    /// The value of <name>, None when it was not given; refused when given
    /// twice.
    pub fn value(&self, name: &str) -> Result<Option<&str>> {
        let mut given = self.flags.iter().filter(|(flag, _)| flag == name);
        let first = given.next();
        if given.next().is_some() {
            fail!("--{name} is given twice");
        }
        Ok(first.and_then(|(_, value)| value.as_deref()))
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
        self.flags
            .iter()
            .filter(|(flag, _)| flag == name)
            .filter_map(|(_, value)| value.as_deref())
            .collect()
    }

    pub fn switch(&self, name: &str) -> bool {
        self.flags.iter().any(|(flag, value)| flag == name && value.is_none())
    }

    /// The value of <name> as a variant of <T> (its serde spelling), if given.
    pub fn choice<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let Some(text) = self.value(name)? else { return Ok(None) };
        serde_json::from_value(serde_json::Value::String(text.to_string()))
            .map(Some)
            .map_err(|error| crate::fail::Fail::Message(format!("--{name}: {error}")))
    }

    /// The id given as flag <name>, if any.
    pub fn id(&self, name: &str) -> Result<Option<Id>> {
        self.value(name)?.map(Id::parse).transpose()
    }

    /// The comma-separated ids given as flag <name>, if any.
    pub fn ids(&self, name: &str) -> Result<Option<Vec<Id>>> {
        self.value(name)?.map(Id::parse_list).transpose()
    }
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
        std::io::stdin().read_to_string(&mut text).context(|| "reading standard input".to_string())?;
        return Ok(text);
    }
    std::fs::read_to_string(spec).context(|| format!("reading {spec}"))
}

/// A JSON request from <spec> (a file, or `-` for standard input).
pub fn read_request<T: DeserializeOwned>(spec: &str) -> Result<T> {
    let text = read_input(spec)?;
    serde_json::from_str(&text)
        .map_err(|error| crate::fail::Fail::Message(format!("the request in {spec} is not valid: {error}")))
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod tests;

/// The directory the command runs in.
pub fn current_dir() -> Result<std::path::PathBuf> {
    std::env::current_dir().context(|| "reading the current directory".to_string())
}
