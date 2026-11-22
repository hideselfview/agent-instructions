//! How a landing puts commits on origin/main: it leads the batch of landings
//! that wait for the same lane's turn, or ends with the verdict of the batch
//! that already took it (land_batch.rs).
//!
//! The leader fetches origin/main, picks the commits of every landing of its
//! batch onto it in the order the landings registered, runs the repo's
//! whole-tree checks on the result, asks each landing's branch which app
//! build the result needs, runs one app build with the steps, test classes
//! and shots of all of them, and pushes the result once it passes. A landing
//! whose commits conflict, or whose branch has no app build of its own, is
//! refused alone. When the checks or the build fail on the result of several
//! landings, the leader runs the same steps on each half, so the halves that
//! pass land and the half that fails is split again until one landing is left,
//! which is refused with the output. Each landing keeps its own refusal, so
//! each branch's agent gets its own result.

use super::*;
use crate::land_batch::{self, Entry, Registered, Verdict};
use crate::lane::Lane;

/// The landings that wait with the landing that leads a batch, and what the
/// leader holds for them.
struct Batch {
    queue: PathBuf,
    /// Each other landing's id in the queue and its entry, oldest first.
    others: Vec<(String, Entry)>,
    _turns: Vec<std::fs::File>,
    _registered: Registered,
}

/// What the leader has settled so far for the landings of a batch: its own
/// (index 0) and each of the batch's others (index 1 and up).
struct Settled<'a> {
    own: &'a Entry,
    batch: Option<&'a Batch>,
    verdicts: Vec<Option<Verdict>>,
}

impl<'a> Settled<'a> {
    fn new(own: &'a Entry, batch: Option<&'a Batch>) -> Settled<'a> {
        let others = batch.map_or(0, |batch| batch.others.len());
        Settled {
            own,
            batch,
            verdicts: vec![None; others + 1],
        }
    }

    fn entry(&self, index: usize) -> &'a Entry {
        match (index.checked_sub(1), self.batch) {
            (Some(other), Some(batch)) => &batch.others[other].1,
            _ => self.own,
        }
    }

    /// Settles the landing <index> with <verdict>: the leader keeps its own,
    /// and every other landing reads its verdict from the queue. When the
    /// verdict cannot be written, the batch ends with the error: the landing
    /// waits for a verdict that never comes, and leads a batch of its own.
    fn deliver(&mut self, index: usize, verdict: Verdict) -> Result<()> {
        if let (Some(other), Some(batch)) = (index.checked_sub(1), self.batch) {
            let (id, entry) = &batch.others[other];
            if let Err(error) = land_batch::write_verdict(&batch.queue, id, &verdict) {
                fail!(
                    "writing the verdict of the landing in {}: {}",
                    entry.worktree,
                    error.text()
                );
            }
        }
        self.verdicts[index] = Some(verdict);
        Ok(())
    }
}

/// Why the result of some landings failed.
enum Failure {
    /// A whole-tree check or the compile check, with what land says.
    Checks(String),
    /// The app build, with the end of what it printed.
    Build(String),
}

impl Failure {
    fn verdict_for(&self, member: &Entry) -> Verdict {
        match self {
            Failure::Checks(said) => Verdict::Refused {
                output: String::new(),
                said: said.clone(),
            },
            Failure::Build(output) => Verdict::Refused {
                output: output.clone(),
                said: format!(
                    "APP BUILD FAILED on the result, nothing pushed:\nthe app build above failed on the result, the branch's commits on origin/main. Next: in {}, run `git fetch origin && git rebase origin/main` on {}, fix what the build names, commit, run `builds app-build` on the branch, review the branch again and land the new commit ids.",
                    member.worktree, member.start
                ),
            },
        }
    }
}

/// What taking the turn of the landings that change the apps gave.
enum Turn {
    /// The landing changes no app, or its repo is in no project: it holds no
    /// turn and puts its own commits on main.
    Alone,
    /// The landing holds the turns of every lane its batch changes and leads
    /// the batch.
    Lead(Batch),
    /// The leader of an earlier batch settled this landing.
    Settled(Verdict),
}

/// Writes to <out> and keeps what it wrote.
struct Tee<'a> {
    out: &'a mut dyn Write,
    kept: Vec<u8>,
}

impl Write for Tee<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.kept.extend_from_slice(bytes);
        self.out.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// The last lines of a build's output that its refusal shows.
const OUTPUT_LINES: usize = 200;

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n")
}

/// What the git command printed, without trailing newlines, failing when git
/// fails.
fn git_must(dir: &Path, args: &[&str]) -> Result<String> {
    git_text(dir, args).ok_or_else(|| {
        Fail::Message(format!(
            "git {} failed in {}",
            args.join(" "),
            dir.display()
        ))
    })
}

/// How picking one landing's commits onto the result ended.
enum Picked {
    /// The commits are on the result: their ids there, oldest first.
    Applied(Vec<String>),
    /// The landing ended there with a verdict.
    Ended(Verdict),
}

impl Landing<'_> {
    /// What land says when the cherry-pick of <member>'s commits onto the
    /// result failed: the files that conflict (<paths>, one per line, None
    /// when git could not list them), or git's own words when none are
    /// listed, and what the agent does next.
    fn pick_refusal(member: &Entry, paths: Option<&str>, said: &str) -> String {
        let start = &member.start;
        let dir = &member.worktree;
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
    fn check_result(&self, out: &mut dyn Write) -> Result<std::result::Result<(), String>> {
        if self.workdir.join(".land-checks").exists() {
            return Ok(Err(
                "CHECK FAILED on the result, nothing pushed: .land-checks\nthe repo has a .land-checks file, which builds land no longer reads; \
                 declare each of its commands as a `whole` line in .checks and delete the file"
                    .to_string(),
            ));
        }
        let failed = match crate::checks_run::run_whole(&self.workdir, out) {
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

    /// In a repo of a project, the app build that the result <landed>, which
    /// land built on origin/main at <before>, needs for <member>'s commits
    /// before its push, when it changes what a lane's apps compile
    /// (app_landing.rs): None when passing app builds cover it, else the
    /// arguments after `builds app-build` of the build of the result with the
    /// steps, test classes and shots of the branch's last app build, since
    /// another landing that changed the apps reached main after that build.
    /// <with_own> says the result holds other landings' commits too, so the
    /// need is that of <member>'s own commits. When the branch has no app
    /// build of its own, the text land says of it.
    fn app_build_needed(
        &self,
        member: &Entry,
        (before, landed): (&str, &str),
        with_own: bool,
        out: &mut dyn Write,
    ) -> std::result::Result<Option<Vec<String>>, String> {
        if self.recording.is_none() {
            return Ok(None);
        }
        let workdir = member.worktree.clone();
        let branch = member.shas.last().expect("a landing names a commit");
        let mut command = Command::new(self.program);
        command
            .args([
                "app-build",
                crate::app_landing::FLAG,
                &workdir,
                before,
                landed,
                branch,
                &member.rev,
            ])
            .stdin(Stdio::null());
        if with_own {
            command.args(&member.shas);
        }
        let uncovered =
            |said: String| format!("APP BUILDS DO NOT COVER the result, nothing pushed:\n{said}");
        let output = command.output().map_err(|error| {
            uncovered(format!(
                "land could not run builds app-build {}: {error}",
                crate::app_landing::FLAG
            ))
        })?;
        let said = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let notes = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            let both = [notes, said].join("\n");
            return Err(uncovered(both.trim().to_string()));
        }
        let (notes, said) = crate::app_landing::split_output(&said);
        for line in notes {
            let _ = writeln!(out, "{line}");
        }
        if said.is_empty() {
            return Ok(None);
        }
        let words: Vec<&str> = said.split_whitespace().collect();
        let ["builds", "app-build", args @ ..] = words.as_slice() else {
            return Err(uncovered(format!(
                "builds app-build {} printed {said:?}, not the builds command of the build the result needs",
                crate::app_landing::FLAG
            )));
        };
        let _ = writeln!(
            out,
            "another landing that changed the apps reached main after the branch's last app build, so land builds the result {} with that build's steps, test classes and shots before it pushes:\n{said}",
            &landed[..landed.len().min(12)]
        );
        Ok(Some(args.iter().map(|arg| arg.to_string()).collect()))
    }

    /// Runs `builds app-build <args>` in this worktree, the build of the
    /// result, and copies its output to <out> as it prints it. Land ends its
    /// use of the stores (<stores>) for the build, which takes many minutes.
    /// When the build fails, the end of its output.
    fn build_result(
        &self,
        args: &[String],
        stores: &mut StoreUse,
        out: &mut dyn Write,
    ) -> std::result::Result<(), String> {
        stores.end();
        let mut build = Command::new(self.program);
        build.arg("app-build").args(args).current_dir(&self.workdir);
        let mut tee = Tee {
            out,
            kept: Vec::new(),
        };
        if streamed(build, &mut tee) {
            return Ok(());
        }
        Err(tail(&tee.kept))
    }

    /// In a repo of a project, waits for the turn of the landings that change
    /// the apps of the landing's home lane, the first lane its commits change
    /// in the order of `Lane::ALL`, after it has registered in that lane's
    /// queue (land_batch.rs). When the leader of an earlier batch settled
    /// this landing, returns its verdict. Otherwise the landing leads the
    /// batch of every landing that waits in the queue, and takes the turn of
    /// each other lane that one of them changes, always in the order of
    /// `Lane::ALL`, so two leaders never wait for each other. So no other
    /// landing that changes a lane's apps reaches main between the batch's
    /// fetch and its push, and the batch builds its result in each lane at
    /// most once. The landing ends its use of the stores (<stores>) while it
    /// waits, which may take as long as the app builds of the batches ahead
    /// of it.
    fn take_app_turn(&self, stores: &mut StoreUse, out: &mut dyn Write) -> Result<Turn> {
        if self.recording.is_none() {
            return Ok(Turn::Alone);
        }
        let lanes = match crate::app_landing::lanes_changed(&self.workdir, &self.request.shas) {
            Ok(lanes) => lanes,
            Err(error) => return Err(refuse(out, format!("refusing: {}", error.text()))),
        };
        let Some(&home) = lanes.first() else {
            return Ok(Turn::Alone);
        };
        let dir = &self.workdir;
        let Some(common) = git_text(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ) else {
            return Err(refuse(
                out,
                format!(
                    "refusing: git could not name the git folder of {}",
                    dir.display()
                ),
            ));
        };
        let Some(checkout) = crate::checkout::Checkout::at(dir)? else {
            fail!("git finds no checkout at {}", dir.display());
        };
        stores.end();
        let queue = land_batch::queue_dir(Path::new(&common), home);
        let registered = land_batch::register(&queue, &self.entry(lanes.clone()))?;
        let me = turn_record(
            std::process::id(),
            &format!("{} from {}", shell_words(&self.request.shas), dir.display()),
        );
        let _timing = self.timing(StepKind::AppWait);
        let mut held: Vec<(Lane, std::fs::File)> = Vec::new();
        let mut take = |lane: Lane, held: &mut Vec<(Lane, std::fs::File)>| -> Result<()> {
            held.push((
                lane,
                take_turn(
                    &PathBuf::from(&common).join(app_turn_file(lane)),
                    &format!(
                        "the landing that changes the {} apps of {} ahead of this one",
                        lane.word(),
                        checkout.repo()
                    ),
                    &me,
                    out,
                )?,
            ));
            Ok(())
        };
        take(home, &mut held)?;
        if let Some(verdict) = registered.verdict()? {
            return Ok(Turn::Settled(verdict));
        }
        let waiting = loop {
            let waiting = land_batch::waiting(&queue)?;
            let wanted: Vec<Lane> = Lane::ALL
                .into_iter()
                .filter(|lane| waiting.iter().any(|(_, entry)| entry.lanes.contains(lane)))
                .collect();
            let missing: Vec<Lane> = wanted
                .into_iter()
                .filter(|lane| !held.iter().any(|(own, _)| own == lane))
                .collect();
            if missing.is_empty() {
                break waiting;
            }
            for lane in missing {
                take(lane, &mut held)?;
            }
        };
        let others = waiting
            .into_iter()
            .filter(|(id, _)| *id != registered.id)
            .collect();
        Ok(Turn::Lead(Batch {
            queue,
            others,
            _turns: held.into_iter().map(|(_, file)| file).collect(),
            _registered: registered,
        }))
    }

    /// What the landing tells the leader of a batch, for the lanes <lanes>.
    fn entry(&self, lanes: Vec<Lane>) -> Entry {
        Entry {
            worktree: self.workdir.display().to_string(),
            shas: self.request.shas.clone(),
            rev: self.start.rev().to_string(),
            start: self.start.name(),
            lanes,
        }
    }

    /// Ends the landing with <verdict>, which is its own: it prints what the
    /// verdict says. <shown> says the leader's output already shows what the
    /// failing build printed.
    fn end_with(verdict: Verdict, shown: bool, out: &mut dyn Write) -> Result<Schema<Vec<String>>> {
        match verdict {
            Verdict::Landed { on_main, said } => {
                if !said.is_empty() {
                    let _ = writeln!(out, "{said}");
                }
                Ok(Schema::Kept(on_main))
            }
            Verdict::Refused { output, said } => {
                if !shown && !output.is_empty() {
                    let _ = writeln!(out, "{output}");
                }
                Err(refuse(out, said))
            }
        }
    }

    /// Puts the commits on origin/main and returns their copies there, or
    /// refuses after it checks out what the worktree had when the landing
    /// began. A landing that changes the apps takes its turn among them
    /// (`take_app_turn`) from before its first fetch, and either ends with
    /// the verdict of the batch that took the turn first or leads the batch
    /// of every landing that waited with it (`lead`). When an install moved
    /// the stores while the landing held no use of them, land pushes nothing
    /// and checks out what the worktree had.
    pub(super) fn put_on_main(
        &self,
        stores: &mut StoreUse,
        out: &mut dyn Write,
    ) -> Result<Schema<Vec<String>>> {
        let batch = match self.take_app_turn(stores, out)? {
            Turn::Settled(verdict) => {
                if let Schema::Moved = stores.take_again()? {
                    return Ok(Schema::Moved);
                }
                return Self::end_with(verdict, false, out);
            }
            Turn::Alone => None,
            Turn::Lead(batch) => Some(batch),
        };
        if let Some(batch) = &batch
            && !batch.others.is_empty()
        {
            let _ = writeln!(
                out,
                "this landing leads a batch of {} landings that waited for the turn, with {}",
                batch.others.len() + 1,
                batch
                    .others
                    .iter()
                    .map(|(_, entry)| entry.worktree.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        match self.lead(batch.as_ref(), stores, out)? {
            Schema::Moved => Ok(Schema::Moved),
            Schema::Kept(verdict) => Self::end_with(verdict, true, out),
        }
    }

    /// Builds this landing's commits and those of the landings of <batch>,
    /// which it leads, on origin/main and pushes those whose result passes,
    /// and returns this landing's own verdict. Each other landing finds its
    /// verdict in the batch's queue. The landing checks out what the
    /// worktree had when it began if its own commits did not land.
    fn lead(
        &self,
        batch: Option<&Batch>,
        stores: &mut StoreUse,
        out: &mut dyn Write,
    ) -> Result<Schema<Verdict>> {
        let own = self.entry(Vec::new());
        let mut settled = Settled::new(&own, batch);
        let all: Vec<usize> = (0..settled.verdicts.len()).collect();
        if let Schema::Moved = self.settle(&all, &mut settled, stores, out)? {
            self.go_back(out);
            let _ = writeln!(
                out,
                "builds install moved the stores while this landing waited for its app build; nothing pushed"
            );
            return Ok(Schema::Moved);
        }
        let Some(verdict) = settled.verdicts[0].take() else {
            fail!("the leader of a batch settled every landing but its own");
        };
        if matches!(verdict, Verdict::Refused { .. }) {
            self.go_back(out);
        }
        Ok(Schema::Kept(verdict))
    }

    /// Settles the landings <indices> of <settled>'s members: fetches
    /// origin/main, picks their commits onto it, checks and builds the
    /// result once and pushes it, and gives each landing its verdict. A
    /// result that fails with several landings in it is settled again in two
    /// halves. A refused push starts again from a fresh fetch.
    fn settle(
        &self,
        indices: &[usize],
        settled: &mut Settled,
        stores: &mut StoreUse,
        out: &mut dyn Write,
    ) -> Result<Schema<()>> {
        let dir = &self.workdir;
        let mut left = indices.to_vec();
        let mut tries = 0;
        while tries < ATTEMPTS && !left.is_empty() {
            let fetched = {
                let _timing = self.timing(StepKind::Fetch);
                git_ok(dir, &["fetch", "-q", "origin"])
            };
            if !fetched {
                let _ = writeln!(out, "fetch failed");
                tries += 1;
                continue;
            }
            let picking = self.timing(StepKind::Pick);
            git_ok(dir, &["checkout", "-q", "--detach", "origin/main"]);
            let before = git_text(dir, &["rev-parse", "origin/main"]).unwrap_or_default();
            let mut picked: Vec<(usize, Vec<String>)> = Vec::new();
            for &index in &left {
                match self.pick(settled.entry(index))? {
                    Picked::Applied(on_main) => picked.push((index, on_main)),
                    Picked::Ended(verdict) => settled.deliver(index, verdict)?,
                }
            }
            left = picked.iter().map(|(index, _)| *index).collect();
            if picked.is_empty() {
                return Ok(Schema::Kept(()));
            }
            let landed = git_text(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            drop(picking);
            let with_own = picked.len() > 1;
            let mut builds: Vec<Vec<String>> = Vec::new();
            let mut uncovered: Vec<(usize, String)> = Vec::new();
            {
                let _timing = self.timing(StepKind::Checks);
                if let Err(said) = self.check_result(out)? {
                    return self.fail(&picked, &Failure::Checks(said), settled, stores, out);
                }
                for (index, _) in &picked {
                    let member = settled.entry(*index);
                    match self.app_build_needed(member, (&before, &landed), with_own, out) {
                        Ok(Some(args)) => builds.push(args),
                        Ok(None) => {}
                        Err(said) => uncovered.push((*index, said)),
                    }
                }
            }
            if !uncovered.is_empty() {
                // The result held commits of a landing that no build covers:
                // pick again without it.
                for (index, said) in uncovered {
                    settled.deliver(
                        index,
                        Verdict::Refused {
                            output: String::new(),
                            said,
                        },
                    )?;
                    left.retain(|other| *other != index);
                }
                continue;
            }
            if !builds.is_empty() {
                let args = if builds.len() == 1 {
                    builds.remove(0)
                } else {
                    let merged = crate::app_landing::merge_builds(&builds)?;
                    let _ = writeln!(
                        out,
                        "{} landings of the batch need a build of the result, so land builds it once with all their steps, test classes and shots:\nbuilds app-build {}",
                        builds.len(),
                        merged.join(" ")
                    );
                    merged
                };
                let _timing = self.timing(StepKind::AppBuild);
                if let Err(output) = self.build_result(&args, stores, out) {
                    return self.fail(&picked, &Failure::Build(output), settled, stores, out);
                }
            }
            if let Schema::Moved = stores.take_again()? {
                return Ok(Schema::Moved);
            }
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
            let pushing = self.timing(StepKind::Push);
            let mut push = git(dir);
            push.args(["push", "-q", "origin", "HEAD:main"]);
            let (pushed, said) = merged(push);
            if pushed {
                git_ok(dir, &["fetch", "-q", "origin"]);
                for (index, on_main) in picked {
                    let shown: Vec<String> = on_main
                        .iter()
                        .map(|sha| {
                            git_text(dir, &["log", "--oneline", "-1", sha]).unwrap_or_default()
                        })
                        .collect();
                    settled.deliver(
                        index,
                        Verdict::Landed {
                            on_main,
                            said: shown.join("\n"),
                        },
                    )?;
                }
                drop(pushing);
                return Ok(Schema::Kept(()));
            }
            drop(pushing);
            let last_line = said
                .lines()
                .rfind(|line| !line.starts_with("hint"))
                .unwrap_or("");
            let _ = writeln!(out, "push failed: {last_line}");
            tries += 1;
        }
        for index in left {
            settled.deliver(
                index,
                Verdict::Refused {
                    output: String::new(),
                    said: format!("LANDING FAILED: no push succeeded in {ATTEMPTS} tries"),
                },
            )?;
        }
        Ok(Schema::Kept(()))
    }

    /// Ends the round of <picked>, whose result failed for <failure>: a
    /// landing that is alone in it is refused with the failure, and several
    /// are settled again in two halves, each on a fresh origin/main, so the
    /// landings that pass land and the one that fails is found.
    fn fail(
        &self,
        picked: &[(usize, Vec<String>)],
        failure: &Failure,
        settled: &mut Settled,
        stores: &mut StoreUse,
        out: &mut dyn Write,
    ) -> Result<Schema<()>> {
        if let [(index, _)] = picked {
            let verdict = failure.verdict_for(settled.entry(*index));
            settled.deliver(*index, verdict)?;
            return Ok(Schema::Kept(()));
        }
        let _ = writeln!(
            out,
            "the result of {} landings failed, so land builds each half of them to find the landing that fails",
            picked.len()
        );
        let (first, second) = picked.split_at(picked.len() / 2);
        for half in [first, second] {
            let indices: Vec<usize> = half.iter().map(|(index, _)| *index).collect();
            if let Schema::Moved = self.settle(&indices, settled, stores, out)? {
                return Ok(Schema::Moved);
            }
        }
        Ok(Schema::Kept(()))
    }

    /// Picks <member>'s commits onto the worktree's HEAD, the result so far:
    /// a fast-forward when they sit on it, else a cherry-pick. A landing
    /// whose commits conflict, or are already on origin/main, ends there.
    fn pick(&self, member: &Entry) -> Result<Picked> {
        let dir = &self.workdir;
        let shas = &member.shas;
        let last = shas.last().expect("a landing names a commit").as_str();
        let base = git_must(dir, &["rev-parse", "HEAD"])?;
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
                let marks = git_must(dir, &["cherry", "origin/main", sha, &parent])?;
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
                    return Ok(Picked::Ended(Verdict::Refused {
                        output: String::new(),
                        said: "LANDED, NOT RECORDED: a commit's copy on origin/main was not found by its subject; record by hand with builds item landed".to_string(),
                    }));
                }
                return Ok(Picked::Ended(Verdict::Landed {
                    on_main,
                    said: format!("already on origin/main:\n{shown}"),
                }));
            }
            let mut pick = git(dir);
            pick.arg("cherry-pick").args(todo);
            let (picked, said) = merged(pick);
            if !picked {
                let paths = git_text(dir, &["diff", "--name-only", "--diff-filter=U"]);
                git_ok(dir, &["cherry-pick", "--abort"]);
                return Ok(Picked::Ended(Verdict::Refused {
                    output: String::new(),
                    said: Self::pick_refusal(member, paths.as_deref(), &said),
                }));
            }
        }
        let on_main = git_must(dir, &["rev-list", "--reverse", &format!("{base}..HEAD")])?
            .lines()
            .map(str::to_string)
            .collect();
        Ok(Picked::Applied(on_main))
    }
}
