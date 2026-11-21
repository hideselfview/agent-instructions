//! The text builds asks every agent to read before it runs a builds command:
//! the workflow help, the overview that opens the lists help, and the rules
//! whose paths cover every file. Claude Code loads ~/.claude/CLAUDE.md and
//! the repo's CLAUDE.md into every agent's context itself, so the text leaves
//! them out.
//! builds reads the rules from disk each time, so an edit of any of them
//! changes the text's version.
//!
//! `builds ack` prints the text in parts of at most `PART_BYTES` bytes each,
//! with no word added to it. Each part's last line gives the command for the
//! next part, with a token that only that part's output holds, and the last
//! part's last line gives the acknowledging command with its token, so the
//! acknowledgement shows that the agent fetched every part, in order. Claude
//! Code shows only the first 2 KB of a command's output when the output
//! passes about 30 KB, so each part stays well under that.

use crate::fail::{Context, Result};
use crate::places::Places;
use crate::review::rules;

/// The most bytes of the text one part holds. Its first and last line add
/// about 400 bytes more.
pub const PART_BYTES: usize = 20_000;

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
    pub sections: Vec<Section>,
}

impl Text {
    /// The text for an agent whose work is in <project>: the workflow help,
    /// the lists overview, and the rules of the agent-instructions checkout
    /// that install.sh links whose paths cover every file (those under
    /// `rules/` and under the project's `projects/<project>-rules/`).
    pub fn read(places: &Places, project: &str) -> Result<Text> {
        let mut sections = Vec::new();
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
        Ok(Text { sections })
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

/// Part <number> of <count> of version <version> as `builds ack part N TOKEN`
/// prints it: a first line that says which part it is, <lines>, and a last
/// line that gives <next>, the command for the next part, or for the
/// acknowledgement after the last part. The command holds a token that only
/// this output shows.
pub fn show_part(
    lines: &[String],
    number: usize,
    count: usize,
    version: &str,
    next: &str,
) -> String {
    let mut out = vec![format!(
        "builds ack: part {number} of {count} of version {version}. Read every line of it; its last line gives the command for the next step."
    )];
    out.extend(lines.iter().cloned());
    out.push(format!("builds ack: the end of part {number} of {count}."));
    out.push(next.to_string());
    out.join("\n") + "\n"
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
