//! A usage line of a command (command.rs) read as the form of the command's
//! words it describes: the flags it takes, each with the number of values
//! that follow it, and the pattern of its positional words.
//!
//! The grammar of a usage line, after the words that name the command:
//!
//! - `--name` or `-n` is a flag. At the top level of the line it takes the
//!   one value word that follows it, if any. Inside brackets or parentheses
//!   it takes every value word up to the next flag, `|` or bracket.
//! - `[...]` is optional, `(...)` is required, and ` | ` separates the
//!   alternatives inside either.
//! - `...` after a word or a group repeats it, and a flag that repeats, or
//!   that the line names twice, may be given more than once.
//! - `<name>`, a word that starts with a capital letter, or a word with
//!   brackets inside it (`N[,N...]`) is a placeholder for any one word.
//! - Any other word is a literal, and `a|b|c` is one of those literals.
//! - `--` is the word that ends the flags: every word after it is
//!   positional.
//! - `<args>` is the first of the words that builds passes on to another
//!   program: every word from it on is positional, a flag's spelling or not.

use crate::fail;
use crate::fail::Result;

/// One flag of a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flag {
    /// Its spelling, with its dashes.
    pub name: &'static str,
    /// How many words follow it as its values.
    pub values: usize,
    /// Whether it may be given more than once.
    pub repeats: bool,
}

/// What a usage line says of a command's words.
#[derive(Debug)]
pub struct Form {
    pub flags: Vec<Flag>,
    pattern: Vec<Element>,
    /// The number of positional words after which every word is passed on
    /// (`<args>`), if the line passes words on after a fixed number of them.
    passes_on_after: Option<usize>,
    /// Whether the line names `--`, which stays among the positional words.
    keeps_double_dash: bool,
}

/// One part of a positional pattern, with whether it repeats.
#[derive(Debug)]
struct Element {
    part: Part,
    repeats: bool,
}

#[derive(Debug)]
enum Part {
    /// Any one word.
    Placeholder,
    /// One of these words.
    Literal(Vec<&'static str>),
    /// `--`.
    DoubleDash,
    /// A group: one of its alternatives, or nothing when it is optional.
    Group {
        optional: bool,
        alternatives: Vec<Vec<Element>>,
    },
}

/// A token of a usage line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Open(char),
    Close(char),
    Bar,
    Repeat,
    Word(&'static str),
}

/// One element of the syntax tree, before its flags are taken out.
#[derive(Debug)]
enum Node {
    Flag(&'static str),
    Word(&'static str),
    Group {
        optional: bool,
        alternatives: Vec<Vec<(Node, bool)>>,
    },
}

impl Form {
    /// The form of <usage>, a usage line without its `builds` and the words
    /// that name the command. Fails on a line it cannot read, which a test of
    /// every declaration rules out.
    pub fn of(usage: &'static str) -> Result<Form> {
        let tokens = tokens(usage)?;
        let mut at = 0;
        let alternatives = sequence(&tokens, &mut at)?;
        if at != tokens.len() {
            fail!("usage line {usage:?}: {:?} closes no group", tokens[at]);
        }
        let [top] = <[_; 1]>::try_from(alternatives).map_err(|_| {
            crate::fail::Fail::Message(format!(
                "usage line {usage:?}: a | outside a group, which would split the whole line"
            ))
        })?;
        let mut flags: Vec<Flag> = Vec::new();
        collect_flags(&top, true, false, &mut flags);
        let pattern = elements(&top);
        let keeps_double_dash = top.iter().any(|(node, _)| matches!(node, Node::Word("--")));
        Ok(Form {
            flags,
            passes_on_after: passes_on_after(&top),
            keeps_double_dash,
            pattern,
        })
    }

    /// The flag spelled <word>, if the form has it.
    pub fn flag(&self, word: &str) -> Option<Flag> {
        self.flags.iter().find(|flag| flag.name == word).copied()
    }

    /// <words> read by this form: the flags given, with their values, and
    /// the positional words, or why they do not fit it.
    pub fn read(&self, words: &[String], global: &[Flag]) -> std::result::Result<Read, Misfit> {
        let mut read = Read::default();
        let mut passing = false;
        let mut rest = words.iter();
        while let Some(word) = rest.next() {
            if passing {
                read.positional.push(word.clone());
                continue;
            }
            if word == "--" {
                if self.keeps_double_dash {
                    read.positional.push(word.clone());
                }
                passing = true;
                continue;
            }
            if is_flag(word) {
                let Some(flag) = self
                    .flag(word)
                    .or_else(|| global.iter().find(|flag| flag.name == word).copied())
                else {
                    return Err(Misfit::Unknown(word.clone()));
                };
                let values: Vec<String> = rest.by_ref().take(flag.values).cloned().collect();
                if values.len() < flag.values {
                    return Err(Misfit::NeedsValue(flag.name));
                }
                if !flag.repeats && read.flags.iter().any(|(given, _)| *given == flag.name) {
                    return Err(Misfit::Twice(flag.name));
                }
                read.flags.push((flag.name, values));
                continue;
            }
            read.positional.push(word.clone());
            if self.passes_on_after == Some(read.positional.len()) {
                passing = true;
            }
        }
        if !ends(&self.pattern, &read.positional, 0).contains(&read.positional.len()) {
            return Err(Misfit::Positional);
        }
        Ok(read)
    }
}

/// Whether <word> is spelled as a flag: `--name`, or `-` and a letter.
pub fn is_flag(word: &str) -> bool {
    match word.strip_prefix("--") {
        Some(name) => !name.is_empty(),
        None => word
            .strip_prefix('-')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|first| first.is_ascii_alphabetic()),
    }
}

/// The words of a command read by one form.
#[derive(Debug, Default)]
pub struct Read {
    pub positional: Vec<String>,
    /// Each flag given, in order, with its values.
    pub flags: Vec<(&'static str, Vec<String>)>,
}

/// Why words do not fit a form.
#[derive(Debug, PartialEq, Eq)]
pub enum Misfit {
    /// A word spelled as a flag that the form lacks.
    Unknown(String),
    /// A flag with fewer words after it than its values.
    NeedsValue(&'static str),
    /// A flag given twice that the form takes once.
    Twice(&'static str),
    /// The positional words fit no pattern of the form.
    Positional,
}

fn tokens(usage: &'static str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let bytes = usage.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b' ' => at += 1,
            b'[' | b'(' => {
                tokens.push(Token::Open(bytes[at] as char));
                at += 1;
            }
            b']' | b')' => {
                tokens.push(Token::Close(bytes[at] as char));
                at += 1;
            }
            b'|' if bytes.get(at + 1).is_none_or(|next| *next == b' ') => {
                tokens.push(Token::Bar);
                at += 1;
            }
            b'.' if usage[at..].starts_with("...") => {
                tokens.push(Token::Repeat);
                at += 3;
            }
            _ => {
                let start = at;
                // Brackets a word opens inside itself (`N[,N...]`) are its
                // own; `<...>` may hold spaces.
                let (mut depth, mut angle) = (0usize, false);
                while at < bytes.len() {
                    match bytes[at] {
                        b'<' => angle = true,
                        b'>' => angle = false,
                        b' ' if !angle => break,
                        b'[' => depth += 1,
                        b']' | b')' if depth == 0 && !angle => break,
                        b']' => depth -= 1,
                        b'.' if depth == 0 && !angle && usage[at..].starts_with("...") => break,
                        _ => {}
                    }
                    at += 1;
                }
                if angle || depth != 0 {
                    fail!("usage line {usage:?}: an unclosed < or [ in a word");
                }
                tokens.push(Token::Word(&usage[start..at]));
            }
        }
    }
    Ok(tokens)
}

/// The alternatives of the group that starts at <at>, up to its closing
/// bracket or the end.
fn sequence(tokens: &[Token], at: &mut usize) -> Result<Vec<Vec<(Node, bool)>>> {
    let mut alternatives = vec![Vec::new()];
    while let Some(token) = tokens.get(*at) {
        let node = match *token {
            Token::Close(_) => break,
            Token::Bar => {
                alternatives.push(Vec::new());
                *at += 1;
                continue;
            }
            Token::Repeat => fail!("a ... that follows nothing"),
            Token::Word(word) if is_flag(word) => {
                *at += 1;
                Node::Flag(word)
            }
            Token::Word(word) => {
                *at += 1;
                Node::Word(word)
            }
            Token::Open(open) => {
                *at += 1;
                let inner = sequence(tokens, at)?;
                let close = if open == '[' { ']' } else { ')' };
                if tokens.get(*at) != Some(&Token::Close(close)) {
                    fail!("a {open} that does not close with {close}");
                }
                *at += 1;
                Node::Group {
                    optional: open == '[',
                    alternatives: inner,
                }
            }
        };
        let repeats = tokens.get(*at) == Some(&Token::Repeat);
        if repeats {
            *at += 1;
        }
        alternatives
            .last_mut()
            .expect("one alternative at least")
            .push((node, repeats));
    }
    Ok(alternatives)
}

/// Adds the flags of <nodes>, a sequence at the top level of the line when
/// <top>, inside a repeating group when <repeating>, to <flags>.
fn collect_flags(nodes: &[(Node, bool)], top: bool, repeating: bool, flags: &mut Vec<Flag>) {
    for (at, (node, repeats)) in nodes.iter().enumerate() {
        match node {
            Node::Flag(name) => {
                let following = nodes[at + 1..]
                    .iter()
                    .take_while(|(node, _)| matches!(node, Node::Word(word) if *word != "--"));
                let taken: Vec<&(Node, bool)> = if top {
                    following.take(1).collect()
                } else {
                    following.collect()
                };
                let flag = Flag {
                    name,
                    values: taken.len(),
                    repeats: repeating || *repeats || taken.iter().any(|(_, repeats)| *repeats),
                };
                match flags.iter_mut().find(|known| known.name == flag.name) {
                    // A flag the line names twice may be given twice.
                    Some(known) => known.repeats = true,
                    None => flags.push(flag),
                }
            }
            Node::Word(_) => {}
            Node::Group { alternatives, .. } => {
                for alternative in alternatives {
                    collect_flags(alternative, false, repeating || *repeats, flags);
                }
            }
        }
    }
}

/// The positional pattern of <nodes>: without the flags and their values.
fn elements(nodes: &[(Node, bool)]) -> Vec<Element> {
    let mut elements = Vec::new();
    let mut values_left = 0;
    for (at, (node, repeats)) in nodes.iter().enumerate() {
        let part = match node {
            Node::Flag(_) => {
                values_left = flag_values_at(nodes, at);
                continue;
            }
            Node::Word(_) if values_left > 0 => {
                values_left -= 1;
                continue;
            }
            Node::Word("--") => Part::DoubleDash,
            Node::Word(word) if is_placeholder(word) => Part::Placeholder,
            Node::Word(word) => Part::Literal(word.split('|').collect()),
            Node::Group {
                optional,
                alternatives,
            } => Part::Group {
                optional: *optional,
                alternatives: alternatives
                    .iter()
                    .map(|nodes| inner_elements(nodes))
                    .collect(),
            },
        };
        elements.push(Element {
            part,
            repeats: *repeats,
        });
    }
    elements
}

/// `elements` of a sequence inside a group, where a flag takes every value
/// word up to the next flag.
fn inner_elements(nodes: &[(Node, bool)]) -> Vec<Element> {
    let mut kept: Vec<Element> = Vec::new();
    let mut in_flag = false;
    for (node, repeats) in nodes {
        match node {
            Node::Flag(_) => in_flag = true,
            Node::Word(word) if in_flag && *word != "--" => {}
            Node::Word("--") => {
                in_flag = false;
                kept.push(Element {
                    part: Part::DoubleDash,
                    repeats: *repeats,
                });
            }
            Node::Word(word) => kept.push(Element {
                part: if is_placeholder(word) {
                    Part::Placeholder
                } else {
                    Part::Literal(word.split('|').collect())
                },
                repeats: *repeats,
            }),
            Node::Group {
                optional,
                alternatives,
            } => {
                in_flag = false;
                kept.push(Element {
                    part: Part::Group {
                        optional: *optional,
                        alternatives: alternatives
                            .iter()
                            .map(|nodes| inner_elements(nodes))
                            .collect(),
                    },
                    repeats: *repeats,
                });
            }
        }
    }
    kept
}

/// How many words follow the top-level flag at <at> of <nodes> as its value.
fn flag_values_at(nodes: &[(Node, bool)], at: usize) -> usize {
    usize::from(matches!(nodes.get(at + 1), Some((Node::Word(word), _)) if *word != "--"))
}

/// Whether <word> stands for any one word.
fn is_placeholder(word: &str) -> bool {
    word.starts_with('<')
        || word
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
        || word.contains('[')
}

/// The number of top-level positional words before `<args>`, when each of
/// them is one word the line requires.
fn passes_on_after(nodes: &[(Node, bool)]) -> Option<usize> {
    let mut count = 0;
    let mut values_left = 0;
    for (at, (node, repeats)) in nodes.iter().enumerate() {
        match node {
            Node::Flag(_) => values_left = flag_values_at(nodes, at),
            Node::Word(_) if values_left > 0 => values_left -= 1,
            Node::Word(word) if *word == PASSED_ON => return Some(count),
            Node::Word(word) if *word == "--" || *repeats => return None,
            Node::Word(_) => count += 1,
            Node::Group { alternatives, .. } => {
                let mut inner = alternatives.iter().flatten();
                if inner.any(|(node, _)| matches!(node, Node::Word(word) if *word == PASSED_ON)) {
                    return Some(count);
                }
                if !holds_only_flags(alternatives) {
                    return None;
                }
            }
        }
    }
    None
}

/// The placeholder of the first word that builds passes on to another
/// program.
const PASSED_ON: &str = "<args>";

/// Whether each of <alternatives> holds flags and their values only.
fn holds_only_flags(alternatives: &[Vec<(Node, bool)>]) -> bool {
    alternatives.iter().all(|nodes| {
        nodes
            .first()
            .is_none_or(|(node, _)| !matches!(node, Node::Word(_)))
            && nodes.iter().all(|(node, _)| match node {
                Node::Group { alternatives, .. } => holds_only_flags(alternatives),
                Node::Word(word) => *word != "--",
                Node::Flag(_) => true,
            })
    })
}

/// Every position in <words> where a match of <pattern> from <start> can end.
fn ends(pattern: &[Element], words: &[String], start: usize) -> Vec<usize> {
    let mut at = vec![start];
    for element in pattern {
        let mut next = Vec::new();
        for from in at {
            for end in element_ends(element, words, from) {
                if !next.contains(&end) {
                    next.push(end);
                }
            }
        }
        at = next;
        if at.is_empty() {
            break;
        }
    }
    at
}

/// Every position where one match of <element> from <start> can end: one or
/// more of its parts when it repeats.
fn element_ends(element: &Element, words: &[String], start: usize) -> Vec<usize> {
    let mut found = part_ends(&element.part, words, start);
    if element.repeats {
        let mut frontier = found.clone();
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for from in frontier {
                for end in part_ends(&element.part, words, from) {
                    // A part that matches nothing ends where it starts; going
                    // on from there would loop.
                    if end > from && !found.contains(&end) {
                        found.push(end);
                        next.push(end);
                    }
                }
            }
            frontier = next;
        }
    }
    found
}

fn part_ends(part: &Part, words: &[String], start: usize) -> Vec<usize> {
    match part {
        Part::Placeholder => {
            if start < words.len() {
                vec![start + 1]
            } else {
                vec![]
            }
        }
        Part::Literal(choices) => match words.get(start) {
            Some(word) if choices.contains(&word.as_str()) => vec![start + 1],
            _ => vec![],
        },
        Part::DoubleDash => match words.get(start) {
            Some(word) if word == "--" => vec![start + 1],
            _ => vec![],
        },
        Part::Group {
            optional,
            alternatives,
        } => {
            let mut found: Vec<usize> = if *optional { vec![start] } else { vec![] };
            for alternative in alternatives {
                for end in ends(alternative, words, start) {
                    if !found.contains(&end) {
                        found.push(end);
                    }
                }
            }
            found
        }
    }
}

impl Form {
    /// Words that fit this form: each of its flags with its values, then
    /// positional words that fit its pattern.
    #[cfg(test)]
    pub fn example(&self) -> Vec<String> {
        let mut words: Vec<String> = Vec::new();
        for flag in &self.flags {
            words.push(flag.name.to_string());
            words.extend((0..flag.values).map(|_| "v".to_string()));
        }
        example_of(&self.pattern, &mut words);
        words
    }
}

/// Adds words that fit <pattern> to <words>: one of each repeated part, the
/// first alternative of each required group and nothing of an optional one.
#[cfg(test)]
fn example_of(pattern: &[Element], words: &mut Vec<String>) {
    for element in pattern {
        match &element.part {
            Part::Placeholder => words.push("w".to_string()),
            Part::Literal(choices) => words.push(choices[0].to_string()),
            Part::DoubleDash => words.push("--".to_string()),
            Part::Group { optional: true, .. } => {}
            Part::Group { alternatives, .. } => example_of(&alternatives[0], words),
        }
    }
}

#[cfg(test)]
#[path = "form_tests.rs"]
mod tests;
