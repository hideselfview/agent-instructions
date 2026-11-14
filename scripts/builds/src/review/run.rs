//! One review of a diff: picks the groups, asks one reviewer per group in
//! parallel, checks each finding's quotes, and says what came back. It writes
//! nothing; `mod.rs` records the calls.

use std::path::Path;
use std::process::Stdio;

use serde_json::json;

use crate::checkout::{git, parallel_map};
use crate::fail;
use crate::fail::{Context, Result};
use crate::review::call;
use crate::review::check::{DiffFile, clause_in, code_in, parse_diff};
use crate::review::files::{self, FileDiff, Source};
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
    /// The text of the change's commit messages, empty when not known.
    pub messages: String,
    /// Where the new text of a changed file is read, when it can be.
    pub source: Option<Source>,
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
    let messages = text_of(&[
        "log",
        "--reverse",
        "--format=%B",
        &format!("{merge_base}..{head}"),
    ])?;
    let head = text_of(&["rev-parse", head])?.trim().to_string();
    Ok(Diff {
        messages,
        source: Some(Source {
            repo: worktree.to_path_buf(),
            rev: head.clone(),
            in_place: true,
        }),
        base: merge_base,
        head,
        text,
        changed: names.lines().map(str::to_string).collect(),
    })
}

/// One reviewer's work: a group, and for a group that lists sites the one
/// file it reads.
struct Unit<'a> {
    group: &'a Group,
    members: &'a [&'a Rule],
    file: Option<FileDiff>,
}

/// Asks the reviewers of <picked> and checks what comes back: one per group,
/// except that a group that lists sites gets one per changed file its rule
/// covers, each given that file's diff and its new text around the hunks. The
/// calls of a group come back as one call with the findings of all.
pub fn ask_groups(
    material: &Material,
    reviewer: &Path,
    picked: &[(&Group, Vec<&Rule>)],
    diff: &Diff,
    brief: Option<&(String, String)>,
    settled: &(dyn Fn(&str) -> Vec<Settled> + Sync),
) -> Asked {
    let mut files = parse_diff(&diff.text);
    let by_file = files::split(&diff.text);
    let message = files::commit_message_file(&diff.messages);
    if let Some((_, quotes)) = &message {
        files.push(quotes.clone());
    }
    // The reviewers read the repository at the commit under review.
    let checkout = match diff.source.as_ref().map(files::Source::checkout) {
        Some(Ok(checkout)) => Some(checkout),
        Some(Err(why)) => {
            return Asked {
                calls: failed_all(picked, &why.text()),
                repeat: Tokens::default(),
            };
        }
        None => None,
    };
    let mut units: Vec<Unit> = Vec::new();
    for (group, members) in picked {
        if group.enumerate {
            for file in &by_file {
                let path = std::slice::from_ref(&file.path);
                if members.iter().any(|rule| rule.matches(path)) {
                    units.push(Unit {
                        group,
                        members,
                        file: Some(file.clone()),
                    });
                }
            }
            if let (true, Some((file, _))) = (group.commit_text, &message) {
                units.push(Unit {
                    group,
                    members,
                    file: Some(file.clone()),
                });
            }
        } else {
            units.push(Unit {
                group,
                members,
                file: None,
            });
        }
    }
    let one = |unit: &Unit| {
        let group = unit.group;
        let principles: Vec<(String, String)> = group
            .principles
            .iter()
            .map(|p| material.principle(p))
            .collect();
        let settled = settled(&group.name);
        let (text_diff, context) = match &unit.file {
            Some(file) => (
                file.text.as_str(),
                files::context(diff.source.as_ref(), &file.path, &file.text),
            ),
            None => (diff.text.as_str(), String::new()),
        };
        let parts = prompt::build(&prompt::Input {
            group,
            rules: unit.members,
            project_text: &material.project_text,
            principles: &principles,
            brief,
            settled: &settled,
            diff: text_diff,
            context: &context,
        });
        let names = prompt::answer_names(group, unit.members);
        let output = call::run(
            reviewer,
            &group.model,
            &names,
            group.enumerate,
            &parts,
            checkout.as_ref().map(|c| c.path.as_path()),
        );
        checked(
            group,
            unit.members,
            &material.project_text,
            &files,
            &names,
            output,
        )
    };
    // The first call of each group runs alone with the others of its group
    // held back: it writes the group's system prompt to the cache, and the
    // rest read it from there. A group's later passes repeat the same
    // prompts after that, so they read the cache too.
    let mut first: Vec<usize> = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        if !first
            .iter()
            .any(|&j| units[j].group.name == unit.group.name)
        {
            first.push(i);
        }
    }
    let rest: Vec<usize> = (0..units.len()).filter(|i| !first.contains(i)).collect();
    let mut done: Vec<Vec<NewCall>> = units.iter().map(|_| Vec::new()).collect();
    for (i, call) in first.iter().zip(parallel_map(&first, |&i| one(&units[i]))) {
        done[*i].push(call);
    }
    for (i, call) in rest.iter().zip(parallel_map(&rest, |&i| one(&units[i]))) {
        done[*i].push(call);
    }
    let most = units.iter().map(|u| u.group.passes).max().unwrap_or(1);
    let mut repeat = Tokens::default();
    for pass in 1..most {
        let again: Vec<usize> = (0..units.len())
            .filter(|&i| units[i].group.passes > pass)
            .collect();
        for (i, call) in again.iter().zip(parallel_map(&again, |&i| one(&units[i]))) {
            repeat.add(&call);
            done[*i].push(call);
        }
    }
    let mut calls: Vec<NewCall> = Vec::new();
    for (unit, made) in units.iter().zip(done) {
        for call in made {
            match calls.iter_mut().find(|c| c.group == unit.group.name) {
                Some(held) => merge(held, call),
                None => calls.push(call),
            }
        }
    }
    Asked { calls, repeat }
}

/// What a review asked: one call per group, and the tokens of the passes
/// after each group's first.
pub struct Asked {
    pub calls: Vec<NewCall>,
    pub repeat: Tokens,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: i64,
    pub cached: i64,
    pub written: i64,
    pub output: i64,
}

impl Tokens {
    fn add(&mut self, call: &NewCall) {
        self.input += call.tokens_in;
        self.cached += call.tokens_cached;
        self.written += call.tokens_written;
        self.output += call.tokens_out;
    }
}

/// A failed call for each group of <picked>, because the checkout the
/// reviewers read could not be made.
fn failed_all(picked: &[(&Group, Vec<&Rule>)], why: &str) -> Vec<NewCall> {
    picked
        .iter()
        .map(|(group, members)| NewCall {
            group: group.name.clone(),
            model: group.model.clone(),
            rules: prompt::answer_names(group, members),
            tokens_in: 0,
            tokens_out: 0,
            tokens_cached: 0,
            tokens_written: 0,
            sites: None,
            failure: Some(why.to_string()),
            verdicts: Vec::new(),
            findings: Vec::new(),
        })
        .collect()
}

/// <held> with <more>: tokens added, verdicts joined (`finding` when either
/// is), findings and sites collected, failures kept.
fn merge(held: &mut NewCall, more: NewCall) {
    held.tokens_in += more.tokens_in;
    held.tokens_out += more.tokens_out;
    held.tokens_cached += more.tokens_cached;
    held.tokens_written += more.tokens_written;
    held.failure = match (held.failure.take(), more.failure) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    for (rule, verdict) in more.verdicts {
        match held.verdicts.iter_mut().find(|(r, _)| *r == rule) {
            Some((_, held_verdict)) => {
                if verdict == "finding" {
                    *held_verdict = verdict;
                }
            }
            None => held.verdicts.push((rule, verdict)),
        }
    }
    for finding in more.findings {
        let same = held.findings.iter().any(|f| {
            f.rule == finding.rule
                && f.file == finding.file
                && crate::review::check::squash(&f.code)
                    == crate::review::check::squash(&finding.code)
        });
        if !same {
            held.findings.push(finding);
        }
    }
    held.sites = match (held.sites.take(), more.sites) {
        (Some(a), Some(b)) => Some(
            json!([
                serde_json::from_str::<serde_json::Value>(&a).unwrap_or_default(),
                serde_json::from_str::<serde_json::Value>(&b).unwrap_or_default()
            ])
            .to_string(),
        ),
        (a, b) => a.or(b),
    };
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
