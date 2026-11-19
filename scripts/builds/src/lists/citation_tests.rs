use super::*;
use crate::lists::fixtures::{db, read};
use crate::testing::Scratch;

/// A home folder holding ~/.claude/CLAUDE.md and the checkout ~/dev/bae
/// with its CLAUDE.md, and a rules checkout holding one rule and one project
/// file.
struct Bench {
    scratch: Scratch,
    lists: Lists,
}

impl Bench {
    fn new(name: &str) -> Bench {
        let scratch = Scratch::new(name);
        for (path, text) in [
            ("home/.claude/CLAUDE.md", "Stage files by path."),
            (
                "home/dev/bae/CLAUDE.md",
                "No agent edits the main checkout.",
            ),
            ("instructions/rules/alpha/rule-a.md", "**Rule A.**"),
            ("instructions/projects/bae.md", "Project facts."),
        ] {
            let file = scratch.dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        Bench {
            scratch,
            lists: read(&db()),
        }
    }

    fn home(&self) -> PathBuf {
        self.scratch.dir.join("home")
    }

    /// What <sentence> cites.
    fn cites(&self, sentence: &str) -> Vec<Cite> {
        let root = self.scratch.dir.join("instructions");
        let home = self.home();
        Sources::new(&self.lists, &root, &home)
            .cites(sentence)
            .unwrap()
    }
}

fn named(cites: &[Cite]) -> Vec<String> {
    cites
        .iter()
        .map(|cite| cite.named().or(cite.missing()).unwrap())
        .collect()
}

#[test]
fn a_repo_s_claude_md_is_cited_by_its_repo_or_its_path() {
    let b = Bench::new("citation-repo");
    let bae = vec!["the CLAUDE.md of repo bae".to_string()];
    assert_eq!(
        named(&b.cites("Never edit ~/dev/bae (CLAUDE.md of repo bae).")),
        bae
    );
    assert_eq!(
        named(&b.cites("Never edit ~/dev/bae (~/dev/bae/CLAUDE.md).")),
        bae
    );
    let absolute = format!(
        "Never edit it ({}).",
        b.home().join("dev/bae/CLAUDE.md").display()
    );
    assert_eq!(named(&b.cites(&absolute)), bae);
    let missing = b.cites("Never edit it (CLAUDE.md of repo coven).");
    assert!(
        missing[0]
            .missing()
            .is_some_and(|why| why.starts_with("it cites the CLAUDE.md of repo coven, and")),
        "{missing:?}"
    );
}

#[test]
fn the_home_claude_md_is_cited_by_its_path_and_no_other_claude_md_is() {
    let b = Bench::new("citation-home");
    assert_eq!(
        named(&b.cites("Stage by path (~/.claude/CLAUDE.md).")),
        vec!["~/.claude/CLAUDE.md".to_string()]
    );
    for sentence in [
        "Stage by path (CLAUDE.md).",
        "Stage by path (/tmp/x/CLAUDE.md).",
    ] {
        let cites = b.cites(sentence);
        assert!(
            cites[0]
                .missing()
                .is_some_and(|why| why.contains("which is neither ~/.claude/CLAUDE.md")),
            "{sentence}: {cites:?}"
        );
    }
    std::fs::remove_file(b.home().join(".claude/CLAUDE.md")).unwrap();
    let gone = b.cites("Stage by path (~/.claude/CLAUDE.md).");
    assert!(
        gone[0]
            .missing()
            .is_some_and(|why| why.contains("does not exist")),
        "{gone:?}"
    );
}

#[test]
fn a_builds_help_section_is_cited_by_its_page_and_heading() {
    let b = Bench::new("citation-help");
    assert_eq!(
        named(&b.cites("Name the hard limits (builds help workflow, DISPATCH).")),
        vec!["builds help workflow, DISPATCH".to_string()]
    );
    assert_eq!(
        named(&b.cites("Run cargo through the wrapper (`builds help`, CARGO).")),
        vec!["builds help, CARGO".to_string()]
    );
    assert_eq!(
        named(&b.cites("Brief it (`builds help workflow` (PLAN, BRIEF, EXECUTE)).")),
        vec!["builds help workflow, PLAN, BRIEF, EXECUTE".to_string()]
    );
    let missing = b.cites("Do it (builds help workflow, NOWHERE).");
    assert_eq!(
        missing[0].missing().as_deref(),
        Some("it cites the section NOWHERE of builds help workflow, which has no such section")
    );
    assert!(
        b.cites("Read builds help for the commands.").is_empty(),
        "a mention with no section cites nothing"
    );
}

#[test]
fn rule_files_are_cited_by_path_and_decisions_by_number() {
    let b = Bench::new("citation-rules");
    assert_eq!(
        named(&b.cites("Do it (rules/alpha/rule-a.md, projects/bae.md, decision #9).")),
        vec![
            "it cites decision #9, and #9 is no record".to_string(),
            "rule rules/alpha/rule-a.md".to_string(),
            "rule projects/bae.md".to_string(),
        ]
    );
}

#[test]
fn decision_citations_name_decision_and_a_number() {
    assert_eq!(
        decision_ids("Decision #12 and decision#3, not decisions #4, and decision #12 again"),
        vec![Id(12), Id(3)]
    );
    assert_eq!(decision_ids("the decision is yours"), Vec::<Id>::new());
}

#[test]
fn markdown_paths_lose_the_marks_around_them() {
    assert_eq!(
        md_paths("(`rules/a/b.md`), see ~/x/projects/bae.md. and README"),
        vec![
            "rules/a/b.md".to_string(),
            "~/x/projects/bae.md".to_string()
        ]
    );
}
