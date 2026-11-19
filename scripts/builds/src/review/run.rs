//! One review of a diff: picks the groups, asks one reviewer per group in
//! parallel, checks each finding's quotes, and says what came back. It writes
//! nothing; `mod.rs` records the calls.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::json;

use crate::checkout::parallel_map;
use crate::fail;
use crate::fail::{Context, Result};
use crate::review::call;
use crate::review::check::{
    DiffFile, clause_in, code_in, names_file, only_on_comment_lines, parse_diff,
};
use crate::review::commits;
use crate::review::files::{self, FileDiff, Source};
use crate::review::groups::{Group, Groups};
use crate::review::prompt::{self, PROJECT_FACTS};
use crate::review::record::{Finding, NewCall, NewExchange, NewFinding, RunRow};
use crate::review::rules::{self, Rule};
use boards::git;

/// What a review reads besides the diff.
#[derive(PartialEq)]
pub struct Material {
    pub groups: Groups,
    /// The rules of the project: `rules/` and its own.
    pub rules: Vec<Rule>,
    /// The project's always-loaded instructions, empty when it has none.
    pub project_text: String,
}

impl Material {
    /// The groups and rules of the checkout at <root> for <project>, after
    /// checking that every rule of the checkout is in one group.
    pub fn load(root: &Path, project: &str) -> Result<Material> {
        let groups = Groups::load(root)?;
        let every = rules::load(root, None)?;
        crate::review::groups::require_whole(&groups, &every)?;
        let rules = rules::load(root, Some(project))?;
        let project_file = root.join("projects").join(format!("{project}.md"));
        let project_text = std::fs::read_to_string(&project_file).unwrap_or_default();
        Ok(Material {
            groups,
            rules,
            project_text,
        })
    }

    /// The folders of the checkout at <root> whose files `load` reads: the
    /// groups and rules of `rules/`, and the projects' files and rules of
    /// `projects/`.
    pub fn folders(root: &Path) -> Vec<PathBuf> {
        vec![root.join("rules"), root.join("projects")]
    }

    /// The names of the groups whose rules cover one of <changed>.
    pub fn group_names(&self, changed: &[String]) -> Vec<String> {
        self.groups
            .pick(&self.rules, changed)
            .into_iter()
            .map(|(group, _)| group.name.clone())
            .collect()
    }

    /// Each group that holds findings of <open>, with those findings, in
    /// <open>'s order: a later run asks each over every change since it last
    /// read the branch with them, whatever files the changes touch, so that
    /// it closes the findings they fix.
    pub fn holding_groups<'a>(&self, open: &'a [Finding]) -> Vec<(String, Vec<&'a Finding>)> {
        self.groups
            .reviewing(&self.rules)
            .filter_map(|(group, members)| {
                let held = held_by(group, &members, open);
                (!held.is_empty()).then(|| (group.name.clone(), held))
            })
            .collect()
    }

    /// The names of the groups that read the commit messages of <since>, the
    /// changes since the last review of a branch whose commits change
    /// <branch>: each group that reads commit messages and whose rules cover
    /// a path of <branch>. The list is empty when <since> holds no message.
    pub fn commit_text_groups(&self, since: &Diff, branch: &[String]) -> Vec<String> {
        if files::commit_message_file(&since.messages).is_none() {
            return Vec::new();
        }
        self.groups
            .pick(&self.rules, branch)
            .into_iter()
            .filter(|(group, _)| group.commit_text)
            .map(|(group, _)| group.name.clone())
            .collect()
    }

    /// Whether the group that holds rule <slug> judges no code comment
    /// (`Group::skips_comments`). The rule's own group answers, so a finding
    /// of the rule counts the same whichever group asked for it, as in an
    /// audit.
    pub fn skips_comments(&self, slug: &str) -> bool {
        self.groups
            .groups
            .iter()
            .any(|group| group.skips_comments && group.rules.iter().any(|rule| rule == slug))
    }

    /// The same material with every group asked in <model>.
    pub fn with_model(mut self, model: &str) -> Material {
        for group in &mut self.groups.groups {
            group.model = model.to_string();
        }
        self
    }
}

/// A change to review.
#[derive(Clone)]
pub struct Diff {
    /// The text of the change's commit messages, empty when not known.
    pub messages: String,
    /// Where the new text of a changed file is read, when it can be.
    pub source: Option<Source>,
    pub text: String,
    /// The paths builds picks groups by: every path a commit of the change
    /// changes (`commits::paths`), or, for a diff since the net change an
    /// earlier run read (`diff_since`), every path where the two differ.
    pub changed: Vec<String>,
}

/// A diff that builds read from a repo, with the two commits of its net
/// change: the head, and the common ancestor of the head and the base. A diff
/// read back from a reviewer's stored message, or from a test case's patch,
/// has no such commits, and is a `Diff` alone.
pub struct NetChange {
    pub base: String,
    pub head: String,
    pub diff: Diff,
}

impl Diff {
    /// Whether the diff changes no file and holds no commit message.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.messages.trim().is_empty()
    }

    /// The lines builds looks for a finding's quoted code in: each changed
    /// file's, and the commit messages' as one more file.
    pub fn quotes(&self) -> Vec<DiffFile> {
        let mut files = parse_diff(&self.text);
        if let Some((_, quotes)) = files::commit_message_file(&self.messages) {
            files.push(quotes);
        }
        files
    }

    /// The diff's text as <group> reads it in one call: with the commit
    /// messages as one more file, `files::COMMIT_MESSAGE`, when the group
    /// reads them.
    pub fn text_for(&self, group: &Group) -> String {
        let mut text = self.text.clone();
        if let (true, Some((file, _))) = (
            group.commit_text,
            files::commit_message_file(&self.messages),
        ) {
            text.push_str(&file.text);
        }
        text
    }

    /// The diff without the files whose paths are in <dropped>, and without
    /// the commit messages when <dropped> holds `files::COMMIT_MESSAGE`.
    pub fn without(&self, dropped: &[String]) -> Diff {
        // `files::split` drops the text before a patch's first file (its mail
        // header), so a diff that drops nothing keeps its own text.
        if dropped.is_empty() {
            return self.clone();
        }
        let kept: Vec<FileDiff> = files::split(&self.text)
            .into_iter()
            .filter(|file| !dropped.contains(&file.path))
            .collect();
        let kept_paths: Vec<String> = kept.iter().flat_map(FileDiff::paths).collect();
        Diff {
            messages: if dropped.iter().any(|path| path == files::COMMIT_MESSAGE) {
                String::new()
            } else {
                self.messages.clone()
            },
            source: self.source.clone(),
            text: kept.iter().map(|file| file.text.as_str()).collect(),
            changed: self
                .changed
                .iter()
                .filter(|path| kept_paths.contains(path))
                .cloned()
                .collect(),
        }
    }
}

/// The change of the worktree's HEAD past <base> (from their common
/// ancestor).
pub fn diff_of(worktree: &Path, base: &str) -> Result<NetChange> {
    diff_between(worktree, base, "HEAD")
}

/// The change of <head> past <base> (from their common ancestor), in
/// <worktree>'s repo, without the test cases (`commits::without_test_cases`).
pub fn diff_between(worktree: &Path, base: &str, head: &str) -> Result<NetChange> {
    let text_of = |args: &[&str]| commits::git_text(worktree, args);
    let merge_base = text_of(&["merge-base", base, head])?.trim().to_string();
    let range = format!("{merge_base}...{head}");
    let without = commits::without_test_cases();
    let text = text_of(&[
        "diff",
        "--no-color",
        "--no-ext-diff",
        &range,
        "--",
        ".",
        &without,
    ])?;
    let messages = text_of(&[
        "log",
        "--reverse",
        "--format=%B",
        &format!("{merge_base}..{head}"),
    ])?;
    let head = text_of(&["rev-parse", head])?.trim().to_string();
    let changed = commits::paths(worktree, &commits::in_range(worktree, &merge_base, &head)?)?;
    Ok(NetChange {
        diff: Diff {
            messages,
            source: Some(Source {
                repo: worktree.to_path_buf(),
                rev: head.clone(),
                in_place: true,
            }),
            text,
            changed,
        },
        base: merge_base,
        head,
    })
}

/// The changes since the last review: how the net change of <head> past
/// <base> (from their common ancestor), in <worktree>'s repo, differs from
/// the net change from <read_base> to <read_head>, which the last review
/// read. They are the lines the branch now adds or removes that it did not
/// then, and the lines it no longer changes, and nothing else.
///
/// builds merges the net change the run read onto the common ancestor in a
/// tree of git's own (`git merge-tree`, which checks nothing out), taking
/// that change's lines where the ancestor changed the same lines, and diffs
/// that tree against <head>. So the commits a rebase brings in from the
/// base are no part of the result, a rebase that keeps every added and removed
/// line gives an empty diff, and a conflict's resolution shows only where it
/// differs from the lines the run read. A file the merge cannot write (one
/// side deleted it, for example) shows its whole net change, and so does every
/// file when the repo no longer has the commits the run read. The diff's
/// commit messages are those the earlier change did not hold, and the diff
/// leaves out the test cases (`commits::without_test_cases`).
pub fn diff_since(
    worktree: &Path,
    read_base: &str,
    read_head: &str,
    base: &str,
    head: &str,
) -> Result<NetChange> {
    let text_of = |args: &[&str]| commits::git_text(worktree, args);
    let merge_base = text_of(&["merge-base", base, head])?.trim().to_string();
    let head = text_of(&["rev-parse", head])?.trim().to_string();
    let read = commits::exists(worktree, read_base)? && commits::exists(worktree, read_head)?;
    let (tree, whole) = if read {
        merged(worktree, read_base, read_head, &merge_base)?
    } else {
        (merge_base.clone(), Vec::new())
    };
    let mut rest = vec![
        "--".to_string(),
        ".".to_string(),
        commits::without_test_cases(),
    ];
    rest.extend(whole.iter().map(|path| format!(":(exclude,literal){path}")));
    let mut only = vec!["--".to_string(), commits::without_test_cases()];
    only.extend(whole.iter().map(|path| format!(":(literal){path}")));
    let diff = |from: &str, options: &[&str], paths: &[String]| -> Result<String> {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        args.extend(options);
        args.extend([from, head.as_str()]);
        args.extend(paths.iter().map(String::as_str));
        text_of(&args)
    };
    let names = ["--name-only", "--no-renames"];
    let mut text = diff(&tree, &[], &rest)?;
    let mut changed: Vec<String> = diff(&tree, &names, &rest)?
        .lines()
        .map(str::to_string)
        .collect();
    if !whole.is_empty() {
        text.push_str(&diff(&merge_base, &[], &only)?);
        changed.extend(
            diff(&merge_base, &names, &only)?
                .lines()
                .map(str::to_string),
        );
    }
    let messages_of = |range: &str| -> Result<Vec<String>> {
        Ok(text_of(&["log", "--reverse", "--format=%B%x00", range])?
            .split('\0')
            .map(|message| message.trim_start_matches('\n').to_string())
            .filter(|message| !message.trim().is_empty())
            .collect())
    };
    let earlier = if read {
        messages_of(&format!("{read_base}..{read_head}"))?
    } else {
        Vec::new()
    };
    let messages: Vec<String> = messages_of(&format!("{merge_base}..{head}"))?
        .into_iter()
        .filter(|message| !earlier.contains(message))
        .collect();
    Ok(NetChange {
        diff: Diff {
            messages: messages.join("\n"),
            source: Some(Source {
                repo: worktree.to_path_buf(),
                rev: head.clone(),
                in_place: true,
            }),
            text,
            changed,
        },
        base: merge_base,
        head,
    })
}

/// The diff <run> read, in <worktree>'s repo, from a copy of its head's
/// files: its whole net change, or the changes since the last review it
/// names, one of <runs>. <net_change> gives the
/// base and head of a run's net change in that repo.
pub fn diff_of_run(
    worktree: &Path,
    runs: &[&RunRow],
    run: &RunRow,
    net_change: impl Fn(&RunRow) -> Result<(String, String)>,
) -> Result<NetChange> {
    let (base, head) = net_change(run)?;
    let mut change = match run.since {
        None => diff_between(worktree, &base, &head)?,
        Some(since) => {
            let Some(earlier) = runs.iter().find(|r| r.id == since) else {
                fail!(
                    "run {} read the difference from run {since}, which is not a run of its landing",
                    run.id
                );
            };
            let (read_base, read_head) = net_change(earlier)?;
            diff_since(worktree, &read_base, &read_head, &base, &head)?
        }
    };
    if let Some(source) = &mut change.diff.source {
        source.in_place = false;
    }
    Ok(change)
}

/// The tree of <read_base>..<read_head>'s change merged onto <base>, taking
/// that change's lines where both changed the same lines, and the paths the
/// merge could not write.
fn merged(
    worktree: &Path,
    read_base: &str,
    read_head: &str,
    base: &str,
) -> Result<(String, Vec<String>)> {
    let merge_base = format!("--merge-base={read_base}");
    let args = [
        "merge-tree",
        "--write-tree",
        "-z",
        "--name-only",
        "--no-messages",
        "-Xtheirs",
        &merge_base,
        base,
        read_head,
    ];
    let output = git(worktree)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git {}", args.join(" ")))?;
    // git merge-tree exits 1 when it wrote the tree with paths it could not
    // merge, and with another status when it wrote no tree.
    if !matches!(output.status.code(), Some(0 | 1)) {
        fail!(
            "git {} failed in {}: {}",
            args.join(" "),
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    let Some(tree) = fields.next() else {
        fail!("git {} printed no tree", args.join(" "));
    };
    let mut failed: Vec<String> = Vec::new();
    for path in fields {
        if !failed.iter().any(|p| p == path) {
            failed.push(path.to_string());
        }
    }
    Ok((tree.to_string(), failed))
}

/// One reviewer's work: a group, the group's diff (its index in the diffs of
/// `ask_groups`), and for a group that lists sites the one file it reads.
struct Unit<'a> {
    group: &'a Group,
    members: &'a [&'a Rule],
    diff: usize,
    file: Option<FileDiff>,
}

/// The findings of <open> that <group>, with <members>, holds: those of its
/// rules.
fn held_by<'a>(group: &Group, members: &[&Rule], open: &'a [Finding]) -> Vec<&'a Finding> {
    let names = prompt::answer_names(group, members);
    open.iter()
        .filter(|finding| names.contains(&finding.rule))
        .collect()
}

/// Whether <group>, with <members>, holds a finding of <open> in a file one
/// of <paths> names.
fn holds_open(group: &Group, members: &[&Rule], paths: &[String], open: &[Finding]) -> bool {
    held_by(group, members, open)
        .iter()
        .any(|finding| paths.iter().any(|path| names_file(path, &finding.file)))
}

/// The units of group <at>, <group> with <members>, over <diff>: a group
/// that lists sites gets one per changed file its rules cover, by either
/// path of a rename, or one per changed file when it holds a finding of
/// <open>, and one for the commit messages when it reads them; any other
/// group gets one for the whole diff, with the commit messages when it reads
/// them (`Diff::text_for`).
fn units_of<'a>(
    group: &'a Group,
    members: &'a [&'a Rule],
    at: usize,
    diff: &Diff,
    open: &[Finding],
) -> Vec<Unit<'a>> {
    let unit = |file| Unit {
        group,
        members,
        diff: at,
        file,
    };
    if !group.enumerate {
        return vec![unit(None)];
    }
    let holds = !held_by(group, members, open).is_empty();
    let mut units: Vec<Unit> = files::split(&diff.text)
        .into_iter()
        .filter(|file| holds || members.iter().any(|rule| rule.matches(&file.paths())))
        .map(|file| unit(Some(file)))
        .collect();
    if let (true, Some((file, _))) = (
        group.commit_text,
        files::commit_message_file(&diff.messages),
    ) {
        units.push(unit(Some(file)));
    }
    units
}

/// The files of its units, <first>, that group <at> of <picked> does not
/// read: none when it holds a finding of <open>, and otherwise each file
/// where other groups of <picked> hold findings of <open>.
fn not_read(
    picked: &[(&Group, Vec<&Rule>)],
    at: usize,
    first: &[FileDiff],
    open: &[Finding],
) -> Vec<String> {
    let (group, members) = &picked[at];
    if !held_by(group, members, open).is_empty() {
        return Vec::new();
    }
    first
        .iter()
        .filter(|file| {
            let paths = file.paths();
            picked
                .iter()
                .any(|(group, members)| holds_open(group, members, &paths, open))
        })
        .map(|file| file.path.clone())
        .collect()
}

/// Asks the reviewers of <picked> and checks what comes back: one per group,
/// except that a group that lists sites gets one per changed file its rule
/// covers by either path of a rename, each given that file's diff and its new
/// text around the hunks. Each call also gets the findings of <open> in its
/// group's rules, and closes each one whose problem the code no longer has.
/// A group that holds findings of <open> reads every file of <diff>, so that
/// it sees a fix in any file. Of the other groups, none reads a file where a
/// group of <picked> holds findings of <open>, and each reads every other
/// file it reads on a first review. The calls of a group come back as one
/// call with the findings of all.
pub fn ask_groups(
    material: &Material,
    reviewer: &Path,
    picked: &[(&Group, Vec<&Rule>)],
    diff: &Diff,
    brief: Option<&(String, String)>,
    open: &[Finding],
) -> Asked {
    // The units a first review asks, and the files of each group's units that
    // it does not read now. A unit of the whole diff reads every file of it,
    // and the commit messages when its group reads them.
    let first: Vec<Vec<Unit>> = picked
        .iter()
        .enumerate()
        .map(|(at, (group, members))| units_of(group, members, at, diff, open))
        .collect();
    let not_read: Vec<Vec<String>> = first
        .iter()
        .enumerate()
        .map(|(at, units)| {
            let read: Vec<FileDiff> = units
                .iter()
                .flat_map(|unit| match &unit.file {
                    Some(file) => vec![file.clone()],
                    None => files::split(&diff.text_for(unit.group)),
                })
                .collect();
            not_read(picked, at, &read, open)
        })
        .collect();
    // Each group's diff: <diff> without the files the group does not read.
    let diffs: Vec<Diff> = not_read.iter().map(|paths| diff.without(paths)).collect();
    let quotes: Vec<Vec<DiffFile>> = diffs.iter().map(Diff::quotes).collect();
    // The reviewers read the repository at the commit under review.
    let checkout = match diff.source.as_ref().map(files::Source::checkout) {
        Some(Ok(checkout)) => Some(checkout),
        Some(Err(why)) => {
            let why = why.text();
            return Asked {
                calls: picked
                    .iter()
                    .map(|(group, members)| bare_call(group, members, Some(why.clone())))
                    .collect(),
                repeat: Tokens::default(),
                nothing_to_read: Vec::new(),
                not_read: Vec::new(),
            };
        }
        None => None,
    };
    // builds does not ask a unit whose files its group does not read now.
    let units: Vec<Unit> = first
        .into_iter()
        .flatten()
        .filter(|unit| match &unit.file {
            Some(file) => !not_read[unit.diff].contains(&file.path),
            None => {
                not_read[unit.diff].is_empty()
                    || !diffs[unit.diff].text_for(unit.group).trim().is_empty()
            }
        })
        .collect();
    let one = |unit: &Unit, pass: usize| {
        let group = unit.group;
        let read = &diffs[unit.diff];
        let (text_diff, context) = match &unit.file {
            Some(file) => (
                file.text.clone(),
                files::context(read.source.as_ref(), &file.path, &file.text),
            ),
            None => (read.text_for(group), String::new()),
        };
        let names = prompt::answer_names(group, unit.members);
        let given: Vec<&Finding> = open.iter().filter(|f| names.contains(&f.rule)).collect();
        let parts = prompt::build(&prompt::Input {
            group,
            rules: unit.members,
            project_text: &material.project_text,
            brief,
            diff: &text_diff,
            context: &context,
            open: &given,
        });
        let setup = prompt::setup(group, unit.members);
        let given_ids: Vec<i64> = given.iter().map(|f| f.id).collect();
        let started_at = crate::app_timing::now();
        let began = std::time::Instant::now();
        let output = call::run(
            reviewer,
            &group.model,
            &names,
            &given_ids,
            group.enumerate,
            &parts,
            checkout.as_ref().map(|c| c.path.as_path()),
        );
        let exchange = NewExchange {
            model: group.model.clone(),
            unit: unit
                .file
                .as_ref()
                .map(|f| f.path.clone())
                .unwrap_or_default(),
            pass: pass as i64,
            system: parts.system.clone(),
            message: parts.message.clone(),
            output: output.output.clone(),
            tokens_in: output.usage.tokens_in,
            tokens_out: output.usage.tokens_out,
            tokens_cached: output.usage.tokens_cached,
            tokens_written: output.usage.tokens_written,
            started_at,
            ms: began.elapsed().as_millis() as i64,
        };
        let mut call = checked(
            material,
            group,
            unit.members,
            &quotes[unit.diff],
            &names,
            setup,
            output,
        );
        call.exchanges.push(exchange);
        call
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
    for (i, call) in first
        .iter()
        .zip(parallel_map(&first, |&i| one(&units[i], 0)))
    {
        done[*i].push(call);
    }
    for (i, call) in rest.iter().zip(parallel_map(&rest, |&i| one(&units[i], 0))) {
        done[*i].push(call);
    }
    let most = units.iter().map(|u| u.group.passes).max().unwrap_or(1);
    let mut repeat = Tokens::default();
    for pass in 1..most {
        let again: Vec<usize> = (0..units.len())
            .filter(|&i| units[i].group.passes > pass)
            .collect();
        for (i, call) in again
            .iter()
            .zip(parallel_map(&again, |&i| one(&units[i], pass)))
        {
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
    // A group that reads one file per call and covers a path some commit
    // changed has no file to read when a later commit changed that path back:
    // the range diff holds none of its files. Such a group has nothing to
    // find, so its answer is no findings. When other groups hold the open
    // findings in every file a group would read, builds does not ask that
    // group.
    let mut nothing_to_read = Vec::new();
    for (at, (group, members)) in picked.iter().enumerate() {
        if not_read[at].is_empty() && !units.iter().any(|unit| unit.group.name == group.name) {
            calls.push(bare_call(group, members, None));
            nothing_to_read.push(group.name.clone());
        }
    }
    Asked {
        calls,
        repeat,
        nothing_to_read,
        not_read: picked
            .iter()
            .zip(not_read)
            .filter(|(_, paths)| !paths.is_empty())
            .map(|((group, _), paths)| (group.name.clone(), paths))
            .collect(),
    }
}

/// What a review asked: one call per group, the tokens of the passes after
/// each group's first, the groups that had no file to read in the diff, and
/// the files each group did not read.
pub struct Asked {
    pub calls: Vec<NewCall>,
    pub repeat: Tokens,
    /// Groups that read one file per call and found no file of theirs in the
    /// diff: each has a call with no findings that ran no reviewer.
    pub nothing_to_read: Vec<String>,
    /// Each group with the files of the diff it did not read, because other
    /// groups hold the open findings in them. A group that read none of the
    /// diff's files has no call.
    pub not_read: Vec<(String, Vec<String>)>,
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

/// A call of <group> that ran no reviewer: failed with <failure>, or, with
/// none, an answer of no findings.
fn bare_call(group: &Group, members: &[&Rule], failure: Option<String>) -> NewCall {
    NewCall {
        group: group.name.clone(),
        model: group.model.clone(),
        prompt: prompt::setup(group, members),
        rules: prompt::answer_names(group, members),
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        tokens_written: 0,
        cost_microdollars: 0,
        sites: None,
        failure,
        verdicts: Vec::new(),
        findings: Vec::new(),
        closes: Vec::new(),
        exchanges: Vec::new(),
    }
}

/// <held> with <more>: merge adds the tokens, joins the verdicts (`finding`
/// when either is), collects the findings, closed findings and sites, and
/// keeps the failures.
fn merge(held: &mut NewCall, more: NewCall) {
    held.exchanges.extend(more.exchanges);
    held.tokens_in += more.tokens_in;
    held.tokens_out += more.tokens_out;
    held.tokens_cached += more.tokens_cached;
    held.tokens_written += more.tokens_written;
    held.cost_microdollars += more.cost_microdollars;
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
    for id in more.closes {
        if !held.closes.contains(&id) {
            held.closes.push(id);
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

/// The call as the store holds it: builds keeps with the reason it dropped
/// it each finding whose clause is not in its rule, that is on the commit
/// messages when <group> does not read them, whose code is not in <quotes>, or
/// whose code sits only on comment lines when its rule's group judges no code
/// comment.
fn checked(
    material: &Material,
    group: &Group,
    members: &[&Rule],
    quotes: &[DiffFile],
    names: &[String],
    setup: String,
    output: call::Output,
) -> NewCall {
    let mut call = NewCall {
        group: group.name.clone(),
        model: group.model.clone(),
        prompt: setup,
        rules: names.to_vec(),
        tokens_in: output.usage.tokens_in,
        tokens_out: output.usage.tokens_out,
        tokens_cached: output.usage.tokens_cached,
        tokens_written: output.usage.tokens_written,
        cost_microdollars: output.usage.cost_microdollars,
        sites: None,
        failure: None,
        verdicts: Vec::new(),
        findings: Vec::new(),
        closes: Vec::new(),
        exchanges: Vec::new(),
    };
    let answer = match output.answer {
        Ok(answer) => answer,
        Err(why) => {
            call.failure = Some(why);
            return call;
        }
    };
    call.closes = answer.closed;
    call.verdicts = answer.verdicts;
    call.sites = answer.sites;
    for raw in answer.findings {
        let (rule_text, blocking) = if raw.rule == PROJECT_FACTS {
            (material.project_text.as_str(), false)
        } else {
            let rule = members
                .iter()
                .find(|r| r.slug == raw.rule)
                .expect("the answer's rules were checked against the group's");
            (rule.text.as_str(), rule.blocking)
        };
        let check_failed = if !clause_in(&raw.clause, rule_text) {
            Some("its clause is not in the rule's text".to_string())
        } else if raw.file == files::COMMIT_MESSAGE && !group.commit_text {
            Some(format!(
                "it is on the commit messages ({}), which group {} does not read",
                files::COMMIT_MESSAGE,
                group.name
            ))
        } else if let Err(why) = code_in(quotes, &raw.file, &raw.code) {
            Some(why)
        } else if material.skips_comments(&raw.rule)
            && only_on_comment_lines(quotes, &raw.file, &raw.code)
        {
            Some(format!(
                "its code is only comment lines, and the group of rule {} judges no code comment",
                raw.rule
            ))
        } else {
            None
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

/// The commit of the rules checkout at <root>, empty when it is no repo.
pub fn rules_version(root: &Path) -> String {
    git(root)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
