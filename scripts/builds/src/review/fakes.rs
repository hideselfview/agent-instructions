//! A stand-in for `claude` and the checkout of rules it reviews against,
//! for the tests of the review.

use std::path::{Path, PathBuf};

/// A reviewer stand-in and the checkout of rules it reviews against, in the
/// scratch folder of <demo>.
pub struct Fake {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub program: super::Program,
}

/// Returns ten lines, "one" to "ten", and puts <line> at line <at> (counting
/// from 1) for each pair in <changes>. The rebase tests change this file on
/// main and on a branch.
pub fn ten_lines(changes: &[(usize, &str)]) -> String {
    let mut lines = vec![
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    for (at, line) in changes {
        lines[at - 1] = line;
    }
    format!("{}\n", lines.join("\n"))
}

/// Writes the `group.toml` of folder <dir>, with <extra> lines after the tier.
pub fn group_file(root: &Path, dir: &str, name: &str, tier: &str, extra: &str) {
    std::fs::write(
        root.join("rules").join(dir).join("group.toml"),
        format!("name = \"{name}\"\nquestion = \"q\"\ntier = \"{tier}\"\n{extra}"),
    )
    .unwrap();
}

/// Writes rule <slug> in folder <dir>, covering the paths <glob> matches.
pub fn rule_file(root: &Path, dir: &str, slug: &str, glob: &str, blocking: bool, body: &str) {
    write_rule(
        root,
        dir,
        slug,
        glob,
        &format!("blocking: {blocking}\n"),
        body,
    );
}

/// Writes rule <slug> in folder <dir>, a blocking rule covering the paths
/// <glob> matches, design files among them: its front matter says it judges
/// design files.
pub fn design_rule_file(root: &Path, dir: &str, slug: &str, glob: &str, body: &str) {
    write_rule(
        root,
        dir,
        slug,
        glob,
        "blocking: true\ndesign: true\n",
        body,
    );
}

/// Writes rule <slug> in folder <dir>, a blocking rule covering the paths
/// <glob> matches, capture files among them: its front matter says it judges
/// capture files.
pub fn capture_rule_file(root: &Path, dir: &str, slug: &str, glob: &str, body: &str) {
    write_rule(
        root,
        dir,
        slug,
        glob,
        "blocking: true\ncapture: true\n",
        body,
    );
}

/// Writes rule <slug> in folder <dir>, covering the paths <glob> matches,
/// with the lines <front> in its front matter after its paths.
fn write_rule(root: &Path, dir: &str, slug: &str, glob: &str, front: &str, body: &str) {
    std::fs::write(
        root.join("rules").join(dir).join(format!("{slug}.md")),
        format!("---\ndigest: d\npaths:\n  - '{glob}'\n{front}---\n\n{body}\n"),
    )
    .unwrap();
}

/// Adds group looks, whose rule-d judges design files and covers every path
/// under design/, and has its reviewer answer no findings.
pub fn looks_group(fake: &Fake) {
    std::fs::create_dir_all(fake.root.join("rules/looks")).unwrap();
    design_rule_file(
        &fake.root,
        "looks",
        "rule-d",
        "design/**",
        "**Rule D.** A menu is as wide as its longest item.",
    );
    group_file(&fake.root, "looks", "looks", "high", "");
    fake.answer("looks", &[("rule-d", None)]);
}

/// Adds group writing, whose rule-w covers every .rs file and whose reviewer
/// reads one file per call and the commit messages, and has that reviewer
/// answer no findings.
pub fn commit_text_group(fake: &Fake) {
    std::fs::create_dir_all(fake.root.join("rules/writing")).unwrap();
    rule_file(
        &fake.root,
        "writing",
        "rule-w",
        "**/*.rs",
        true,
        "**Rule W.** Write plain sentences.",
    );
    group_file(
        &fake.root,
        "writing",
        "writing",
        "high",
        "enumerate = true\nsites = \"is a sentence\"\ncommit_text = true\n",
    );
    fake.answer_task("writing", serde_json::json!({"summary": "s", "sites": []}));
}

/// Adds group style, whose rule-s covers every .rs file and whose reviewer
/// reads one file per call, judges no code comment and reads no commit
/// message, and has that reviewer judge <sites>.
pub fn style_group(fake: &Fake, sites: serde_json::Value) {
    std::fs::create_dir_all(fake.root.join("rules/style")).unwrap();
    rule_file(
        &fake.root,
        "style",
        "rule-s",
        "**/*.rs",
        true,
        "**Rule S.** Write plain sentences.",
    );
    group_file(
        &fake.root,
        "style",
        "style",
        "high",
        "enumerate = true\nsites = \"is a sentence\"\ncomments = \"skipped\"\n",
    );
    fake.answer_task("style", serde_json::json!({"summary": "s", "sites": sites}));
}

/// Adds group copy, a copy group (`Group::copy`) whose rule-c covers every
/// .rs file and whose reviewer reads one file per call and judges no code
/// comment, and has that reviewer judge <sites>.
pub fn copy_group(fake: &Fake, sites: serde_json::Value) {
    std::fs::create_dir_all(fake.root.join("rules/copy")).unwrap();
    rule_file(
        &fake.root,
        "copy",
        "rule-c",
        "**/*.rs",
        true,
        "**Rule C.** Write plain sentences.",
    );
    group_file(
        &fake.root,
        "copy",
        "copy",
        "high",
        "enumerate = true\nsites = \"is a sentence\"\ncomments = \"skipped\"\ncopy = true\n",
    );
    fake.answer_task("copy", serde_json::json!({"summary": "s", "sites": sites}));
}

/// A finding site of rule-s, as the style group's reviewer words it.
pub fn style_site(file: &str, code: &str) -> serde_json::Value {
    serde_json::json!({
        "file": file, "line": 1, "kind": "sentence", "code": code, "verdict": "finding",
        "rule": "rule-s", "clause": "Write plain sentences.", "problem": "The sentence is not plain."
    })
}

pub fn fake(scratch: &Path) -> Fake {
    let root = scratch.join("instructions");
    let dir = scratch.join("reviewer");
    for sub in ["rules/alpha", "rules/beta", "projects"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::create_dir_all(dir.join("calls")).unwrap();
    rule_file(
        &root,
        "alpha",
        "rule-a",
        "**/*.rs",
        true,
        "**Rule A.** Never do the bad thing here.",
    );
    rule_file(
        &root,
        "beta",
        "rule-b",
        "**/*.swift",
        false,
        "**Rule B.** Never do the other thing.",
    );
    group_file(&root, "alpha", "alpha", "medium", "");
    group_file(&root, "beta", "beta", "high", "");
    std::fs::write(root.join("projects/demo.md"), "Demo project facts.\n").unwrap();
    let program = dir.join("claude");
    let script = format!(
        r#"#!/bin/bash
D='{d}'
cat > "$D/calls/$$.prompt"
printf '%s\n' "$@" >> "$D/calls/$$.prompt"
group=$(sed -n 's/^Group: //p;s/^Task: //p' "$D/calls/$$.prompt" | head -1)
if [ -f "$D/fail-$group" ]; then echo boom >&2; exit 3; fi
while [ "$#" -gt 0 ]; do
    if [ "$1" = --model ]; then model="$2"; break; fi
    shift
done
file=$(sed -n 's|^diff --git a/[^ ]* b/||p' "$D/calls/$$.prompt" | head -1)
answer="$D/answer-$group.json"
if [ -f "$D/answer-$group-$file.json" ]; then answer="$D/answer-$group-$file.json"; fi
sed "s/^{{/{{\"modelUsage\":{{\"$model\":{{}}}},/" "$answer"
"#,
        d = dir.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let fake = Fake {
        root,
        dir,
        program: super::Program::new(program, scratch.to_path_buf()),
    };
    fake.answer("alpha", &[("rule-a", None)]);
    fake.answer("beta", &[("rule-b", None)]);
    fake
}

/// A finding as a reviewer words it: clause, file, line, code, problem.
pub type Raised = (&'static str, &'static str, i64, &'static str, &'static str);

impl Fake {
    /// The reviewer finds no new issues and closes these findings.
    pub fn closes(&self, group: &str, rule: &str, ids: &[i64]) {
        let follow_up: Vec<_> = ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "verdict": "fixed"}))
            .collect();
        self.answer_task(
            group,
            serde_json::json!({
                "rules": [{"rule": rule, "verdict": "none", "findings": []}],
                "follow_up": follow_up,
            }),
        );
    }

    /// What the reviewer of <group> answers: a verdict per rule.
    pub fn answer(&self, group: &str, rules: &[(&str, Option<Raised>)]) {
        let entries: Vec<serde_json::Value> = rules
            .iter()
            .map(|(rule, finding)| match finding {
                None => serde_json::json!({"rule": rule, "verdict": "none", "findings": []}),
                Some((clause, file, line, code, problem)) => serde_json::json!({
                    "rule": rule, "verdict": "finding",
                    "findings": [{"clause": clause, "file": file, "line": line, "code": code, "problem": problem}]
                }),
            })
            .collect();
        let body = serde_json::json!({
            "is_error": false,
            "total_cost_usd": 0.01,
            "usage": {"input_tokens": 100, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 10},
            "structured_output": {"rules": entries}
        });
        std::fs::write(
            self.dir.join(format!("answer-{group}.json")),
            body.to_string(),
        )
        .unwrap();
    }

    /// What the program prints for the prompt that begins `Task: <task>`:
    /// the structured answer <body>.
    pub fn answer_task(&self, task: &str, body: serde_json::Value) {
        let envelope = serde_json::json!({
            "is_error": false,
            "total_cost_usd": 0.005,
            "usage": {"input_tokens": 50, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 5},
            "structured_output": body
        });
        std::fs::write(
            self.dir.join(format!("answer-{task}.json")),
            envelope.to_string(),
        )
        .unwrap();
    }

    pub fn fail(&self, group: &str) {
        std::fs::write(self.dir.join(format!("fail-{group}")), "").unwrap();
    }

    /// The prompts the program sent the reviewer since the last call to
    /// `prompts`, by group, or by task when a prompt names one.
    pub fn prompts(&self) -> Vec<(String, String)> {
        let mut prompts = Vec::new();
        for entry in std::fs::read_dir(self.dir.join("calls")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let group = text
                .lines()
                .find_map(|l| {
                    l.strip_prefix("Group: ")
                        .or_else(|| l.strip_prefix("Task: "))
                })
                .unwrap()
                .to_string();
            prompts.push((group, text));
            std::fs::remove_file(path).unwrap();
        }
        prompts.sort();
        prompts
    }
}
