//! The pages `builds help [TOPIC]` prints and the sections they hold, which
//! main prints and `builds ack` asks agents to read.

use std::sync::LazyLock;

use crate::command::Command;

/// The text of `builds help`, with a marker for each command's usage lines
/// (`with_usage`).
const HELP_SOURCE: &str = include_str!("help.txt");
/// `builds help`.
pub static HELP: LazyLock<String> = LazyLock::new(|| with_usage(HELP_SOURCE));
/// What the lists are and who writes them, which every agent reads.
pub const LISTS_OVERVIEW: &str = include_str!("lists/lists-overview.txt");
/// The text of `builds help lists`, with markers as in `HELP_SOURCE`: the
/// overview, then each command.
const LISTS_SOURCE: &str = concat!(
    include_str!("lists/lists-overview.txt"),
    "\n\n",
    include_str!("lists/lists-help.txt")
);
/// `builds help lists`.
pub static LISTS_HELP: LazyLock<String> = LazyLock::new(|| with_usage(LISTS_SOURCE));
/// `builds help workflow`.
pub const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
/// `builds help briefs`.
pub const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

/// The page `builds help <topic>` prints; <topic> None is `builds help`. None
/// when no page has the topic.
pub fn page(topic: Option<&str>) -> Option<&'static str> {
    match topic {
        None => Some(HELP.as_str()),
        Some("lists") => Some(LISTS_HELP.as_str()),
        Some("workflow") => Some(WORKFLOW_HELP),
        Some("briefs") => Some(BRIEFS_HELP),
        Some(_) => None,
    }
}

/// The command that prints the page of <topic>: `builds help`, or `builds
/// help <topic>`.
pub fn name(topic: Option<&str>) -> String {
    match topic {
        Some(topic) => format!("builds help {topic}"),
        None => "builds help".to_string(),
    }
}

/// What `builds help <args>` prints: with no <args>, `builds help`; with a
/// page's topic, that page; and with the words of a section's heading after
/// the topic, or alone for a section of `builds help`, that section only, so
/// that a citation such as (`builds help lists`, DECISION LOG) is the command
/// `builds help lists DECISION LOG`. The heading's words match in any case. Err
/// names the page's headings when none matches.
pub fn lookup(args: &[String]) -> Result<&'static str, String> {
    let (topic, words) = match args.split_first() {
        Some((topic, words)) if page(Some(topic)).is_some() => (Some(topic.as_str()), words),
        _ => (None, args),
    };
    let text = page(topic).expect("a known topic or none");
    if words.is_empty() {
        return Ok(text);
    }
    let heading = words.join(" ");
    chunks(text)
        .into_iter()
        .find(|(name, _)| !name.is_empty() && name.eq_ignore_ascii_case(&heading))
        .map(|(_, section)| section)
        .ok_or_else(|| {
            format!(
                "{} has no section {heading}. Its sections are {}.",
                name(topic),
                sections(text).join("; ")
            )
        })
}

/// Whether <line> is the heading of a section: it starts at the margin and
/// holds capital letters, spaces and commas only.
fn is_heading(line: &str) -> bool {
    line.starts_with(|c: char| c.is_ascii_uppercase())
        && line
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == ' ' || c == ',')
}

/// The headings of the sections of <page>.
pub fn sections(page: &str) -> Vec<&str> {
    page.lines()
        .filter(|line| is_heading(line))
        .map(str::trim_end)
        .collect()
}

/// <page> cut into its sections, each as its heading (empty for the text
/// before the first heading) and its text, which runs from the heading to the
/// next heading with every line between kept, so that the texts of all the
/// sections, joined, are the page.
pub fn chunks(page: &str) -> Vec<(&str, &str)> {
    let mut chunks = Vec::new();
    let (mut heading, mut start, mut at) = ("", 0, 0);
    for line in page.split_inclusive('\n') {
        let text = line.trim_end_matches('\n');
        if is_heading(text) {
            chunks.push((heading, &page[start..at]));
            (heading, start) = (text.trim_end(), at);
        }
        at += line.len();
    }
    chunks.push((heading, &page[start..]));
    chunks
}

/// <page> with each usage marker replaced by the usage lines of its command
/// (`Command::usage`), so that help shows the usage that the command's
/// refusals show. A marker is a line of its own, two spaces in: `{WORDS}`
/// stands for every usage line of the command that WORDS name, `{WORDS #N}`
/// for its Nth. Text after the marker describes the command: it starts on
/// the last usage line at the column it starts at after the marker, or, when
/// that line reaches the column, on the next line at that column.
pub fn with_usage(page: &str) -> String {
    let mut text = String::new();
    for line in page.split_inclusive('\n') {
        match marker(line.trim_end_matches('\n')) {
            Some(Marker {
                command,
                lines,
                description,
            }) => {
                let (last, before) = lines.split_last().expect("a command has usage lines");
                for usage in before {
                    text.push_str(&wrapped(command, usage));
                    text.push('\n');
                }
                let last = wrapped(command, last);
                let tail = last.len() - last.rfind('\n').map_or(0, |at| at + 1);
                match description {
                    None => text.push_str(&last),
                    Some((column, words)) if tail + 2 <= column => {
                        let pad = column - tail;
                        text.push_str(&format!("{last}{:pad$}{words}", ""));
                    }
                    Some((column, words)) => {
                        text.push_str(&format!("{last}\n{:column$}{words}", ""));
                    }
                }
                if line.ends_with('\n') {
                    text.push('\n');
                }
            }
            None => text.push_str(line),
        }
    }
    text
}

/// The width help's usage lines keep to, where a line can break.
const WIDTH: usize = 80;

/// <usage>, a usage line of <command>, two spaces in, broken where it passes
/// `WIDTH` at the spaces between its arguments, outside brackets and
/// parentheses, with each further line starting under the first argument.
fn wrapped(command: Command, usage: &str) -> String {
    let named = format!("builds {} ", command.words()[0].join(" "));
    let Some(arguments) = usage.strip_prefix(&named) else {
        return format!("  {usage}");
    };
    let indent = 2 + named.len();
    let mut text = format!("  {}", named.trim_end());
    // The length of the line the next argument goes on, and whether it holds
    // an argument yet.
    let (mut line, mut holds) = (text.len(), false);
    for argument in top_level(arguments) {
        if holds && line + 1 + argument.len() > WIDTH {
            text.push_str(&format!("\n{:indent$}{argument}", ""));
            line = indent + argument.len();
        } else {
            text.push(' ');
            text.push_str(argument);
            line += 1 + argument.len();
        }
        holds = true;
    }
    text
}

/// <arguments> split at each space outside brackets and parentheses, but
/// for the space between a flag and its value.
fn top_level(arguments: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    for (at, character) in arguments.char_indices() {
        match character {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            ' ' if depth == 0 => {
                let part = &arguments[start..at];
                let next = &arguments[at + 1..];
                let value = !next.starts_with(['-', '[', '(', '|']);
                if !(part.starts_with("--") && !part.contains(' ') && value) {
                    parts.push(part);
                    start = at + 1;
                }
            }
            _ => {}
        }
    }
    parts.push(&arguments[start..]);
    parts
}

/// A marker of a page (`with_usage`).
struct Marker<'a> {
    command: Command,
    /// The usage lines it stands for.
    lines: Vec<&'static str>,
    /// The column and the words of the description after it.
    description: Option<(usize, &'a str)>,
}

/// <line> as a marker, None when it is none.
fn marker(line: &str) -> Option<Marker<'_>> {
    let inside = line.strip_prefix("  {")?;
    let (inside, after) = inside.split_once('}')?;
    let (words, nth) = match inside.split_once(" #") {
        Some((words, nth)) => (words, Some(nth.parse::<usize>().ok()?)),
        None => (inside, None),
    };
    let command = *Command::ALL
        .iter()
        .find(|command| command.words()[0].join(" ") == words)?;
    let usage = command.usage();
    let lines = match nth {
        Some(nth) => vec![*usage.get(nth.checked_sub(1)?)?],
        None => usage.to_vec(),
    };
    let words = after.trim_start();
    let description = (!words.is_empty()).then(|| (line.len() - words.len(), words));
    Some(Marker {
        command,
        lines,
        description,
    })
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
