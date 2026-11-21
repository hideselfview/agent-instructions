//! The text builds asks every agent to read before it runs a builds command:
//! ~/.claude/CLAUDE.md, the CLAUDE.md of the repo the agent works in, the
//! workflow help, the overview that opens the lists help, and the rules whose
//! paths cover every file.
//! builds reads the rules and the CLAUDE.md files from disk each time, so an
//! edit of any of them changes the text's version.
//!
//! `builds ack` prints the text in parts of at most `PART_BYTES` bytes each,
//! with one word blanked in each third of each part: the agent answers with
//! the words it reads from the text around each blank, and no line of output
//! holds an answer. Claude Code
//! shows only the first 2 KB of a command's output when the output passes
//! about 30 KB, so each part stays well under that.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::review::rules;

/// The most bytes of the text one part holds. Its first and last line add
/// about 300 bytes more.
pub const PART_BYTES: usize = 20_000;

/// The blanked words in each part: one in each third of its lines.
pub const WORDS_PER_PART: usize = 3;

/// One file or help of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub text: String,
}

impl Section {
    /// The hash of the section's text, which builds keeps with each version
    /// an agent acknowledged, so that a later reading holds only the
    /// sections that changed.
    pub fn hash(&self) -> String {
        format!("{:08x}", crate::text::fnv1a(&self.text))
    }
}

/// The whole text, its sections in the order builds lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// The repo whose CLAUDE.md the text holds, or None when it holds no
    /// repo's CLAUDE.md. builds keeps an agent's acknowledgement of each
    /// repo's text apart (record.rs).
    pub repo: Option<String>,
    pub sections: Vec<Section>,
}

impl Text {
    /// The text for an agent whose work is in <project>, in <checkout> when
    /// it works in one: ~/.claude/CLAUDE.md, the checkout's CLAUDE.md when it
    /// differs from ~/.claude/CLAUDE.md, the workflow help, the lists
    /// overview, and the rules of the agent-instructions checkout that
    /// install.sh links whose paths cover every file (those under `rules/`
    /// and under the project's `projects/<project>-rules/`).
    pub fn read(places: &Places, project: &str, checkout: Option<&Checkout>) -> Result<Text> {
        let mut sections = Vec::new();
        let mut repo = None;
        let home = read_if_there(&places.home.join(".claude/CLAUDE.md"))?;
        if let Some(text) = &home {
            sections.push(Section {
                name: "~/.claude/CLAUDE.md".to_string(),
                text: text.clone(),
            });
        }
        if let Some(checkout) = checkout
            && let Some(text) = read_if_there(&checkout.top.join("CLAUDE.md"))?
            && Some(&text) != home.as_ref()
        {
            sections.push(Section {
                name: format!("the CLAUDE.md of repo {}", checkout.repo()),
                text,
            });
            repo = Some(checkout.repo());
        }
        sections.push(Section {
            name: "builds help workflow".to_string(),
            text: crate::help::WORKFLOW_HELP.to_string(),
        });
        sections.push(Section {
            name: "builds help lists overview".to_string(),
            text: crate::help::LISTS_OVERVIEW.to_string(),
        });
        let root = crate::review::installed_rules_root(&places.home);
        for rule in rules::load(&root, Some(project))? {
            if covers_every_file(&rule) {
                sections.push(Section {
                    name: rule.path.clone(),
                    text: rule.text,
                });
            }
        }
        Ok(Text { repo, sections })
    }

    /// The version: the hash of every section's name and text, in order.
    pub fn version(&self) -> String {
        let mut whole = String::new();
        for section in &self.sections {
            whole.push_str(&section.name);
            whole.push('\0');
            whole.push_str(&section.text);
            whole.push('\0');
        }
        format!("{:08x}", crate::text::fnv1a(&whole))
    }

    /// Each section's name and hash, as an acknowledgement keeps them.
    pub fn hashes(&self) -> Vec<(String, String)> {
        self.sections
            .iter()
            .map(|section| (section.name.clone(), section.hash()))
            .collect()
    }

    /// The names of the sections whose text differs from <acknowledged>
    /// (each section's name and hash as an acknowledgement kept them), or
    /// that it lacks, in the text's order.
    pub fn changed_since(&self, acknowledged: &[(String, String)]) -> Vec<String> {
        self.sections
            .iter()
            .filter(|section| {
                !acknowledged
                    .iter()
                    .any(|(name, hash)| *name == section.name && *hash == section.hash())
            })
            .map(|section| section.name.clone())
            .collect()
    }

    /// Splits the lines of the sections named <shown>, in the text's order,
    /// into parts of at most `PART_BYTES` bytes. Each section starts with a
    /// line that names it, and a part that starts inside a section says so
    /// on its first line.
    pub fn parts(&self, shown: &[String]) -> Vec<Vec<String>> {
        let sections: Vec<&Section> = self
            .sections
            .iter()
            .filter(|section| shown.contains(&section.name))
            .collect();
        let count = sections.len();
        let mut parts: Vec<Vec<String>> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let mut bytes = 0;
        for (index, section) in sections.into_iter().enumerate() {
            let heading = format!(
                "===== {} (section {} of {count}) =====",
                section.name,
                index + 1
            );
            let continued = format!("===== {}, continued =====", section.name);
            for (at, line) in std::iter::once(heading.as_str())
                .chain(section.text.lines())
                .enumerate()
            {
                if bytes + line.len() + 1 > PART_BYTES && !lines.is_empty() {
                    parts.push(std::mem::take(&mut lines));
                    bytes = 0;
                    if at > 0 {
                        bytes += continued.len() + 1;
                        lines.push(continued.clone());
                    }
                }
                bytes += line.len() + 1;
                lines.push(line.to_string());
            }
        }
        if !lines.is_empty() {
            parts.push(lines);
        }
        parts
    }
}

/// Whether <rule>'s paths cover every file: it names no paths, or one of its
/// globs is `**/*` or `**`.
fn covers_every_file(rule: &rules::Rule) -> bool {
    match &rule.paths {
        None => true,
        Some(globs) => globs.iter().any(|glob| glob == "**/*" || glob == "**"),
    }
}

/// The text of the file at <path>, or None when there is none.
fn read_if_there(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// The shortest words a part blanks, tried in this order.
const BLANK_LENGTHS: [usize; 3] = [7, 5, 3];

/// A word of a part that the part can blank: a lowercase word of at least
/// `min` letters in running prose, not part of a name, path or code, that
/// occurs once in the whole part, so that blanking it leaves no copy to read.
fn blankable(lines: &[String], min: usize) -> Vec<(usize, String)> {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for line in lines {
        for (_, word) in letter_runs(line) {
            *counts.entry(word.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if line.starts_with("=====") {
            continue;
        }
        let bytes = line.as_bytes();
        for (start, word) in letter_runs(line) {
            let before = start.checked_sub(1).map(|i| bytes[i]);
            let after = bytes.get(start + word.len()).copied();
            if word.len() >= min
                && word.bytes().all(|b| b.is_ascii_lowercase())
                && counts[word] == 1
                && before.is_none_or(|b| b" (\"".contains(&b))
                && after.is_none_or(|b| b" ,;:).\"?!".contains(&b))
            {
                found.push((at, word.to_string()));
            }
        }
    }
    found
}

/// The maximal runs of ASCII letters in <line>, each with its byte offset.
fn letter_runs(line: &str) -> Vec<(usize, &str)> {
    let mut runs = Vec::new();
    let mut start = None;
    for (at, byte) in line.bytes().chain(std::iter::once(b' ')).enumerate() {
        match (start, byte.is_ascii_alphabetic()) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                runs.push((from, &line[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The words to blank in a part with <lines>: one from each third of its
/// lines, a blankable word of the most letters among 7, 5 and 3 that the
/// third has, picked with <random>; a third with no such word takes one from
/// anywhere in the part. The words come in the order the part shows them.
/// Fails when the part has fewer than `WORDS_PER_PART` blankable words, so
/// that no part has a weaker check than the others.
pub fn pick_blanks(lines: &[String], random: &mut dyn FnMut() -> usize) -> Result<Vec<String>> {
    let thirds = WORDS_PER_PART;
    let by_length: Vec<Vec<(usize, String)>> = BLANK_LENGTHS
        .iter()
        .map(|min| blankable(lines, *min))
        .collect();
    let mut picked: Vec<(usize, String)> = Vec::new();
    for third in 0..thirds {
        let range = lines.len() * third / thirds..lines.len() * (third + 1) / thirds;
        let draw = random();
        for anywhere in [false, true] {
            let options = by_length.iter().find_map(|found| {
                let options: Vec<&(usize, String)> = found
                    .iter()
                    .filter(|(at, word)| {
                        (anywhere || range.contains(at)) && !picked.iter().any(|(_, w)| w == word)
                    })
                    .collect();
                (!options.is_empty()).then_some(options)
            });
            if let Some(options) = options {
                picked.push(options[draw % options.len()].clone());
                break;
            }
        }
    }
    if picked.len() < thirds {
        fail!(
            "a part of {} lines holds only {} words that builds can blank, and each part needs {thirds}",
            lines.len(),
            picked.len()
        );
    }
    let column = |(at, word): &(usize, String)| {
        letter_runs(&lines[*at])
            .into_iter()
            .find(|(_, run)| run == word)
            .map(|(start, _)| (*at, start))
    };
    picked.sort_by_key(column);
    Ok(picked.into_iter().map(|(_, word)| word).collect())
}

/// <line> with the word that starts at byte <start> and has <len> bytes
/// replaced by its first letter and underscores for the rest.
fn blanked(line: &str, start: usize, len: usize) -> String {
    format!(
        "{}{}{}{}",
        &line[..start],
        &line[start..start + 1],
        "_".repeat(len - 1),
        &line[start + len..]
    )
}

/// Part <number> of <count> of version <version> as `builds ack N` prints it:
/// its lines, with each of <words> (blankable words that occur once in the
/// part) replaced by its first letter and underscores for the rest.
pub fn show_part(
    lines: &[String],
    number: usize,
    count: usize,
    version: &str,
    words: &[String],
) -> Result<String> {
    let mut shown: Vec<String> = lines.to_vec();
    for word in words {
        let blank = shown.iter_mut().find_map(|line| {
            let start = letter_runs(line)
                .into_iter()
                .find(|(_, run)| run == word)?
                .0;
            *line = blanked(line, start, word.len());
            Some(())
        });
        if blank.is_none() {
            fail!(
                "part {number} of version {version} holds no word {word} to blank; run `builds ack` again"
            );
        }
    }
    let mut out = vec![format!(
        "builds ack: part {number} of {count} of version {version}. Read every line of it. {} words in it are blanked: each shows its first letter and an underscore for every other letter. Acknowledge with the blanked words of every part, in the order they appear.",
        words.len()
    )];
    out.extend(shown);
    out.push(format!("builds ack: the end of part {number} of {count}."));
    Ok(out.join("\n") + "\n")
}

/// A random number from the system's random source.
pub fn random() -> Result<usize> {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .context(|| "reading /dev/urandom".to_string())?;
    Ok(u64::from_le_bytes(bytes) as usize)
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
