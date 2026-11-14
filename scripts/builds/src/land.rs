//! `builds land WORKTREE [--done N]... [--part N]... [--design N] SHA...`: puts
//! the commits on origin/main (a fast-forward when they sit on it, else a
//! cherry-pick), retrying the whole step on a refused push.
//!
//! WORKTREE is a detached landing worktree, claimed by whoever lands (`builds
//! claim`; the claim names who lands, and the store records that name). One
//! landing at a time runs in a worktree: two landings in one worktree move
//! each other's HEAD, and one pushed the other's commits and called them
//! landed, so each waits for the worktree to itself (an exclusive flock on
//! `land.lock` in the worktree's git dir). An agent's worktree sits on its
//! branch; landing there would move the agent off it, so a worktree on a branch
//! is refused.
//!
//! A repo that belongs to a project of the lists (`builds lists project-of`)
//! names the work items a landing finishes (`--done N`) or advances (`--part
//! N`) and, when the commits change design places, the design change (`--design
//! N`), which the person must have approved (`builds design check` says so).
//! Each is checked before anything is fetched or picked, and recorded after the
//! push. `builds design places` prints one line per changed design path
//! (`place`, `source`, `spec` or `shared`, then the path), nothing when none;
//! any line, shared-only included, requires `--design`. When every commit's
//! paths are design files (`builds design only`: boards, the design system,
//! spec lines), `--design N` alone satisfies the `--done`/`--part`
//! requirement: the design change holds the app work. A repo in no project
//! takes none of these flags and lands as before.
//!
//! A cherry-picked result is a tree no hook has seen, so before it is pushed
//! the repo's whole-tree checks run: every `whole` line of the repo's `.checks`
//! (the static checks that read the whole tree, which the pre-commit hook
//! also runs when a path matches; checks.rs says how a line is written), each
//! with its time. Every one runs even after another fails; the failing ones
//! are named with their output, nothing is pushed, and the worktree goes back
//! to origin/main (exit 1, no retry). A leftover `.land-checks` file fails the
//! landing the same way. Then the repo's `.land-compile`, if present, runs (`bash
//! .land-compile` from the repo root, reading what the commits changed from
//! `git diff origin/main HEAD`): the compile check of the result, which the
//! static checks can't do, since two commits that each compile can break each
//! other. It fails the same way. A fast-forward runs both too: a branch rebased
//! after its hook ran is a tree no hook has seen.
//!
//! The store's checks and records run as `builds` commands of this same
//! program, so their output is held back and shown only when one fails.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::git;
use crate::fail;
use crate::fail::{Context, Fail, Result};

const USAGE: &str = "usage: builds land <worktree> [--done N]... [--part N]... [--design N] <sha>... (the landing worktree must be claimed; --design is required exactly when the commits change a design place or shared design file, and alone it lands commits that are all design files)";

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

/// <arg> as a shell would need it written, for a command the person reruns.
fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Waits for, then holds, the exclusive lock of one landing in a worktree.
fn lock_worktree(worktree: &Path) -> Result<std::fs::File> {
    let git_dir = git_text(worktree, &["rev-parse", "--absolute-git-dir"])
        .ok_or_else(|| Fail::Message(format!("{} is not a git worktree", worktree.display())))?;
    let path = PathBuf::from(git_dir).join("land.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .context(|| format!("opening {}", path.display()))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        fail!(
            "locking {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(file)
}

/// What a landing was asked for, from the arguments after the worktree.
struct Request {
    /// `--done N` and `--part N`, as given.
    items: Vec<String>,
    design: Option<String>,
    shas: Vec<String>,
}

fn parse(args: &[String], out: &mut dyn Write) -> Result<Request> {
    let mut items = Vec::new();
    let mut design: Option<String> = None;
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
                if design.is_some() {
                    return Err(refuse(out, "refusing: --design given twice"));
                }
                design = Some(rest[1].clone());
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
        design,
        shas: rest.to_vec(),
    })
}

struct Landing<'a> {
    program: &'a Path,
    workdir: PathBuf,
    request: Request,
    /// The project of the repo; None when it is in no project.
    project: Option<String>,
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
        let Some(project) = &self.project else {
            if !request.items.is_empty() || request.design.is_some() {
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
        if request.items.is_empty() {
            let Some(design) = &request.design else {
                return Err(refuse(
                    out,
                    format!(
                        "refusing: {workdir} is in the project {project}; name the work item this lands with --done N or --part N (commits that are all design files take --design N alone)"
                    ),
                ));
            };
            if places.is_empty() {
                return Err(refuse(
                    out,
                    format!(
                        "refusing: --design {design} given, but the commits change no design place; name the work item with --done N or --part N"
                    ),
                ));
            }
            let (only, said) =
                self.run_line(&self.with_items(&["design", "only"], false, &request.shas));
            if !only {
                let _ = writeln!(
                    out,
                    "LANDING REFUSED, nothing fetched or pushed: --design alone lands only commits that are all design files; name the work item with --done N or --part N"
                );
                return Err(refuse(out, said));
            }
        } else {
            let (ok, said) =
                self.run_line(&self.with_items(&["item", "check-landing"], true, &request.shas));
            if !ok {
                let _ = writeln!(out, "LANDING REFUSED, nothing fetched or pushed:");
                return Err(refuse(out, said));
            }
        }
        if !places.is_empty() && request.design.is_none() {
            let _ = writeln!(
                out,
                "LANDING REFUSED, nothing fetched or pushed: the commits change design places; name the design change with --design N"
            );
            return Err(refuse(out, places));
        }
        if places.is_empty()
            && let Some(design) = &request.design
        {
            return Err(refuse(
                out,
                format!(
                    "refusing: --design {design} given, but the commits change no design place"
                ),
            ));
        }
        if let Some(design) = &request.design {
            let mut line = vec!["design".to_string(), "check".to_string(), design.clone()];
            line.push(workdir.to_string());
            line.extend(request.shas.iter().cloned());
            let (ok, said) = self.run_line(&line);
            if !ok {
                let _ = writeln!(out, "LANDING REFUSED, nothing fetched or pushed:");
                return Err(refuse(out, said));
            }
        }
        Ok(())
    }

    /// Records what landed, as `builds item landed` and `builds design landed`
    /// with the commits now on origin/main. Both are idempotent, so a failure
    /// prints the rerun. Says whether every record was written.
    fn record(&self, on_main: &[String], out: &mut dyn Write) -> bool {
        if self.project.is_none() {
            return true;
        }
        let mut all_written = true;
        let mut lines: Vec<Vec<String>> = Vec::new();
        if !self.request.items.is_empty() {
            lines.push(self.with_items(&["item", "landed"], true, on_main));
        }
        if let Some(design) = &self.request.design {
            let mut line = vec!["design".to_string(), "landed".to_string(), design.clone()];
            line.push(self.workdir.display().to_string());
            line.extend(on_main.iter().cloned());
            lines.push(line);
        }
        for line in lines {
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

    /// Runs the repo's whole-tree checks and compile check on the result.
    /// On a failure the worktree goes back to origin/main and the failure is said.
    fn check_result(&self, out: &mut dyn Write) -> Result<()> {
        let back = |out: &mut dyn Write, headline: String, said: String| {
            git_ok(&self.workdir, &["reset", "-q", "--hard", "origin/main"]);
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

    fn put_on_main(&self, out: &mut dyn Write) -> Result<()> {
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
                    if self.project.is_some() && on_main.iter().any(String::is_empty) {
                        return Err(refuse(
                            out,
                            "LANDED, NOT RECORDED: a commit's copy on origin/main was not found by its subject; record by hand with builds item landed",
                        ));
                    }
                    return if self.record(&on_main, out) {
                        Ok(())
                    } else {
                        Err(Fail::Exit(1))
                    };
                }
                let mut pick = git(dir);
                pick.arg("cherry-pick").args(todo);
                pick.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if !pick.status().is_ok_and(|status| status.success()) {
                    git_ok(dir, &["cherry-pick", "--abort"]);
                    return Err(refuse(out, "CONFLICT"));
                }
            }
            let landed = git_text(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            self.check_result(out)?;
            // Something else that moved this worktree's HEAD during the checks
            // (a checkout run by hand) leaves nothing of the landing's to push,
            // and the push of origin/main to itself would report success. HEAD
            // is compared with the commit the landing made, not by patch: a
            // cherry-pick onto changed surrounding lines has another patch id
            // though it holds the change.
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
                return if self.record(&on_main, out) {
                    Ok(())
                } else {
                    Err(Fail::Exit(1))
                };
            }
            let last_line = said
                .lines()
                .rfind(|line| !line.starts_with("hint"))
                .unwrap_or("");
            let _ = writeln!(out, "push failed: {last_line}");
        }
        Err(Fail::Exit(1))
    }
}

/// The landing, with the program that answers the `builds` commands it runs
/// (this program; tests pass a stand-in) and <out> for what it says.
pub fn land(program: &Path, args: &[String], out: &mut dyn Write) -> Result<()> {
    let Some((worktree, rest)) = args.split_first() else {
        return Err(refuse(out, USAGE));
    };
    let workdir = std::path::absolute(worktree).context(|| format!("resolving {worktree}"))?;
    let _lock = lock_worktree(&workdir)?;
    // An agent's worktree sits on its branch; the landing worktree is
    // detached. Landing there would move the agent off its branch.
    if let Some(branch) = git_text(&workdir, &["symbolic-ref", "-q", "--short", "HEAD"]) {
        return Err(refuse(
            out,
            format!(
                "refusing: {} is on branch {branch}; land from the detached landing worktree",
                workdir.display()
            ),
        ));
    }
    let request = parse(rest, out)?;
    let mut landing = Landing {
        program,
        workdir,
        request,
        project: None,
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
    landing.project = (!project.is_empty()).then_some(project);
    landing.check_lists(out)?;
    landing.put_on_main(out)
}

#[cfg(test)]
#[path = "land_tests.rs"]
mod tests;
