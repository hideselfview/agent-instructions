//! The pages `builds help [TOPIC]` prints and the sections they hold, which
//! main prints, `builds ack` asks agents to read and a ruling may cite
//! (`lists::rulings`).

/// `builds help`.
pub const HELP: &str = include_str!("help.txt");
/// What the lists are and who writes them, which every agent reads.
pub const LISTS_OVERVIEW: &str = include_str!("lists/lists-overview.txt");
/// `builds help lists`: the overview, then each command.
pub const LISTS_HELP: &str = concat!(
    include_str!("lists/lists-overview.txt"),
    "\n\n",
    include_str!("lists/lists-help.txt")
);
/// `builds help workflow`.
pub const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
/// `builds help briefs`.
pub const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

/// The page `builds help <topic>` prints; <topic> None is `builds help`. None
/// when no page has the topic.
pub fn page(topic: Option<&str>) -> Option<&'static str> {
    match topic {
        None => Some(HELP),
        Some("lists") => Some(LISTS_HELP),
        Some("workflow") => Some(WORKFLOW_HELP),
        Some("briefs") => Some(BRIEFS_HELP),
        Some(_) => None,
    }
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

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
