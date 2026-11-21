use super::*;
use crate::lists::design_command::run_as;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::lists::{Id, Stamp};
use crate::review::Reviewer;
use crate::review::fakes::{Fake, Raised, design_rule_file, fake};
use crate::testing::git;
use serde_json::json;
use std::path::PathBuf;

const BOARD: &str = "design/mocks/A.dc.html";

const RAW_COLOR: Raised = (
    "Draw boards with tokens.",
    BOARD,
    1,
    "color: red",
    "The board draws a raw color.",
);

/// A demo whose repo keeps its boards under design/mocks/, a reviewer whose
/// group alpha covers them with rule-d, which judges design files, and
/// answers no finding, and a side worktree on branch `work`.
fn designed(name: &str) -> (Demo, Fake, PathBuf) {
    let mut demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    design_rule_file(
        &fake.root,
        "alpha",
        "rule-d",
        "design/**",
        "**Rule D.** Draw boards with tokens.",
    );
    fake.answer("alpha", &[("rule-a", None), ("rule-d", None)]);
    demo.tool.reviewer = Reviewer {
        program: fake.program.clone(),
        rules_root: fake.root.clone(),
    };
    demo.store
        .write(&stamp(), "design-globs", |db| {
            crate::lists::project::set_design_globs(
                db,
                crate::lists::project::GlobKind::Place,
                &[(
                    "demo".to_string(),
                    vec!["design/mocks/*.dc.html".to_string()],
                )]
                .into(),
            )
        })
        .unwrap();
    let side = demo.add_side(1, "designer", "work");
    (demo, fake, side)
}

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

/// Commits <body> as <path> in <side>.
fn draw(side: &Path, path: &str, body: &str) {
    let file = side.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, body).unwrap();
    git(side, &["add", path]);
    git(side, &["commit", "-q", "-m", &format!("Draw {path}")]);
}

/// Runs `builds design <args>` as the designer, as no agent.
fn design_command(demo: &Demo, args: &[&str]) -> crate::fail::Result<String> {
    let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    args.extend(["--by".to_string(), "designer".to_string()]);
    run_as(&demo.tool, &args, || Ok(None), || Ok(None))
}

/// Records, or with <id> amends, the change that draws BOARD over the
/// branch in <side>, and returns what the command printed.
fn record(demo: &Demo, side: &Path, id: Option<Id>) -> String {
    let request = json!({
        "title": "Cover grid", "details": "The grid draws wider gaps.",
        "worktree": side.display().to_string(), "range": "origin/main..HEAD",
        "boards": { BOARD: { "link": "https://canvas.example/#a" } },
        "review": { "heading": "Cover grid", "boards": [
            { "board": BOARD, "caption": "the grid", "theme": "dark", "circles": [{ "selector": "#r" }] }
        ] }
    });
    let file = demo.scratch.dir.join("request.json");
    std::fs::write(&file, request.to_string()).unwrap();
    let file = file.display().to_string();
    match id {
        None => design_command(demo, &["record", "--request", &file]),
        Some(id) => design_command(demo, &["amend", &id.0.to_string(), "--request", &file]),
    }
    .unwrap()
}

fn standing(demo: &Demo, fake: &Fake) -> Standing {
    let lists = demo.store.load().unwrap();
    let material = Material::load(&fake.root, "demo").unwrap();
    read(
        &demo.store,
        &lists,
        lists.design(Id(1)).unwrap(),
        &material,
        &crate::lists::design_drawn::NeverAlike,
    )
    .unwrap()
}

fn findings(demo: &Demo) -> Vec<crate::review::record::Finding> {
    let conn = demo.store.open_existing().unwrap();
    let change = Change {
        repo: "demo",
        key: "branch:work",
    };
    let landing = crate::review::record::open_landing(&conn, change)
        .unwrap()
        .unwrap();
    crate::review::record::findings_of_landing(&conn, landing).unwrap()
}

/// Record starts the rules review of the change's branch, and the change
/// waits on the person's review only once that review has read the branch
/// as it is and no finding waits; a later commit takes it off again until a
/// review reads it (decision #362).
#[test]
fn a_design_change_waits_on_the_person_only_while_its_rules_review_takes_the_branch() {
    let (demo, fake, side) = designed("design-review-first");
    draw(&side, BOARD, "<div style=\"color: red\"></div>\n");
    fake.answer("alpha", &[("rule-a", None), ("rule-d", Some(RAW_COLOR))]);
    let recorded = record(&demo, &side, None);
    assert!(recorded.contains("review of branch:work"), "{recorded}");
    assert!(recorded.contains("in review"), "{recorded}");
    let found = findings(&demo);
    assert_eq!(found.len(), 1, "{found:?}");
    let unsettled = standing(&demo, &fake);
    assert!(
        matches!(&unsettled, Standing::InReview(Verdict::Unfinished(_, u)) if u.unsettled.len() == 1),
        "{unsettled:?}"
    );
    assert!(!unsettled.lacks_a_read());
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle", |db| {
            crate::review::record::settle(db, found[0].id, "confirmed", "real", NOON, "judge")
        })
        .unwrap();
    let open = standing(&demo, &fake);
    assert!(
        matches!(&open, Standing::InReview(Verdict::Unfinished(_, u)) if u.open.len() == 1),
        "{open:?}"
    );
    // The fix: the amend reads it in a new run, which closes the finding.
    draw(&side, BOARD, "<div class=\"accent\"></div>\n");
    fake.answer_task(
        "alpha",
        json!({
            "rules": [
                {"rule": "rule-a", "verdict": "none", "findings": []},
                {"rule": "rule-d", "verdict": "none", "findings": []}
            ],
            "closed": [found[0].id],
        }),
    );
    let amended = record(&demo, &side, Some(Id(1)));
    assert!(amended.contains("review of branch:work"), "{amended}");
    assert!(
        amended.contains("waits on the person's review"),
        "{amended}"
    );
    assert_eq!(standing(&demo, &fake), Standing::Read);
    let shown = design_command(&demo, &["show", "1", "--project", "demo"]).unwrap();
    assert!(shown.contains("waits on the person's review"), "{shown}");
    // A commit after the review takes the change off Waiting on you.
    draw(&side, "notes.rs", "fn main() {}\n");
    let later = standing(&demo, &fake);
    assert!(later.lacks_a_read(), "{later:?}");
    let shown = design_command(&demo, &["show", "1", "--project", "demo"]).unwrap();
    assert!(shown.contains("in review"), "{shown}");
}

/// A change whose worktree has another branch checked out is in review: no
/// review of that branch reads the change.
#[test]
fn a_design_change_whose_worktree_holds_other_commits_is_in_review() {
    let (demo, fake, side) = designed("design-review-elsewhere");
    draw(&side, BOARD, "<div class=\"accent\"></div>\n");
    let recorded = record(&demo, &side, None);
    assert!(
        recorded.contains("waits on the person's review"),
        "{recorded}"
    );
    assert_eq!(standing(&demo, &fake), Standing::Read);
    git(&side, &["checkout", "-q", "-b", "other", "origin/main"]);
    draw(&side, "notes.rs", "fn main() {}\n");
    assert_eq!(standing(&demo, &fake), Standing::Uncovered);
}
