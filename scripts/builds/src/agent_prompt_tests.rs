use super::*;

fn opening<'a>(
    work: &'a Work,
    items: &'a [Item<'a>],
    brief: &'a Brief<'a>,
    message: Option<&'a str>,
) -> Opening<'a> {
    Opening {
        project: "demo",
        description: "demo is a tool.\nIt runs here.\n",
        work,
        items,
        brief,
        message,
        hard_limits_file: "/rules/projects/demo.md",
        hard_limits: "- Never edit the main checkout.\n",
        trailer: "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>",
        scratch: "/Volumes/Builds/agents/s1/scratch",
    }
}

#[test]
fn an_implementing_prompt_holds_each_part_in_the_order_the_prompt_prints_them() {
    let work = Work::Implementing {
        places: vec![Place {
            worktree: "/w/side-3".into(),
            branch: "work".into(),
        }],
    };
    let items = [
        Item {
            id: 7,
            text: "#7 First item\nwhat it should do\n",
        },
        Item {
            id: 9,
            text: "#9 Second item\n",
        },
    ];
    let brief = Brief {
        name: "plans/briefs/a.md",
        text: "Brief line one.\nBrief line two.\n",
    };
    let text = opening(&work, &items, &brief, Some("Start with item 9.")).text();
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("no {needle:?} in {text}"))
    };
    let order = [
        "demo is a tool.\nIt runs here.",
        "You are an implementing agent of project demo. Run `builds ack --project demo`",
        "Your worktree is /w/side-3, on branch work. Land it with `builds land /w/side-3 --done 7 --done 9 --project demo`.",
        "#7 First item\nwhat it should do",
        "#9 Second item",
        "The brief, plans/briefs/a.md:\n\nBrief line one.\nBrief line two.",
        "Start with item 9.",
        "The hard limits of project demo, from /rules/projects/demo.md:\n\n- Never edit the main checkout.",
        "End every commit message with this line:\nCo-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>",
        "Keep your scratch files in /Volumes/Builds/agents/s1/scratch.",
    ];
    let positions: Vec<usize> = order.iter().map(|needle| at(needle)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{positions:?}"
    );
}

#[test]
fn a_planner_prompt_names_the_coordinator_role_and_no_worktree() {
    let items = [Item {
        id: 7,
        text: "#7 Item\n",
    }];
    let brief = Brief {
        name: "b.md",
        text: "Plan it.\n",
    };
    let text = opening(&Work::Planner, &items, &brief, None).text();
    assert!(text.contains(
        "You are a planner of project demo. Run `builds ack --project demo --role coordinator` and read every part. Then do the items below as builds help workflow, WHAT A PLANNER DOES says."
    ));
    assert!(!text.contains("Your worktree"));
}

#[test]
fn every_section_a_role_line_names_is_a_heading_of_the_workflow_page() {
    let headings = crate::help::sections(crate::help::WORKFLOW_HELP);
    for role in [AgentRole::Implementing, AgentRole::Planner] {
        assert!(headings.contains(&role.section()), "{}", role.section());
        assert!(role.line("demo").contains(role.section()));
    }
}

#[test]
fn a_resume_message_adds_the_new_items_the_land_command_and_the_trailer() {
    let places = [Place {
        worktree: "/w/side-3".into(),
        branch: "work".into(),
    }];
    let new = [Item {
        id: 9,
        text: "#9 New item\n",
    }];
    let text = resume_message(
        "demo",
        "Take this next.\n",
        &places,
        &[7, 9],
        &new,
        "Co-Authored-By: X",
    );
    assert_eq!(
        text,
        "Take this next.\n\nYour worktree is /w/side-3, on branch work. Land it with `builds land /w/side-3 --done 7 --done 9 --project demo`.\n\n#9 New item\n\nEnd every commit message with this line:\nCo-Authored-By: X\n"
    );
    assert_eq!(
        resume_message("demo", "Go on.", &places, &[7], &[], "T"),
        "Go on.\n"
    );
}
