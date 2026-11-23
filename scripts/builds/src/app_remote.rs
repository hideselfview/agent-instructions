//! App checkouts on a host. A lane's app checkout can live on another Mac:
//! `builds app-checkout android --host m1 <path>` records it as the entry
//! `m1:<name>` in the lane's set, beside the entries of the checkouts on this
//! Mac. The lock, the queue and the record of what the checkout last built
//! stay on this Mac, under the entry's name, so the queue, takeovers, the
//! dashboard and `builds health` treat a checkout on a host as any other.
//!
//! The checkout on the host is the worktree `~/dev/<repo>-worktrees/<name>` of
//! the host's clone of the repo, set up as a side worktree is here: made with
//! `git worktree add`, then the host's `builds setup` and the repo's
//! `scripts/hooks/post-checkout`, with the host's builds root
//! (`/Volumes/Builds` when the host has it, else `~/builds-root.noindex`) so
//! the build folders live under `<root>/<repo>/<name>`, and, in the android
//! lane, a `local.properties` pointing each Gradle project at the host's
//! Android SDK.
//! A build sends its commit to the host's clone as a job ref
//! (`refs/builds/jobs/<id>`, never to GitHub), switches the checkout to it
//! (the checkout must have no changes, as here) and deletes the ref; the
//! checkout's detached HEAD keeps the commit.
//!
//! Each step runs there through `builds remote step`, a process of this
//! Mac that `lock::run_in_group_within` times and stops like a local step: its
//! output comes back as the step prints it, and a time limit or a takeover
//! stops the step on the host.
//!
//! The repo's `results <lane> <path>` lines in `.app-build` name the folders
//! of the checkout that agents read after a run of tests or screenshots; they
//! are copied back to `<root>/<repo>/<entry>/<path>` on this Mac (the folder
//! of the checkout's build output, where a local checkout's `<path>` links to).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::app_config::{app_checkout_top, run_git};
use crate::app_lock_guard::CheckoutLock;
use crate::awake::KeepAwake;
use crate::checkout::{APP_LANE_FILE, Checkout};
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{self, Host, Ssh, clone_path, sh_quote};
use crate::lane::Lane;
use crate::places::Places;
use crate::remote::{
    JOB_WORKTREE_PREFIX, JOBS_REF, Transport, job_id, release_script, root_script, worktree_in,
};
use crate::text::note;

/// What separates the host from the checkout's name in a lane's entry.
const SEPARATOR: char = ':';

/// How long after its last change a step's picture folder on a host counts
/// as left behind by a step that died: no step runs that long.
const CRASHED_MINUTES: u32 = 24 * 60;

/// The folder under the host's home where it keeps the board pictures it
/// was sent.
const HOST_PICTURES: &str = ".cache/builds-pictures";

/// The host and the name in the entry <entry>, when it is a checkout on a
/// host rather than one of this Mac's side worktrees.
pub fn split(entry: &str) -> Option<(&str, &str)> {
    entry.split_once(SEPARATOR)
}

/// The host the app checkout <entry> is on: the host before the colon, or
/// this Mac.
pub fn host_of(entry: &str) -> &str {
    split(entry).map_or(host::LOCAL, |(host, _)| host)
}

/// The entry for the checkout <name> on <host>.
pub fn entry_of(host: &str, name: &str) -> String {
    format!("{host}{SEPARATOR}{name}")
}

/// The host and the name that <path> (a folder on the host, as `builds
/// app-checkout --host` is given it) names in <repo>'s worktrees folder there:
/// `.../dev/<repo>-worktrees/<name>`.
pub fn name_in(path: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let [.., dev, folder, name] = parts[..] else {
        fail!(
            "{path:?} is not a folder of a host's app checkout; it is ~/dev/<repo>-worktrees/<name> there"
        );
    };
    let Some(repo) = folder
        .strip_suffix("-worktrees")
        .filter(|repo| !repo.is_empty())
    else {
        fail!(
            "{path:?} is not in a <repo>-worktrees folder; a host's app checkout is ~/dev/<repo>-worktrees/<name>"
        );
    };
    if dev != "dev" {
        fail!("{path:?} is not under dev/; a host's app checkout is ~/dev/<repo>-worktrees/<name>");
    }
    if name.starts_with(JOB_WORKTREE_PREFIX) {
        fail!(
            "{name} is a name `builds remote run` gives its job worktrees; give the app checkout another"
        );
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        fail!("{name:?} must be letters, digits, dashes and underscores");
    }
    Ok((repo.to_string(), name.to_string()))
}

/// Where a build reads its commits from on this Mac: the checkout it runs in
/// when that is a checkout of <repo>, else the first of <repo>'s app
/// checkouts here, else the clone `~/dev/<repo>`.
pub fn local_source(places: &Places, repo: &str) -> Result<PathBuf> {
    let here = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    if let Some(checkout) = Checkout::at(&here)?
        && checkout.repo() == repo
    {
        return Ok(checkout.top);
    }
    for lane in Lane::ALL {
        for entry in places.app_checkouts_of(repo, lane)? {
            if split(&entry).is_none() {
                return app_checkout_top(places, repo, &entry);
            }
        }
    }
    let clone = places.home.join(clone_path(repo));
    if !clone.join(".git").exists() {
        fail!(
            "{repo} has no app checkout here and no clone at {} to read commits from",
            clone.display()
        );
    }
    Ok(clone)
}

/// `local_source`, after a fetch from origin there, so that it holds the
/// commits origin has now.
pub fn fetched_local_source(places: &Places, repo: &str) -> Result<PathBuf> {
    let source = local_source(places, repo)?;
    run_git(&source, &["fetch", "-q", "origin"])?;
    Ok(source)
}

/// <command> as one line for `sh`, each word quoted.
pub fn shell_line(command: &[String]) -> String {
    command
        .iter()
        .map(|word| sh_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// <command>, run under `caffeinate -d -u` in the mac lane, which holds a
/// display assertion on the host for as long as the command runs, so the
/// host's display neither sleeps nor locks during a step that opens windows
/// (host_screen_lock.rs). Another lane's <command> stays as it is.
pub fn keeping_display_on(lane: Lane, command: &[String]) -> Vec<String> {
    if !lane.uses_xcode() {
        return command.to_vec();
    }
    ["/usr/bin/caffeinate", "-d", "-u"]
        .iter()
        .map(|word| word.to_string())
        .chain(command.iter().cloned())
        .collect()
}

/// The commands that make, switch and read the checkout `<name>` of <repo> on
/// a host. While a `RemoteApp` exists, this Mac stays awake (awake.rs): its
/// steps and copies run over ssh, and a sleep here ends their connections.
#[derive(Debug)]
pub struct RemoteApp<'a> {
    places: Places,
    ssh: Ssh,
    pub host: Host,
    repo: String,
    name: String,
    lane: Lane,
    /// The checkout's folder on the host, once `switch_to` or `locate` has
    /// asked.
    worktree: Option<String>,
    checkout: Option<CheckoutLock<'a>>,
    _awake: KeepAwake,
}

impl<'a> RemoteApp<'a> {
    /// The checkout <entry> of <repo>'s <lane>, when it is on a host.
    pub fn find(
        places: &Places,
        ssh: &Ssh,
        repo: &str,
        lane: Lane,
        entry: &str,
        checkout: Option<CheckoutLock<'a>>,
    ) -> Result<Option<Self>> {
        let Some((host_name, name)) = split(entry) else {
            return Ok(None);
        };
        let host = host::remote_named(places, entry, host_name)?;
        let awake = KeepAwake::for_host(&host, &format!("{repo}'s app checkout {name}"))?;
        Ok(Some(RemoteApp {
            places: places.clone(),
            ssh: ssh.clone(),
            host,
            repo: repo.to_string(),
            name: name.to_string(),
            lane,
            worktree: None,
            checkout,
            _awake: awake,
        }))
    }

    fn transport(&self) -> Transport<'_> {
        Transport {
            places: &self.places,
            ssh: &self.ssh,
            host: &self.host,
        }
    }

    fn capture(&self, script: &str, what: &str) -> Result<String> {
        let transport = self.transport();
        self.run(transport.command(script), |command| {
            transport.capture_command(command, what)
        })
    }

    fn run<T>(&self, command: Command, run: impl FnOnce(Command) -> Result<T>) -> Result<T> {
        match self.checkout {
            Some(checkout) => checkout.run(command, run),
            None => run(command),
        }
    }

    fn unpack(&self, script: &str, into: &Path, what: &str) -> Result<()> {
        self.run(self.transport().command(script), |command| {
            crate::remote::unpack(command, into, what)
        })
    }

    /// The folder on this Mac where the checkout's lock, its record of what it
    /// built and the results copied back live.
    pub fn local_folder(&self) -> PathBuf {
        self.places
            .root
            .join(&self.repo)
            .join(entry_of(&self.host.name, &self.name))
    }

    /// The checkout's folder on the host; it need not exist yet.
    pub fn locate(&mut self) -> Result<String> {
        if let Some(worktree) = &self.worktree {
            return Ok(worktree.clone());
        }
        let said = self.capture(
            &format!(
                "echo \"worktree=$HOME/dev/{}-worktrees/{}\"",
                self.repo, self.name
            ),
            "finding the app checkout",
        )?;
        let worktree = worktree_in(&said)?;
        self.worktree = Some(worktree.clone());
        Ok(worktree)
    }

    /// The commit the checkout is on, or "" when the host has none yet.
    pub fn head(&mut self) -> Result<String> {
        let worktree = self.locate()?;
        let said = self.capture(
            &format!(
                "git -C {} rev-parse HEAD 2>/dev/null || true",
                sh_quote(&worktree)
            ),
            "reading the app checkout's commit",
        )?;
        Ok(said.trim().to_string())
    }

    /// Sends <sha> from <source> to the host's clone, makes the checkout there
    /// when it has none (set up as a side worktree is) and switches it to
    /// <sha>. Fails, naming the host, when the checkout has changes.
    pub fn switch_to(&mut self, source: &Path, sha: &str) -> Result<()> {
        let id = self.send(source, sha)?;
        let made = self.capture(
            &ensure_script(&self.host, &self.repo, &self.name, sha, self.lane),
            "switching the app checkout",
        );
        let dropped = self.unsend(&id);
        let made = crate::fail::with_cleanup(made, dropped)?;
        self.worktree = Some(worktree_in(&made)?);
        if made.lines().any(|line| line == NEW_CHECKOUT) {
            self.start_from_warmest()?;
        }
        Ok(())
    }

    /// Fills the build folders of the checkout that `ensure_script` just made
    /// from the lane's warmest checkout on this host (`builds app-clone`, run
    /// there), while this build holds that checkout's lock. A host with no
    /// such checkout, or none that came free in time, leaves the first build
    /// cold. When the clone fails, the new checkout is removed, so the next
    /// build makes it and clones again rather than finding it half filled.
    fn start_from_warmest(&mut self) -> Result<()> {
        let at = crate::lane_size::RepoLane {
            places: &self.places,
            ssh: &self.ssh,
            repo: &self.repo,
            lane: self.lane,
        };
        let me = entry_of(&self.host.name, &self.name);
        let Some(taken) = crate::app_clone::take_source(at, &self.host.name, &me)? else {
            note(format!(
                "{} has no warm {} checkout to clone into {me}; its first build is cold",
                self.host.name,
                self.lane.word()
            ));
            return Ok(());
        };
        let source_name = split(&taken.entry).map_or(taken.entry.as_str(), |(_, name)| name);
        let script = CLONE
            .replace("@ROOT@", &root_script(&self.host))
            .replace("@REPO@", &self.repo)
            .replace("@SOURCE@", source_name)
            .replace("@NAME@", &self.name)
            .replace("@LANE@", self.lane.word());
        let cloned = self
            .capture(&script, "cloning the warmest app checkout")
            .map(|_| ());
        let removed = match &cloned {
            Ok(()) => Ok(()),
            Err(_) => self.remove().map_err(|said| {
                crate::fail::Fail::Message(format!(
                    "removing {me} on {} after its clone failed: {said}",
                    self.host.name
                ))
            }),
        };
        crate::fail::with_cleanup(cloned, removed)
    }

    /// Runs git with <args> in the checkout on the host, with the index file
    /// <index> when given, and returns what it printed; a failure names the
    /// host and what git said.
    pub fn git(&mut self, index: Option<&str>, args: &[&str]) -> Result<String> {
        let index = index
            .map(|index| format!("GIT_INDEX_FILE={} ", sh_quote(index)))
            .unwrap_or_default();
        let quoted: Vec<String> = args.iter().map(|arg| sh_quote(arg)).collect();
        self.run_in(
            &format!("{index}git {}", quoted.join(" ")),
            &format!("git {}", args.join(" ")),
        )
    }

    /// Runs <script> with `sh` in the checkout's folder on the host and
    /// returns what it printed; a failure names the host and <what>.
    pub fn run_in(&mut self, script: &str, what: &str) -> Result<String> {
        let worktree = self.locate()?;
        self.capture(&format!("cd {} && {script}", sh_quote(&worktree)), what)
    }

    /// Runs <script> with `sh` on the host, in no checkout, and returns what
    /// it printed; a failure names the host and <what>.
    pub fn on_host(&self, script: &str, what: &str) -> Result<String> {
        self.capture(script, what)
    }

    /// Sends <sha> from <source> to the host's clone as the ref of a new job,
    /// so that git there can read it, and returns the job's id; `unsend`
    /// deletes the ref.
    pub fn send(&self, source: &Path, sha: &str) -> Result<String> {
        let id = job_id();
        self.transport().send(source, &self.repo, sha, &id)?;
        Ok(id)
    }

    /// Deletes the ref that `send` made for job <id>.
    pub fn unsend(&self, id: &str) -> Result<()> {
        self.capture(
            &release_script(&self.repo, None, id),
            "removing the job's ref",
        )
        .map(|_| ())
    }

    /// Removes the checkout from the host, with its build folders there, so
    /// the host gets back the disk space they used. When the host has no such
    /// checkout, `remove` succeeds and does nothing. The caller holds the
    /// checkout's lock, so no build runs in it. A failure names the host and
    /// what the removal printed there.
    pub fn remove(&self) -> std::result::Result<(), String> {
        let script = REMOVE
            .replace("@ROOT@", &root_script(&self.host))
            .replace("@CLONE@", &clone_path(&self.repo))
            .replace("@REPO@", &self.repo)
            .replace("@NAME@", &self.name);
        self.capture(&script, "removing the app checkout")
            .map(|_| ())
            .map_err(|error| error.text())
    }

    /// Writes the lane to the checkout's lane file on the host
    /// (`Checkout::app_lane_file`), so the host's builds keeps the checkout's
    /// builds warm, as a person adds it to the lane. When the host has no such
    /// worktree yet, `mark` does nothing: the first build makes the checkout
    /// and writes the file before the host's setup runs. Returns whether it
    /// wrote the file. A failure, such as a host that does not answer, names
    /// the host and what the script printed there.
    pub fn mark(&self) -> Result<bool> {
        marked_in(&self.lane_file_script(MARK, "writing the app checkout's lane file")?)
    }

    /// Deletes the checkout's lane file on the host, so the host's builds
    /// stops keeping the checkout's builds warm once it is no lane's. The
    /// worktree stays. When the host has no such worktree, `unmark` does
    /// nothing. A failure names the host, as `mark`'s does.
    pub fn unmark(&self) -> Result<()> {
        self.lane_file_script(UNMARK, "deleting the app checkout's lane file")
            .map(|_| ())
    }

    /// Runs <script> on the host and returns what it printed.
    fn lane_file_script(&self, script: &str, what: &str) -> Result<String> {
        let script = script
            .replace("@MARK_LANE@", MARK_LANE)
            .replace("@REPO@", &self.repo)
            .replace("@NAME@", &self.name)
            .replace("@LANE@", self.lane.word())
            .replace("@LANE_FILE@", APP_LANE_FILE);
        self.capture(&script, what)
    }

    /// <command> for a step that opens windows, as `keeping_display_on`
    /// makes it for this checkout's lane.
    pub fn keeping_display_on(&self, command: &[String]) -> Vec<String> {
        keeping_display_on(self.lane, command)
    }

    /// The command that runs <command> in the checkout on the host and is
    /// stopped as a step here is: a `builds remote step` process.
    pub fn step_command(&mut self, command: &[String]) -> Result<Command> {
        let worktree = self.locate()?;
        let exe = crate::this_program()?;
        let mut start = Command::new(exe);
        start
            .args(["remote", "step", &self.host.name, &worktree, "--"])
            .args(command);
        Ok(start)
    }

    /// <command>, run under the build's builds <pinned> on the host:
    /// `<folder>/builds programs hold -- <command>`, where <folder> is the
    /// host's folder of its version (programs.rs). The function copies the
    /// builds of this Mac's folder of the version when the host's folder
    /// lacks it.
    pub fn with_programs(
        &mut self,
        pinned: &crate::programs::Pinned,
        command: &[String],
    ) -> Result<Vec<String>> {
        let (version, local) = (pinned.version(), pinned.dir());
        let said = self.capture(
            &format!(
                r#"dir="$HOME/{folder}/{version}"
mkdir -p "$dir"
if [ -x "$dir/builds" ]; then echo "present=$dir"; else echo "absent=$dir"; fi"#,
                folder = crate::programs::FOLDER,
            ),
            "finding this builds' programs",
        )?;
        let (present, there) = match said.lines().find_map(|line| {
            line.strip_prefix("present=")
                .map(|dir| (true, dir))
                .or_else(|| line.strip_prefix("absent=").map(|dir| (false, dir)))
        }) {
            Some((present, dir)) => (present, dir.to_string()),
            None => fail!(
                "host {} did not say where this builds' programs are: {said}",
                self.host.name
            ),
        };
        if !present {
            let mut cat = Command::new("cat");
            cat.arg(local.join("builds"));
            let into = format!("{there}/builds");
            let staged = format!("{there}/.builds.{}", crate::text::staged_suffix());
            crate::remote::pipe(
                cat,
                self.transport().command(&format!(
                    "cat > {staged} && chmod 755 {staged} && mv -f {staged} {into}",
                    staged = sh_quote(&staged),
                    into = sh_quote(&into),
                )),
                &format!(
                    "{} to host {}",
                    local.join("builds").display(),
                    self.host.name
                ),
            )?;
        }
        let mut held = vec![
            format!("{there}/builds"),
            "programs".to_string(),
            "hold".to_string(),
            "--".to_string(),
        ];
        held.extend(command.iter().cloned());
        Ok(held)
    }

    /// Copies the file <local> here to a new temporary file on the host and
    /// returns its path there; `remove_temporary` deletes it.
    pub fn put_temporary(&mut self, local: &Path) -> Result<String> {
        let made = self.capture(
            "mktemp \"${TMPDIR:-/tmp}/builds-before.XXXXXX\"",
            "making a temporary file",
        )?;
        let there = made.trim().to_string();
        let mut cat = Command::new("cat");
        cat.arg(local);
        crate::remote::pipe(
            cat,
            self.transport()
                .command(&format!("cat > {}", sh_quote(&there))),
            &format!("{} to host {}", local.display(), self.host.name),
        )?;
        Ok(there)
    }

    /// Where the host keeps the board pictures it has been sent: one folder
    /// for each repo, which every step on the host shares. A picture's name
    /// is the hash of its board, so a file in it never changes.
    fn host_boards(&self) -> String {
        format!(
            "$HOME/{HOST_PICTURES}/{}/{}",
            self.repo,
            crate::shots_cache::BOARD_PICTURES
        )
    }

    /// Where the host keeps one folder for each step that uses its board
    /// pictures, for as long as the step runs.
    fn host_steps(&self) -> String {
        format!("$HOME/{HOST_PICTURES}/{}/steps", self.repo)
    }

    /// Makes a temporary picture folder on the host for one step and returns
    /// its path there. The folder's `boards` is a link to the host's shared
    /// board folder, which first gets the pictures of <local> it lacks, and
    /// loses the ones <local> no longer holds. The folder also gets the
    /// folder <tree> of <local> (the cached shots of the lane's app tree)
    /// when <local> has it. Board pictures, which are most of the bytes, so
    /// go over the first time a host needs them only.
    pub fn put_pictures(&mut self, local: &Path, tree: &str) -> Result<String> {
        let started = std::time::Instant::now();
        let boards = self.host_boards();
        let steps = self.host_steps();
        let said = self.capture(
            &format!(
                r#"mkdir -p "{steps}" || exit 1
there=$(mktemp -d "{steps}/step.XXXXXX") || exit 1
boards="{boards}"
mkdir -p "$boards" || exit 1
ln -s "$boards" "$there/boards" || exit 1
echo "there=$there"
ls -1A "$boards" > "$there/.listed" || exit 1
sed 's/^/have=/' "$there/.listed""#
            ),
            "making a temporary folder and listing its pictures",
        )?;
        let mut there = None;
        let mut have = std::collections::BTreeSet::new();
        for line in said.lines() {
            if let Some(path) = line.strip_prefix("there=") {
                there = Some(path.to_string());
            } else if let Some(name) = line.strip_prefix("have=") {
                have.insert(name.to_string());
            }
        }
        let Some(there) = there else {
            fail!(
                "host {} did not say where its temporary folder is: {said}",
                self.host.name
            );
        };
        let ours = picture_names(&local.join(crate::shots_cache::BOARD_PICTURES))?;
        let missing: Vec<&String> = ours.difference(&have).collect();
        let sent = missing.len();
        self.send_boards(local, &boards, &missing)?;
        // Hidden files are pictures a step is still writing. A picture this
        // Mac lacks may be one another running step uses or has made and not
        // yet brought back, so the cleanup runs only when no other step has a
        // folder on the host.
        let gone: Vec<&String> = have
            .iter()
            .filter(|name| !ours.contains(*name) && !name.starts_with('.'))
            .collect();
        self.delete_boards(&boards, &steps, &there, &gone)?;
        let names: Vec<&String> = ours.iter().collect();
        self.send_names(&names, &boards, &format!("{there}/.sent"))?;
        if local.join(tree).is_dir() {
            let mut tar = Command::new("tar");
            tar.arg("-cf").arg("-").arg("-C").arg(local).arg(tree);
            crate::remote::pipe(
                tar,
                self.transport()
                    .command(&format!("tar -xf - -C {}", sh_quote(&there))),
                &format!("{}/{tree} to host {}", local.display(), self.host.name),
            )?;
        }
        note(format!(
            "sent {sent} of {} board pictures to host {} in {}s",
            ours.len(),
            self.host.name,
            started.elapsed().as_secs()
        ));
        Ok(there)
    }

    /// Copies the board pictures <names> of <local> to the host's board
    /// folder <boards>: into a folder of their own first, so that a step on
    /// the host never finds half a picture, with the element lists before the
    /// pictures that need them.
    fn send_boards(&mut self, local: &Path, boards: &str, names: &[&String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let list = ListFile::of(
            &self.places,
            names
                .iter()
                .map(|name| format!("{}/{name}", crate::shots_cache::BOARD_PICTURES)),
        )?;
        let mut tar = Command::new("tar");
        tar.arg("-cf")
            .arg("-")
            .arg("-C")
            .arg(local)
            .arg("-T")
            .arg(list.path());
        let script = format!(
            r#"base=$(dirname "{boards}")
stage=$(mktemp -d "$base/.incoming.XXXXXX") || exit 1
tar -xf - -C "$stage" || {{ rm -rf "$stage"; exit 1; }}
cd "$stage/{folder}" || exit 1
find . -type f -name '*.json' -exec mv -n {{}} "{boards}/" \; || exit 1
find . -type f ! -name '*.json' -exec mv -n {{}} "{boards}/" \; || exit 1
cd / && rm -rf "$stage""#,
            folder = crate::shots_cache::BOARD_PICTURES
        );
        crate::remote::pipe(
            tar,
            self.transport().command(&script),
            &format!("board pictures to host {}", self.host.name),
        )
    }

    /// Deletes the board pictures <names> from the host's board folder
    /// <boards> when no step but this one, whose folder is <there>, has a
    /// folder in <steps> that changed within `CRASHED_MINUTES`: a step that
    /// runs now keeps its pictures. A folder older than that is a step that
    /// died without removing it, which no step waits for.
    fn delete_boards(
        &mut self,
        boards: &str,
        steps: &str,
        there: &str,
        names: &[&String],
    ) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let host = self.host.name.clone();
        let there = sh_quote(there);
        self.send_list(
            names,
            &format!(
                r#"cd "{boards}" || exit 1
for folder in "{steps}"/*; do
  if [ -d "$folder" ] && [ "$folder" != {there} ] && [ -n "$(find "$folder" -maxdepth 0 -mmin -{CRASHED_MINUTES})" ]; then exit 0; fi
done
while IFS= read -r name; do
  rm -f -- "$name" || exit 1
done"#
            ),
            &format!("the list of board pictures to delete on host {host}"),
        )
    }

    /// Writes <names>, one a line, to the file <there> on the host and fails
    /// when one of them is missing from the host's board folder <boards>.
    fn send_names(&mut self, names: &[&String], boards: &str, there: &str) -> Result<()> {
        let host = self.host.name.clone();
        let there = sh_quote(there);
        self.send_list(
            names,
            &format!(
                r#"cat > {there} || exit 1
if [ -s {there} ]; then
  cd "{boards}" || exit 1
  xargs -n 500 ls -d -- < {there} > /dev/null || exit 1
fi"#
            ),
            &format!("the list of board pictures sent to host {host}"),
        )
    }

    /// Sends <names>, one a line, to the standard input of <script> on the
    /// host.
    fn send_list(&mut self, names: &[&String], script: &str, what: &str) -> Result<()> {
        let list = ListFile::of(&self.places, names.iter().map(|name| name.to_string()))?;
        let mut cat = Command::new("cat");
        cat.arg(list.path());
        crate::remote::pipe(cat, self.transport().command(script), what)
    }

    /// Copies what a step made in the picture folder <there> on the host into
    /// the new folder <into> here: the shots and marks of the folder, and the
    /// board pictures the host's board folder holds beyond the ones
    /// `put_pictures` found there, which go to `<into>/boards`.
    pub fn fetch_pictures(&mut self, there: &str, into: &Path) -> Result<()> {
        let started = std::time::Instant::now();
        std::fs::create_dir_all(into).context(|| format!("creating {}", into.display()))?;
        let host = &self.host.name;
        self.unpack(
            &format!(
                "cd {} && tar -cf - --exclude=./boards --exclude=./.sent --exclude=./.new --exclude=./.sent.sorted --exclude=./.listed --exclude=./.have .",
                sh_quote(there)
            ),
            into,
            &format!("{there} from host {host}"),
        )?;
        let boards = into.join(crate::shots_cache::BOARD_PICTURES);
        std::fs::create_dir_all(&boards).context(|| format!("creating {}", boards.display()))?;
        let made = self.capture(
            &format!(
                r#"there={there}
cd "$there" || exit 1
LC_ALL=C sort .sent > .sent.sorted || exit 1
ls -1 boards/ > .listed || exit 1
LC_ALL=C sort .listed > .have || exit 1
LC_ALL=C comm -13 .sent.sorted .have > .new || exit 1
cat .new"#,
                there = sh_quote(there)
            ),
            "listing the board pictures the step made",
        )?;
        // An empty list gives tar nothing to archive, which some tars refuse.
        if !made.trim().is_empty() {
            self.unpack(
                &format!(
                    "cd {boards} && tar -cf - -T {new}",
                    boards = sh_quote(&format!("{there}/boards")),
                    new = sh_quote(&format!("{there}/.new"))
                ),
                &boards,
                &format!("the new board pictures from host {host}"),
            )?;
        }
        note(format!(
            "brought back the pictures of the step from host {host} in {}s",
            started.elapsed().as_secs()
        ));
        Ok(())
    }

    /// Deletes the temporary file or folder <there> that `put_temporary` or
    /// `put_pictures` made on the host.
    pub fn remove_temporary(&mut self, there: &str) -> Result<()> {
        self.capture(
            &format!("rm -rf {}", sh_quote(there)),
            &format!("deleting {there}"),
        )
        .map(|_| ())
    }

    /// Copies the shots report in <dir> (a folder of the checkout on the
    /// host: its `index.html`, the files that page shows and its findings as
    /// records, when it has them) into the new folder <into> here.
    pub fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()> {
        let html = self.capture(
            &format!("cat {}", sh_quote(&format!("{dir}/index.html"))),
            "reading the shots report",
        )?;
        std::fs::create_dir_all(into).context(|| format!("creating {}", into.display()))?;
        let files = crate::shot_findings::report_files(&html);
        self.unpack(
            &crate::shot_findings::report_copy_script(dir, &files),
            into,
            &format!("the shots report from host {}", self.host.name),
        )
    }

    /// Copies each of <paths> (folders of the checkout) from the host to
    /// `local_folder()/<path>`, replacing what is there; a path the checkout
    /// has not made is left out.
    pub fn pull(&mut self, paths: &[String]) -> Result<()> {
        self.pull_with(paths, "-chf")
    }

    /// `pull` for a built app: symbolic links stay links, since a bundle's
    /// frameworks hold some and a copy of what they point at breaks its
    /// signature.
    pub fn pull_bundle(&mut self, path: &str) -> Result<()> {
        self.pull_with(&[path.to_string()], "-cf")
    }

    fn pull_with(&mut self, paths: &[String], tar_flags: &str) -> Result<()> {
        let worktree = self.locate()?;
        let into = self.local_folder();
        std::fs::create_dir_all(&into).context(|| format!("creating {}", into.display()))?;
        for path in paths {
            let target = into.join(path);
            if target.is_dir() {
                std::fs::remove_dir_all(&target)
                    .context(|| format!("removing {}", target.display()))?;
            }
            let script = format!(
                "cd {} && if [ -e {path} ]; then tar {tar_flags} - {path}; fi",
                sh_quote(&worktree),
                path = sh_quote(path)
            );
            self.unpack(
                &script,
                &into,
                &format!("{path} from host {}", self.host.name),
            )?;
        }
        Ok(())
    }
}

/// The names of the files that are not hidden in the folder <folder>, none
/// when it does not exist.
fn picture_names(folder: &Path) -> Result<std::collections::BTreeSet<String>> {
    let mut names = std::collections::BTreeSet::new();
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if crate::fail::gone(&error) => return Ok(names),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && entry.path().is_file() {
            names.insert(name);
        }
    }
    Ok(names)
}

/// A temporary file of lines in builds' scratch folder, deleted when it
/// drops.
struct ListFile(PathBuf);

impl ListFile {
    fn of(places: &Places, lines: impl Iterator<Item = String>) -> Result<ListFile> {
        let folder = places.root.join(".lists");
        std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
        let path = folder.join(crate::text::staged_suffix());
        let mut text = String::new();
        for line in lines {
            text.push_str(&line);
            text.push('\n');
        }
        std::fs::write(&path, text).context(|| format!("writing {}", path.display()))?;
        Ok(ListFile(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ListFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            note(format!("could not delete {}: {error}", self.0.display()));
        }
    }
}

/// The script that makes the checkout <name> of <repo> on the host when it
/// has none, set up as a side worktree is, or switches it to <sha> when it
/// has no changes; it prints `worktree=<folder>`. A new checkout's setup
/// writes <lane> to the checkout's lane file (`Checkout::app_lane_file`),
/// which the host's builds reads to keep the checkout's builds warm (a
/// checkout the host already had got the file when a person added it to the
/// lane, `RemoteApp::mark`), then runs the host's `builds setup`, which links
/// the build folders into the host's builds root, and then the repo's
/// post-checkout, as a job's worktree gets
/// (remote.rs `claim_script`): a repo with no post-checkout would otherwise
/// build into real folders in the checkout, outside the build volume and
/// every cap. A checkout whose setup fails is removed, so the next build sets
/// it up again rather than finding half of it.
fn ensure_script(host: &Host, repo: &str, name: &str, sha: &str, lane: Lane) -> String {
    let android = match lane {
        Lane::Android => ANDROID_SETUP,
        Lane::Mac => "",
    };
    ENSURE
        .replace("@MARK_LANE@", MARK_LANE)
        .replace("@ROOT@", &root_script(host))
        .replace("@CLONE@", &clone_path(repo))
        .replace("@REPO@", repo)
        .replace("@NAME@", name)
        .replace("@SHA@", &sh_quote(sha))
        .replace("@JOBS@", JOBS_REF)
        .replace("@ANDROID@", android)
        .replace("@LANE@", lane.word())
        .replace("@LANE_FILE@", APP_LANE_FILE)
        .replace("@NEW_CHECKOUT@", NEW_CHECKOUT)
}

/// What a lane's checkout needs besides the repo's post-checkout: in the
/// android lane, Gradle's `local.properties` naming the host's SDK in each
/// Gradle project (a folder with a `gradlew`, at the top or one folder down).
const ANDROID_SETUP: &str = r#"  sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
  if [ ! -d "$sdk" ]; then echo "no Android SDK at $sdk"; return 1; fi
  for gradlew in gradlew */gradlew; do
    [ -f "$gradlew" ] || continue
    dir=$(dirname "$gradlew")
    if [ ! -e "$dir/local.properties" ]; then
      echo "sdk.dir=$sdk" > "$dir/local.properties" || return 1
    fi
  done
"#;

const ENSURE: &str = r#"@ROOT@set -e
clone="$HOME/@CLONE@"
if [ ! -e "$clone/.git" ]; then echo "no clone of @REPO@ at $clone" >&2; exit 90; fi
wts="$HOME/dev/@REPO@-worktrees"
wt="$wts/@NAME@"
mkdir -p "$wts" "$wt.job"
@MARK_LANE@
setup() {
  cd "$wt" || return 1
  mark_lane || return 1
  "$HOME/.local/bin/builds" setup "$wt" || return 1
  if [ -x scripts/hooks/post-checkout ]; then
    scripts/hooks/post-checkout 0000000000000000000000000000000000000000 @SHA@ 1 || return 1
  fi
@ANDROID@}
# The post-checkout links the build folders into the builds root; the main
# checkout here ignores those links in .git/info/exclude (which the host's clone
# lacks), so the host's clone ignores each one a worktree shows as new.
exclude_links() {
  exclude="$(git -C "$wt" rev-parse --git-common-dir)/info/exclude"
  case "$exclude" in /*) ;; *) exclude="$wt/$exclude" ;; esac
  mkdir -p "$(dirname "$exclude")"
  git -C "$wt" ls-files --others --exclude-standard | while IFS= read -r path; do
    if [ -L "$wt/$path" ]; then echo "/$path" >> "$exclude"; fi
  done
}
if [ -e "$wt/.git" ]; then
  exclude_links
  if [ -n "$(git -C "$wt" status --porcelain)" ]; then
    echo "the app checkout $wt has changes, so nothing was built:" >&2
    git -C "$wt" status --porcelain >&2
    exit 94
  fi
  git -C "$wt" checkout -q --detach @SHA@
else
  git -C "$clone" worktree add -q --detach "$wt" @SHA@
  if ! said=$(setup 2>&1); then
    echo "setting up $wt failed: $said" >&2
    git -C "$clone" worktree remove --force "$wt"
    exit 93
  fi
  exclude_links
  echo "@NEW_CHECKOUT@"
fi
echo "worktree=$wt""#;

/// The line `ENSURE` prints when it made the checkout, before the
/// `worktree=` line.
const NEW_CHECKOUT: &str = "new-checkout=yes";

/// The script that clones the build folders of the checkout <@SOURCE@> of
/// <@REPO@> into the new checkout <@NAME@> on the host, with the host's
/// `builds app-clone`.
const CLONE: &str = r#"@ROOT@set -e
src="$HOME/dev/@REPO@-worktrees/@SOURCE@"
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ ! -e "$src/.git" ]; then echo "no checkout @SOURCE@ to clone" >&2; exit 95; fi
"$HOME/.local/bin/builds" app-clone @LANE@ "$src" "$wt""#;

/// The shell function that writes @LANE@ to the lane file of the checkout
/// "$wt"; it fails when git cannot name the checkout's git dir.
const MARK_LANE: &str = r#"mark_lane() {
  git_dir=$(git -C "$wt" rev-parse --absolute-git-dir) || return 1
  printf '%s\n' @LANE@ > "$git_dir/@LANE_FILE@"
}"#;

/// The script that writes the lane file of the checkout <name> of <repo> on
/// the host, when the host has the worktree, and says which it did
/// (`marked_in`).
const MARK: &str = r#"@MARK_LANE@
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then
  mark_lane || exit 1
  echo lane-file=written
else
  echo lane-file=no-worktree
fi"#;

/// Whether `MARK` wrote the lane file, from the last `lane-file=` line it
/// printed: `written`, or `no-worktree` when the host has no such worktree.
/// Fails on any other output.
fn marked_in(output: &str) -> Result<bool> {
    match output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("lane-file="))
    {
        Some("written") => Ok(true),
        Some("no-worktree") => Ok(false),
        _ => fail!("the host did not say whether it wrote the lane file: {output:?}"),
    }
}

/// The script that deletes the lane file of the checkout <name> of <repo> on
/// the host, when the host has the worktree.
const UNMARK: &str = r#"set -e
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then
  git_dir=$(git -C "$wt" rev-parse --absolute-git-dir)
  rm -f "$git_dir/@LANE_FILE@"
fi"#;

/// The script that removes the checkout <name> of <repo> on the host: the
/// worktree from the host's clone, the folder `ENSURE` makes beside it, and
/// its build folders under the host's builds root.
const REMOVE: &str = r#"@ROOT@clone="$HOME/@CLONE@"
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then git -C "$clone" worktree remove --force "$wt" || exit 1; fi
rm -rf "$wt" "$wt.job" "$BUILDS_ROOT/@REPO@/@NAME@" || exit 1
if [ -e "$clone/.git" ]; then git -C "$clone" worktree prune; fi"#;

#[cfg(test)]
#[path = "app_remote_tests.rs"]
mod tests;
