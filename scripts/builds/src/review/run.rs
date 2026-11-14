//! One review of a diff: picks the groups, asks one reviewer per group in
//! parallel, checks each finding's quotes, and says what came back. It writes
//! nothing; `mod.rs` records the calls.

use std::path::Path;
use std::process::Stdio;

use crate::checkout::{git, parallel_map};
use crate::fail;
use crate::fail::{Context, Result};
use crate::review::call;
use crate::review::check::{DiffFile, clause_in, code_in, parse_diff};
use crate::review::groups::{Group, Groups};
use crate::review::prompt::{self, PROJECT_FACTS, Settled};
use crate::review::record::{NewCall, NewFinding};
use crate::review::rules::{self, Rule};

/// What a review reads besides the diff.
pub struct Material {
    pub groups: Groups,
    /// A hash of the groups file's text, recorded with every run.
    pub grouping: String,
    /// The rules of the project: `rules/` and its own.
    pub rules: Vec<Rule>,
    /// The project's always-loaded instructions, empty when it has none.
    pub project_text: String,
    root: std::path::PathBuf,
}

impl Material {
    /// The groups and rules of the checkout at <root> for <project>, after
    /// checking that every rule of the checkout is in one group.
    pub fn load(root: &Path, project: &str) -> Result<Material> {
        let groups = Groups::load(root)?;
        let grouping = hash(
            &std::fs::read_to_string(root.join(crate::review::groups::FILE)).unwrap_or_default(),
        );
        let every = rules::load(root, None)?;
        crate::review::groups::require_whole(&groups, &every)?;
        let rules = rules::load(root, Some(project))?;
        let project_file = root.join("projects").join(format!("{project}.md"));
        let project_text = std::fs::read_to_string(&project_file).unwrap_or_default();
        Ok(Material {
            groups,
            grouping,
            rules,
            project_text,
            root: root.to_path_buf(),
        })
    }

    /// The same material with <groups> in place of the checkout's own, after
    /// checking that every rule of the checkout is in one of them.
    pub fn with_groups(mut self, groups: Groups) -> Result<Material> {
        let every = rules::load(&self.root, None)?;
        crate::review::groups::require_whole(&groups, &every)?;
        self.grouping = "alternative".to_string();
        self.groups = groups;
        Ok(self)
    }

    fn principle(&self, name: &str) -> (String, String) {
        let path = self.root.join("principles").join(format!("{name}.md"));
        (
            name.to_string(),
            std::fs::read_to_string(path).unwrap_or_default(),
        )
    }
}

/// FNV-1a of <text> as 16 hex digits.
pub fn hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A change to review.
pub struct Diff {
    pub base: String,
    pub head: String,
    pub text: String,
    pub changed: Vec<String>,
}

/// The change of the worktree's HEAD past <base> (from their common
/// ancestor).
pub fn diff_of(worktree: &Path, base: &str) -> Result<Diff> {
    diff_between(worktree, base, "HEAD")
}

/// The change of <head> past <base> (from their common ancestor), in <worktree>'s repo.
pub fn diff_between(worktree: &Path, base: &str, head: &str) -> Result<Diff> {
    let text_of = |args: &[&str]| -> Result<String> {
        let output = git(worktree)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .context(|| format!("running git {}", args.join(" ")))?;
        if !output.status.success() {
            fail!(
                "git {} failed in {}: {}",
                args.join(" "),
                worktree.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let merge_base = text_of(&["merge-base", base, head])?.trim().to_string();
    let range = format!("{merge_base}...{head}");
    let text = text_of(&["diff", "--no-color", "--no-ext-diff", &range])?;
    let names = text_of(&["diff", "--no-color", "--name-only", &range])?;
    let head = text_of(&["rev-parse", head])?.trim().to_string();
    Ok(Diff {
        base: merge_base,
        head,
        text,
        changed: names.lines().map(str::to_string).collect(),
    })
}

/// Asks one reviewer per group of <picked> and checks what comes back.
pub fn ask_groups(
    material: &Material,
    reviewer: &Path,
    picked: &[(&Group, Vec<&Rule>)],
    diff: &Diff,
    brief: Option<&(String, String)>,
    settled: &(dyn Fn(&str) -> Vec<Settled> + Sync),
) -> Vec<NewCall> {
    let files = parse_diff(&diff.text);
    let one = |(group, members): &(&Group, Vec<&Rule>)| {
        let principles: Vec<(String, String)> = group
            .principles
            .iter()
            .map(|p| material.principle(p))
            .collect();
        let settled = settled(&group.name);
        let text = prompt::build(&prompt::Input {
            group,
            rules: members,
            project_text: &material.project_text,
            principles: &principles,
            brief,
            settled: &settled,
            diff: &diff.text,
        });
        let names = prompt::answer_names(group, members);
        let output = call::run(reviewer, &group.model, &names, group.enumerate, &text);
        checked(
            group,
            members,
            &material.project_text,
            &files,
            &names,
            output,
        )
    };
    // The first call writes the shared prefix of the prompt to the cache and
    // the rest, started after it answers, read it from there.
    let Some((first, rest)) = picked.split_first() else {
        return Vec::new();
    };
    let mut calls = vec![one(first)];
    calls.extend(parallel_map(rest, one));
    calls
}

/// The call as the store holds it: findings whose clause or code is not
/// there are kept with the reason they were dropped.
fn checked(
    group: &Group,
    members: &[&Rule],
    project_text: &str,
    files: &[DiffFile],
    names: &[String],
    output: call::Output,
) -> NewCall {
    let mut call = NewCall {
        group: group.name.clone(),
        model: group.model.clone(),
        rules: names.to_vec(),
        tokens_in: output.tokens_in,
        tokens_out: output.tokens_out,
        tokens_cached: output.tokens_cached,
        tokens_written: output.tokens_written,
        sites: None,
        failure: None,
        verdicts: Vec::new(),
        findings: Vec::new(),
    };
    let answer = match output.answer {
        Ok(answer) => answer,
        Err(why) => {
            call.failure = Some(why);
            return call;
        }
    };
    call.verdicts = answer.verdicts;
    call.sites = answer.sites;
    for raw in answer.findings {
        let (rule_text, blocking) = if raw.rule == PROJECT_FACTS {
            (project_text, false)
        } else {
            let rule = members
                .iter()
                .find(|r| r.slug == raw.rule)
                .expect("the answer's rules were checked against the group's");
            (rule.text.as_str(), rule.blocking)
        };
        let check_failed = if !clause_in(&raw.clause, rule_text) {
            Some("its clause is not in the rule's text".to_string())
        } else {
            code_in(files, &raw.file, &raw.code).err()
        };
        call.findings.push(NewFinding {
            rule: raw.rule,
            clause: raw.clause,
            file: raw.file,
            line: raw.line,
            code: raw.code,
            problem: raw.problem,
            blocking,
            check_failed,
        });
    }
    call
}
