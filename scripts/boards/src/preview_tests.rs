use super::*;
use crate::testing::{Scratch, commit, git, repo};

/// A repo with components `Row` (a card 120 high), `Bare` (a preview whose
/// first line gives no height), `Mac-Row` (a preview in a folder whose name
/// is no component name) and `notes` (a folder with no preview).
fn components(scratch: &Scratch) -> std::path::PathBuf {
    let dir = scratch.dir.join("repo");
    repo(&dir);
    let folder = dir.join(COMPONENTS);
    for (name, text) in [
        (
            "Row",
            "<!-- @dsCard group=\"Mac\" height=120 -->\n<!doctype html>",
        ),
        ("Bare", "<!doctype html>"),
        ("Mac-Row", "<!-- @dsCard height=90 -->\n<!doctype html>"),
    ] {
        std::fs::create_dir_all(folder.join(name)).unwrap();
        std::fs::write(folder.join(name).join("preview.html"), text).unwrap();
    }
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("notes/README.md"), "no preview").unwrap();
    dir
}

#[test]
fn each_component_with_a_preview_has_a_card_of_its_height_at_a_folder_and_a_commit() {
    let scratch = Scratch::new("preview-cards");
    let dir = components(&scratch);
    let at = commit(&dir);
    let cache = scratch.dir.join("cache");
    for from in [
        Source::Dir(&dir),
        Source::Commit {
            repo: &dir,
            rev: &at,
        },
    ] {
        let cards = cards(&from, &cache).unwrap();
        assert_eq!(
            cards,
            [
                Card {
                    name: "Bare".into(),
                    height: Err("The first line of design/system/components/Bare/preview.html gives its card no height, as <!-- @dsCard height=N --> does.".into()),
                },
                Card {
                    name: "Mac-Row".into(),
                    height: Err("design/system/components/Mac-Row holds a preview.html, and its folder's name is no component name: a component's name is letters and digits.".into()),
                },
                Card {
                    name: "Row".into(),
                    height: Ok(120),
                },
            ]
        );
    }
    let empty = git(&dir, &["rev-list", "--max-parents=0", "HEAD"]);
    let none = cards(
        &Source::Commit {
            repo: &dir,
            rev: &empty,
        },
        &cache,
    )
    .unwrap();
    assert!(none.is_empty(), "a commit with no components has no cards");
}

#[test]
fn the_loads_go_right_after_the_head_tag_and_a_page_with_no_head_has_none() {
    assert_eq!(
        with_loads(
            "<html><header></header><HEAD lang=\"en\"><title>x</title></head>",
            "<script></script>"
        )
        .unwrap(),
        "<html><header></header><HEAD lang=\"en\"><script></script><title>x</title></head>"
    );
    assert_eq!(with_loads("<body><header></header></body>", "<x>"), None);
}

/// A repo whose components `Row`, `Box`, `Tip` and `Stray` have previews,
/// and whose groups.json lists `Box` in a group of its own, `Row` and
/// `Tip` in subgroups, `Row` twice, a `Gone` that has no preview and an
/// empty subgroup, and leaves out `Stray`.
#[test]
fn contents_puts_each_card_in_its_listed_group_and_names_each_disagreement() {
    let scratch = Scratch::new("preview-contents");
    let dir = scratch.dir.join("repo");
    repo(&dir);
    for name in ["Row", "Box", "Tip", "Stray"] {
        let folder = dir.join(COMPONENTS).join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("preview.html"),
            "<!-- @dsCard group=\"G\" height=50 -->\n<!doctype html>",
        )
        .unwrap();
    }
    let cache = scratch.dir.join("cache");
    let error = contents(&Source::Dir(&dir), &cache).unwrap_err().text();
    assert!(
        error.contains("design/system/groups.json is not there"),
        "{error}"
    );
    std::fs::write(
        dir.join(GROUPS),
        serde_json::json!([
            { "name": "Boxes", "about": "What holds rows.", "components": ["Box"] },
            { "name": "Rows", "about": "The rows.", "subgroups": [
                { "name": "Plain", "about": "Plain rows.", "components": ["Row", "Gone"] },
                { "name": "Tips", "about": "Rows with a tip.", "components": ["Tip", "Row"] },
                { "name": "Empty", "about": "Nothing.", "components": [] },
            ] },
        ])
        .to_string(),
    )
    .unwrap();
    let card = |name: &str| Card {
        name: name.into(),
        height: Ok(50),
    };
    let found = contents(&Source::Dir(&dir), &cache).unwrap();
    assert_eq!(
        found,
        Contents {
            groups: vec![
                Group {
                    name: "Boxes".into(),
                    about: "What holds rows.".into(),
                    cards: vec![card("Box")],
                    subgroups: vec![],
                },
                Group {
                    name: "Rows".into(),
                    about: "The rows.".into(),
                    cards: vec![],
                    subgroups: vec![
                        Subgroup {
                            name: "Plain".into(),
                            about: "Plain rows.".into(),
                            cards: vec![card("Row")],
                        },
                        Subgroup {
                            name: "Tips".into(),
                            about: "Rows with a tip.".into(),
                            cards: vec![card("Tip")],
                        },
                        Subgroup {
                            name: "Empty".into(),
                            about: "Nothing.".into(),
                            cards: vec![],
                        },
                    ],
                },
            ],
            problems: vec![
                "design/system/groups.json lists Gone in the subgroup Plain of Rows, and no folder of design/system/components holds a preview of that name.".into(),
                "design/system/groups.json lists Row in the subgroup Plain of Rows and again in the subgroup Tips of Rows.".into(),
                "design/system/groups.json lists no components in the subgroup Empty of Rows.".into(),
                "design/system/groups.json lists no group for Stray.".into(),
            ],
        }
    );
    std::fs::write(
        dir.join(GROUPS),
        "[{\"name\": \"G\", \"about\": \"x\", \"cards\": []}]",
    )
    .unwrap();
    let error = contents(&Source::Dir(&dir), &cache).unwrap_err().text();
    assert!(error.contains("unknown field `cards`"), "{error}");
    std::fs::remove_dir_all(dir.join(COMPONENTS)).unwrap();
    let none = contents(&Source::Dir(&dir), &cache).unwrap();
    assert!(none.groups.is_empty() && none.problems.is_empty());
}
