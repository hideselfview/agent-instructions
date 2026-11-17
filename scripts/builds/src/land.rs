//! `builds land WORKTREE [--done N]... [--part N]... [--design N]... SHA...`: puts
//! the commits on origin/main from the worktree whose branch holds them (a
//! fast-forward when they sit on origin/main, else a cherry-pick). When origin
//! refuses the push, land fetches again and repeats the whole step.
//!
//! WORKTREE is the side worktree the branch's agent worked in, once the agent
//! is done there. Before it fetches anything, land refuses the worktree while
//! it has uncommitted changes, untracked files included: they are the agent's
//! work in progress, and the checkouts land makes would carry them into the
//! result or stop on them. Land also refuses a commit that is not on the
//! worktree's HEAD, since land puts a branch on origin/main only from the
//! worktree that has the branch checked out. In a repo of a project, land
//! refuses an unclaimed worktree: each record after the push names who landed.
//! Land reads that name from the worktree's claim before it starts and passes
//! it to every record as `--by`, so the records after `builds item landed`
//! still have the name after that command releases the claim.
//!
//! Land detaches HEAD at origin/main and builds the result there. After a
//! refusal, land checks out the branch again. After the push, land leaves the
//! worktree detached at the landed commit when its branch has nothing left to
//! land, which `builds new` and `prune` treat as free once the claim is gone;
//! when commits on the branch still wait to land, land checks the branch out
//! again. Once land writes every record, a landing that leaves nothing to land
//! and names no `--part` releases the worktree (`builds release`) when `builds
//! item landed` has not released it already. A `--part` leaves its item running
//! there, so that landing keeps the claim.
//!
//! One landing at a time runs in a worktree: two landings in one worktree would
//! each change the HEAD the other builds on, and once one pushed the other's
//! commits and recorded them as its own. So each landing waits until no other
//! landing runs in the worktree (an exclusive flock on `land.lock` in the
//! worktree's git dir, held through the push and the records), and prints which
//! landing it waits for. Landings in different worktrees run at the same time:
//! when origin/main moves after a landing fetched it, origin refuses that
//! landing's push, and the landing fetches again and tries again.
//!
//! A repo that belongs to a project of the lists (`builds lists project-of`)
//! names the work items a landing finishes (`--done N`) or advances (`--part
//! N`) and, when the commits change design places, the design changes
//! (`--design N`, once per change), which the person must have approved and
//! which together must resolve every design place the commits change (`builds
//! design check` says so for all of them at once). Land checks each before it
//! fetches or cherry-picks anything, and records each after the push. `builds
//! item check-landing` refuses an item that runs in another worktree of the
//! repo, since landing it here would check out other commits in the worktree
//! that item's agent works in. `builds design places`
//! prints one line per changed design path (`place`, `source`, `spec` or
//! `shared`, then the path), nothing when none;
//! any line, shared-only included, requires `--design`. When every commit's
//! paths are design files (`builds design only`: boards, the design system,
//! spec lines), `--design` alone satisfies the `--done`/`--part`
//! requirement: the design changes hold the app work. A repo in no project
//! takes none of these flags and lands as before.
//!
//! A cherry-picked result is a tree no hook has seen, so before it is pushed
//! the repo's whole-tree checks run: every `whole` line of the repo's `.checks`
//! (the static checks that read the whole tree, which the pre-commit hook
//! also runs when a path matches; checks.rs says how a line is written), each
//! with its time. Every one runs even after another fails; land names the
//! failing ones with their output, pushes nothing, and checks the branch out
//! again (exit 1, no retry). A leftover `.land-checks` file fails the
//! landing the same way. Then the repo's `.land-compile`, if present, runs (`bash
//! .land-compile` from the repo root, reading what the commits changed from
//! `git diff origin/main HEAD`): the compile check of the result, which the
//! static checks can't do, since two commits that each compile can break each
//! other. It fails the same way. A fast-forward runs both too: a branch rebased
//! after its hook ran is a tree no hook has seen.
//!
//! In a repo of a project, the commits must have been reviewed against the
//! rules before they land. No diff is reviewed more than once. Each change
//! gets one review; the coordinator reads the fixes for its confirmed
//! findings. `builds review check-landing`, run after the lists' checks and
//! before anything is fetched, refuses unless a change's review (the one whose
//! runs read one of the commits, found by the patch id of its added and
//! removed lines) has read the branch's net change as it is now, so a rebase
//! that keeps every line the branch adds and removes counts, and the review is
//! finished: every group asked over a diff a run read has answered, the
//! coordinator has settled every finding, and each confirmed finding on the
//! commits has its fix among them, unless builds finds the finding's code on
//! no line of the branch's change past origin/main; check-landing then records
//! that. Commits whose paths no group covers need no review. After
//! the push, `builds review landed` records the commits pushed beside the
//! items and the design change. When commits of the branch still wait to land,
//! land passes `--more-to-land` to it and the review's landing stays open for
//! them; otherwise `builds review landed` records the landing as landed and
//! starts the audit of a sampled one, which never fails the landing.
//!
//! After the records, land fast-forwards the repo's main checkout to
//! origin/main (`git merge --ff-only`), so the person and the agents who read
//! code there see what landed (main_checkout.rs). In agent-instructions the
//! checkout's post-merge hook then runs install.sh, which installs the landed
//! builds and links the landed rules and skills. The main checkout is the
//! person's: when it is not on main, or git refuses the fast-forward because
//! the person's commits or uncommitted edits there are in the way, land
//! leaves it as it is and prints a line that starts with MAIN CHECKOUT
//! BEHIND, says which checkout and why, and gives the command to run once the
//! person checks out main there, or pushes, commits or removes those commits
//! and edits. The landing still succeeds: the
//! commits stay on origin/main and the records stay written.
//!
//! The store's checks and records run as `builds` commands of this same
//! program, so their output is held back and shown only when one fails.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::git;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Id;
use crate::text::shell_word;

const USAGE: &str = "usage: builds land <worktree> [--done N]... [--part N]... [--design N]... <sha>... (run it in the worktree whose branch holds the commits, with nothing uncommitted there, and in a repo of a project with a claim on it; land requires --design exactly when the commits change a design place or shared design file, once for each design change that resolves some of them, and takes --design alone for commits that are all design files)";

/// How many times a refused push is tried again from a fresh fetch.
const ATTEMPTS: usize = 5;

pub fn cmd_land(args: &[String]) -> Result<()> {
    let program = std::env::current_exe().context(|| "finding this program".to_string())?;
    let stdout = std::io::stdout();
    land(&program, args, &mut stdout.lock())
}

/// Ends the landing with <message> on <out> and exit status 1.
fn refuse(out: &mut dyn Write, message: impl AsRef<str>) -> Fail {
    let _ = writeln!(out, "{}", message.as_ref());
    Fail::Exit(1)
}

/// Runs <command> with stdin closed and stdout and stderr in one stream.
/// Says whether it succeeded and what it printed, without trailing newlines.
fn merged(mut command: Command) -> (bool, String) {
    let (mut reader, writer) = match std::io::pipe() {
        Ok(pipe) => pipe,
        Err(error) => return (false, error.to_string()),
    };
    let again = match writer.try_clone() {
        Ok(again) => again,
        Err(error) => return (false, error.to_string()),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(again));
    let child = command.spawn();
    // The command holds the write ends until it is dropped; the reader sees
    // the end of the stream only once they are closed.
    drop(command);
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return (false, error.to_string()),
    };
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    let ok = child.wait().is_ok_and(|status| status.success());
    let text = String::from_utf8_lossy(&bytes);
    (ok, text.trim_end_matches('\n').to_string())
}

/// Whether the git command succeeded, saying nothing.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What the git command printed, without trailing newlines, or nothing when it failed.
fn git_text(dir: &Path, args: &[&str]) -> Option<String> {
    let output = git(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches('\n')
            .to_string()
    })
}

/// Waits for, then holds, the exclusive lock of one landing in a worktree.
/// While another landing holds it, says on <out> that this one waits and which
/// landing it waits behind (the holder writes its process and arguments into
/// the lock file once it has the lock).
fn lock_worktree(worktree: &Path, args: &[String], out: &mut dyn Write) -> Result<std::fs::File> {
    let git_dir = git_text(worktree, &["rev-parse", "--absolute-git-dir"])
        .ok_or_else(|| Fail::Message(format!("{} is not a git worktree", worktree.display())))?;
    let path = PathBuf::from(git_dir).join("land.lock");
    let mut file = crate::flock::open_creating(&path)?;
    if !crate::flock::try_lock(&file, libc::LOCK_EX, &path)? {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        let holder = holder.trim();
        let _ = writeln!(
            out,
            "waiting for the landing in {} to finish first: {}",
            worktree.display(),
            if holder.is_empty() {
                "another landing holds it"
            } else {
                holder
            }
        );
        let _ = out.flush();
        crate::flock::lock(&file, libc::LOCK_EX, &path)?;
        let _ = writeln!(out, "the landing in {} has its turn", worktree.display());
    }
    let text = format!(
        "process {} landing {}",
        std::process::id(),
        args.iter()
            .map(|arg| shell_word(arg))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let _ = file.set_len(0);
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.write_all(text.as_bytes());
    Ok(file)
}

/// What a landing was asked for, from the arguments after the worktree.
struct Request {
    /// `--done N` and `--part N`, as given.
    items: Vec<String>,
    /// Each `--design N`, in the order given.
    designs: Vec<Id>,
    shas: Vec<String>,
}

impl Request {
    /// Does the landing advance an item without finishing it?
    fn has_part(&self) -> bool {
        self.items.iter().any(|word| word == "--part")
    }

    /// The `--design N` flags the landing names, joined with spaces, for the
    /// refusal messages.
    fn design_flags(&self) -> String {
        let flags: Vec<String> = self
            .designs
            .iter()
            .map(|id| format!("--design {}", id.0))
            .collect();
        flags.join(" ")
    }
}

/// Where the worktree's HEAD was when the landing began: on its branch, or
/// detached at a commit. Land checks it out again after a refusal, and after a
/// landing that leaves commits there to land later.
enum Start {
    Branch(String),
    Detached(String),
}

impl Start {
    fn read(dir: &Path) -> Result<Start> {
        if let Some(branch) = git_text(dir, &["symbolic-ref", "-q", "--short", "HEAD"]) {
            return Ok(Start::Branch(branch));
        }
        match git_text(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            Some(sha) => Ok(Start::Detached(sha)),
            None => fail!("{} has no commit checked out", dir.display()),
        }
    }

    /// The rev git knows it by.
    fn rev(&self) -> &str {
        match self {
            Start::Branch(name) | Start::Detached(name) => name,
        }
    }

    /// What the landing calls it.
    fn name(&self) -> String {
        match self {
            Start::Branch(branch) => format!("branch {branch}"),
            Start::Detached(sha) => format!("detached HEAD {}", &sha[..sha.len().min(12)]),
        }
    }

    /// Checks it out in <dir> again; says whether git did.
    fn check_out(&self, dir: &Path) -> bool {
        match self {
            Start::Branch(branch) => git_ok(dir, &["checkout", "-q", branch, "--"]),
            Start::Detached(sha) => git_ok(dir, &["checkout", "-q", "--detach", sha]),
        }
    }
}

/// Refuses a worktree with uncommitted changes, untracked files included:
/// they are its agent's work in progress, and the checkouts land makes would
/// carry them into the result or stop on them.
fn require_clean(dir: &Path, out: &mut dyn Write) -> Result<()> {
    let Some(status) = git_text(dir, &["status", "--porcelain"]) else {
        return Err(refuse(
            out,
            format!("refusing: git status failed in {}", dir.display()),
        ));
    };
    if status.is_empty() {
        return Ok(());
    }
    Err(refuse(
        out,
        format!(
            "refusing: {} has uncommitted changes, so its agent is not done there; land fetched and pushed nothing. Commit or remove them first:\n{status}",
            dir.display()
        ),
    ))
}

/// Refuses a commit that is not on the worktree's HEAD: land puts a branch on
/// origin/main only from the worktree that has the branch checked out.
fn require_on_head(dir: &Path, start: &Start, shas: &[String], out: &mut dyn Write) -> Result<()> {
    for sha in shas {
        let commit = format!("{sha}^{{commit}}");
        if git_text(dir, &["rev-parse", "--verify", "--quiet", &commit]).is_none() {
            return Err(refuse(
                out,
                format!("refusing: {sha} is not a commit in {}", dir.display()),
            ));
        }
        if !git_ok(dir, &["merge-base", "--is-ancestor", sha, "HEAD"]) {
            return Err(refuse(
                out,
                format!(
                    "refusing: {sha} is not on {} in {}; run builds land in the worktree that has the commit's branch checked out",
                    start.name(),
                    dir.display()
                ),
            ));
        }
    }
    Ok(())
}

fn parse(args: &[String], out: &mut dyn Write) -> Result<Request> {
    let mut items = Vec::new();
    let mut designs: Vec<Id> = Vec::new();
    let mut rest = args;
    while let Some(arg) = rest.first() {
        match arg.as_str() {
            "--done" | "--part" => {
                if rest.len() < 2 {
                    return Err(refuse(out, format!("refusing: {arg} needs an item number")));
                }
                items.push(arg.clone());
                items.push(rest[1].clone());
                rest = &rest[2..];
            }
            "--design" => {
                if rest.len() < 2 {
                    return Err(refuse(
                        out,
                        "refusing: --design needs a design change number",
                    ));
                }
                let id = match Id::parse(&rest[1]) {
                    Ok(id) => id,
                    Err(error) => {
                        return Err(refuse(out, format!("refusing: --design: {}", error.text())));
                    }
                };
                if designs.contains(&id) {
                    return Err(refuse(
                        out,
                        format!("refusing: the landing names --design {} twice", id.0),
                    ));
                }
                designs.push(id);
                rest = &rest[2..];
            }
            "--" => {
                rest = &rest[1..];
                break;
            }
            flag if flag.starts_with("--") => {
                return Err(refuse(out, format!("refusing: unknown option {flag}")));
            }
            _ => break,
        }
    }
    if rest.is_empty() {
        return Err(refuse(out, USAGE));
    }
    Ok(Request {
        items,
        designs,
        shas: rest.to_vec(),
    })
}

/// The project a landing's records go to, and who they name as having landed.
struct Recording {
    project: String,
    /// The name on the worktree's claim, which land reads before it starts.
    by: String,
}

struct Landing<'a> {
    program: &'a Path,
    workdir: PathBuf,
    request: Request,
    start: Start,
    /// What the store records the landing under; None when the repo is in no
    /// project, which records nothing.
    recording: Option<Recording>,
}

impl Landing<'_> {
    /// Runs `builds <args>` (this program), merged output.
    fn builds(&self, args: &[&str]) -> (bool, String) {
        let mut command = Command::new(self.program);
        command.args(args);
        merged(command)
    }

    /// The `builds` command line for <verb> with the landing's worktree, items and <shas>.
    fn with_items(&self, head: &[&str], with_items: bool, shas: &[String]) -> Vec<String> {
        let mut line: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        line.push(self.workdir.display().to_string());
        if with_items {
            line.extend(self.request.items.iter().cloned());
        }
        line.extend(shas.iter().cloned());
        line
    }

    fn run_line(&self, line: &[String]) -> (bool, String) {
        let args: Vec<&str> = line.iter().map(String::as_str).collect();
        self.builds(&args)
    }

    /// The checks of the lists that run before anything is fetched or picked.
    fn check_lists(&self, out: &mut dyn Write) -> Result<()> {
        let request = &self.request;
        let workdir = self.workdir.display();
        let Some(Recording { project, by }) = &self.recording else {
            if !request.items.is_empty() || !request.designs.is_empty() {
                return Err(refuse(
                    out,
                    format!(
                        "refusing: {workdir} is in no project of the lists, so --done, --part and --design don't apply"
                    ),
                ));
            }
            return Ok(());
        };
        let (ok, places) =
            self.run_line(&self.with_items(&["design", "places"], false, &request.shas));
        if !ok {
            return Err(refuse(
                out,
                format!("refusing: builds design places failed: {places}"),
            ));
        }
        let designs = request.design_flags();
        if request.items.is_empty() {
            if request.designs.is_empty() {
                return Err(refuse(
                    out,
                    format!(
                        "refusing: {workdir} is in the project {project}; name the work item this lands with --done N or --part N (commits that are all design files take --design N alone)"
                    ),
                ));
            }
            if places.is_empty() {
                return Err(refuse(
                    out,
                    format!(
                        "refusing: the landing names {designs}, but the commits change no design place; name the work item with --done N or --part N"
                    ),
                ));
            }
            let (only, said) =
                self.run_line(&self.with_items(&["design", "only"], false, &request.shas));
            if !only {
                let _ = writeln!(
                    out,
                    "LANDING REFUSED, land fetched and pushed nothing: --design alone lands only commits that are all design files; name the work item with --done N or --part N"
                );
                return Err(refuse(out, said));
            }
        } else {
            let (ok, said) =
                self.run_line(&self.with_items(&["item", "check-landing"], true, &request.shas));
            if !ok {
                let _ = writeln!(out, "LANDING REFUSED, land fetched and pushed nothing:");
                return Err(refuse(out, said));
            }
        }
        if !places.is_empty() && request.designs.is_empty() {
            let _ = writeln!(
                out,
                "LANDING REFUSED, land fetched and pushed nothing: the commits change design places; name each design change that resolves them with --design N"
            );
            return Err(refuse(out, places));
        }
        if places.is_empty() && !request.designs.is_empty() {
            return Err(refuse(
                out,
                format!(
                    "refusing: the landing names {designs}, but the commits change no design place"
                ),
            ));
        }
        if !request.designs.is_empty() {
            // Land checks all the design changes in one `builds design
            // check`, since together they must resolve every place the
            // commits change.
            let ids: Vec<String> = request.designs.iter().map(|id| id.0.to_string()).collect();
            let mut line = vec!["design".to_string(), "check".to_string(), ids.join(",")];
            line.push(workdir.to_string());
            line.extend(request.shas.iter().cloned());
            let (ok, said) = self.run_line(&line);
            if !ok {
                let _ = writeln!(out, "LANDING REFUSED, land fetched and pushed nothing:");
                return Err(refuse(out, said));
            }
        }
        // check-landing records each confirmed finding whose code is no longer
        // in the branch's change, under the name on the worktree's claim, which
        // land passes as --by.
        let mut line = self.review_line("check-landing", project);
        line.extend(["--by".to_string(), by.clone()]);
        let (ok, said) = self.run_line(&line);
        if !ok {
            let _ = writeln!(out, "LANDING REFUSED, land fetched and pushed nothing:");
            return Err(refuse(out, said));
        }
        Ok(())
    }

    /// `builds review <verb>` for the landing's worktree and commits, as given:
    /// their patch ids find the review that read them.
    fn review_line(&self, verb: &str, project: &str) -> Vec<String> {
        let mut line = vec![
            "review".to_string(),
            verb.to_string(),
            self.workdir.display().to_string(),
            "--project".to_string(),
            project.to_string(),
        ];
        line.extend(self.request.shas.iter().cloned());
        line
    }

    /// Records what landed, as `builds item landed` and one `builds design
    /// landed` for each design change, with the commits now on origin/main,
    /// and `builds review landed` with the commits as given and, when commits
    /// of the branch still wait to land (<waiting>), `--more-to-land`. Each is idempotent, so a failure prints
    /// the rerun. Says whether every record was written.
    fn record(&self, on_main: &[String], waiting: bool, out: &mut dyn Write) -> bool {
        let Some(Recording { project, by }) = &self.recording else {
            return true;
        };
        let mut all_written = true;
        let mut lines: Vec<Vec<String>> = Vec::new();
        if !self.request.items.is_empty() {
            lines.push(self.with_items(&["item", "landed"], true, on_main));
        }
        for design in &self.request.designs {
            let mut line = vec![
                "design".to_string(),
                "landed".to_string(),
                design.0.to_string(),
            ];
            line.push(self.workdir.display().to_string());
            line.extend(on_main.iter().cloned());
            lines.push(line);
        }
        let mut review = self.review_line("landed", project);
        if waiting {
            review.push("--more-to-land".to_string());
        }
        lines.push(review);
        for mut line in lines {
            line.push("--by".to_string());
            line.push(by.clone());
            let (ok, said) = self.run_line(&line);
            if !ok {
                all_written = false;
                let _ = writeln!(out, "LANDED, NOT RECORDED: {said}");
                let words: Vec<String> = line.iter().map(|word| shell_word(word)).collect();
                let _ = writeln!(out, "rerun: builds {}", words.join(" "));
            }
        }
        all_written
    }

    /// The commits named, as the copies on origin/main found by their subject.
    fn copies_on_main(&self, out: &mut dyn Write) -> Vec<String> {
        let mut on_main = Vec::new();
        for sha in &self.request.shas {
            let subject =
                git_text(&self.workdir, &["log", "-1", "--format=%s", sha]).unwrap_or_default();
            let escaped: String = subject
                .chars()
                .flat_map(|c| {
                    if "[]\\.*^$".contains(c) {
                        vec!['\\', c]
                    } else {
                        vec![c]
                    }
                })
                .collect();
            let found = git_text(
                &self.workdir,
                &[
                    "log",
                    "--format=%H",
                    "-1",
                    &format!("--grep=^{escaped}$"),
                    "origin/main",
                ],
            )
            .unwrap_or_default();
            if !found.is_empty() {
                let line = git_text(&self.workdir, &["log", "--oneline", "-1", &found])
                    .unwrap_or_default();
                let _ = writeln!(out, "{line}");
            }
            on_main.push(found);
        }
        on_main
    }

    /// Checks out the branch or commit the worktree had when the landing
    /// began, after a refusal, and prints what went wrong when git fails.
    fn go_back(&self, out: &mut dyn Write) {
        let mut reset = git(&self.workdir);
        reset.args(["reset", "-q", "--hard"]);
        let (reset, said) = merged(reset);
        if !reset {
            // A checkout after a failed reset would carry the refused
            // landing's files onto the branch as uncommitted changes.
            let _ = writeln!(
                out,
                "could not reset {} to drop the refused landing's files, so it stays on the refused result; reset it and check out {} by hand:\n{said}",
                self.workdir.display(),
                self.start.name()
            );
            return;
        }
        if !self.start.check_out(&self.workdir) {
            let _ = writeln!(
                out,
                "could not check out {} again in {}; check it out by hand",
                self.start.name(),
                self.workdir.display()
            );
        }
    }

    /// Runs the repo's whole-tree checks and compile check on the result.
    /// When one fails, land checks out what the worktree had when the landing
    /// began and prints the failure.
    fn check_result(&self, out: &mut dyn Write) -> Result<()> {
        let back = |out: &mut dyn Write, headline: String, said: String| {
            self.go_back(out);
            let _ = writeln!(out, "{headline}");
            refuse(out, said)
        };
        if self.workdir.join(".land-checks").exists() {
            return Err(back(
                out,
                "CHECK FAILED on the result, nothing pushed: .land-checks".to_string(),
                "the repo has a .land-checks file, which builds land no longer reads; \
                 declare each of its commands as a `whole` line in .checks and delete the file"
                    .to_string(),
            ));
        }
        let failed = match crate::checks_run::run_whole(&self.workdir, out) {
            Ok(failed) => failed,
            Err(error) => {
                return Err(back(
                    out,
                    "CHECK FAILED on the result, nothing pushed: .checks".to_string(),
                    error.text(),
                ));
            }
        };
        if !failed.is_empty() {
            let names: Vec<&str> = failed.iter().map(|f| f.name.as_str()).collect();
            let said: Vec<String> = failed
                .iter()
                .map(|f| format!("--- {}\n{}", f.name, f.output.trim_end()))
                .collect();
            return Err(back(
                out,
                format!(
                    "CHECK FAILED on the result, nothing pushed: {}",
                    names.join(", ")
                ),
                said.join("\n"),
            ));
        }
        if self.workdir.join(".land-compile").is_file() {
            let mut command = Command::new("bash");
            command.arg(".land-compile").current_dir(&self.workdir);
            let (ok, said) = merged(command);
            if !ok {
                return Err(back(
                    out,
                    "COMPILE CHECK FAILED on the result, nothing pushed: .land-compile".to_string(),
                    said,
                ));
            }
        }
        Ok(())
    }

    /// Puts the commits on origin/main and returns their copies there, or
    /// refuses after it checks out what the worktree had when the landing
    /// began.
    fn put_on_main(&self, out: &mut dyn Write) -> Result<Vec<String>> {
        let dir = &self.workdir;
        let shas = &self.request.shas;
        let last = shas.last().expect("a landing names a commit").as_str();
        for _ in 0..ATTEMPTS {
            if !git_ok(dir, &["fetch", "-q", "origin"]) {
                let _ = writeln!(out, "fetch failed");
                continue;
            }
            git_ok(dir, &["checkout", "-q", "--detach", "origin/main"]);
            let before = git_text(dir, &["rev-parse", "origin/main"]).unwrap_or_default();
            // A fast-forward lands every commit between origin/main and the
            // last one, so it is taken only when those are exactly the
            // commits named: an unnamed commit below them (an earlier one on
            // the agent's branch) is never carried in.
            let mut all_in = shas
                .iter()
                .all(|sha| git_ok(dir, &["merge-base", "--is-ancestor", sha, last]));
            let between = git_text(
                dir,
                &["rev-list", "--count", &format!("origin/main..{last}")],
            );
            if between.as_deref() != Some(shas.len().to_string().as_str()) {
                all_in = false;
            }
            if !all_in || !git_ok(dir, &["merge", "-q", "--ff-only", last]) {
                git_ok(dir, &["checkout", "-q", "--detach", "origin/main"]);
                // A push that reported failure may still have landed, and
                // someone else may have landed the same change: pick only the
                // commits whose change isn't on origin/main yet, so a retry
                // doesn't call an applied commit a conflict.
                let todo: Vec<&String> = shas
                    .iter()
                    .filter(|sha| {
                        let parent = format!("{sha}^");
                        let marks = git_text(dir, &["cherry", "origin/main", sha, &parent])
                            .unwrap_or_default();
                        let firsts: Vec<String> = marks
                            .lines()
                            .map(|line| line.chars().take(1).collect())
                            .collect();
                        firsts == ["+"]
                    })
                    .collect();
                if todo.is_empty() {
                    let _ = writeln!(out, "already on origin/main:");
                    let on_main = self.copies_on_main(out);
                    // An earlier push that reported failure may have landed
                    // without its record.
                    if self.recording.is_some() && on_main.iter().any(String::is_empty) {
                        self.leave(out);
                        return Err(refuse(
                            out,
                            "LANDED, NOT RECORDED: a commit's copy on origin/main was not found by its subject; record by hand with builds item landed",
                        ));
                    }
                    return Ok(on_main);
                }
                let mut pick = git(dir);
                pick.arg("cherry-pick").args(todo);
                pick.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if !pick.status().is_ok_and(|status| status.success()) {
                    git_ok(dir, &["cherry-pick", "--abort"]);
                    self.go_back(out);
                    return Err(refuse(out, "CONFLICT"));
                }
            }
            let landed = git_text(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            self.check_result(out)?;
            // Something else that moved this worktree's HEAD during the checks
            // (a checkout run by hand) leaves nothing of the landing's to push,
            // and the push of origin/main to itself would report success. Land
            // compares HEAD with the commit it made, not its patch id: another
            // commit that holds the same change has the same patch id.
            let head = git_text(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            if head != landed {
                let name =
                    |rev: &str| git_text(dir, &["log", "--oneline", "-1", rev]).unwrap_or_default();
                return Err(refuse(
                    out,
                    format!(
                        "HEAD moved during the landing: it is {}, not {}; nothing pushed",
                        name("HEAD"),
                        name(&landed)
                    ),
                ));
            }
            let mut push = git(dir);
            push.args(["push", "-q", "origin", "HEAD:main"]);
            let (pushed, said) = merged(push);
            if pushed {
                git_ok(dir, &["fetch", "-q", "origin"]);
                let count = format!("-{}", shas.len());
                let shown =
                    git_text(dir, &["log", "--oneline", &count, "origin/main"]).unwrap_or_default();
                let _ = writeln!(out, "{shown}");
                let on_main: Vec<String> =
                    git_text(dir, &["rev-list", "--reverse", &format!("{before}..HEAD")])
                        .unwrap_or_default()
                        .lines()
                        .map(str::to_string)
                        .collect();
                return Ok(on_main);
            }
            let last_line = said
                .lines()
                .rfind(|line| !line.starts_with("hint"))
                .unwrap_or("");
            let _ = writeln!(out, "push failed: {last_line}");
        }
        self.go_back(out);
        Err(refuse(
            out,
            format!("LANDING FAILED: no push succeeded in {ATTEMPTS} tries"),
        ))
    }

    /// After the push, leaves the worktree detached at the landed commit when
    /// the branch it started on has nothing left to land, and otherwise checks
    /// that branch out again. Prints which, and how many commits wait to land
    /// there. When git fails to say what waits, land treats it as commits
    /// waiting, so the worktree keeps its claim.
    fn leave(&self, out: &mut dyn Write) -> bool {
        let dir = self.workdir.display();
        let start = self.start.name();
        let waiting = match crate::side_git::waiting_to_land(&self.workdir, self.start.rev()) {
            Ok(waiting) if waiting.is_empty() => {
                let _ = writeln!(
                    out,
                    "{dir} is detached at the landed commit: nothing on its {start} waits to land"
                );
                return false;
            }
            Ok(waiting) => format!("{} commit(s) there wait to land", waiting.len()),
            Err(error) => format!("reading what waits to land there failed: {error}"),
        };
        if self.start.check_out(&self.workdir) {
            let _ = writeln!(out, "{dir} is back on its {start}: {waiting}");
        } else {
            let _ = writeln!(
                out,
                "{dir} stays at the landed commit, since git could not check out its {start} again: {waiting}"
            );
        }
        true
    }

    /// Releases the worktree when it still has a claim (`builds item landed`
    /// releases the worktree of an item the landing finishes), and prints the
    /// result. When `builds release` refuses, for example because an item
    /// still runs there, the worktree keeps its claim and the landing still
    /// succeeds: the commits are on origin/main.
    fn release(&self, out: &mut dyn Write) {
        let dir = self.workdir.display().to_string();
        let checkout = match crate::checkout::Checkout::at(&self.workdir) {
            Ok(Some(checkout)) => checkout,
            Ok(None) => {
                let _ = writeln!(out, "kept the claim of {dir}: git finds no checkout there");
                return;
            }
            Err(error) => {
                let _ = writeln!(out, "kept the claim of {dir}: {}", error.text());
                return;
            }
        };
        if !checkout.claim_file().exists() {
            return;
        }
        let (ok, said) = self.builds(&["release", &dir]);
        if ok {
            let _ = writeln!(out, "released {dir}");
        } else {
            let _ = writeln!(out, "kept the claim of {dir}: {said}");
        }
    }

    /// Fast-forwards the repo's main checkout to origin/main, and prints a
    /// MAIN CHECKOUT BEHIND line, which says why and gives the command to
    /// run, when the checkout is not on main or git refuses the fast-forward
    /// (the person's commits or uncommitted edits there stop it).
    fn forward_main_checkout(&self, out: &mut dyn Write) {
        let behind = |out: &mut dyn Write, what: String| {
            let _ = writeln!(
                out,
                "MAIN CHECKOUT BEHIND: the landing is on origin/main, but {what}"
            );
        };
        let main = match crate::checkout::Checkout::at(&self.workdir) {
            Ok(Some(checkout)) => checkout.main,
            found => {
                let why = match found {
                    Err(error) => error.text(),
                    _ => "git finds no checkout there".to_string(),
                };
                behind(
                    out,
                    format!(
                        "land could not find the main checkout of {}: {why}",
                        self.workdir.display()
                    ),
                );
                return;
            }
        };
        let command = crate::main_checkout::forward_command(&main);
        let Some(on) = git_text(&main, &["branch", "--show-current"]) else {
            behind(
                out,
                format!(
                    "git could not say which branch the main checkout {} is on",
                    main.display()
                ),
            );
            return;
        };
        let on = (!on.is_empty()).then_some(on);
        if let Some(why) = crate::main_checkout::off_main(on.as_deref()) {
            behind(
                out,
                format!(
                    "the main checkout {} {why}, so land left it as it is; once the person checks out main there, run: {command}",
                    main.display()
                ),
            );
            return;
        }
        let mut merge = git(&main);
        merge.args(["merge", "--ff-only", "origin/main"]);
        let (ok, said) = merged(merge);
        if ok {
            let _ = writeln!(
                out,
                "fast-forwarded the main checkout {} to origin/main",
                main.display()
            );
            if !said.is_empty() {
                let _ = writeln!(out, "{said}");
            }
        } else {
            behind(
                out,
                format!(
                    "git did not fast-forward the main checkout {}:\n{said}\nThe person's commits or uncommitted edits there stop the fast-forward; once the person pushes, commits or removes them, run: {command}",
                    main.display()
                ),
            );
        }
    }

    /// After the push: leaves the worktree, records the landing, releases the
    /// worktree when its agent has nothing left there, and fast-forwards the
    /// repo's main checkout. Land fast-forwards the main checkout last,
    /// because in agent-instructions its post-merge hook may install a new
    /// builds, and the records run as this program.
    fn finish(&self, on_main: &[String], out: &mut dyn Write) -> Result<()> {
        let waiting = self.leave(out);
        let recorded = self.record(on_main, waiting, out);
        if recorded && !waiting && !self.request.has_part() {
            self.release(out);
        }
        self.forward_main_checkout(out);
        if recorded { Ok(()) } else { Err(Fail::Exit(1)) }
    }
}

/// The name on the worktree's claim, which the records after the push give as
/// who landed; refuses a worktree with no claim.
fn require_claim(workdir: &Path, out: &mut dyn Write) -> Result<String> {
    let claim = match crate::checkout::Checkout::at(workdir)? {
        Some(checkout) => crate::claim::Claim::read(&checkout.claim_file())?,
        None => None,
    };
    match claim {
        Some(claim) if !claim.who.is_empty() => Ok(claim.who),
        _ => Err(refuse(
            out,
            format!(
                "refusing: {} is not claimed; claim it for its agent (builds claim), since the store records the landing under that name",
                workdir.display()
            ),
        )),
    }
}

/// The landing, with the program that answers the `builds` commands it runs
/// (this program; tests pass a stand-in) and <out> for what it says.
pub fn land(program: &Path, args: &[String], out: &mut dyn Write) -> Result<()> {
    let Some((worktree, rest)) = args.split_first() else {
        return Err(refuse(out, USAGE));
    };
    let workdir = std::path::absolute(worktree).context(|| format!("resolving {worktree}"))?;
    let _lock = lock_worktree(&workdir, rest, out)?;
    let request = parse(rest, out)?;
    let start = Start::read(&workdir)?;
    require_clean(&workdir, out)?;
    require_on_head(&workdir, &start, &request.shas, out)?;
    let mut landing = Landing {
        program,
        workdir,
        request,
        start,
        recording: None,
    };
    let (ok, project) = landing.builds(&[
        "lists",
        "project-of",
        &landing.workdir.display().to_string(),
    ]);
    if !ok {
        return Err(refuse(
            out,
            format!("refusing: builds lists project-of failed: {project}"),
        ));
    }
    if !project.is_empty() {
        let by = require_claim(&landing.workdir, out)?;
        landing.recording = Some(Recording { project, by });
    }
    landing.check_lists(out)?;
    let on_main = landing.put_on_main(out)?;
    landing.finish(&on_main, out)
}

#[cfg(test)]
#[path = "land_tests.rs"]
mod tests;
