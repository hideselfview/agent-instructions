use super::*;
use crate::lists::citation::CitedDecision;
use crate::lists::cmd_item;
use crate::lists::decision::{NewDecision, add_decision};
use crate::lists::fixtures::{NOON, stamp};
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::view::View;
use crate::review::fakes::{Fake, fake};

const RULED: &str = "You ruled that agents never reword commit messages.";

/// A demo project whose tool asks a reviewer stand-in over a rules checkout
/// that holds rules/alpha/rule-a.md.
struct Bench {
    demo: Demo,
    fake: Fake,
}

fn bench(name: &str) -> Bench {
    let mut demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    demo.tool.reviewer = Reviewer {
        program: fake.program.clone(),
        rules_root: fake.root.clone(),
    };
    Bench { demo, fake }
}

impl Bench {
    /// The reviewer quotes each of <sentences> as a ruling of the person's.
    fn quotes(&self, sentences: &[&str]) {
        let claims: Vec<Value> = sentences
            .iter()
            .map(|sentence| json!({"sentence": sentence, "why": "says the person ruled"}))
            .collect();
        self.fake
            .answer_task("rulings", json!({ "claims": claims }));
    }

    /// Records a decision of the person's and returns its id.
    fn decision(&self, title: &str) -> Id {
        self.demo
            .store
            .write(&stamp(NOON), "decision add", |db| {
                let new = NewDecision {
                    title: title.to_string(),
                    by: "person".into(),
                    body: "The person said so.".into(),
                    question: None,
                    replaces: None,
                    plan: None,
                };
                Ok(add_decision(db, &new, &[], &stamp(NOON))?.decision)
            })
            .unwrap()
    }

    /// `builds question ask` with <details>, blocking nothing.
    fn ask(&self, details: &str) -> Result<String> {
        let file = self.demo.scratch.dir.join("details.md");
        std::fs::write(&file, details).unwrap();
        let file = file.display().to_string();
        self.question(&[
            "ask",
            "--title",
            "Reword the two messages",
            "--kind",
            "question",
            "--effect",
            "cleanup",
            "--details-file",
            &file,
            "--blocks-nothing",
            "the branch waits on it",
        ])
    }

    fn question(&self, args: &[&str]) -> Result<String> {
        crate::lists::question_command::run(
            &self.demo.tool,
            &words(args),
            &self.demo.main,
            &|args| panic!("gh must not run: {args:?}"),
        )
    }

    /// Takes item 1, which it adds, in side worktree 1 with a brief that
    /// holds <brief>.
    fn take(&self, brief: &str) -> Result<()> {
        let side = self
            .demo
            .add_side(1, "agent A", "work")
            .display()
            .to_string();
        let item = self.demo.brief("item.md");
        cmd_item(
            &self.demo.tool,
            &words(&[
                "add",
                "--title",
                "Do the first thing now",
                "--area",
                "Alpha",
                "--details-file",
                &item,
            ]),
        )
        .unwrap();
        let file = self.demo.scratch.dir.join("brief.md");
        std::fs::write(&file, brief).unwrap();
        cmd_item(
            &self.demo.tool,
            &words(&[
                "take",
                "1",
                "--worktree",
                &side,
                "--branch",
                "work",
                "--brief",
                &file.display().to_string(),
            ]),
        )
    }

    /// `builds item check-brief` over a brief that holds <brief>, in side
    /// worktree 1.
    fn check_brief(&self, brief: &str) -> Result<()> {
        let side = self
            .demo
            .add_side(1, "agent A", "work")
            .display()
            .to_string();
        let file = self.demo.scratch.dir.join("brief.md");
        std::fs::write(&file, brief).unwrap();
        cmd_item(
            &self.demo.tool,
            &words(&[
                "check-brief",
                "--worktree",
                &side,
                "--brief",
                &file.display().to_string(),
            ]),
        )
    }

    /// The prompts of the rulings check the commands gave the reviewer.
    fn prompts(&self) -> Vec<String> {
        self.fake
            .prompts()
            .into_iter()
            .filter(|(task, _)| task == "rulings")
            .map(|(_, prompt)| prompt)
            .collect()
    }
}

#[test]
fn a_question_whose_ruling_cites_nothing_is_refused_quoting_the_sentence() {
    let b = bench("rulings-uncited");
    b.quotes(&[RULED]);
    let before = b.demo.state();
    let refused = refusal(b.ask(&format!("The branch rewords two messages. {RULED}\n")));
    assert!(
        refused.contains(&format!("in the question's details: \"{RULED}\"")),
        "{refused}"
    );
    assert!(
        refused.contains("it names no decision, rule file, CLAUDE.md or builds help section"),
        "{refused}"
    );
    assert_eq!(b.demo.state(), before, "a refused ask writes nothing");
    let prompts = b.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].starts_with(TASK) && prompts[0].contains(RULED),
        "the reviewer reads the details: {}",
        prompts[0]
    );
    assert!(
        !prompts[0].contains("standing prohibition"),
        "only a brief's check lists standing prohibitions"
    );
}

#[test]
fn a_question_whose_ruling_cites_a_missing_decision_is_refused() {
    let b = bench("rulings-missing");
    let cited = "You ruled that agents never reword commit messages (decision #40).";
    b.quotes(&[cited]);
    let refused = refusal(b.ask(&format!("{cited}\n")));
    assert!(
        refused.contains("it cites decision #40, and #40 is no record"),
        "{refused}"
    );

    let item = b
        .demo
        .store
        .write(&stamp(NOON), "item", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let on_item = format!("You ruled it out in decision #{}.", item.0);
    b.quotes(&[&on_item]);
    let refused = refusal(b.ask(&format!("{on_item}\n")));
    assert!(
        refused.contains(&format!("it cites decision {item}, and {item} is the item")),
        "{refused}"
    );
    assert!(b.demo.store.load().unwrap().questions.is_empty());
}

#[test]
fn a_question_whose_ruling_cites_a_decision_passes_and_show_prints_its_title() {
    let b = bench("rulings-cited");
    let decision = b.decision("Leave commit messages as they are");
    let cited = format!(
        "You chose to leave landed commit messages alone (decision #{}).",
        decision.0
    );
    b.quotes(&[&cited]);
    let out = b.ask(&format!("{cited}\n")).unwrap();
    assert!(
        out.contains(&format!(
            "\"{cited}\" cites decision {decision} \"Leave commit messages as they are\""
        )),
        "{out}"
    );
    let lists = b.demo.store.load().unwrap();
    let question = lists.questions[0].id;
    let shown = b.question(&["show", &question.0.to_string()]).unwrap();
    assert!(
        shown.contains(&format!(
            "cites decision {decision}: Leave commit messages as they are"
        )),
        "{shown}"
    );
    let view = View::build(&lists, NOON).unwrap();
    assert_eq!(
        view.stack[0].cites,
        vec![CitedDecision::Decision {
            id: decision,
            title: "Leave commit messages as they are".into()
        }]
    );
}

#[test]
fn a_rule_path_counts_as_a_citation_and_a_made_up_one_does_not() {
    let b = bench("rulings-rule");
    let real = "You forbid the bad thing here (rules/alpha/rule-a.md).";
    b.quotes(&[real]);
    let out = b.ask(&format!("{real}\n")).unwrap();
    assert!(
        out.contains(&format!("\"{real}\" cites rule rules/alpha/rule-a.md")),
        "{out}"
    );

    let made_up = "You forbid rewording (plans/briefs/item-591.md).";
    b.quotes(&[made_up]);
    let refused = refusal(b.ask(&format!("{made_up}\n")));
    assert!(
        refused.contains("it cites plans/briefs/item-591.md, which is no rule"),
        "{refused}"
    );
}

#[test]
fn a_quote_that_no_text_holds_is_set_aside() {
    let b = bench("rulings-set-aside");
    b.quotes(&[RULED]);
    let out = b.ask("The branch rewords two messages.\n").unwrap();
    assert!(
        out.contains(&format!(
            "the rulings check set aside a quote it found in no text: \"{RULED}\""
        )),
        "{out}"
    );
}

#[test]
fn an_item_whose_details_state_an_uncited_ruling_is_not_added() {
    let b = bench("rulings-item");
    b.quotes(&[RULED]);
    let details = b.demo.scratch.dir.join("item.md");
    std::fs::write(&details, format!("Reword them. {RULED}\n")).unwrap();
    let refused = refusal(cmd_item(
        &b.demo.tool,
        &words(&[
            "add",
            "--title",
            "Reword the two commit messages",
            "--area",
            "Alpha",
            "--details-file",
            &details.display().to_string(),
        ]),
    ));
    assert!(
        refused.contains(&format!(
            "in the new work item \"Reword the two commit messages\": \"{RULED}\""
        )),
        "{refused}"
    );
    assert!(b.demo.store.load().unwrap().items.is_empty());
}

#[test]
fn a_brief_whose_standing_prohibition_cites_nothing_refuses_the_take() {
    let b = bench("rulings-brief");
    let prohibition = "Agents never reword commit messages.";
    b.quotes(&[prohibition]);
    let refused = refusal(b.take(&format!("# Brief\n\nFix the parser. {prohibition}\n")));
    assert!(refused.contains(&format!("\"{prohibition}\"")), "{refused}");
    assert!(
        b.demo.store.load().unwrap().items[0].running.is_empty(),
        "a refused take starts nothing"
    );
    let prompts = b.prompts();
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt.contains(prohibition) && prompt.contains("standing prohibition")),
        "a brief's check lists standing prohibitions: {prompts:?}"
    );
}

/// The hard limits of the coordinator's brief template, each citing where
/// it is written.
const HARD_LIMITS: [&str; 6] = [
    "Never edit, build or commit in ~/dev/bae, ~/dev/coven or ~/dev/agent-instructions (builds help workflow, WORKTREES).",
    "Cargo only through the repo's wrapper (`builds help`, CARGO).",
    "Stage by path; no `git add -A`, no stash, no --no-verify (builds help workflow, DISPATCH).",
    "Never write to the person's keychain (CLAUDE.md of repo demo).",
    "Pass `--agent <your id>` on every builds command (builds help, ACKNOWLEDGING THE RULES).",
    "Commit messages stay brief (~/.claude/CLAUDE.md).",
];

#[test]
fn a_brief_whose_hard_limits_cite_claude_md_and_builds_help_is_taken() {
    let b = bench("rulings-hard-limits");
    let home = &b.demo.tool.places.home;
    for file in [".claude/CLAUDE.md", "dev/demo/CLAUDE.md"] {
        std::fs::create_dir_all(home.join(file).parent().unwrap()).unwrap();
        std::fs::write(home.join(file), "The person's instructions.").unwrap();
    }
    b.quotes(&HARD_LIMITS);
    b.take(&format!(
        "# Brief\n\nFix the parser.\n\n{}\n",
        HARD_LIMITS.join(" ")
    ))
    .unwrap();
    assert_eq!(b.demo.store.load().unwrap().items[0].running.len(), 1);
}

#[test]
fn check_brief_refuses_a_brief_whose_standing_prohibition_cites_nothing() {
    let b = bench("rulings-check-brief");
    let prohibition = "Agents never reword commit messages.";
    b.quotes(&[prohibition]);
    let refused = refusal(b.check_brief(&format!("# Brief\n\nFix the parser. {prohibition}\n")));
    assert!(refused.contains(&format!("\"{prohibition}\"")), "{refused}");
    assert!(b.demo.store.load().unwrap().items.is_empty());
}
