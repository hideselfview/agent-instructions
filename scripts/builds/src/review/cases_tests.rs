use super::*;

#[test]
fn a_case_filter_cannot_keep_an_undecodable_header_with_the_previous_file() {
    let patch = concat!(
        "diff --git a/kept.rs b/kept.rs\n@@ -0,0 +1 @@\n+right file\n",
        "diff --git \"a/r\\377.rs\" \"b/r\\377.rs\"\n@@ -0,0 +1 @@\n+wrong file\n",
    );
    let error = keep_files(patch, &["kept.rs".to_string()])
        .unwrap_err()
        .text();
    assert!(error.contains("could not read a file path"), "{error}");
}

#[test]
fn a_clean_case_with_an_unreadable_file_header_is_refused_before_writing() {
    let scratch = crate::testing::Scratch::new("review-invalid-clean-case");
    let name = "unreadable";
    let patch = "diff --git \"a/r\\377.rs\" \"b/r\\377.rs\"\n@@ -0,0 +1 @@\n+code\n";
    let error = add(
        &scratch.dir,
        name,
        "demo",
        "repo",
        patch,
        &[Addition::Clean { rule: "rule-a" }],
    )
    .unwrap_err()
    .text();
    assert!(error.contains("could not read a file path"), "{error}");
    assert!(!scratch.dir.join(DIR).join(name).exists());
}
use crate::review::fakes::{Raised, fake};
use crate::testing::Scratch;

const PATCH: &str = "From 1234 Mon Sep 17 00:00:00 2001\nSubject: [PATCH] x\n\n---\n a.rs | 1 +\n\ndiff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1 @@\n+a.rs\n-- \n2.50\n";

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

#[test]
fn adding_a_case_writes_its_patch_and_findings_and_refuses_another_change_under_the_name() {
    let scratch = Scratch::new("case-add");
    let root = scratch.dir.join("root");
    add(
        &root,
        "one",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        }],
    )
    .unwrap();
    add(
        &root,
        "one",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-b",
            file: "a.rs",
        }],
    )
    .unwrap();
    add(
        &root,
        "one",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-b",
            file: "a.rs",
        }],
    )
    .unwrap();
    let cases = load(&root).unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(
        (cases[0].project.as_str(), cases[0].repo.as_str()),
        ("demo", "demo")
    );
    assert_eq!(cases[0].expected.len(), 2);
    add(
        &root,
        "quiet",
        "demo",
        "demo",
        PATCH,
        &[Addition::Clean { rule: "rule-a" }],
    )
    .unwrap();
    let quiet = load(&root)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "quiet")
        .unwrap();
    assert_eq!(quiet.clean_for, ["rule-a"]);
    assert!(quiet.expected.is_empty());
    let other = PATCH.replace("+a.rs", "+b");
    let refused = add(
        &root,
        "one",
        "demo",
        "demo",
        &other,
        &[Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        }],
    )
    .unwrap_err()
    .text();
    assert!(refused.contains("another change"), "{refused}");
    let refused = add(
        &root,
        "one",
        "demo",
        "other-repo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        }],
    )
    .unwrap_err()
    .text();
    assert!(
        refused.contains("holds a change of repo demo, not other-repo"),
        "{refused}"
    );
    let refused = add(
        &root,
        "two",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-a",
            file: "z.rs",
        }],
    )
    .unwrap_err()
    .text();
    assert!(refused.contains("does not change z.rs"), "{refused}");
}

#[test]
fn a_case_that_expects_nothing_or_a_file_the_patch_leaves_alone_is_refused() {
    let scratch = Scratch::new("case-load");
    let root = scratch.dir.join("root");
    let dir = root.join(DIR).join("bad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(PATCH_FILE), PATCH).unwrap();
    std::fs::write(
        dir.join("expected.toml"),
        "project = \"demo\"\nrepo = \"demo\"\n",
    )
    .unwrap();
    assert!(load(&root).unwrap_err().text().contains("expects nothing"));
    std::fs::write(
        dir.join("expected.toml"),
        "project = \"demo\"\nrepo = \"demo\"\n[[finding]]\nrule = \"r\"\nfile = \"z.rs\"\n",
    )
    .unwrap();
    assert!(load(&root).unwrap_err().text().contains("does not change"));
}

const PATCH_FILE: &str = "change.patch";

fn material(fake: &crate::review::fakes::Fake) -> Material {
    Material::load(&fake.root, "demo").unwrap()
}

#[test]
fn a_replay_counts_the_expected_findings_found_and_the_false_blocking_ones() {
    let scratch = Scratch::new("case-replay");
    let fake = fake(&scratch.dir);
    add(
        &fake.root,
        "hit",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        }],
    )
    .unwrap();
    let case = load(&fake.root).unwrap().remove(0);
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let found = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    assert_eq!(
        (found.expected, found.found, found.false_blocking),
        (1, 1, 0)
    );
    let usage = totals(std::slice::from_ref(&found)).usage.tokens.unwrap();
    assert_eq!((usage.input, usage.output), (Some(100), Some(10)));
    fake.answer("alpha", &[("rule-a", None)]);
    let missed = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    assert_eq!((missed.found, missed.missed.len()), (0, 1));
    // A finding whose code is not in the patch does not count as found.
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "a.rs", 1, "not in it", "p")),
        )],
    );
    assert_eq!(
        replay_case(
            Scope {
                material: &material(&fake),
                files: &Globs::default()
            },
            &fake.program,
            &case,
            None
        )
        .unwrap()
        .found,
        0
    );
    // On a clean case, a blocking finding of the quiet rule is a false one.
    let clean = Case {
        name: "clean".to_string(),
        project: "demo".to_string(),
        repo: "demo".to_string(),
        patch: PATCH.to_string(),
        clean_for: vec!["rule-a".to_string()],
        expected: Vec::new(),
        labels: Vec::new(),
    };
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    assert_eq!(
        replay_case(
            Scope {
                material: &material(&fake),
                files: &Globs::default()
            },
            &fake.program,
            &clean,
            None
        )
        .unwrap()
        .false_blocking,
        1
    );
    fake.answer("alpha", &[("rule-a", None)]);
    let quiet = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &clean,
        None,
    )
    .unwrap();
    assert_eq!(quiet.false_blocking, 0);
    let text = report(&[found, missed, quiet]);
    assert!(
        text.contains("recall 1 of 2 expected findings (50%), 0 false blocking findings"),
        "{text}"
    );
    assert!(text.contains("missed rule-a in a.rs"));
}

#[test]
fn a_failed_call_in_a_replay_is_counted_and_named() {
    let scratch = Scratch::new("case-replay-failed");
    let fake = fake(&scratch.dir);
    add(
        &fake.root,
        "hit",
        "demo",
        "demo",
        PATCH,
        &[Addition::Finding {
            rule: "rule-a",
            file: "a.rs",
        }],
    )
    .unwrap();
    let case = load(&fake.root).unwrap().remove(0);
    fake.fail("alpha");
    let result = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    let t = totals(std::slice::from_ref(&result));
    assert_eq!((t.failed_calls, t.found), (1, 0));
    assert!(report(&[result]).contains("group alpha failed"));
}

#[test]
fn a_group_that_lists_sites_is_asked_once_per_file_its_rule_covers() {
    let scratch = Scratch::new("case-per-file");
    let fake = fake(&scratch.dir);
    crate::review::fakes::group_file(
        &fake.root,
        "alpha",
        "alpha",
        "medium",
        "enumerate = true\npasses = 2\nsites = \"handles rows\"\n",
    );
    fake.answer_task(
        "alpha",
        serde_json::json!({
            "summary": "s",
            "sites": [{"file": "a.rs", "line": 1, "kind": "filter", "code": "a.rs", "verdict": "finding",
                       "clause": "Never do the bad thing here.", "problem": "p"}]
        }),
    );
    let patch = format!(
        "{PATCH}diff --git a/b.rs b/b.rs\nindex 1..2 100644\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+b.rs\ndiff --git a/c.swift b/c.swift\nindex 1..2 100644\n--- a/c.swift\n+++ b/c.swift\n@@ -0,0 +1 @@\n+c\n"
    );
    let case = Case {
        name: "n".to_string(),
        project: "demo".to_string(),
        repo: "demo".to_string(),
        patch,
        clean_for: Vec::new(),
        expected: Vec::new(),
        labels: Vec::new(),
    };
    let result = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    let alpha: Vec<_> = result.calls.iter().filter(|c| c.group == "alpha").collect();
    assert_eq!(alpha.len(), 1, "one merged call per group");
    assert_eq!(
        alpha[0].findings.len(),
        1,
        "the same finding raised by several calls is kept once"
    );
    assert_eq!(
        alpha[0].usage.tokens.input,
        Some(200),
        "four calls of 50 tokens"
    );
    assert_eq!(
        result.repeat.unwrap().input,
        Some(100),
        "the second pass is two calls"
    );
    let prompts = fake.prompts();
    let alpha_prompts: Vec<_> = prompts.iter().filter(|(g, _)| g == "alpha").collect();
    assert_eq!(alpha_prompts.len(), 4, "two files, two passes");
    assert!(alpha_prompts.iter().all(|(_, t)| !t.contains("c.swift")));
    assert_eq!(prompts.iter().filter(|(g, _)| g == "beta").count(), 1);
}

#[test]
fn a_patch_gives_its_commit_messages_without_the_mark_or_the_stat() {
    let patch = "From 0123456789012345678901234567890123456789 Mon Sep 17 00:00:00 2001\nFrom: A <a@b>\nSubject: [PATCH] Do the thing\n\nWhy it is done.\n---\n a.rs | 1 +\n\ndiff --git a/a.rs b/a.rs\n+x\n-- \n2.50\n";
    assert_eq!(
        messages_of_patch(patch),
        "Do the thing\n\nWhy it is done.\n\n"
    );
}

fn label(file: &str, code: &str, verdict: &str, rule: Option<&str>) -> SiteLabel {
    SiteLabel {
        file: file.to_string(),
        code: code.to_string(),
        verdict: verdict.to_string(),
        rule: rule.map(str::to_string),
    }
}

#[test]
fn site_labels_score_recall_wrong_findings_and_unlabelled_ones_per_rule() {
    let scratch = Scratch::new("case-site-labels");
    let fake = fake(&scratch.dir);
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let case = |labels: Vec<SiteLabel>| Case {
        name: "n".to_string(),
        project: "demo".to_string(),
        repo: "demo".to_string(),
        patch: PATCH.to_string(),
        clean_for: Vec::new(),
        expected: Vec::new(),
        labels,
    };
    let score = |labels| {
        let result = replay_case(
            Scope {
                material: &material(&fake),
                files: &Globs::default(),
            },
            &fake.program,
            &case(labels),
            None,
        )
        .unwrap();
        result
            .sites
            .into_iter()
            .find(|s| s.rule == "rule-a")
            .unwrap()
    };
    // The reviewer's finding quotes `a.rs` in a.rs.
    let hit = score(vec![label("a.rs", "a.rs", "finding", Some("rule-a"))]);
    assert_eq!(
        (hit.found, hit.missed, hit.wrong, hit.unlabelled),
        (1, 0, 0, 0)
    );
    assert_eq!(hit.group, "alpha");
    let wrong = score(vec![
        label("a.rs", "a.rs", "none", None),
        label("a.rs", "elsewhere", "finding", Some("rule-a")),
    ]);
    assert_eq!(
        (wrong.found, wrong.missed, wrong.wrong, wrong.unlabelled),
        (0, 1, 1, 0)
    );
    let unlabelled = score(vec![label("b.rs", "b", "none", None)]);
    assert_eq!(unlabelled.unlabelled, 1);
    assert!(
        site_report(&[CaseResult {
            sites: vec![hit],
            ..replay_case(
                Scope {
                    material: &material(&fake),
                    files: &Globs::default()
                },
                &fake.program,
                &case(Vec::new()),
                None
            )
            .unwrap()
        }])
        .contains("recall 100%")
    );
}

#[test]
fn a_case_takes_site_labels_and_a_finding_in_the_commit_messages() {
    let scratch = Scratch::new("case-add-sites");
    let root = scratch.dir.join("root");
    let site = label("a.rs", "a.rs", "none", None);
    let patch = PATCH.replace("From 1234 ", &format!("From {} ", "0".repeat(40)));
    add(
        &root,
        "one",
        "demo",
        "demo",
        &patch,
        &[
            Addition::Finding {
                rule: "rule-a",
                file: crate::review::files::COMMIT_MESSAGE,
            },
            Addition::Site(site.clone()),
            Addition::Site(site.clone()),
        ],
    )
    .unwrap();
    let case = load(&root).unwrap().remove(0);
    assert_eq!(case.expected[0].file, "COMMIT_MESSAGE");
    assert_eq!(case.labels, [site]);
}

#[test]
fn a_comparison_prints_each_setups_caught_missed_and_wrong_flags_with_tokens_and_cost() {
    let scratch = Scratch::new("case-compare");
    let fake = fake(&scratch.dir);
    let case = Case {
        name: "n".to_string(),
        project: "demo".to_string(),
        repo: "demo".to_string(),
        patch: PATCH.to_string(),
        clean_for: Vec::new(),
        expected: vec![Expect {
            rule: "rule-a".to_string(),
            file: "a.rs".to_string(),
        }],
        labels: vec![label("a.rs", "a.rs", "none", None)],
    };
    fake.answer("alpha", &[("rule-a", None)]);
    let current = replay_case(
        Scope {
            material: &material(&fake),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    let named = replay_case(
        Scope {
            material: &material(&fake).with_model("haiku"),
            files: &Globs::default(),
        },
        &fake.program,
        &case,
        None,
    )
    .unwrap();
    let text = comparison(&[current], &[named], "haiku");
    assert!(
        text.contains(
            "current setup: caught 0 of 1, missed 1, wrong flags 0, 100 tokens in, 10 out, $0.01\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "haiku: caught 1 of 1, missed 0, wrong flags 1, 100 tokens in, 10 out, $0.01\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("n: current setup caught 0 of 1, 0 wrong; haiku caught 1 of 1, 1 wrong\n"),
        "{text}"
    );
}

#[test]
fn a_patch_keeps_only_the_files_named_with_every_commit_message() {
    let sha = "0".repeat(40);
    let patch = format!(
        "From {sha} Mon Sep 17 00:00:00 2001\nSubject: [PATCH] One\n\nWhy.\n---\n a.rs | 1 +\n b.rs | 1 +\n\ndiff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1 @@\n+a\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+b\n-- \n2.50\n\nFrom {sha} Mon Sep 17 00:00:00 2001\nSubject: [PATCH] Two\n\n---\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-b\n+c\n"
    );
    let kept = keep_files(&patch, &["a.rs".to_string()]).unwrap();
    assert_eq!(files::paths(&kept).unwrap(), ["a.rs"]);
    assert_eq!(messages_of_patch(&kept), messages_of_patch(&patch));
    assert!(kept.contains("+a\n") && !kept.contains("+b\n"));
}
