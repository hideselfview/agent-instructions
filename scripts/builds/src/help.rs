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
/// `builds help workflow`, before the section builds adds from the reviews'
/// records (`review::workflow_section`).
pub const WORKFLOW_HELP: &str = include_str!("workflow-help.txt");
/// `builds help briefs`.
pub const BRIEFS_HELP: &str = include_str!("briefs-help.txt");

/// The page `builds help <topic>` prints, without the section `workflow`
/// adds from the reviews' records; <topic> None is `builds help`. None when
/// no page has the topic.
pub fn page(topic: Option<&str>) -> Option<&'static str> {
    match topic {
        None => Some(HELP),
        Some("lists") => Some(LISTS_HELP),
        Some("workflow") => Some(WORKFLOW_HELP),
        Some("briefs") => Some(BRIEFS_HELP),
        Some(_) => None,
    }
}

/// The headings of the sections of <page>: each line that starts at the
/// margin and holds capital letters, spaces and commas only.
pub fn sections(page: &str) -> Vec<&str> {
    page.lines()
        .filter(|line| {
            line.starts_with(|c: char| c.is_ascii_uppercase())
                && line
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == ' ' || c == ',')
        })
        .map(str::trim_end)
        .collect()
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
