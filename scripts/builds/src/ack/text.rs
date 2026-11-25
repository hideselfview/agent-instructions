//! The text builds asks agents to acknowledge: the workflow help and the
//! overview that opens the lists help.
//!
//! Every agent reads the sections that all agents use. A coordinator, a design
//! session and a judge also read the sections of their role (`Role`,
//! `GROUPS`), which `builds ack --role` asks for. `builds help workflow` and
//! `builds help lists` print every section.
//!
//! `builds ack` prints the text in parts of at most `PART_BYTES` bytes each,
//! with no word added to it. Each part's last line gives the command for the
//! next part, with a token that only that part's output holds, and the last
//! part's last line gives the acknowledging command with its token, so the
//! acknowledgement shows that the agent fetched every part, in order. Claude
//! Code shows only the first 2 KB of a command's output when the output
//! passes about 30 KB, so each part stays well under that.

use crate::fail::{Context, Result};
use crate::help::{LISTS_OVERVIEW, WORKFLOW_HELP, chunks};

/// The most bytes of the text one part holds. Its first and last line add
/// about 400 bytes more.
pub const PART_BYTES: usize = 20_000;

/// A role whose sections only that role's agents read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Coordinator,
    Design,
    Judge,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Coordinator, Role::Design, Role::Judge];

    pub fn name(self) -> &'static str {
        match self {
            Role::Coordinator => "coordinator",
            Role::Design => "design",
            Role::Judge => "judge",
        }
    }

    pub fn parse(name: &str) -> Result<Role> {
        match Role::ALL.into_iter().find(|role| role.name() == name) {
            Some(role) => Ok(role),
            None => {
                crate::fail!("{name:?} is no role: the roles are coordinator, design and judge")
            }
        }
    }

    /// The name of the section of the workflow help that holds this role's
    /// sections: an agent holds the role once it acknowledged that section.
    fn marker(self) -> &'static str {
        GROUPS
            .iter()
            .find(|group| group.roles == [self])
            .map(|group| group.name)
            .expect("each role has a group of its own")
    }
}

/// The page whose sections a group takes.
#[derive(Clone, Copy)]
enum Page {
    Workflow,
    ListsOverview,
}

impl Page {
    fn text(self) -> &'static str {
        match self {
            Page::Workflow => WORKFLOW_HELP,
            Page::ListsOverview => LISTS_OVERVIEW,
        }
    }
}

/// The sections of a page that only the agents of some roles read, kept as one
/// section of the text. Every other section of the page is read by all
/// agents.
struct Group {
    name: &'static str,
    page: Page,
    headings: &'static [&'static str],
    roles: &'static [Role],
}

const GROUPS: [Group; 4] = [
    Group {
        name: "builds help workflow, coordinator",
        page: Page::Workflow,
        headings: &["PLAN, BRIEF, EXECUTE", "DISPATCH", "HEALTH CHECK"],
        roles: &[Role::Coordinator],
    },
    Group {
        name: "builds help workflow, design",
        page: Page::Workflow,
        headings: &["DESIGN CHANGES", "PUBLISHING DESIGNS"],
        roles: &[Role::Design],
    },
    Group {
        name: "builds help workflow, judge",
        page: Page::Workflow,
        headings: &["JUDGING"],
        roles: &[Role::Judge],
    },
    Group {
        name: "builds help lists overview, the order of the stack",
        page: Page::ListsOverview,
        headings: &["EFFECTS AND ORDER OF THE STACK"],
        roles: &[Role::Coordinator, Role::Design, Role::Judge],
    },
];

/// One file or help of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub text: String,
}

impl Section {
    /// The hash of the section's text, which builds keeps with each version
    /// an agent acknowledged, so that the notice of a change names only the
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
    /// The sections that every agent reads, then those of each of <roles>.
    pub fn for_roles(roles: &[Role]) -> Text {
        let section = |page: Page, name: &str, wanted: &dyn Fn(&str) -> bool| Section {
            name: name.to_string(),
            text: chunks(page.text())
                .into_iter()
                .filter(|(heading, _)| wanted(heading))
                .map(|(_, text)| text)
                .collect(),
        };
        let page_without_groups = |page: Page, name: &str| {
            section(page, name, &|heading| {
                !GROUPS.iter().any(|group| group.headings.contains(&heading))
            })
        };
        let mut sections = vec![
            page_without_groups(Page::Workflow, "builds help workflow"),
            page_without_groups(Page::ListsOverview, "builds help lists overview"),
        ];
        for group in &GROUPS {
            if group.roles.iter().any(|role| roles.contains(role)) {
                sections.push(section(group.page, group.name, &|heading| {
                    group.headings.contains(&heading)
                }));
            }
        }
        Text { sections }
    }

    /// The roles whose sections <known> (each section's name and hash) holds.
    pub fn roles_held(known: &[(String, String)]) -> Vec<Role> {
        Role::ALL
            .into_iter()
            .filter(|role| known.iter().any(|(name, _)| name == role.marker()))
            .collect()
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

    /// The sections whose name and hash <known> (each section's name and
    /// hash) lacks: what an agent that acknowledged <known> still has to
    /// read.
    pub fn unread(&self, known: &[(String, String)]) -> Text {
        let names = self.changed_since(known);
        Text {
            sections: self
                .sections
                .iter()
                .filter(|section| names.contains(&section.name))
                .cloned()
                .collect(),
        }
    }

    /// The names of the sections that <acknowledged> (each section's name and
    /// hash as an acknowledgement kept them) holds and this text lacks, in
    /// the order of the names.
    pub fn removed_since(&self, acknowledged: &[(String, String)]) -> Vec<String> {
        acknowledged
            .iter()
            .filter(|(name, _)| !self.sections.iter().any(|section| section.name == *name))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Splits the lines of every section, in the text's order,
    /// into parts of at most `PART_BYTES` bytes. Each section starts with a
    /// line that names it, and a part that starts inside a section says so
    /// on its first line.
    pub fn parts(&self) -> Vec<Vec<String>> {
        let sections = &self.sections;
        let count = sections.len();
        let mut parts: Vec<Vec<String>> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let mut bytes = 0;
        for (index, section) in sections.iter().enumerate() {
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
        "builds ack: part {number} of {count} of version {version}."
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
