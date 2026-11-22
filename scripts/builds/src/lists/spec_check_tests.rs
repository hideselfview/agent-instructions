use super::*;
use crate::lists::cmd_item;
use crate::lists::fixtures::{NOON, Refusals, row, stamp, v};
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::project::{GlobKind, parse_globs, set_design_globs};
use crate::review::fakes::{Fake, fake};
use crate::testing::git as run_git;

const SPEC: &str = "# Demo spec

## Import list

A release that finishes importing stays selected,
with its pane open.

## Search

Search matches titles and artists.
";

const BRIEF: &str = "# Brief

A release that finishes importing leaves the selection.
The search box keeps its text after a search.
Move the selection code into its own module.
";

const CONFLICT: &str = "A release that finishes importing leaves the selection.";
const UNSTATED: &str = "The search box keeps its text after a search.";
const SPEC_TEXT: &str = "A release that finishes importing stays selected, with its pane open.";

/// A demo project whose repo holds spec/demo.md, with item 1 to take in its
/// side worktree 1, the brief, and a reviewer stand-in over a rules checkout
/// that holds the spec rule.
struct Bench {
    demo: Demo,
    fake: Fake,
    side: String,
    brief: String,
}

fn bench(name: &str, spec_globs: bool) -> Bench {
    let demo = demo(name);
    std::fs::create_dir_all(demo.main.join("spec")).unwrap();
    std::fs::write(demo.main.join("spec/demo.md"), SPEC).unwrap();
    run_git(&demo.main, &["add", "spec/demo.md"]);
    run_git(&demo.main, &["commit", "-q", "-m", "spec"]);
    if spec_globs {
        demo.store
            .write(&stamp(NOON), "lists design-globs demo", |db| {
                set_design_globs(
                    db,
                    GlobKind::Spec,
                    &parse_globs("spec-files", "demo:spec/*.md")?,
                )
            })
            .unwrap();
    }
    let side = demo.add_side(1, "agent A", "work").display().to_string();
    let details = demo.brief("details.txt");
    cmd_item(
        &demo.tool,
        &words(&[
            "add",
            "--title",
            "Do the first thing now",
            "--area",
            "Alpha",
            "--details-file",
            &details,
        ]),
    )
    .unwrap();
    let brief = demo.scratch.dir.join("brief.md");
    std::fs::write(&brief, BRIEF).unwrap();
    let fake = fake(&demo.scratch.dir);
    std::fs::write(
        fake.root.join("rules/alpha").join(format!("{RULE}.md")),
        "---\ndigest: d\npaths:\n  - '**/*'\nblocking: true\n---\n\n**Spec.** Every change agrees with its product spec.\n",
    )
    .unwrap();
    Bench {
        demo,
        fake,
        side,
        brief: brief.display().to_string(),
    }
}

impl Bench {
    /// What the reviewer answers.
    fn answer(&self, conflicts: Value, unstated: Value) {
        self.fake.answer_task(
            "spec-check",
            json!({"conflicts": conflicts, "unstated": unstated}),
        );
    }

    /// `builds item take 1` with the brief, in side worktree 1, with <extra>.
    fn take(&self, extra: &[&str]) -> Result<()> {
        let root = self.fake.root.display().to_string();
        let program = self.fake.program.executable().display().to_string();
        let mut args = vec![
            "take",
            "1",
            "--worktree",
            &self.side,
            "--branch",
            "work",
            "--brief",
            &self.brief,
            "--rules-root",
            &root,
            "--reviewer",
            &program,
        ];
        args.extend_from_slice(extra);
        cmd_item(&self.demo.tool, &words(&args))
    }

    fn lists(&self) -> Lists {
        self.demo.store.load().unwrap()
    }

    /// The prompts of the spec check the take gave the reviewer, without
    /// those of the rulings check.
    fn prompts(&self) -> Vec<String> {
        std::fs::read_dir(self.fake.dir.join("calls"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .filter(|prompt| prompt.starts_with(TASK))
            .collect()
    }

    /// The target of side worktree 1.
    fn target(&self) -> Target {
        let lists = self.lists();
        let mut targets =
            targets(&lists, &[("demo".to_string(), PathBuf::from(&self.side))]).unwrap();
        assert_eq!(targets.len(), 1);
        targets.remove(0)
    }

    /// What the check of the brief in side worktree 1 found.
    fn check(&self) -> Checked {
        let setup = Setup::load(&self.fake.root).unwrap();
        let brief = Brief::read(&self.brief, Path::new(&self.brief)).unwrap();
        check_all(&self.fake.program, &setup, &brief, &[self.target()]).remove(0)
    }
}

fn conflict(spec: &str) -> Value {
    json!([{
        "brief": CONFLICT,
        "spec_file": "spec/demo.md",
        "spec": spec,
        "why": "The brief clears the selection; the spec keeps it."
    }])
}

#[test]
fn a_brief_that_contradicts_the_spec_refuses_the_take_quoting_both_texts() {
    let b = bench("spec-check-conflict", true);
    b.answer(conflict(SPEC_TEXT), json!([]));
    let refused = refusal(b.take(&[]));
    assert!(
        refused.contains(&format!("the brief says: \"{CONFLICT}\"")),
        "{refused}"
    );
    assert!(
        refused.contains(&format!("spec/demo.md:5 (repo demo) says: \"{SPEC_TEXT}\"")),
        "{refused}"
    );
    assert!(refused.contains("--spec-decided N"), "{refused}");
    let lists = b.lists();
    let item = &lists.items[0];
    assert!(item.running.is_empty(), "a refused take starts nothing");
    assert_eq!(
        item.spec_checks.len(),
        1,
        "the take records the refused check"
    );
    let check = &item.spec_checks[0];
    assert_eq!(
        (check.conflicts, check.unstated, check.decided),
        (1, 0, None)
    );
    let prompts = b.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Spec files: spec/demo.md") && prompts[0].contains(BRIEF.trim_end()),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("Every change agrees with its product spec."),
        "the reviewer follows the rule's text"
    );
}

#[test]
fn a_brief_that_agrees_with_the_spec_passes_and_the_check_is_recorded() {
    let b = bench("spec-check-agrees", true);
    b.answer(json!([]), json!([]));
    b.take(&[]).unwrap();
    let lists = b.lists();
    let item = &lists.items[0];
    assert_eq!(item.running.len(), 1);
    let check = &item.spec_checks[0];
    assert_eq!(
        (check.conflicts, check.unstated, check.failure.as_deref()),
        (0, 0, None)
    );
    assert_eq!(check.repo, "demo");
    assert!(
        crate::lists::item_show::render_item(&lists, Id(1), 0)
            .unwrap()
            .contains("spec check 1 of "),
        "item show lists the check"
    );
}

#[test]
fn an_unstated_behavior_is_printed_and_the_take_passes() {
    let b = bench("spec-check-unstated", true);
    b.answer(
        json!([]),
        json!([{"brief": UNSTATED, "why": "searched for search box and text"}]),
    );
    b.take(&[]).unwrap();
    let lists = b.lists();
    assert_eq!(lists.items[0].running.len(), 1);
    assert_eq!(lists.items[0].spec_checks[0].unstated, 1);
    let notes = notes(&[b.check()]);
    assert_eq!(
        notes,
        vec![format!(
            "the spec of repo demo (spec/demo.md) states nothing on this behavior of the brief; tell the agent to add it to the spec in its commit: \"{UNSTATED}\""
        )]
    );
}

#[test]
fn a_project_with_no_spec_files_skips_the_check() {
    let b = bench("spec-check-no-spec", false);
    let decided = refusal(b.take(&["--spec-decided", "1"]));
    assert!(decided.contains("names the item #1"), "{decided}");
    b.take(&[]).unwrap();
    let lists = b.lists();
    assert_eq!(lists.items[0].running.len(), 1);
    assert!(lists.items[0].spec_checks.is_empty());
    assert!(b.prompts().is_empty(), "no reviewer ran");
}

#[test]
fn the_decided_flag_records_its_ref_and_takes_the_item_despite_the_conflict() {
    let b = bench("spec-check-decided", true);
    b.answer(conflict(SPEC_TEXT), json!([]));
    let question = b
        .demo
        .store
        .write(&stamp(NOON), "question ask", |db| {
            Ok(crate::lists::fixtures::question(db))
        })
        .unwrap();
    let not_a_choice = refusal(b.take(&["--spec-decided", "1"]));
    assert!(not_a_choice.contains("names the item #1"), "{not_a_choice}");
    assert!(
        b.prompts().is_empty(),
        "a bad ref stops the take before the reviewer runs"
    );
    b.take(&["--spec-decided", &question.0.to_string()])
        .unwrap();
    let lists = b.lists();
    let item = &lists.items[0];
    assert_eq!(item.running.len(), 1);
    let check = &item.spec_checks[0];
    assert_eq!((check.conflicts, check.decided), (1, Some(question)));
    assert!(
        crate::lists::item_show::render_item(&lists, Id(1), 0)
            .unwrap()
            .contains(&format!("the person decided the conflicts in {question}")),
    );
}

#[test]
fn a_decided_ref_with_no_spec_check_to_record_it_on_is_refused() {
    let b = bench("spec-check-decided-no-spec", false);
    let question = b
        .demo
        .store
        .write(&stamp(NOON), "question ask", |db| {
            Ok(crate::lists::fixtures::question(db))
        })
        .unwrap();
    let said = refusal(b.take(&["--spec-decided", &question.0.to_string()]));
    assert!(said.contains("no spec check runs"), "{said}");
    assert!(b.lists().items[0].running.is_empty());
}

#[test]
fn a_conflict_whose_spec_quote_is_not_in_the_spec_is_set_aside_and_refuses_nothing() {
    let b = bench("spec-check-set-aside", true);
    b.answer(
        conflict("A release that finishes importing goes away."),
        json!([]),
    );
    b.take(&[]).unwrap();
    let lists = b.lists();
    assert_eq!(lists.items[0].running.len(), 1);
    assert_eq!(lists.items[0].spec_checks[0].conflicts, 0);
    let checked = b.check();
    assert_eq!(
        checked.findings[0].kind.set_aside(),
        Some("spec/demo.md holds no such text")
    );
    assert_eq!(
        notes(&[checked]),
        vec![format!(
            "the spec check set aside a conflict in repo demo because spec/demo.md holds no such text: \"{CONFLICT}\""
        )]
    );
}

#[test]
fn a_check_that_fails_refuses_the_take_and_is_recorded() {
    let b = bench("spec-check-fails", true);
    b.answer(json!([]), json!([]));
    b.fake.fail("spec-check");
    let refused = refusal(b.take(&[]));
    assert!(refused.contains("gave no usable answer"), "{refused}");
    let lists = b.lists();
    assert!(lists.items[0].running.is_empty());
    assert!(lists.items[0].spec_checks[0].failure.is_some());
}

#[test]
fn spec_globs_that_match_no_tracked_file_fail() {
    let b = bench("spec-check-no-file", true);
    b.demo
        .store
        .write(&stamp(NOON), "lists design-globs demo", |db| {
            set_design_globs(
                db,
                GlobKind::Spec,
                &parse_globs("spec-files", "demo:docs/*.md")?,
            )
        })
        .unwrap();
    let said = refusal(targets(
        &b.lists(),
        &[("demo".to_string(), PathBuf::from(&b.side))],
    ));
    assert!(said.contains("tracks none of them"), "{said}");
}

#[test]
fn a_quote_is_found_on_the_line_it_starts_on_across_lines_and_marks() {
    assert_eq!(line_of(SPEC, SPEC_TEXT), Some(5));
    assert_eq!(line_of(SPEC, "Search matches *titles*"), Some(10));
    assert_eq!(line_of(SPEC, "with its pane open. ## Search"), Some(6));
    assert_eq!(line_of(SPEC, "nothing like this"), None);
    assert_eq!(line_of(SPEC, "  "), None);
}

/// Records one check of a brief in bae for item <item>, with one
/// conflict and one unstated behavior, through `record`.
pub fn record_one(db: &Connection, item: Id, model: &str, usage: Usage) {
    let checked = Checked {
        target: Target {
            repo: "bae".to_string(),
            worktree: PathBuf::from("/d/wt"),
            head: "a".repeat(40),
            spec_files: vec!["spec/demo.md".to_string()],
        },
        model: model.to_string(),
        rules_version: String::new(),
        system: "s".to_string(),
        message: "m".to_string(),
        output: "{}".to_string(),
        usage,
        failure: None,
        findings: vec![
            Finding {
                kind: FindingKind::Conflict(SpecQuote {
                    file: "spec/demo.md".to_string(),
                    quote: SPEC_TEXT.to_string(),
                    line: Quotes::Found(5),
                }),
                brief_quote: CONFLICT.to_string(),
                why: "w".to_string(),
            },
            Finding {
                kind: FindingKind::Unstated(Quotes::Found(())),
                brief_quote: UNSTATED.to_string(),
                why: "w".to_string(),
            },
        ],
    };
    let brief = Brief {
        name: "plans/briefs/b.md".to_string(),
        text: BRIEF.to_string(),
    };
    let ids = record(db, &stamp(NOON), &brief, &[item], &[checked], None).unwrap();
    let all = read_all(db).unwrap();
    let recorded = all[&item].iter().find(|check| check.id == ids[0]).unwrap();
    assert_eq!((recorded.conflicts, recorded.unstated), (1, 1));
}

#[test]
fn the_tables_refuse_a_row_that_breaks_a_constraint() {
    let db = crate::lists::fixtures::db();
    let item = crate::lists::fixtures::item(&db, Vec::new());
    record_one(
        &db,
        item,
        "opus",
        Usage {
            cost_microdollars: Some(5),
            ..Usage::ZERO
        },
    );
    let check = [
        ("at", v(1)),
        ("by", v("tester")),
        ("brief", v("b.md")),
        ("brief_text", v("t")),
        ("repo", v("bae")),
        ("worktree", v("/d/wt")),
        ("head", v("b".repeat(40))),
        ("spec_files", v("[]")),
        ("model", v("opus")),
        ("rules_version", v("")),
        ("system", v("s")),
        ("message", v("m")),
        ("output", v("o")),
        ("tokens_in", v(0)),
        ("tokens_out", v(0)),
        ("tokens_cached", v(0)),
        ("tokens_written", v(0)),
        ("cost_microdollars", v(0)),
    ];
    let mut refused = Refusals::new(include_str!("spec_check.sql"));
    for (name, over) in [
        ("spec_check_by_says_something", vec![("by", v(" "))]),
        ("spec_check_brief_says_something", vec![("brief", v(""))]),
        (
            "spec_check_worktree_is_absolute",
            vec![("worktree", v("d/wt"))],
        ),
        ("spec_check_head_is_a_commit", vec![("head", v("abc"))]),
        (
            "spec_check_tokens_in_not_negative",
            vec![("tokens_in", v(-1))],
        ),
        (
            "spec_check_tokens_out_not_negative",
            vec![("tokens_out", v(-1))],
        ),
        (
            "spec_check_tokens_cached_not_negative",
            vec![("tokens_cached", v(-1))],
        ),
        (
            "spec_check_tokens_written_not_negative",
            vec![("tokens_written", v(-1))],
        ),
        (
            "spec_check_tokens_uncached_not_negative",
            vec![("tokens_uncached", v(-1))],
        ),
        (
            "spec_check_cost_not_negative",
            vec![("cost_microdollars", v(-1))],
        ),
        (
            "spec_check_decided_by_a_decision_or_question",
            vec![("decided_id", v(1)), ("decided_kind", v("item"))],
        ),
        ("spec_check_decided_whole", vec![("decided_id", v(1))]),
    ] {
        refused.check(name, plant(&db, "spec_check", &row(&check, &over)));
    }
    refused.check_as(
        "spec_check_decided_names_its_record",
        "FOREIGN KEY constraint failed",
        plant(
            &db,
            "spec_check",
            &row(
                &check,
                &[("decided_id", v(99)), ("decided_kind", v("decision"))],
            ),
        ),
    );
    let check_id: i64 = db
        .query_row("SELECT id FROM spec_check", [], |r| r.get(0))
        .unwrap();
    let null = rusqlite::types::Value::Null;
    let finding = [
        ("check_id", v(check_id)),
        ("kind", v("conflict")),
        ("brief_quote", v("b")),
        ("spec_file", v("spec/demo.md")),
        ("spec_line", v(3)),
        ("spec_quote", v("s")),
        ("why", v("w")),
    ];
    for (name, over) in [
        ("spec_check_finding_kind_known", vec![("kind", v("other"))]),
        (
            "spec_check_finding_line_positive",
            vec![("spec_line", v(0))],
        ),
        (
            "spec_check_finding_a_conflict_quotes_the_spec",
            vec![("spec_quote", null.clone())],
        ),
        (
            "spec_check_finding_a_conflict_quotes_the_spec",
            vec![("kind", v("unstated")), ("spec_line", null.clone())],
        ),
        (
            "spec_check_finding_a_kept_conflict_has_its_line",
            vec![("spec_line", null.clone())],
        ),
        (
            "spec_check_finding_a_kept_conflict_has_its_line",
            vec![("check_failed", v("no such text"))],
        ),
    ] {
        refused.check(
            name,
            plant(&db, "spec_check_finding", &row(&finding, &over)),
        );
    }
    plant(&db, "spec_check_finding", &finding).unwrap();
    refused.done();
}

#[test]
fn recording_keeps_unknown_cost_distinct_from_zero() {
    let db = crate::lists::fixtures::db();
    let item = crate::lists::fixtures::item(&db, Vec::new());
    record_one(&db, item, "gpt-6.2-sol", Usage::default());
    // Each attempt belongs to the same item and has its own recorded cost.
    record_one(&db, item, "opus", Usage::ZERO);
    assert_eq!(recorded_usage(&db), [(None, None), (Some(0), Some(0))]);
}

/// Plants a check in the schema before the token counters could be unknown.
pub fn plant_before_unreported_usage(db: &Connection, item: Id, model: &str) {
    db.execute(
        r#"INSERT INTO spec_check (at, by, brief, brief_text, repo, worktree, head, spec_files, model, rules_version, system, message, output, tokens_in, tokens_out, tokens_cached, tokens_written, cost_microdollars)
         VALUES (?1, 'tester', 'plans/briefs/b.md', ?2, 'bae', '/d/wt', ?3, '["spec/demo.md"]', ?4, '', 's', 'm', '{}', 0, 0, 0, 0, 0)"#,
        rusqlite::params![NOON, BRIEF, "a".repeat(40), model],
    ).unwrap();
    let check = db.last_insert_rowid();
    db.execute(
        "INSERT INTO spec_check_item (check_id, item_id) VALUES (?1, ?2)",
        rusqlite::params![check, item],
    )
    .unwrap();
    db.execute(
        "INSERT INTO spec_check_finding (check_id, kind, brief_quote, spec_file, spec_line, spec_quote, why)
         VALUES (?1, 'conflict', ?2, 'spec/demo.md', 5, ?3, 'w'), (?1, 'unstated', ?4, NULL, NULL, NULL, 'w')",
        rusqlite::params![check, CONFLICT, SPEC_TEXT, UNSTATED],
    ).unwrap();
}

pub fn recorded_usage(db: &Connection) -> Vec<(Option<i64>, Option<i64>)> {
    db.prepare("SELECT tokens_in, cost_microdollars FROM spec_check ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}
