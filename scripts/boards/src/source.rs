//! Where a repo's design files are read from: a folder's files as they are,
//! or the files at a commit, which git reads without checking anything out,
//! so two design changes can share a worktree and a change is read from its
//! own branch.
//!
//! The design system's bundles (`bundle.css`, `bundle.js`) are generated
//! files that no commit holds. A source's own `build-bundle.py` builds them
//! in a scratch copy of the design system, never in the repo's files: a
//! commit's are kept in a cache under the hash of the files they come from,
//! one build per distinct design system, and a folder's, whose files may
//! change at any time, are built on each read.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::tokens::{TOKENS_JSON, tokens_css};
use crate::{Context, Error, Result, fail};

/// Where a repo's design files are read from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// The files of a directory as they are.
    Dir(&'a Path),
    /// The files at revision <rev> of the repo that <repo> (any of its
    /// worktrees) is in.
    Commit { repo: &'a Path, rev: &'a str },
}

/// The design system's bundles, the paths each is read from.
const BUNDLES: [&str; 2] = [
    "design/system/components/bundle.css",
    "design/system/components/bundle.js",
];
/// The folder the bundles are built from, and the script that builds them.
const BUNDLE_SOURCES: &str = "design/system";
const BUNDLER: &str = "design/system/build-bundle.py";

static WORK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// `git -C <dir>`.
pub fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    command
}

/// stdout of a git command that must succeed, as text. <what> says what the
/// command does in an error: "git failed to <what>".
pub fn git_text(command: &mut Command, what: &str) -> Result<String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git to {what}"))?;
    if !output.status.success() {
        fail!(
            "git failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether <path> is a file or folder at <rev> of the repo <top> is in.
pub fn exists_in(top: &Path, rev: &str, path: &str) -> Result<bool> {
    let output = git(top)
        .args(["cat-file", "-e", &format!("{rev}:{path}")])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git to look for {path} in {rev}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(128) => Ok(false),
        _ => fail!(
            "git failed to look for {path} in {rev}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

impl Source<'_> {
    /// How an error names <rel> of this source.
    pub fn label(&self, rel: &str) -> String {
        match self {
            Source::Dir(dir) => dir.join(rel).display().to_string(),
            Source::Commit { rev, .. } => format!("{rev}:{rel}"),
        }
    }

    /// How an error names the source itself.
    fn name(&self) -> String {
        match self {
            Source::Dir(dir) => dir.display().to_string(),
            Source::Commit { rev, .. } => rev.to_string(),
        }
    }

    /// The full id of the commit that a commit source's revision names, or
    /// None for a folder; fails when the revision names no commit.
    pub fn check(&self) -> Result<Option<String>> {
        let Source::Commit { repo, rev } = self else {
            return Ok(None);
        };
        let commit = git_text(
            git(repo).args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ]),
            &format!("find the commit {rev}"),
        )?;
        Ok(Some(commit.trim().to_string()))
    }

    /// The text of file <rel>; None when there is no such file.
    fn text(&self, rel: &str) -> Result<Option<String>> {
        match self {
            Source::Dir(dir) => match std::fs::read_to_string(dir.join(rel)) {
                Ok(text) => Ok(Some(text)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => fail!("reading {}: {error}", self.label(rel)),
            },
            Source::Commit { repo, rev } => {
                if !exists_in(repo, rev, rel)? {
                    return Ok(None);
                }
                let text = git_text(
                    git(repo).args(["cat-file", "blob", &format!("{rev}:{rel}")]),
                    &format!("read {}", self.label(rel)),
                )?;
                Ok(Some(text))
            }
        }
    }

    /// Whether the source has file <rel>.
    pub fn has(&self, rel: &str) -> Result<bool> {
        Ok(self.text(rel)?.is_some())
    }

    /// The text of file <rel>, which <what> names in the error when the
    /// source has no such file.
    pub fn read(&self, rel: &str, what: &str) -> Result<String> {
        match self.text(rel)? {
            Some(text) => Ok(text),
            None => fail!("{what} {} is not there", self.label(rel)),
        }
    }

    /// The design system's tokens.css, bundle.css and bundle.js. A commit's
    /// bundles are kept in <cache>, and every build works in a scratch folder
    /// there.
    pub fn design_system(&self, cache: &Path) -> Result<[String; 3]> {
        let css = self.tokens_css()?;
        let [bundle_css, bundle_js] = self.bundles(cache)?;
        Ok([css, bundle_css, bundle_js])
    }

    /// The design system as a board draws in Chrome on a Mac: its
    /// tokens.css ends with the `@font-face` rules for the SF Mono files in
    /// <fonts> (`fonts`), which a page loads from the server that serves it.
    /// A page that is a file cannot load those files. What is published or
    /// written to the repo takes `design_system`.
    pub fn board_design_system(&self, cache: &Path, fonts: &Path) -> Result<[String; 3]> {
        let [css, bundle_css, bundle_js] = self.design_system(cache)?;
        let rules = crate::fonts::font_face_css(fonts)?;
        Ok([format!("{css}\n{rules}"), bundle_css, bundle_js])
    }

    /// The tokens.css of the source's tokens.json; an error names the file.
    pub fn tokens_css(&self) -> Result<String> {
        let tokens = self.read(TOKENS_JSON, "the design system's tokens")?;
        tokens_css(&tokens)
            .map_err(|error| Error::new(format!("{}: {error}", self.label(TOKENS_JSON))))
    }

    /// The design system's bundle.css and bundle.js, which the source's
    /// build-bundle.py builds in a scratch copy of what it reads.
    fn bundles(&self, cache: &Path) -> Result<[String; 2]> {
        let inputs = self.design_system_inputs().map_err(|error| {
            Error::new(format!(
                "the design system of {} cannot be generated: {error}",
                self.name()
            ))
        })?;
        let Source::Commit { repo, rev } = self else {
            return in_scratch(cache, |work| {
                self.copy_out(&inputs, work)?;
                self.run_bundler(work)?;
                let [css, js] = BUNDLES.map(|rel| {
                    std::fs::read_to_string(work.join(rel))
                        .context(|| format!("reading the generated {rel} of {}", self.name()))
                });
                Ok([css?, js?])
            });
        };
        let mut key = Vec::new();
        for input in &inputs {
            let hash = git_text(
                git(repo).args(["rev-parse", "--verify", &format!("{rev}:{input}")]),
                &format!("find {rev}:{input}"),
            )?;
            key.push(hash.trim().to_string());
        }
        // One folder name for the inputs' hashes: git's hash of the list of them.
        let entry = cache.join(git_hash(repo, &key.join("\n"))?);
        if !entry.is_dir() {
            self.generate(&inputs, cache, &entry)?;
        }
        let read = |rel: &str| {
            let name = rel.rsplit('/').next().expect("a path has a name");
            std::fs::read_to_string(entry.join(name))
                .context(|| format!("reading the generated {name} of {rev}"))
        };
        Ok([read(BUNDLES[0])?, read(BUNDLES[1])?])
    }

    /// Writes <inputs>, paths of the source, into the folder <into> at their
    /// paths in the repo.
    pub(crate) fn copy_out(&self, inputs: &[String], into: &Path) -> Result<()> {
        match self {
            Source::Commit { repo, rev } => export(repo, rev, inputs, into, "the design system"),
            Source::Dir(dir) => {
                for input in inputs {
                    copy_tree(&dir.join(input), &into.join(input))?;
                }
                Ok(())
            }
        }
    }

    /// Runs build-bundle.py in <work>, a copy of what it reads, and fails
    /// unless it wrote both bundles.
    fn run_bundler(&self, work: &Path) -> Result<()> {
        let what = self.name();
        let ran = Command::new("python3")
            .arg("-I")
            .arg(BUNDLER)
            .current_dir(work)
            .stdin(Stdio::null())
            .output()
            .context(|| format!("running python3 {BUNDLER} for {what}"))?;
        if !ran.status.success() {
            fail!(
                "{BUNDLER} failed on the design system of {what}: {}",
                String::from_utf8_lossy(&ran.stderr).trim()
            );
        }
        for rel in BUNDLES {
            if !work.join(rel).is_file() {
                fail!("{BUNDLER} did not write {rel} for {what}");
            }
        }
        Ok(())
    }

    /// Exports <inputs> at the revision, runs build-bundle.py in the export
    /// and moves the two bundles to <entry>.
    fn generate(&self, inputs: &[String], cache: &Path, entry: &Path) -> Result<()> {
        let what = self.name();
        in_scratch(cache, |work| {
            self.copy_out(inputs, work)?;
            self.run_bundler(work)?;
            let out = work.join("out");
            std::fs::create_dir_all(&out).context(|| format!("making {}", out.display()))?;
            for rel in BUNDLES {
                let name = rel.rsplit('/').next().expect("a path has a name");
                let from = work.join(rel);
                std::fs::rename(&from, out.join(name))
                    .context(|| format!("moving {}", from.display()))?;
            }
            // A build that finished first for the same design system wins;
            // both hold the same files.
            if let Err(error) = std::fs::rename(&out, entry)
                && !entry.is_dir()
            {
                fail!(
                    "moving the design system of {what} to {}: {error}",
                    entry.display()
                );
            }
            Ok(())
        })
    }

    /// The paths the design system's files are read from: its folder, and
    /// what a symbolic link in it points at inside the repo (bae's sheep logo
    /// links to the repo's icon source), followed until no new path turns up.
    /// build-bundle.py reads them, and an export of them holds every file of
    /// the design system with its links resolved.
    pub fn design_system_inputs(&self) -> Result<Vec<String>> {
        let mut inputs = vec![BUNDLE_SOURCES.to_string()];
        let mut next = 0;
        while next < inputs.len() {
            let input = inputs[next].clone();
            next += 1;
            for (path, target) in self.links_under(&input)? {
                let Some(resolved) = resolve_link(&path, &target) else {
                    fail!("the link {} points outside the repo", self.label(&path));
                };
                let covered = inputs
                    .iter()
                    .any(|input| resolved == *input || resolved.starts_with(&format!("{input}/")));
                if !covered {
                    inputs.push(resolved);
                }
            }
        }
        Ok(inputs)
    }

    /// Each symbolic link at or under <input>, as its path in the repo and
    /// the target it holds. Fails when the source has no <input>.
    fn links_under(&self, input: &str) -> Result<Vec<(String, String)>> {
        match self {
            Source::Commit { repo, rev } => {
                let listing = git_text(
                    git(repo).args(["ls-tree", "-r", "-z", rev, "--", input]),
                    &format!("list {rev}:{input}"),
                )?;
                if listing.is_empty() {
                    fail!("{rev} has no {input}");
                }
                let mut links = Vec::new();
                for entry in listing.split('\0').filter(|e| !e.is_empty()) {
                    let Some((meta, path)) = entry.split_once('\t') else {
                        continue;
                    };
                    if !meta.starts_with("120000 ") {
                        continue;
                    }
                    let target = git_text(
                        git(repo).args(["cat-file", "blob", &format!("{rev}:{path}")]),
                        &format!("read the link {rev}:{path}"),
                    )?;
                    links.push((path.to_string(), target.trim_end_matches('\n').to_string()));
                }
                Ok(links)
            }
            Source::Dir(dir) => {
                let mut links = Vec::new();
                let mut todo = vec![input.to_string()];
                while let Some(rel) = todo.pop() {
                    let path = dir.join(&rel);
                    let meta = std::fs::symlink_metadata(&path)
                        .context(|| format!("reading {}", path.display()))?;
                    if meta.file_type().is_symlink() {
                        let target = std::fs::read_link(&path)
                            .context(|| format!("reading the link {}", path.display()))?;
                        links.push((rel, target.to_string_lossy().into_owned()));
                    } else if meta.is_dir() {
                        for entry in std::fs::read_dir(&path)
                            .context(|| format!("listing {}", path.display()))?
                        {
                            let entry = entry.context(|| format!("listing {}", path.display()))?;
                            todo.push(format!("{rel}/{}", entry.file_name().to_string_lossy()));
                        }
                    }
                }
                links.sort();
                Ok(links)
            }
        }
    }
}

/// Copies the file, link or folder <from> to <to>, keeping each symbolic
/// link as a link.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(from).context(|| format!("reading {}", from.display()))?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).context(|| format!("making {}", parent.display()))?;
    }
    if meta.file_type().is_symlink() {
        let target =
            std::fs::read_link(from).context(|| format!("reading the link {}", from.display()))?;
        std::os::unix::fs::symlink(&target, to)
            .context(|| format!("copying the link {}", from.display()))?;
    } else if meta.is_dir() {
        std::fs::create_dir_all(to).context(|| format!("making {}", to.display()))?;
        for entry in std::fs::read_dir(from).context(|| format!("listing {}", from.display()))? {
            let entry = entry.context(|| format!("listing {}", from.display()))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to).context(|| format!("copying {}", from.display()))?;
    }
    Ok(())
}

/// Runs <use_it> on a new scratch folder in <cache> and removes the folder
/// after it, whatever <use_it> returns. A folder that cannot be removed fails
/// the call: <use_it>'s own error comes first when it failed too.
pub fn in_scratch<T>(cache: &Path, use_it: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let work = cache.join(format!(
        ".work-{}-{}",
        std::process::id(),
        WORK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = use_it(&work);
    let removed = match std::fs::remove_dir_all(&work) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "removing the scratch folder {}: {error}",
            work.display()
        )),
        _ => Ok(()),
    };
    match (result, removed) {
        (result, Ok(())) => result,
        (Ok(_), Err(removing)) => Err(Error::new(removing)),
        (Err(error), Err(removing)) => Err(Error::new(format!("{error}; then {removing}"))),
    }
}

/// Writes <paths> as they are at <rev> of the repo <repo> is in into <into>,
/// which it creates, at their paths in the repo. <what> names them in an
/// error.
pub fn export(repo: &Path, rev: &str, paths: &[String], into: &Path, what: &str) -> Result<()> {
    std::fs::create_dir_all(into).context(|| format!("making {}", into.display()))?;
    let archive = git(repo)
        .arg("archive")
        .arg(rev)
        .arg("--")
        .args(paths)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git to export {what} of {rev}"))?;
    if !archive.status.success() {
        fail!(
            "git failed to export {what} of {rev}: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        );
    }
    let mut tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(into)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("running tar to unpack {what}"))?;
    // tar stops reading at the archive's end marker, before the padding that
    // git archive writes after it, so the writer can meet a closed pipe after
    // tar has unpacked everything. A tar that failed says so in its status.
    match std::io::Write::write_all(
        &mut tar.stdin.take().expect("tar's stdin is piped"),
        &archive.stdout,
    ) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
        written => written.context(|| "feeding tar".to_string())?,
    }
    let unpacked = tar
        .wait_with_output()
        .context(|| "waiting for tar".to_string())?;
    if !unpacked.status.success() {
        fail!(
            "tar failed to unpack {what} of {rev}: {}",
            String::from_utf8_lossy(&unpacked.stderr).trim()
        );
    }
    Ok(())
}

/// The repo path that link <path> pointing at <target> names; None for one
/// outside the repo or an absolute target.
fn resolve_link(path: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// Git's object hash of <text>, as `git hash-object --stdin` writes it.
fn git_hash(repo: &Path, text: &str) -> Result<String> {
    let mut child = git(repo)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| "running git to hash the design system's inputs".to_string())?;
    std::io::Write::write_all(
        &mut child.stdin.take().expect("git's stdin is piped"),
        text.as_bytes(),
    )
    .context(|| "feeding git".to_string())?;
    let output = child
        .wait_with_output()
        .context(|| "waiting for git".to_string())?;
    if !output.status.success() {
        fail!(
            "git failed to hash the design system's inputs: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
