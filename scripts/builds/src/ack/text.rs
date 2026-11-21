//! The text builds asks every agent to read before it runs a builds command:
//! ~/.claude/CLAUDE.md, the CLAUDE.md of the repo the agent works in, the
//! workflow help, the overview that opens the lists help, and the rules whose
//! paths cover every file.
//! builds reads the rules and the CLAUDE.md files from disk each time, so an
//! edit of any of them changes the text's version.
//!
//! `builds ack` prints the text in parts of at most `PART_BYTES` bytes each,
//! and puts one made-up word into a sentence in each third of each part. The
//! agent answers with the made-up words it noted while it read. No line marks
//! a made-up word, so a search for a marker finds none, and an answer copies
//! no word of the text, nor any word the output leaves out. Claude Code
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

/// The made-up words in each part: one in each third of its places.
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

/// A made-up word that a part shows, and where the part shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeUp {
    /// The index of the word's place among the part's `places`.
    pub place: usize,
    pub word: String,
}

/// The places in a part with <lines> where builds can put a made-up word, in
/// the order the part shows them: the start of each lowercase word of running
/// prose, outside a code span and outside the lines that name a section. Each
/// place is the index of its line and the byte where the word starts.
fn places(lines: &[String]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if line.starts_with("=====") {
            continue;
        }
        let bytes = line.as_bytes();
        for (start, word) in letter_runs(line) {
            let before = start.checked_sub(1).map(|i| bytes[i]);
            let after = bytes.get(start + word.len()).copied();
            let in_code = bytes[..start].iter().filter(|b| **b == b'`').count() % 2 == 1;
            if word.bytes().all(|b| b.is_ascii_lowercase())
                && !in_code
                && before.is_none_or(|b| b" (\"".contains(&b))
                && after.is_none_or(|b| b" ,;:).\"?!".contains(&b))
            {
                found.push((at, start));
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

/// The letters that open a syllable of a made-up word.
const ONSETS: &[u8] = b"bdfgklmnprstvz";
/// The letters that follow a syllable's first letter.
const VOWELS: &[u8] = b"aeiou";
/// What closes a syllable: nothing in a third of the syllables, else one
/// consonant. Three syllables of this shape make a word of 6 to 9 letters,
/// which /usr/share/dict/words holds about once in 7,000 draws.
const CODAS: [&str; 9] = ["", "", "", "n", "r", "l", "s", "k", "m"];

/// How many words `pick_made_up` draws for one place before it fails. A word
/// drawn from the system's random source repeats a word of the part or an
/// earlier draw far less often than once in this many draws.
const DRAWS: usize = 100;

/// A made-up word of three syllables, drawn with <random>, such as kodamir.
fn made_up_word(random: &mut dyn FnMut() -> Result<usize>) -> Result<String> {
    let mut word = String::new();
    for _ in 0..3 {
        word.push(ONSETS[random()? % ONSETS.len()] as char);
        word.push(VOWELS[random()? % VOWELS.len()] as char);
        word.push_str(CODAS[random()? % CODAS.len()]);
    }
    Ok(word)
}

/// The made-up words to put in a part with <lines>, drawn with <random>: one
/// in each third of the part's places, each at a place of that third and none
/// a word the part holds or another of the part's made-up words. The words
/// come in the order the part shows them. Fails when the part has fewer
/// places than `WORDS_PER_PART`, so that no part has a weaker check than the
/// others.
pub fn pick_made_up(
    lines: &[String],
    random: &mut dyn FnMut() -> Result<usize>,
) -> Result<Vec<MadeUp>> {
    let places = places(lines);
    let thirds = WORDS_PER_PART;
    if places.len() < thirds {
        fail!(
            "a part of {} lines has only {} places for a made-up word, and each part needs {thirds}",
            lines.len(),
            places.len()
        );
    }
    let held: std::collections::HashSet<String> = lines
        .iter()
        .flat_map(|line| letter_runs(line))
        .map(|(_, run)| run.to_ascii_lowercase())
        .collect();
    let mut picked: Vec<MadeUp> = Vec::new();
    for third in 0..thirds {
        let range = places.len() * third / thirds..places.len() * (third + 1) / thirds;
        let place = range.start + random()? % range.len();
        let mut word = None;
        for _ in 0..DRAWS {
            let drawn = made_up_word(random)?;
            if !held.contains(&drawn) && !picked.iter().any(|made| made.word == drawn) {
                word = Some(drawn);
                break;
            }
        }
        let Some(word) = word else {
            fail!("builds drew {DRAWS} made-up words for a part, and the part holds each of them");
        };
        picked.push(MadeUp { place, word });
    }
    Ok(picked)
}

/// Part <number> of <count> of version <version> as `builds ack N` prints it:
/// its lines, with each of <made_up> put at its place, followed by a space.
pub fn show_part(
    lines: &[String],
    number: usize,
    count: usize,
    version: &str,
    made_up: &[MadeUp],
) -> Result<String> {
    let places = places(lines);
    let mut shown: Vec<String> = lines.to_vec();
    // Later places first, so that each insertion leaves the bytes of the
    // places before it where they were.
    let mut last_first: Vec<&MadeUp> = made_up.iter().collect();
    last_first.sort_by_key(|made| std::cmp::Reverse(made.place));
    for made in last_first {
        let Some(&(line, at)) = places.get(made.place) else {
            fail!(
                "part {number} of version {version} has {} places for a made-up word, and no place {}; run `builds ack` again",
                places.len(),
                made.place + 1
            );
        };
        shown[line].insert_str(at, &format!("{} ", made.word));
    }
    let mut out = vec![format!(
        "builds ack: part {number} of {count} of version {version}. Read every line of it. builds put {} made-up words into its sentences, one in each third of the part: note each one as you read. Acknowledge with the made-up words of every part, in the order they appear.",
        made_up.len()
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
