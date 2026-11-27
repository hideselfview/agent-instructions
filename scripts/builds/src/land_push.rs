//! How a landing puts its commits on origin/main: it fetches origin/main,
//! picks the commits onto it, runs the repo's whole-tree checks on the result
//! and pushes it. A refused push starts again from a fresh fetch. A landing
//! that changes the apps runs no app build of the result and takes no turn:
//! the branch's own passing build, which `builds app-build --landing-coverage`
//! checked before the landing fetched anything (app_landing.rs), is what it
//! trusts. A failed check, or commits that conflict with origin/main, refuse
//! the landing with the output.

use super::*;

/// How picking the landing's commits onto the result ended.
enum Picked {
    /// The commits are on the result: their ids there, oldest first.
    Applied(Vec<String>),
    /// The commits are on origin/main already: their copies there, and what
    /// the landing prints of them.
    OnMain { on_main: Vec<String>, said: String },
    /// The landing ends there with what it prints.
    Refused(String),
}

impl Landing<'_> {
    /// What land says when the cherry-pick of the landing's commits onto the
    /// result failed: the files that conflict (<paths>, one per line, None
    /// when git could not list them), or git's own words when none are
    /// listed, and what the agent does next.
    fn pick_refusal(&self, paths: Option<&str>, said: &str) -> String {
        let start = self.start.name();
        let dir = self.workdir.display();
        let what = if paths.is_none() {
            format!(
                "the cherry-pick onto origin/main failed and git could not list the conflicting files:\n{said}"
            )
        } else if paths.is_some_and(str::is_empty) {
            format!("the cherry-pick onto origin/main failed with no conflicting file:\n{said}")
        } else {
            let list: String = paths
                .unwrap_or_default()
                .lines()
                .map(|path| format!("\n  {path}"))
                .collect();
            format!("these files conflict with origin/main:{list}")
        };
        format!(
            "CONFLICT: {what}\nNothing landed. Next: in {dir}, run `git fetch origin && git rebase origin/main` on {start}, resolve the conflicts, commit, review the branch again and land the new commit ids."
        )
    }

    /// The copies on origin/main of <shas>, found by their subject, and the
    /// lines that show them.
    fn copies_on_main(&self, shas: &[String]) -> (Vec<String>, String) {
        let mut on_main = Vec::new();
        let mut shown = String::new();
        for sha in shas {
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
                shown.push_str(&format!("{line}\n"));
            }
            on_main.push(found);
        }
        (on_main, shown.trim_end().to_string())
    }

    /// Runs the repo's whole-tree checks and compile check on the result:
    /// the text of what failed, when something did.
    fn check_result(&self, out: &mut Report) -> Result<std::result::Result<(), String>> {
        if self.workdir.join(".land-checks").exists() {
            return Ok(Err(
                "CHECK FAILED on the result, nothing pushed: .land-checks\nthe repo has a .land-checks file, which builds land no longer reads; \
                 declare each of its commands as a `whole` line in .checks and delete the file"
                    .to_string(),
            ));
        }
        let failed = match crate::checks_run::run_whole(self.machine, &self.workdir, out) {
            Ok(failed) => failed,
            Err(error) => {
                return Ok(Err(format!(
                    "CHECK FAILED on the result, nothing pushed: .checks\n{}",
                    error.text()
                )));
            }
        };
        if !failed.is_empty() {
            let names: Vec<&str> = failed.iter().map(|f| f.name.as_str()).collect();
            let said: Vec<String> = failed
                .iter()
                .map(|f| format!("--- {}\n{}", f.name, f.output.trim_end()))
                .collect();
            return Ok(Err(format!(
                "CHECK FAILED on the result, nothing pushed: {}\n{}",
                names.join(", "),
                said.join("\n")
            )));
        }
        if self.workdir.join(".land-compile").is_file() {
            let mut command = Command::new("bash");
            command.arg(".land-compile").current_dir(&self.workdir);
            let (ok, said) = merged(command);
            if !ok {
                return Ok(Err(format!(
                    "COMPILE CHECK FAILED on the result, nothing pushed: .land-compile\n{said}"
                )));
            }
        }
        Ok(Ok(()))
    }

    /// Puts the commits on origin/main and returns their copies there, or
    /// refuses after it checks out what the worktree had when the landing
    /// began. A refused push starts again from a fresh fetch.
    pub(super) fn put_on_main(&self, out: &mut Report) -> Result<Vec<String>> {
        let dir = &self.workdir;
        for _ in 0..ATTEMPTS {
            let fetched = {
                let _timing = self.timing(StepKind::Fetch);
                git_ok(dir, &["fetch", "-q", "origin"])
            };
            if !fetched {
                out.say("fetch failed");
                continue;
            }
            let picking = self.timing(StepKind::Pick);
            crate::app_config::run_git(dir, &["checkout", "-q", "--detach", "origin/main"])?;
            let on_main = match self.pick()? {
                Picked::Applied(on_main) => on_main,
                Picked::OnMain { on_main, said } => {
                    out.say(&said);
                    return Ok(on_main);
                }
                Picked::Refused(said) => {
                    self.go_back(out);
                    return Err(refuse(out, said));
                }
            };
            let landed = crate::app_config::run_git(dir, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
            drop(picking);
            {
                let _timing = self.timing(StepKind::Checks);
                if let Err(said) = self.check_result(out)? {
                    self.go_back(out);
                    return Err(refuse(out, said));
                }
            }
            // Something else that moved this worktree's HEAD during the checks
            // (a checkout run by hand) leaves nothing of the landing's to push,
            // and the push of origin/main to itself would report success. Land
            // compares HEAD with the commit it made, not its patch id: another
            // commit that holds the same change has the same patch id.
            let head = crate::app_config::run_git(dir, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
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
            let pushing = self.timing(StepKind::Push);
            let mut push = git(dir);
            push.args(["push", "-q", "origin", "HEAD:main"]);
            let (pushed, said) = merged(push);
            if pushed {
                git_ok(dir, &["fetch", "-q", "origin"]);
                let shown: Vec<String> = on_main
                    .iter()
                    .map(|sha| git_text(dir, &["log", "--oneline", "-1", sha]).unwrap_or_default())
                    .collect();
                if !shown.is_empty() {
                    out.say(shown.join("\n"));
                }
                drop(pushing);
                return Ok(on_main);
            }
            drop(pushing);
            let last_line = said
                .lines()
                .rfind(|line| !line.starts_with("hint"))
                .unwrap_or("");
            out.say(format!("push failed: {last_line}"));
        }
        self.go_back(out);
        Err(refuse(
            out,
            format!("LANDING FAILED: no push succeeded in {ATTEMPTS} tries"),
        ))
    }

    /// Picks the landing's commits onto the worktree's HEAD, origin/main:
    /// a fast-forward when they sit on it, else a cherry-pick. A landing
    /// whose commits conflict, or are already on origin/main, ends there.
    fn pick(&self) -> Result<Picked> {
        let dir = &self.workdir;
        let shas = &self.request.shas;
        let last = shas.last().expect("a landing names a commit").as_str();
        let base = crate::app_config::resolve(dir, "HEAD")?;
        // A fast-forward lands every commit between the result and the last
        // one, so it is taken only when those are exactly the commits named:
        // an unnamed commit below them (an earlier one on the agent's branch)
        // is never carried in.
        let mut all_in = shas
            .iter()
            .all(|sha| git_ok(dir, &["merge-base", "--is-ancestor", sha, last]));
        let between = git_text(dir, &["rev-list", "--count", &format!("HEAD..{last}")]);
        if between.as_deref() != Some(shas.len().to_string().as_str()) {
            all_in = false;
        }
        if !all_in || !git_ok(dir, &["merge", "-q", "--ff-only", last]) {
            // A push that reported failure may still have landed, and someone
            // else may have landed the same change: pick only the commits
            // whose change isn't on origin/main yet, so a retry doesn't call
            // an applied commit a conflict.
            let mut todo: Vec<&String> = Vec::new();
            for sha in shas {
                let parent = format!("{sha}^");
                let marks =
                    crate::app_config::run_git(dir, &["cherry", "origin/main", sha, &parent])?;
                let firsts: Vec<String> = marks
                    .lines()
                    .map(|line| line.chars().take(1).collect())
                    .collect();
                if firsts == ["+"] {
                    todo.push(sha);
                }
            }
            if todo.is_empty() {
                let (on_main, shown) = self.copies_on_main(shas);
                // An earlier push that reported failure may have landed
                // without its record.
                if self.recording.is_some() && on_main.iter().any(String::is_empty) {
                    return Ok(Picked::Refused("LANDED, NOT RECORDED: a commit's copy on origin/main was not found by its subject; record by hand with builds item landed".to_string()));
                }
                return Ok(Picked::OnMain {
                    on_main,
                    said: format!("already on origin/main:\n{shown}"),
                });
            }
            let mut pick = git(dir);
            pick.arg("cherry-pick").args(todo);
            let (picked, said) = merged(pick);
            if !picked {
                let paths = git_text(dir, &["diff", "--name-only", "--diff-filter=U"]);
                git_ok(dir, &["cherry-pick", "--abort"]);
                return Ok(Picked::Refused(self.pick_refusal(paths.as_deref(), &said)));
            }
        }
        let on_main =
            crate::app_config::run_git(dir, &["rev-list", "--reverse", &format!("{base}..HEAD")])?
                .lines()
                .map(str::to_string)
                .collect();
        Ok(Picked::Applied(on_main))
    }
}
