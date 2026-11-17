//! The text builds asks every agent to read before it runs a builds command:
//! ~/.claude/CLAUDE.md, the CLAUDE.md of the repo the agent works in, the
//! workflow help, the overview that opens the lists help, and the rules whose
//! paths cover every file.
//! builds reads the rules and the CLAUDE.md files from disk each time, so an
//! edit of any of them changes the text's version.
//!
//! `builds ack` prints the text in parts of at most `PART_BYTES` bytes each,
//! with check words at the start, middle and end of each part. Claude Code
//! shows only the first 2 KB of a command's output when the output passes
//! about 30 KB, so each part stays well under that.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::review::rules;

/// The most bytes of the text one part holds. Its first line, its check
/// words and its last line add about 300 bytes more.
pub const PART_BYTES: usize = 20_000;

/// The check words in each part: at its start, its middle and its end.
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
            text: crate::WORKFLOW_HELP.to_string(),
        });
        sections.push(Section {
            name: "builds help lists overview".to_string(),
            text: crate::LISTS_OVERVIEW.to_string(),
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

/// Part <number> of <count> of version <version> as `builds ack N` prints it:
/// its lines with <words> on lines of their own at its start, its middle and
/// its end.
pub fn show_part(
    lines: &[String],
    number: usize,
    count: usize,
    version: &str,
    words: &[String],
) -> String {
    let word_line = |position: usize| {
        format!(
            "CHECK WORD {} OF {} IN PART {number}: {}",
            position + 1,
            words.len(),
            words[position]
        )
    };
    let middle = lines.len() / 2;
    let mut out = vec![format!(
        "builds ack: part {number} of {count} of version {version}. Read every line of it."
    )];
    out.push(word_line(0));
    out.extend(lines[..middle].iter().cloned());
    out.push(word_line(1));
    out.extend(lines[middle..].iter().cloned());
    out.push(word_line(2));
    out.push(format!("builds ack: the end of part {number} of {count}."));
    out.join("\n") + "\n"
}

/// A part's `WORDS_PER_PART` new check words, drawn from the system's random
/// source. Each word has three syllables, and each syllable is a consonant
/// and then a vowel.
pub fn new_words() -> Result<Vec<String>> {
    use std::io::Read;
    const CONSONANTS: &[u8] = b"bdfgklmnprstvz";
    const VOWELS: &[u8] = b"aeiou";
    let mut bytes = vec![0u8; WORDS_PER_PART * 6];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .context(|| "reading /dev/urandom".to_string())?;
    Ok(bytes
        .chunks(6)
        .map(|chunk| {
            chunk
                .chunks(2)
                .flat_map(|pair| {
                    [
                        CONSONANTS[pair[0] as usize % CONSONANTS.len()] as char,
                        VOWELS[pair[1] as usize % VOWELS.len()] as char,
                    ]
                })
                .collect()
        })
        .collect())
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
