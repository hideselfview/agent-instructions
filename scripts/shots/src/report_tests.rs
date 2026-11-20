use super::*;
use crate::compare::normalize::Entry;
use crate::lint::Problem;
use crate::lists::Lists;
use crate::names::Name;
use crate::shots::tests::{LIST, png};
use crate::shots::{ElementList, Role};

fn entry(index: usize, words: &str, kind: Role) -> Entry {
    Entry {
        index,
        folded: Vec::new(),
        kind,
        words: words.to_string(),
        value: String::new(),
        enabled: true,
        selected: false,
    }
}

fn header() -> Header {
    Header {
        commit: "abc123def".to_string(),
        base: Some("origin/main".to_string()),
        runtime: "version v1, canvas v2 published 2026-11-11T23:20:00Z".to_string(),
        chrome: "Chrome/157.0.8103.78".to_string(),
    }
}

/// A run of one failing pair, one with a warning and one skipped, with its
/// pictures written under `dir`.
fn outcome(dir: &Path, words: &str) -> Outcome {
    let name = Name::parse("mac.welcome.a").unwrap();
    let other = Name::parse("mac.welcome.b").unwrap();
    let pictures = dir.join("in");
    std::fs::create_dir_all(&pictures).unwrap();
    std::fs::write(pictures.join("board.png"), png(400, 190)).unwrap();
    std::fs::write(pictures.join("shot.png"), png(800, 380)).unwrap();
    let pair = |name: &Name, findings, warnings| Checked {
        name: name.clone(),
        findings,
        warnings,
        against_base: Some(Standing::New),
        board: ElementList::parse(LIST).unwrap(),
        shot: ElementList::parse(LIST).unwrap(),
        pairs: vec![(2, 2)],
        offset: (0.0, 0.0),
        board_png: pictures.join("board.png"),
        shot_png: pictures.join("shot.png"),
    };
    Outcome {
        runtime: "version v1, canvas v2 published 2026-11-11T23:20:00Z".to_string(),
        checked: vec![
            pair(
                &name,
                vec![
                    Finding::Missing(entry(2, words, Role::Button)),
                    Finding::Extra(entry(0, "Beta", Role::Text)),
                ],
                Vec::new(),
            ),
            pair(
                &other,
                Vec::new(),
                vec![Warning::Moved {
                    entry: entry(2, "Continue", Role::Button),
                    dx: 0.0,
                    dy: 4.0,
                }],
            ),
        ],
        skipped: vec![Skipped {
            name: Name::parse("mac.welcome.c").unwrap(),
            findings: Vec::new(),
            warnings: vec!["“Theme” moved 3 pt down in the shot".to_string()],
            against_base: None,
        }],
        problems: vec![Problem {
            name: "mac.welcome.gone".to_string(),
            declared: true,
            sentence: "shot 'mac.welcome.gone' has no board: x is missing".to_string(),
        }],
        lists: Lists {
            to_build: vec!["mac.welcome.todo".to_string()],
            to_update: Vec::new(),
            to_delete: vec!["mac.welcome.gone".to_string()],
        },
        not_taken: vec![Name::parse("mac.settings.toolbar").unwrap()],
        gate: crate::verify::Gate::EveryPair,
        base_empty: false,
    }
}

#[test]
fn the_page_holds_each_finding_sentence_and_every_picture_it_names_exists() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = outcome(dir.path(), "Set up");
    let out = dir.path().join("report");
    write(&out, &outcome, &header()).unwrap();
    let html = std::fs::read_to_string(out.join("index.html")).unwrap();
    for sentence in [
        "Shot is missing the button “Set up”",
        "Shot has the text “Beta”, which the board does not",
        "“Continue” moved 4 pt down in the shot",
        "“Theme” moved 3 pt down in the shot",
        "shot 'mac.welcome.gone' has no board",
    ] {
        let sentence = esc(sentence);
        assert!(html.contains(&sentence), "{sentence} is not in the page");
    }
    for list in ["mac.welcome.todo", "mac.welcome.gone"] {
        assert!(html.contains(list));
    }
    let source = regex::Regex::new(r#"<img src="([^"]+)""#).unwrap();
    let named: Vec<_> = source
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(named.len(), 4, "{named:?}");
    for picture in named {
        assert!(
            out.join(&picture).is_file(),
            "{picture} is not beside the page"
        );
    }
    assert!(html.contains("abc123def") && html.contains("2026-11-11T23:20:00Z"));
    assert!(html.contains("Chrome/157.0.8103.78"));
    assert!(html.contains("new against the base"));
    assert!(out.join(crate::findings_file::FILE).is_file());
}

#[test]
fn words_are_escaped_in_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = outcome(dir.path(), "<b>\"Go\" & stop</b>");
    let html = page(&outcome, &header());
    assert!(html.contains("&lt;b&gt;&quot;Go&quot; &amp; stop&lt;/b&gt;"));
    assert!(!html.contains("<b>"));
}

#[test]
fn failures_come_before_warnings_and_confirmed_pairs() {
    let dir = tempfile::tempdir().unwrap();
    let html = page(&outcome(dir.path(), "Set up"), &header());
    let at = |text: &str| html.find(text).unwrap();
    assert!(at("<h2>Failures") < at("<h2>Warnings"));
    assert!(at("<h2>Warnings") < at("<h2>Confirmed"));
}

#[test]
fn the_summary_has_counts_the_failures_and_the_lists() {
    let dir = tempfile::tempdir().unwrap();
    let text = summary(&outcome(dir.path(), "Set up"), &header());
    assert!(
        text.contains("2 failing, 1 with warnings, 1 confirmed (1 skipped on the record)"),
        "{text}"
    );
    assert!(
        text.contains(
            "- `mac.welcome.a` (new against the base): Shot is missing the button “Set up”"
        )
    );
    assert!(text.contains("- shot 'mac.welcome.gone' has no board: x is missing"));
    assert!(text.contains("### Boards with no shot (to build)\n\n- `mac.welcome.todo`"));
    assert!(!text.contains("<img"));
}

#[test]
fn the_page_and_the_summary_name_the_shots_not_taken_and_why() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = outcome(dir.path(), "Set up");
    let html = page(&outcome, &header());
    assert!(html.contains("mac.settings.toolbar"), "{html}");
    assert!(html.contains(crate::capture::NEEDS_WINDOW));
    let text = summary(&outcome, &header());
    assert!(
        text.contains(
            "### Not taken here\n\n- `mac.settings.toolbar`: not taken here (needs a window)"
        ),
        "{text}"
    );
    assert!(
        !text.contains("1 failing"),
        "a shot not taken is not a failure: {text}"
    );
}
