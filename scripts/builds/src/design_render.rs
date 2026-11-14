//! Boards and review pages. A board is a design file,
//! `design/mocks/<name>.dc.html`; `render_board` draws one as a page of HTML
//! the way the design canvas does: it follows the boards it mounts
//! (`dc-import`), applies each board's own values (`renderVals`), `sc-if` and
//! `sc-for`, and uses the worktree's design system (`bundle.js`, `bundle.css`,
//! `tokens.css`). The page's script is `design_render/board.html`.
//!
//! A board is drawn from a `Source`: a directory's files, or the files at a
//! commit, which git reads without checking anything out, so two design
//! changes can share a worktree and a change is drawn from its own branch.
//!
//! A design change's review page (`design_render/review.html`) shows the
//! change's words under their headings, and each board twice, side by side in
//! frames that load the board's own pages: now, the board as it stands on
//! origin/main, and proposed, the board at the last commit of the change's
//! range, each drawn with its own commit's design system. A board the change
//! adds has no now, and a board it deletes has only a now. The daemon draws
//! each frame on every request, so the page shows a redraw on reload. The
//! sizes of the frames come from each commit's `canvas.json` in the board's
//! own folder (`design/mocks/canvas.json` for a canvas board).
//!
//! A design place of another project, such as the builds dashboard's
//! `scripts/builds/design/dashboard.html`, is a plain HTML page: it is no
//! canvas board, so nothing is applied to it and no design system is read;
//! the page is drawn as the file at its commit has it, in the theme the
//! viewer's browser asks for.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::design_paths::exists_in;
use crate::lists::view::DesignView;
use crate::lists::{DesignChange, ReviewBoard, Theme};

const BOARD_PAGE: &str = include_str!("design_render/board.html");
const REVIEW_PAGE: &str = include_str!("design_render/review.html");

const MOCKS: &str = "design/mocks";
const BOARD_SUFFIX: &str = ".dc.html";
/// What a board with no script of its own runs.
const NO_SCRIPT: &str =
    "class Component extends DCLogic { renderVals() { return { yes: true }; } }";

/// <template> with each `@@NAME@@` replaced by its part, in one pass: a part
/// holding such a marker is not filled again.
fn fill(template: &str, parts: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let found = after.find("@@").and_then(|end| {
            let name = &after[..end];
            parts
                .iter()
                .find(|(part, _)| *part == name)
                .map(|(_, value)| (*value, &after[end + 2..]))
        });
        match found {
            Some((value, next)) => {
                out.push_str(value);
                rest = next;
            }
            None => {
                out.push_str("@@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Is <board> a plain HTML page (a path that is no canvas board), such as a
/// project's own board under its design places? A canvas board is a name or
/// `design/mocks/<name>.dc.html`.
pub fn is_plain_page(board: &str) -> bool {
    board.contains('/') && !board.starts_with(&format!("{MOCKS}/")) && board.ends_with(".html")
}

/// The folder a plain page's `canvas.json` is in, and its key there: the
/// page's own folder and file name.
fn plain_canvas(board: &str) -> (String, String) {
    match board.rsplit_once('/') {
        Some((dir, file)) => (dir.to_string(), file.to_string()),
        None => (String::new(), board.to_string()),
    }
}

/// A board's name, from its name or its path `design/mocks/<name>.dc.html`.
/// A name is one file's, so nothing in it leaves the folder.
pub fn board_name(board: &str) -> Result<String> {
    let name = board.strip_prefix(&format!("{MOCKS}/")).unwrap_or(board);
    let name = name.strip_suffix(BOARD_SUFFIX).unwrap_or(name);
    let fine = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if !fine {
        fail!(
            "{board:?} is no board: a board is design/mocks/<name>.dc.html, its name letters, digits, `.`, `-` and `_`"
        );
    }
    Ok(name.to_string())
}

/// The revision the boards of "now" are drawn at.
pub const NOW: &str = "origin/main";

/// Where a board's files are read from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// The files of a directory as they are.
    Dir(&'a Path),
    /// The files at revision <rev> of the repo that <repo> (any of its
    /// worktrees) is in.
    Commit { repo: &'a Path, rev: &'a str },
}

impl Source<'_> {
    /// How an error names <rel> of this source.
    fn label(&self, rel: &str) -> String {
        match self {
            Source::Dir(dir) => dir.join(rel).display().to_string(),
            Source::Commit { rev, .. } => format!("{rev}:{rel}"),
        }
    }

    /// Fails unless the revision of a commit source is a commit.
    fn check(&self) -> Result<()> {
        if let Source::Commit { repo, rev } = self {
            git_text(
                git(repo).args([
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{rev}^{{commit}}"),
                ]),
                &format!("find the commit {rev}"),
            )?;
        }
        Ok(())
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
                let bytes = git(repo)
                    .args(["cat-file", "blob", &format!("{rev}:{rel}")])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .context(|| format!("running git to read {}", self.label(rel)))?;
                if !bytes.status.success() {
                    fail!(
                        "git failed to read {}: {}",
                        self.label(rel),
                        String::from_utf8_lossy(&bytes.stderr).trim()
                    );
                }
                Ok(Some(String::from_utf8_lossy(&bytes.stdout).into_owned()))
            }
        }
    }

    fn has(&self, rel: &str) -> Result<bool> {
        Ok(self.text(rel)?.is_some())
    }

    fn read(&self, rel: &str, what: &str) -> Result<String> {
        match self.text(rel)? {
            Some(text) => Ok(text),
            None => fail!("{what} {} is not there", self.label(rel)),
        }
    }
}

/// The design system's generated files, the paths each is read from.
const GENERATED: [&str; 3] = [
    "design/system/tokens.css",
    "design/system/components/bundle.css",
    "design/system/components/bundle.js",
];
/// The folders the generators read, and the scripts that run in them.
const GENERATOR_INPUTS: [&str; 2] = ["design/system", "scripts/design"];
const GENERATORS: [&str; 2] = [
    "scripts/design/tokens-css.py",
    "design/system/build-bundle.py",
];

static WORK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl Source<'_> {
    /// The design system's tokens.css, bundle.css and bundle.js. A directory
    /// has them on disk. No commit holds them (they are generated and
    /// ignored), so a commit's are generated from its own sources and kept in
    /// <cache> under the hash of the folders they come from: one build per
    /// distinct design system.
    fn design_system(&self, cache: &Path) -> Result<[String; 3]> {
        let Source::Commit { repo, rev } = self else {
            let [a, b, c] = GENERATED.map(|rel| self.read(rel, "the design system's"));
            return Ok([a?, b?, c?]);
        };
        let inputs = generator_inputs(repo, rev).map_err(|error| {
            fail::Fail::Message(format!(
                "the design system of {rev} cannot be generated: {}",
                error.text()
            ))
        })?;
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
        Ok([
            read(GENERATED[0])?,
            read(GENERATED[1])?,
            read(GENERATED[2])?,
        ])
    }

    /// Exports <inputs> at the revision, runs the generators in the export
    /// and moves the three files to <entry>.
    fn generate(&self, inputs: &[String], cache: &Path, entry: &Path) -> Result<()> {
        let Source::Commit { repo, rev } = self else {
            unreachable!("only a commit is generated")
        };
        let work = cache.join(format!(
            ".work-{}-{}",
            std::process::id(),
            WORK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(&work).context(|| format!("making {}", work.display()))?;
            let archive = git(repo)
                .arg("archive")
                .arg(rev)
                .arg("--")
                .args(inputs)
                .stdin(Stdio::null())
                .output()
                .context(|| format!("running git to export the design system of {rev}"))?;
            if !archive.status.success() {
                fail!(
                    "git failed to export the design system of {rev}: {}",
                    String::from_utf8_lossy(&archive.stderr).trim()
                );
            }
            let mut tar = Command::new("tar")
                .arg("-x")
                .arg("-C")
                .arg(&work)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context(|| "running tar to unpack the design system".to_string())?;
            std::io::Write::write_all(
                &mut tar.stdin.take().expect("tar's stdin is piped"),
                &archive.stdout,
            )
            .context(|| "feeding tar".to_string())?;
            let unpacked = tar
                .wait_with_output()
                .context(|| "waiting for tar".to_string())?;
            if !unpacked.status.success() {
                fail!(
                    "tar failed to unpack the design system of {rev}: {}",
                    String::from_utf8_lossy(&unpacked.stderr).trim()
                );
            }
            for script in GENERATORS {
                let ran = Command::new("python3")
                    .arg("-I")
                    .arg(script)
                    .current_dir(&work)
                    .stdin(Stdio::null())
                    .output()
                    .context(|| format!("running python3 {script} for {rev}"))?;
                if !ran.status.success() {
                    fail!(
                        "{script} failed on the design system of {rev}: {}",
                        String::from_utf8_lossy(&ran.stderr).trim()
                    );
                }
            }
            let out = work.join("out");
            std::fs::create_dir_all(&out).context(|| format!("making {}", out.display()))?;
            for rel in GENERATED {
                let name = rel.rsplit('/').next().expect("a path has a name");
                let from = work.join(rel);
                if !from.is_file() {
                    fail!("{} did not write {rel} for {rev}", GENERATORS.join(" and "));
                }
                std::fs::rename(&from, out.join(name))
                    .context(|| format!("moving {}", from.display()))?;
            }
            // A build that finished first for the same design system wins;
            // both hold the same files.
            if let Err(error) = std::fs::rename(&out, entry)
                && !entry.is_dir()
            {
                fail!(
                    "moving the design system of {rev} to {}: {error}",
                    entry.display()
                );
            }
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&work);
        result
    }
}

/// The paths at <rev> the generators read: the generator folders, and what a
/// symbolic link in them points at inside the repo (bae's sheep logo links
/// to the repo's icon source), followed until no new path turns up.
fn generator_inputs(repo: &Path, rev: &str) -> Result<Vec<String>> {
    let mut inputs: Vec<String> = GENERATOR_INPUTS.iter().map(|s| s.to_string()).collect();
    let mut next = 0;
    while next < inputs.len() {
        let listing = git_text(
            git(repo).args(["ls-tree", "-r", "-z", rev, "--", &inputs[next]]),
            &format!("list {rev}:{}", inputs[next]),
        )?;
        next += 1;
        if listing.is_empty() {
            fail!("{rev} has no {}", inputs[next - 1]);
        }
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
            let Some(resolved) = resolve_link(path, target.trim_end_matches('\n')) else {
                fail!("the link {rev}:{path} points outside the repo");
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

fn board_rel(name: &str) -> String {
    format!("{MOCKS}/{name}{BOARD_SUFFIX}")
}

/// What a board file holds that the page needs.
struct Part {
    /// The markup between `<x-dc>` and `</x-dc>`, less its `<helmet>`.
    body: String,
    /// The first `<style>` of the markup (the helmet's usually).
    style: String,
    script: String,
}

/// The text between the first <open> of <text> and the <close> after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

fn parse_part(from: &Source, rel: &str) -> Result<Part> {
    let path = from.label(rel);
    let source = from.read(rel, "the board")?;
    let (Some(open), Some(close)) = (source.find("<x-dc>"), source.rfind("</x-dc>")) else {
        fail!("{path} has no <x-dc>...</x-dc>");
    };
    if close < open {
        fail!("{path} closes its <x-dc> before it opens");
    }
    let mut body = source[open + "<x-dc>".len()..close].to_string();
    let style = between(&body, "<style>", "</style>")
        .unwrap_or("")
        .to_string();
    while let Some(start) = body.find("<helmet>") {
        let Some(end) = body[start..].find("</helmet>") else {
            fail!("{path} has a <helmet> that never closes")
        };
        body.replace_range(start..start + end + "</helmet>".len(), "");
    }
    let script = source
        .find("<script type=\"text/x-dc\"")
        .and_then(|start| {
            let tag_end = source[start..].find('>')? + start + 1;
            let end = source[tag_end..].find("</script>")? + tag_end;
            Some(source[tag_end..end].to_string())
        })
        .unwrap_or_else(|| NO_SCRIPT.to_string());
    Ok(Part {
        body,
        style,
        script,
    })
}

/// The names of the boards <body> mounts.
fn imports(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<dc-import name=\"") {
        let after = &rest[start + "<dc-import name=\"".len()..];
        let Some(end) = after.find('"') else { break };
        names.push(after[..end].to_string());
        rest = &after[end..];
    }
    names
}

/// Inline in a `<script>`: the one string that would end it early is written
/// so that it doesn't, which a script's strings read as the same text.
fn script_safe(js: &str) -> String {
    js.replace("</script", "<\\/script")
}

/// Board <board> of <from> as one page of HTML in <theme>, drawn with the
/// design system of <from>; a commit's is generated into <cache>
/// (`Places::design_system_cache`).
pub fn render_board(from: Source, board: &str, theme: Theme, cache: &Path) -> Result<String> {
    from.check()?;
    if is_plain_page(board) {
        return from.read(board, "the page");
    }
    let name = board_name(board)?;
    let mut parts: BTreeMap<String, Part> = BTreeMap::new();
    let mut todo = vec![("__main__".to_string(), board_rel(&name))];
    while let Some((key, rel)) = todo.pop() {
        let part = parse_part(&from, &rel)?;
        for dep in imports(&part.body) {
            let Ok(dep_name) = board_name(&dep) else {
                fail!(
                    "{} mounts {dep:?}, which is no board name",
                    from.label(&rel)
                )
            };
            if !parts.contains_key(&dep_name) && !todo.iter().any(|(k, _)| *k == dep_name) {
                if !from.has(&board_rel(&dep_name))? {
                    fail!(
                        "{} mounts the board {dep_name}, which is no file in {}",
                        from.label(&rel),
                        from.label(MOCKS)
                    );
                }
                todo.push((dep_name.clone(), board_rel(&dep_name)));
            }
        }
        parts.insert(key, part);
    }
    let [tokens_css, bundle_css, bundle_js] = from.design_system(cache)?;
    let styles: String = parts.values().map(|p| p.style.as_str()).collect();
    let templates: String = parts
        .iter()
        .map(|(k, p)| format!("<template id=\"t-{k}\">{}</template>\n", p.body))
        .collect();
    let classes: String = parts
        .iter()
        .map(|(k, p)| {
            format!(
                "C[\"{k}\"] = (function () {{ {} ; return Component; }})();\n",
                script_safe(&p.script)
            )
        })
        .collect();
    Ok(fill(
        BOARD_PAGE,
        &[
            ("THEME", theme.word()),
            ("TOKENS_CSS", &tokens_css),
            ("BUNDLE_CSS", &bundle_css),
            ("BUNDLE_JS", &script_safe(&bundle_js)),
            ("STYLES", &styles),
            ("TEMPLATES", &templates),
            ("CLASSES", &classes),
        ],
    ))
}

/// The width and height <canvas> (`canvas.json`) gives board <name>.
fn canvas_size(canvas: &Value, file: &str, canvas_path: &str) -> Result<(u64, u64)> {
    let board = &canvas["boards"][file];
    match (board["w"].as_u64(), board["h"].as_u64()) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => fail!("{canvas_path} gives no width and height for {file}"),
    }
}

/// The <n>th board of <design>'s review page, counting through its sections
/// in order.
pub fn review_board(design: &DesignChange, n: usize) -> Option<&ReviewBoard> {
    design
        .review
        .iter()
        .flat_map(|section| &section.boards)
        .nth(n)
}

/// The <k>th board <design> deletes, in path order.
pub fn deleted_board(design: &DesignChange, k: usize) -> Option<&String> {
    design.deleted.iter().nth(k)
}

/// Which drawing of a board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The board as it stands on origin/main.
    Now,
    /// The board at the last commit of the change's range.
    Proposed,
}

impl Side {
    pub fn word(self) -> &'static str {
        match self {
            Side::Now => "now",
            Side::Proposed => "proposed",
        }
    }
}

/// Where <side>'s files of <design> are read from. The change's repo is read
/// through its worktree, whatever branch the worktree has checked out.
pub fn source_of(design: &DesignChange, side: Side) -> Result<Source<'_>> {
    let repo = Path::new(&design.worktree);
    match (side, &design.tip) {
        (Side::Now, _) => Ok(Source::Commit { repo, rev: NOW }),
        (Side::Proposed, Some(tip)) => Ok(Source::Commit { repo, rev: tip }),
        (Side::Proposed, None) => fail!(
            "design change {} was recorded before its last commit was kept; amend it to draw its boards",
            design.id.0
        ),
    }
}

/// How <side> of board <board> shows on the review page: its frame (`w`, `h`
/// and the `src` the frame loads), `absent` (the board is no file there), or
/// the `error` that stops it.
fn drawing(design: &DesignChange, side: Side, board: &str, src: &str) -> Value {
    let drawn = (|| -> Result<Value> {
        let from = source_of(design, side)?;
        from.check()?;
        let (rel, folder, file) = if is_plain_page(board) {
            let (folder, file) = plain_canvas(board);
            (board.to_string(), folder, file)
        } else {
            let name = board_name(board)?;
            (
                board_rel(&name),
                MOCKS.to_string(),
                format!("{name}{BOARD_SUFFIX}"),
            )
        };
        if !from.has(&rel)? {
            return Ok(json!({ "absent": true }));
        }
        let canvas_path = format!("{folder}/canvas.json");
        let text = from.read(&canvas_path, "the canvas file")?;
        let canvas: Value = serde_json::from_str(&text)
            .context(|| format!("reading {}", from.label(&canvas_path)))?;
        let (w, h) = canvas_size(&canvas, &file, &canvas_path)?;
        Ok(json!({ "w": w, "h": h, "src": format!("{src}/{}", side.word()) }))
    })();
    // A board that is no file where the change draws it is that change's fault, not an absence.
    match drawn {
        Ok(value) if side == Side::Proposed && value.get("absent").is_some() => {
            json!({ "error": format!("{board} is no file at the change's last commit") })
        }
        Ok(value) => value,
        Err(error) => json!({ "error": error.text() }),
    }
}

/// <design>'s review page. <view> is the design as the dashboard's lists JSON
/// has it, which holds the app work its places resolve to. A board whose size
/// or file can't be read shows as that problem in its place, not as a page
/// that fails whole.
pub fn review_page(design: &DesignChange, view: &DesignView, project: &str) -> Result<String> {
    let mut n = 0;
    let mut sections = Vec::new();
    for section in &design.review {
        let mut boards = Vec::new();
        for board in &section.boards {
            let src = format!("/design/{project}/{}/board/{n}", design.id.0);
            boards.push(json!({
                "path": board.board,
                "caption": board.caption,
                "theme": board.theme.word(),
                "link": design.boards.get(&board.board).map(|b| b.link.clone()),
                "now": drawing(design, Side::Now, &board.board, &src),
                "proposed": drawing(design, Side::Proposed, &board.board, &src),
            }));
            n += 1;
        }
        sections
            .push(json!({ "heading": section.heading, "words": section.words, "boards": boards }));
    }
    let deleted: Vec<Value> = design
        .deleted
        .iter()
        .enumerate()
        .map(|(k, path)| {
            let src = format!("/design/{project}/{}/deleted/{k}", design.id.0);
            // A deleted board is a file on origin/main, so it is never `absent` there.
            json!({ "path": path, "now": drawing(design, Side::Now, path, &src) })
        })
        .collect();
    let mut data = serde_json::to_value(view).expect("the view serializes");
    data["sections"] = Value::Array(sections);
    data["deleted"] = Value::Array(deleted);
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = data.to_string().replace('<', "\\u003c");
    Ok(fill(REVIEW_PAGE, &[("DATA", &data)]))
}

#[cfg(test)]
#[path = "design_render_tests.rs"]
mod tests;
