use super::*;
use serde_json::json;

fn instance(component: &str, props: serde_json::Value) -> Instance {
    Instance {
        component: component.to_string(),
        props: serde_json::from_value(props).unwrap(),
        words: Vec::new(),
        in_row: false,
        draws: true,
        by_page: true,
    }
}

fn row(component: &str, props: serde_json::Value) -> Instance {
    Instance {
        in_row: true,
        ..instance(component, props)
    }
}

fn drawn(boards: Vec<(&str, Vec<Instance>)>, previews: Vec<(&str, Vec<Instance>)>) -> Drawn {
    Drawn {
        boards: boards
            .into_iter()
            .map(|(name, found)| (name.to_string(), found))
            .collect(),
        previews: previews
            .into_iter()
            .map(|(name, found)| (name.to_string(), found))
            .collect(),
    }
}

/// Each uncovered state as its line, without the boards.
fn listed(drawn: &Drawn) -> Vec<String> {
    uncovered(drawn)
        .iter()
        .map(|u| u.to_string().split(':').next().unwrap().to_string())
        .collect()
}

fn word(text: &str) -> Value {
    Value::Word(text.to_string())
}

#[test]
fn a_code_word_starts_lower_case_and_holds_no_space_or_punctuation() {
    for text in ["primary", "moveFailed", "expand_more", "us-east-1", "s3"] {
        assert!(code_word(text), "{text}");
    }
    for text in ["", "Library", "Set up", "5:03", "0.4", "a,b", "-x"] {
        assert!(!code_word(text), "{text}");
    }
}

#[test]
fn a_board_instance_shows_when_each_state_prop_has_its_value_in_some_row() {
    let previews = vec![(
        "Button",
        vec![
            row("Button", json!({"kind": "primary", "title": "Set up"})),
            row("Button", json!({"off": true, "title": "Set up"})),
            row("Button", json!({"title": "Set up"})),
        ],
    )];
    let boards = vec![(
        "mac.one",
        vec![instance(
            "Button",
            json!({"kind": "primary", "off": true, "title": "Done"}),
        )],
    )];
    assert_eq!(listed(&drawn(boards, previews)), Vec::<String>::new());
}

#[test]
fn a_value_no_row_shows_is_listed_with_the_boards_that_draw_it() {
    let previews = vec![("Button", vec![row("Button", json!({"kind": "primary"}))])];
    let boards = vec![
        (
            "mac.one",
            vec![instance("Button", json!({"kind": "borderless"}))],
        ),
        (
            "mac.two",
            vec![
                instance("Button", json!({"kind": "borderless", "tinted": true})),
                instance("Button", json!({"kind": "borderless"})),
            ],
        ),
    ];
    let found = uncovered(&drawn(boards, previews));
    assert_eq!(
        found,
        [
            Uncovered {
                component: "Button".to_string(),
                gap: Gap::Value {
                    prop: "kind".to_string(),
                    value: word("borderless"),
                },
                boards: vec!["mac.one".to_string(), "mac.two".to_string()],
            },
            Uncovered {
                component: "Button".to_string(),
                gap: Gap::Value {
                    prop: "tinted".to_string(),
                    value: Value::On,
                },
                boards: vec!["mac.two".to_string()],
            },
        ]
    );
}

#[test]
fn an_unset_prop_needs_a_row_that_leaves_it_unset() {
    let previews = vec![(
        "Button",
        vec![
            row("Button", json!({"kind": "primary"})),
            row("Button", json!({"kind": "icon"})),
        ],
    )];
    let boards = vec![(
        "mac.one",
        vec![instance("Button", json!({"kind": "primary", "off": false}))],
    )];
    assert_eq!(
        listed(&drawn(boards.clone(), previews)),
        Vec::<String>::new()
    );
    let previews = vec![("Button", vec![row("Button", json!({"kind": "primary"}))])];
    let boards = vec![("mac.one", vec![instance("Button", json!({}))])];
    assert_eq!(listed(&drawn(boards, previews)), ["Button kind unset"]);
}

#[test]
fn numbers_words_and_strings_the_component_draws_as_words_are_no_state() {
    let mut drawn_label = row("Button", json!({"label": "copy"}));
    drawn_label.words = vec!["label".to_string()];
    let previews = vec![(
        "Button",
        vec![drawn_label, row("Button", json!({"width": 40}))],
    )];
    let boards = vec![(
        "mac.one",
        vec![instance(
            "Button",
            json!({"width": 300, "title": "Delete this album?", "label": "check", "held": "0-0.3"}),
        )],
    )];
    assert_eq!(listed(&drawn(boards, previews)), Vec::<String>::new());
}

#[test]
fn only_the_components_own_instances_in_rows_show_its_states_and_one_with_no_preview_is_left_out() {
    let previews = vec![
        (
            "Button",
            vec![
                instance("Button", json!({"kind": "heading"})),
                row("Notice", json!({"tone": "warning"})),
                row("Button", json!({})),
            ],
        ),
        (
            "Notice",
            vec![instance("Notice", json!({"tone": "warning"}))],
        ),
    ];
    let boards = vec![(
        "mac.one",
        vec![
            instance("Button", json!({"kind": "heading"})),
            instance("Notice", json!({"tone": "warning"})),
            instance("Sym", json!({"name": "copy"})),
        ],
    )];
    assert_eq!(
        listed(&drawn(boards, previews)),
        [
            "Button kind \"heading\"",
            "Notice, which its preview draws in no row"
        ]
    );
}

#[test]
fn a_line_names_three_boards_and_counts_the_rest() {
    let state = |boards: &[&str]| Uncovered {
        component: "Button".to_string(),
        gap: Gap::Value {
            prop: "size".to_string(),
            value: word("alert"),
        },
        boards: boards.iter().map(ToString::to_string).collect(),
    };
    assert_eq!(
        state(&["a", "b", "c", "d", "e"]).to_string(),
        "Button size \"alert\": a, b, c and 2 more boards"
    );
    assert_eq!(
        state(&["a", "b", "c", "d"]).to_string(),
        "Button size \"alert\": a, b, c and 1 more board"
    );
    assert_eq!(state(&["a"]).to_string(), "Button size \"alert\": a");
}

#[test]
fn a_change_adds_the_states_its_base_does_not_draw() {
    let state = |prop: &str, boards: &[&str]| Uncovered {
        component: "Button".to_string(),
        gap: Gap::Value {
            prop: prop.to_string(),
            value: Value::On,
        },
        boards: boards.iter().map(ToString::to_string).collect(),
    };
    let before = [state("tinted", &["a"])];
    let now = vec![state("tinted", &["a", "b"]), state("muted", &["b"])];
    assert_eq!(added(now, &before), [state("muted", &["b"])]);
}

#[test]
fn the_last_line_counts_the_states_and_their_components() {
    let state = |component: &str| Uncovered {
        component: component.to_string(),
        gap: Gap::NoRow,
        boards: vec!["a".to_string()],
    };
    assert_eq!(
        lines(&[], None),
        ["Every component state that a board draws shows in a preview row."]
    );
    assert_eq!(
        lines(&[state("Button"), state("Notice")], None)
            .last()
            .unwrap(),
        "The boards draw 2 component states of 2 components that no preview row shows."
    );
    let added = lines(&[state("Button")], Some("origin/main"));
    assert_eq!(added.len(), 2);
    assert_eq!(
        added[1],
        "The boards draw 1 component state of 1 component that no preview row shows and the listing at origin/main does not hold: add each state as a row of its component's preview."
    );
}

#[test]
fn a_class_name_and_an_icon_name_are_no_state() {
    let previews = vec![("Button", vec![row("Button", json!({"kind": "primary"}))])];
    let boards = vec![(
        "mac.one",
        vec![instance(
            "Button",
            json!({"kind": "primary", "className": "bae-accent", "icon": "copy", "leadingIcon": "add", "name": "check"}),
        )],
    )];
    assert_eq!(listed(&drawn(boards, previews)), Vec::<String>::new());
}

#[test]
fn an_instance_that_draws_nothing_is_no_state() {
    let previews = vec![("Button", vec![row("Button", json!({}))])];
    let boards = vec![(
        "mac.one",
        vec![Instance {
            draws: false,
            ..instance("Button", json!({"kind": "primary"}))
        }],
    )];
    assert_eq!(listed(&drawn(boards, previews)), Vec::<String>::new());
}

#[test]
fn props_a_component_gives_to_a_component_it_draws_are_no_state() {
    let previews = vec![("Chip", vec![row("Chip", json!({}))])];
    let boards = vec![(
        "mac.one",
        vec![Instance {
            by_page: false,
            ..instance("Chip", json!({"tone": "accent"}))
        }],
    )];
    assert_eq!(listed(&drawn(boards, previews)), Vec::<String>::new());
}

#[test]
fn the_components_of_each_page_come_from_the_picture_folder_by_its_hash() {
    let repo = tempfile::tempdir().unwrap();
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../design");
    copy_tree(&here, &repo.path().join("design"));
    // The design system the boards load, which the repo's own canvas.json
    // does not name.
    let canvas = repo.path().join("design/mocks/canvas.json");
    let text = std::fs::read_to_string(&canvas).unwrap();
    let text = text.replacen(
        "\"attachments\": {},",
        "\"attachments\": {}, \"designSystems\": [{\"namespace\": \"demo\"}],",
        1,
    );
    std::fs::write(&canvas, text).unwrap();
    // One preview with a row, and the marks that name its rows.
    let preview = repo
        .path()
        .join(boards::preview::COMPONENTS)
        .join("MacRow")
        .join("preview.html");
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    std::fs::write(
        &preview,
        "<html><head></head><body><div class='preview-row'><p>a row</p></div></body></html>",
    )
    .unwrap();
    std::fs::write(
        repo.path().join(".app-build"),
        "shots-preview .preview-row .preview-name .preview-cell\n",
    )
    .unwrap();
    let folders = boards::Folders {
        cache: crate::render::fixture::cache(),
        fonts: None,
        volume: crate::render::fixture::volume(),
    };
    let folder = tempfile::tempdir().unwrap();
    let pictures = Pictures::new(folder.path().to_path_buf(), std::time::SystemTime::now());
    let store = Store {
        capture_version: "c0c0c0c0c0c0c0c0",
        pictures: Some(&pictures),
    };
    let first = read(repo.path(), None, &folders, &store).unwrap();
    let kept: Vec<_> = std::fs::read_dir(folder.path().join("boards"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(!kept.is_empty(), "every page's components are kept");
    let second = read(repo.path(), None, &folders, &store).unwrap();
    assert_eq!(first, second, "the kept components give the same states");
    std::fs::write(&kept[0], "not json").unwrap();
    let error = read(repo.path(), None, &folders, &store).unwrap_err();
    assert!(error.contains("the instances kept for"), "{error}");
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
