//! The opening prompt of an agent that `builds agent run` starts, and the
//! message that `builds agent resume` adds items with. builds writes both
//! from what the store and the files hold: no template file exists, and the
//! only text the code holds is each role's opening line.

use crate::agent_record::AgentRole;

/// The section of `builds help workflow` that says what an implementing agent
/// does.
pub const IMPLEMENTING_SECTION: &str = "WHAT AN IMPLEMENTING AGENT DOES";

/// The section of `builds help workflow` that says what a planner does.
pub const PLANNER_SECTION: &str = "WHAT A PLANNER DOES";

impl AgentRole {
    /// The section of the workflow page that holds this role's steps.
    pub fn section(self) -> &'static str {
        match self {
            AgentRole::Implementing => IMPLEMENTING_SECTION,
            AgentRole::Planner => PLANNER_SECTION,
        }
    }

    /// The role's opening line, which names the commands and the section the
    /// agent reads first.
    pub fn line(self, project: &str) -> String {
        match self {
            AgentRole::Implementing => format!(
                "You are an implementing agent of project {project}. Run `builds ack --project {project}` and read every part. Then do the items below as builds help workflow, {} says.",
                self.section()
            ),
            AgentRole::Planner => format!(
                "You are a planner of project {project}. Run `builds ack --project {project} --role coordinator` and read every part. Then do the items below as builds help workflow, {} says.",
                self.section()
            ),
        }
    }
}

/// A worktree where the items run, with its branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub worktree: String,
    pub branch: String,
}

/// What the opening prompt holds, in the order it prints it.
pub struct Opening<'a> {
    pub project: &'a str,
    pub description: &'a str,
    pub role: AgentRole,
    pub places: &'a [Place],
    pub items: &'a [Item<'a>],
    pub brief: &'a Brief<'a>,
    /// The coordinator's own words for this start, after the brief.
    pub message: Option<&'a str>,
    pub hard_limits_file: &'a str,
    pub hard_limits: &'a str,
    pub trailer: &'a str,
    pub scratch: &'a str,
}

/// An item, by its id and its `builds item show` text.
pub struct Item<'a> {
    pub id: u32,
    pub text: &'a str,
}

/// A brief, by its name and its text.
pub struct Brief<'a> {
    pub name: &'a str,
    pub text: &'a str,
}

/// The worktrees, the branch of each and the command that lands it with
/// <ids>, the items of the agent.
pub fn places(project: &str, places: &[Place], ids: &[u32]) -> String {
    let done: String = ids.iter().map(|id| format!(" --done {id}")).collect();
    places
        .iter()
        .map(|place| {
            format!(
                "Your worktree is {}, on branch {}. Land it with `builds land {}{done} --project {project}`.",
                place.worktree, place.branch, place.worktree
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn items_and_brief(items: &[Item], brief: &Brief) -> String {
    let mut text = String::new();
    for item in items {
        text.push_str(item.text.trim_end());
        text.push_str("\n\n");
    }
    text.push_str(&format!(
        "The brief, {}:\n\n{}",
        brief.name,
        brief.text.trim_end()
    ));
    text
}

fn trailer(trailer: &str) -> String {
    format!("End every commit message with this line:\n{trailer}")
}

impl Opening<'_> {
    /// The prompt: the project's description, the role's line, the worktrees
    /// of an implementing agent, each item and the brief whole, the project's
    /// hard limits, the commit trailer and the scratch folder.
    pub fn text(&self) -> String {
        let mut parts = vec![
            self.description.trim_end().to_string(),
            self.role.line(self.project),
        ];
        if self.role == AgentRole::Implementing {
            let ids: Vec<u32> = self.items.iter().map(|item| item.id).collect();
            parts.push(places(self.project, self.places, &ids));
        }
        parts.push(items_and_brief(self.items, self.brief));
        if let Some(message) = self.message {
            parts.push(message.trim_end().to_string());
        }
        parts.push(format!(
            "The hard limits of project {}, from {}:\n\n{}",
            self.project,
            self.hard_limits_file,
            self.hard_limits.trim_end()
        ));
        parts.push(trailer(self.trailer));
        parts.push(format!("Keep your scratch files in {}.", self.scratch));
        parts.join("\n\n") + "\n"
    }
}

/// The message that gives a resumed agent <message>, then, for each of the
/// <new_items>, the worktrees with the land command for <all_ids>, the item
/// text and the commit trailer again.
pub fn resume_message(
    project: &str,
    message: &str,
    place_list: &[Place],
    all_ids: &[u32],
    new_items: &[Item],
    trailer_line: &str,
) -> String {
    let mut parts = vec![message.trim_end().to_string()];
    if !new_items.is_empty() {
        if !place_list.is_empty() {
            parts.push(places(project, place_list, all_ids));
        }
        parts.push(
            new_items
                .iter()
                .map(|item| item.text.trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        parts.push(trailer(trailer_line));
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n\n") + "\n"
}

#[cfg(test)]
#[path = "agent_prompt_tests.rs"]
mod tests;
