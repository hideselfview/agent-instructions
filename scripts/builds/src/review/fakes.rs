//! A stand-in for `claude` and the checkout of rules it reviews against,
//! for the tests of the review.

use std::path::{Path, PathBuf};

/// A reviewer stand-in and the checkout of rules it reviews against, in the
/// scratch folder of <demo>.
pub struct Fake {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub program: PathBuf,
}

/// Writes the `group.toml` of folder <dir>, with <extra> lines after the model.
pub fn group_file(root: &Path, dir: &str, name: &str, model: &str, extra: &str) {
    std::fs::write(
        root.join("rules").join(dir).join("group.toml"),
        format!("name = \"{name}\"\nquestion = \"q\"\nmodel = \"{model}\"\n{extra}"),
    )
    .unwrap();
}

pub fn fake(scratch: &Path) -> Fake {
    let root = scratch.join("instructions");
    let dir = scratch.join("reviewer");
    for sub in ["rules/alpha", "rules/beta", "projects"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::create_dir_all(dir.join("calls")).unwrap();
    let rule = |group: &str, slug: &str, glob: &str, blocking: bool, body: &str| {
        std::fs::write(
            root.join("rules").join(group).join(format!("{slug}.md")),
            format!("---\ndigest: d\npaths:\n  - '{glob}'\nblocking: {blocking}\n---\n\n{body}\n"),
        )
        .unwrap();
    };
    rule(
        "alpha",
        "rule-a",
        "**/*.rs",
        true,
        "**Rule A.** Never do the bad thing here.",
    );
    rule(
        "beta",
        "rule-b",
        "**/*.swift",
        false,
        "**Rule B.** Never do the other thing.",
    );
    group_file(&root, "alpha", "alpha", "sonnet", "");
    group_file(&root, "beta", "beta", "opus", "");
    std::fs::write(root.join("projects/demo.md"), "Demo project facts.\n").unwrap();
    let program = dir.join("claude");
    let script = format!(
        "#!/bin/bash\nD='{d}'\ncat > \"$D/calls/$$.prompt\"\nprintf '%s\\n' \"$@\" >> \"$D/calls/$$.prompt\"\ngroup=$(sed -n 's/^Group: //p;s/^Task: //p' \"$D/calls/$$.prompt\" | head -1)\nif [ -f \"$D/fail-$group\" ]; then echo boom >&2; exit 3; fi\ncat \"$D/answer-$group.json\"\n",
        d = dir.display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let fake = Fake { root, dir, program };
    fake.answer("alpha", &[("rule-a", None)]);
    fake.answer("beta", &[("rule-b", None)]);
    fake
}

/// A finding as a reviewer words it: clause, file, line, code, problem.
pub type Raised = (&'static str, &'static str, i64, &'static str, &'static str);

impl Fake {
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

    /// The prompts the reviewer was run with since the last call of this, by group.
    pub fn prompts(&self) -> Vec<(String, String)> {
        let mut prompts = Vec::new();
        for entry in std::fs::read_dir(self.dir.join("calls")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let group = text
                .lines()
                .find_map(|l| l.strip_prefix("Group: "))
                .unwrap()
                .to_string();
            prompts.push((group, text));
            std::fs::remove_file(path).unwrap();
        }
        prompts.sort();
        prompts
    }
}
