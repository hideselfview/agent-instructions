use super::*;
use crate::lists::design_paths::drawing_at;
use crate::testing::{Scratch, git, repo};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Says which places draw alike, and counts its questions.
struct Says {
    alike: &'static [&'static str],
    asked: AtomicUsize,
}

impl Renders for Says {
    fn alike(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        places: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(places
            .iter()
            .filter(|place| self.alike.contains(&place.as_str()))
            .cloned()
            .collect())
    }
}

fn says(alike: &'static [&'static str]) -> Says {
    Says {
        alike,
        asked: AtomicUsize::new(0),
    }
}

fn set(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|p| p.to_string()).collect()
}

const A: &str = "design/mocks/A.dc.html";
const B: &str = "design/mocks/B.dc.html";
const TOKENS: &str = "design/system/tokens.json";
const CANVAS: &str = "design/mocks/canvas.json";

/// The text of <path>: for canvas.json, JSON that names a design system or
/// not, with <text> in it; for another file, <text>.
fn canvas_or(path: &str, design_system: bool, text: &str) -> String {
    if path != CANVAS {
        return text.to_string();
    }
    let systems = if design_system {
        r#""designSystems": [{"namespace": "demo"}],"#
    } else {
        ""
    };
    format!(r#"{{{systems} "v": "{text}"}}"#)
}

/// Two commits, `before` and `after`, where `after` rewrites <changed>, and
/// each sameness of <held> places and <sources> over <paths>, as <renders>
/// draws them.
fn between(
    renders: &Says,
    changed: &[&str],
    held: &[&str],
    sources: &[&str],
    paths: &[&str],
) -> Sameness {
    between_in(renders, true, changed, held, sources, paths)
}

/// `between`, in a repo whose canvas.json names a design system or not.
fn between_in(
    renders: &Says,
    design_system: bool,
    changed: &[&str],
    held: &[&str],
    sources: &[&str],
    paths: &[&str],
) -> Sameness {
    let scratch = Scratch::new("design-drawn");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1\n");
    for path in [A, B, TOKENS, CANVAS] {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, canvas_or(path, design_system, "one")).unwrap();
        git(&dir, &["add", path]);
    }
    git(&dir, &["commit", "-q", "-m", "before"]);
    for path in changed {
        std::fs::write(dir.join(path), canvas_or(path, design_system, "two")).unwrap();
        git(&dir, &["add", path]);
    }
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "after"]);
    let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let asked: BTreeSet<&String> = paths.iter().collect();
    let before = drawing_at(&dir, "HEAD~1", asked.iter().copied()).unwrap();
    let after = drawing_at(&dir, "HEAD", asked.iter().copied()).unwrap();
    Sameness::between(
        renders,
        &dir,
        ("HEAD~1", &before),
        ("HEAD", &after),
        &set(held),
        &set(sources),
        &asked,
    )
    .unwrap()
}

#[test]
fn files_that_hold_the_same_bytes_draw_the_same_and_draw_nothing() {
    let renders = says(&[]);
    let same = between(&renders, &[], &[A], &[TOKENS], &[A, TOKENS]);
    assert!(same.path(A) && same.path(TOKENS));
    assert_eq!(renders.asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_board_is_the_same_when_it_draws_alike_whatever_its_bytes() {
    let renders = says(&[A]);
    let same = between(&renders, &[A, B], &[A, B], &[], &[A, B]);
    assert!(same.path(A));
    assert!(!same.path(B));
    assert_eq!(renders.asked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_shared_file_is_the_same_only_when_every_board_of_the_change_draws_alike() {
    let all = between(
        &says(&[A, B]),
        &[TOKENS, CANVAS],
        &[A, B],
        &[TOKENS],
        &[A, B, TOKENS, CANVAS],
    );
    assert!(all.path(TOKENS) && all.path(CANVAS));
    let one = between(
        &says(&[A]),
        &[TOKENS, CANVAS],
        &[A, B],
        &[TOKENS],
        &[A, B, TOKENS, CANVAS],
    );
    assert!(!one.path(TOKENS) && !one.path(CANVAS));
    assert!(one.path(A) && one.path(B), "their bytes did not change");
}

#[test]
fn a_repo_whose_canvas_names_no_design_system_is_judged_by_bytes_and_draws_nothing() {
    let renders = says(&[A, B]);
    let same = between_in(&renders, false, &[A, CANVAS], &[A, B], &[], &[A, B, CANVAS]);
    assert!(!same.path(A) && !same.path(CANVAS));
    assert!(same.path(B), "its bytes did not change");
    assert_eq!(renders.asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_shared_file_of_a_change_that_holds_no_board_is_never_the_same() {
    let none = between(&says(&[]), &[TOKENS], &[], &[TOKENS], &[TOKENS]);
    assert!(!none.path(TOKENS));
}

#[test]
fn a_board_the_change_does_not_hold_is_the_same_only_by_its_bytes() {
    let renders = says(&[A, B]);
    let same = between(&renders, &[B], &[A], &[], &[A, B]);
    assert!(same.path(A));
    assert!(!same.path(B), "the person approved no drawing of B");
}

#[test]
fn nothing_draws_the_same_for_a_change_with_no_last_commit() {
    assert!(!Sameness::never().path(A));
}

#[test]
fn the_program_is_asked_for_each_place_by_its_flag_and_name() {
    let scratch = Scratch::new("design-drawn-program");
    let program = scratch.script(
        "draws",
        r#"echo "args: $*" >&2
echo "alike board A"
echo "differs component Row""#,
    );
    let asked = set(&[A, "design/system/components/Row/"]);
    let alike = ProgramRenders(program)
        .alike(&scratch.dir, "one", "two", &asked)
        .unwrap();
    assert_eq!(alike, set(&[A]));
}

#[test]
fn a_program_that_leaves_a_place_unanswered_or_fails_is_an_error() {
    let scratch = Scratch::new("design-drawn-silent");
    let silent = ProgramRenders(scratch.script("silent", "echo 'alike board A'"));
    let error = silent
        .alike(&scratch.dir, "one", "two", &set(&[A, B]))
        .unwrap_err();
    assert!(
        matches!(&error, crate::fail::Fail::Message(m) if m.contains(B) && m.contains("did not say")),
        "{error:?}"
    );
    let failing = ProgramRenders(scratch.script("failing", "echo 'no Chrome' >&2; exit 1"));
    let error = failing
        .alike(&scratch.dir, "one", "two", &set(&[A]))
        .unwrap_err();
    assert!(
        matches!(&error, crate::fail::Fail::Message(m) if m.contains("no Chrome")),
        "{error:?}"
    );
}
