//! The report of a verification: `index.html` with its pictures and its findings
//! as records (`findings.json`) beside it, and
//! the same findings as Markdown for a CI step summary.

use crate::compare::{self, Finding, Warning};
use crate::component_files::{self, Listing};
use crate::verify::{Checked, Outcome};
use std::path::Path;

/// Appends formatted text to a string.
macro_rules! put {
    ($dst:expr, $($arg:tt)*) => {
        $dst.push_str(&format!($($arg)*))
    };
}

/// Appends a formatted line to a string.
macro_rules! putln {
    ($dst:expr, $($arg:tt)*) => {{
        $dst.push_str(&format!($($arg)*));
        $dst.push('\n');
    }};
}

/// What the report says about the run it describes.
pub(crate) struct Header {
    pub(crate) commit: String,
    /// The commit of the run the pairs were compared with, when there was
    /// one.
    pub(crate) base: Option<String>,
    /// What the run says of the runtime copy it drew the boards with.
    pub(crate) runtime: String,
    pub(crate) chrome: String,
}

/// `&`, `<`, `>` and `"` as HTML text and attribute values hold them.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The picture file names a checked pair uses inside the report directory.
pub(crate) fn board_picture(name: &str) -> String {
    format!("img/{name}.board.png")
}

pub(crate) fn shot_picture(name: &str) -> String {
    format!("img/{name}.shot.png")
}

/// A box drawn over a picture: where, what kind of finding, and its sentence.
struct Mark {
    class: &'static str,
    bounds: crate::shots::Bounds,
    sentence: String,
}

fn marks(pair: &Checked) -> (Vec<Mark>, Vec<Mark>) {
    let (mut board, mut shot) = (Vec::new(), Vec::new());
    let on_board = |index: usize, class, sentence: &str| Mark {
        class,
        bounds: pair.board.elements[index].bounds,
        sentence: sentence.to_string(),
    };
    let on_shot = |index: usize, class, sentence: &str| Mark {
        class,
        bounds: pair.shot.elements[index].bounds,
        sentence: sentence.to_string(),
    };
    for finding in &pair.findings {
        let sentence = finding.sentence();
        match finding {
            Finding::Missing(e) => board.push(on_board(e.index, "missing", &sentence)),
            Finding::Extra(e) => shot.push(on_shot(e.index, "extra", &sentence)),
            Finding::OutOfOrder { board: b, shot: s } => {
                board.push(on_board(b.index, "order", &sentence));
                shot.push(on_shot(s.index, "order", &sentence));
            }
            Finding::RoleDiffers { board: b, shot: s } => {
                board.push(on_board(b.index, "role", &sentence));
                shot.push(on_shot(s.index, "role", &sentence));
            }
            Finding::StateDiffers {
                board: b, shot: s, ..
            } => {
                board.push(on_board(b.index, "state", &sentence));
                shot.push(on_shot(s.index, "state", &sentence));
            }
        }
    }
    let partner = |index: usize| {
        pair.pairs
            .iter()
            .find(|(b, _)| *b == index)
            .map(|(_, s)| *s)
    };
    for warning in &pair.warnings {
        let sentence = warning.sentence();
        match warning {
            Warning::FrameOffset { .. } => {}
            Warning::Region { bounds, .. } => {
                board.push(Mark {
                    class: "region",
                    bounds: *bounds,
                    sentence: sentence.clone(),
                });
                shot.push(Mark {
                    class: "region",
                    bounds: crate::shots::Bounds {
                        x: bounds.x + pair.offset.0,
                        y: bounds.y + pair.offset.1,
                        ..*bounds
                    },
                    sentence,
                });
            }
            Warning::Moved { entry, .. }
            | Warning::Resized { entry, .. }
            | Warning::FontSize { entry, .. }
            | Warning::FontWeight { entry, .. }
            | Warning::Colour { entry, .. } => {
                board.push(on_board(entry.index, "warning", &sentence));
                if let Some(s) = partner(entry.index) {
                    shot.push(on_shot(s, "warning", &sentence));
                }
            }
        }
    }
    (board, shot)
}

fn frame(src: &str, size: &crate::shots::Size, marks: &[Mark]) -> String {
    let mut html = format!(
        "<div class=\"frame\" style=\"width:{w}px;height:{h}px\"><img src=\"{src}\" width=\"{w}\" height=\"{h}\" alt=\"\">",
        src = esc(src),
        w = size.width,
        h = size.height
    );
    for mark in marks {
        put!(
            html,
            "<div class=\"box {class}\" style=\"left:{x}px;top:{y}px;width:{w}px;height:{h}px\" title=\"{title}\"></div>",
            class = mark.class,
            x = mark.bounds.x,
            y = mark.bounds.y,
            w = mark.bounds.width,
            h = mark.bounds.height,
            title = esc(&mark.sentence)
        );
    }
    html.push_str("</div>");
    html
}

fn checked_row(pair: &Checked) -> String {
    let name = pair.name.to_string();
    let (board_marks, shot_marks) = marks(pair);
    let mut html = format!("<article id=\"{id}\"><h3>{id}", id = esc(&name));
    let note = crate::verify::note(pair.against);
    if !note.is_empty() {
        put!(html, " <span class=\"note\">{note}</span>");
    }
    html.push_str("</h3>");
    for (items, class) in [
        (
            pair.findings
                .iter()
                .map(Finding::sentence)
                .collect::<Vec<_>>(),
            "findings",
        ),
        (
            pair.warnings
                .iter()
                .map(Warning::sentence)
                .collect::<Vec<_>>(),
            "warnings",
        ),
    ] {
        if !items.is_empty() {
            put!(html, "<ul class=\"{class}\">");
            for item in items {
                put!(html, "<li>{}</li>", esc(&item));
            }
            html.push_str("</ul>");
        }
    }
    put!(
        html,
        "<div class=\"pair\"><figure>{board}<figcaption>Board</figcaption></figure><figure>{shot}<figcaption>Shot</figcaption></figure></div></article>",
        board = frame(&board_picture(&name), &pair.board.size, &board_marks),
        shot = frame(&shot_picture(&name), &pair.shot.size, &shot_marks),
    );
    html
}

fn list_section(title: &str, items: &[String]) -> String {
    let mut html = format!("<section><h2>{} ({})</h2>", esc(title), items.len());
    if items.is_empty() {
        html.push_str("<p>None.</p>");
    } else {
        html.push_str("<ul>");
        for item in items {
            put!(html, "<li><code>{}</code></li>", esc(item));
        }
        html.push_str("</ul>");
    }
    html.push_str("</section>");
    html
}

const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fff;--fg:#1a1a1f;--muted:#666;--line:#d8d8de}\
@media (prefers-color-scheme:dark){:root{--bg:#141418;--fg:#e8e8ee;--muted:#9a9aa6;--line:#34343c}}\
body{background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif;margin:0 auto;max-width:1200px;padding:16px}\
h1,h2,h3{line-height:1.2}dl{display:grid;grid-template-columns:max-content 1fr;gap:2px 12px}dt{color:var(--muted)}\
article{border-top:1px solid var(--line);padding:8px 0 16px}.pair{display:flex;flex-wrap:wrap;gap:16px;align-items:flex-start}\
figure{margin:0}figcaption{color:var(--muted)}.frame{position:relative;max-width:100%}.frame img{display:block}\
.box{position:absolute;box-sizing:border-box;border:2px solid;pointer-events:auto}\
.missing{border-color:#e5484d}.extra{border-color:#f5a623}.order{border-color:#8e4ec6}.role{border-color:#d6409f}\
.state{border-color:#0091ff}.warning{border-color:#f5d90a}.region{border-color:#12a594;background:#12a59422}\
.note{color:var(--muted);font-weight:400;font-size:12px}.findings li{color:#e5484d}code{font:12px ui-monospace,monospace}";

/// The report page. Its pictures are named by [`board_picture`] and
/// [`shot_picture`].
pub(crate) fn page(outcome: &Outcome, header: &Header) -> String {
    let failing: Vec<&Checked> = outcome
        .checked
        .iter()
        .filter(|c| !c.findings.is_empty())
        .collect();
    let warned: Vec<&Checked> = outcome
        .checked
        .iter()
        .filter(|c| c.findings.is_empty() && !c.warnings.is_empty())
        .collect();
    let clean: Vec<&Checked> = outcome
        .checked
        .iter()
        .filter(|c| c.findings.is_empty() && c.warnings.is_empty())
        .collect();
    let mut html = String::from("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    html.push_str("<title>Shots against boards</title>");
    put!(
        html,
        "<style>{STYLE}</style></head><body><h1>Shots against boards</h1><dl>"
    );
    put!(html, "<dt>Commit</dt><dd>{}</dd>", esc(&header.commit));
    if let Some(base) = &header.base {
        put!(html, "<dt>Base</dt><dd>the run of {}</dd>", esc(base));
    }
    put!(
        html,
        "<dt>Runtime</dt><dd>{}</dd><dt>Chrome</dt><dd>{}</dd><dt>Tolerances</dt><dd>{}</dd>",
        esc(&header.runtime),
        esc(&header.chrome),
        esc(&compare::tolerances().join(", "))
    );
    put!(
        html,
        "<dt>Pairs</dt><dd>{} failing, {} with warnings, {} confirmed</dd></dl>",
        failing.len() + outcome.problems.len(),
        warned.len(),
        clean.len()
    );
    put!(
        html,
        "<section><h2>Failures ({})</h2>",
        failing.len() + outcome.problems.len()
    );
    if failing.is_empty() && outcome.problems.is_empty() {
        html.push_str("<p>None.</p>");
    }
    if !outcome.problems.is_empty() {
        html.push_str("<ul class=\"findings\">");
        for problem in &outcome.problems {
            put!(html, "<li>{}</li>", esc(&problem.sentence));
        }
        html.push_str("</ul>");
    }
    for pair in &failing {
        html.push_str(&checked_row(pair));
    }
    put!(
        html,
        "</section><section><h2>Warnings ({})</h2>",
        warned.len()
    );
    if warned.is_empty() {
        html.push_str("<p>None.</p>");
    }
    for pair in &warned {
        html.push_str(&checked_row(pair));
    }
    put!(
        html,
        "</section><section><h2>Confirmed ({})</h2>",
        clean.len()
    );
    if clean.is_empty() {
        html.push_str("<p>None.</p>");
    }
    for pair in &clean {
        html.push_str(&checked_row(pair));
    }
    html.push_str("</section>");
    if !outcome.not_taken.is_empty() {
        html.push_str("<section><h2>Not taken here</h2><ul>");
        for name in &outcome.not_taken {
            put!(
                html,
                "<li><code>{}</code>: {}</li>",
                esc(&name.to_string()),
                crate::capture::NEEDS_WINDOW
            );
        }
        html.push_str("</ul></section>");
    }
    html.push_str(&list_section(
        "Boards with no shot (to build)",
        &outcome.lists.to_build,
    ));
    html.push_str(&list_section(
        "Shots with no board (to delete)",
        &outcome.lists.to_delete,
    ));
    html.push_str(&components_section(&outcome.components));
    html.push_str("</body></html>");
    html
}

/// The components' capture files: each component with a preview row its
/// capture file does not declare, and each capture file with no preview, as
/// `builds shots components` prints them.
fn components_section(listing: &Listing) -> String {
    let mut html = String::from("<section><h2>Component capture files</h2><pre>");
    html.push_str(&esc(&component_files::lines(listing, &[]).join("\n")));
    html.push_str("</pre></section>");
    html
}

/// The findings as Markdown for a CI step summary: counts, every failure and
/// the lists; no pictures.
pub(crate) fn summary(outcome: &Outcome, header: &Header) -> String {
    let failing = outcome
        .checked
        .iter()
        .filter(|c| !c.findings.is_empty())
        .count();
    let warned = outcome
        .checked
        .iter()
        .filter(|c| c.findings.is_empty() && !c.warnings.is_empty())
        .count();
    let confirmed = outcome.checked.len() - failing - warned;
    let mut text = format!("## Shots against boards\n\nCommit `{}`", header.commit);
    if let Some(base) = &header.base {
        put!(text, ", base: the run of `{base}`");
    }
    put!(
        text,
        ". {} failing, {} with warnings, {} confirmed. Runtime: {}; Chrome {}.\n\n",
        failing + outcome.problems.len(),
        warned,
        confirmed,
        header.runtime,
        header.chrome
    );
    if failing + outcome.problems.len() > 0 {
        text.push_str("### Failures\n\n");
        for problem in &outcome.problems {
            putln!(text, "- {}", problem.sentence);
        }
        for pair in outcome.checked.iter().filter(|c| !c.findings.is_empty()) {
            let note = crate::verify::mark(pair.against);
            for finding in &pair.findings {
                putln!(text, "- `{}`{note}: {}", pair.name, finding.sentence());
            }
        }
        text.push('\n');
    }
    if !outcome.not_taken.is_empty() {
        text.push_str("### Not taken here\n\n");
        for name in &outcome.not_taken {
            putln!(text, "- `{name}`: {}", crate::capture::NEEDS_WINDOW);
        }
        text.push('\n');
    }
    for (title, items) in [
        ("Boards with no shot (to build)", &outcome.lists.to_build),
        ("Shots with no board (to delete)", &outcome.lists.to_delete),
    ] {
        if !items.is_empty() {
            put!(text, "### {title}\n\n");
            for item in items {
                putln!(text, "- `{item}`");
            }
            text.push('\n');
        }
    }
    if outcome.components.has_gaps() {
        putln!(
            text,
            "### Component capture files\n\n{} `builds shots components` lists them.",
            outcome.components.counts()
        );
    }
    text
}

/// Writes `index.html`, the pictures of the checked pairs and the findings as
/// records (findings_file.rs) into `dir`.
pub(crate) fn write(dir: &Path, outcome: &Outcome, header: &Header) -> Result<(), String> {
    let images = dir.join("img");
    std::fs::create_dir_all(&images)
        .map_err(|e| format!("cannot prepare {}: {e}", images.display()))?;
    for pair in &outcome.checked {
        let name = pair.name.to_string();
        for (from, to) in [
            (&pair.board_png, dir.join(board_picture(&name))),
            (&pair.shot_png, dir.join(shot_picture(&name))),
        ] {
            std::fs::copy(from, &to)
                .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))?;
        }
    }
    let index = dir.join("index.html");
    std::fs::write(&index, page(outcome, header))
        .map_err(|e| format!("cannot write {}: {e}", index.display()))?;
    crate::findings_file::write(dir, &header.commit, &outcome.checked)
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
