use super::*;
use crate::fail::Fail;
use crate::lists::design_command::run_in_session;
use crate::lists::fixtures::{NOON, Refusals, design_drawn, row, stamp, v};
use crate::lists::project::NewProject;
use crate::lists::store::Store;
use crate::testing::{Scratch, git, repo};
use crate::tool::Tool;
use serde_json::json;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

/// A scratch repo `bae` whose one commit holds boards A and B, and the store
/// of project `demo` whose design places are the boards <glob> names in
/// bae, with open design change 1 drawn in that repo.
struct World {
    _scratch: Scratch,
    tool: Tool,
    dir: std::path::PathBuf,
    store: Store,
}

fn world(name: &str) -> World {
    world_of(name, "design/mocks/*.dc.html")
}

fn world_of(name: &str, glob: &str) -> World {
    let scratch = Scratch::new(name);
    let dir = scratch.dir.join("bae");
    repo(&dir, "");
    let mocks = dir.join("design/mocks");
    std::fs::create_dir_all(&mocks).unwrap();
    for board in ["A", "B"] {
        std::fs::write(
            mocks.join(format!("{board}.dc.html")),
            format!("<x-dc><p>{board}</p></x-dc>"),
        )
        .unwrap();
    }
    git(&dir, &["add", "design"]);
    git(&dir, &["commit", "-q", "-m", "boards"]);
    let tool = scratch.tool(0);
    let store = Store::named(&tool.places, "demo").unwrap();
    let new = NewProject {
        name: "demo".into(),
        canvas: false,
        repos: vec!["bae".into(), "coven".into()],
        places: std::collections::BTreeMap::from([("bae".to_string(), vec![glob.to_string()])]),
        sources: Default::default(),
        specs: Default::default(),
    };
    store.create(&new, &stamp(NOON), "lists init demo").unwrap();
    let worktree = dir.display().to_string();
    store
        .write(&stamp(NOON), "design record", |db| {
            design_drawn(db, &worktree, &[], None);
            Ok(())
        })
        .unwrap();
    World {
        _scratch: scratch,
        tool,
        dir,
        store,
    }
}

impl World {
    /// Runs `design <args>` in project demo as `tester`.
    fn run(&self, args: &[&str]) -> Result<String> {
        let mut args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        args.extend(["--by", "tester", "--project", "demo"].map(String::from));
        run_in_session(&self.tool, &args, || Ok(None))
    }

    fn variants(&self) -> Option<Variants> {
        self.store
            .load()
            .unwrap()
            .design(Id(1))
            .and_then(|d| d.variants.clone())
    }

    fn head(&self) -> String {
        crate::lists::design_paths::full_sha(&self.dir, "HEAD").unwrap()
    }
}

const TWO: &str = r#"[{"storage": "dropbox"}, {"storage": "s3", "keyOwner": "mine", "rows": 3}]"#;

#[test]
fn variants_keep_the_board_selector_theme_commit_and_each_set_of_props_and_show_prints_them() {
    let w = world("variants-record");
    let out = w
        .run(&[
            "variants",
            "1",
            "--board",
            "A",
            "--selector",
            ".fields",
            "--props",
            TWO,
            "--theme",
            "light",
        ])
        .unwrap();
    let head = w.head();
    assert!(
        out.contains("design change #1 shows 2 variants of design/mocks/A.dc.html")
            && out.contains(&head),
        "{out}"
    );
    let variants = w.variants().unwrap();
    assert_eq!(
        variants,
        Variants {
            board: "design/mocks/A.dc.html".into(),
            selector: ".fields".into(),
            theme: Theme::Light,
            tip: head.clone(),
            props: vec![
                json!({"storage": "dropbox"}).as_object().unwrap().clone(),
                json!({"storage": "s3", "keyOwner": "mine", "rows": 3})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        }
    );
    let shown = w.run(&["show", "1"]).unwrap();
    assert!(
        shown.contains(&format!(
            "  variants of design/mocks/A.dc.html (light) at {head}, cropped to .fields:\n    storage: dropbox\n    keyOwner: mine, rows: 3, storage: s3\n"
        )),
        "{shown}"
    );
}

#[test]
fn variants_again_replace_the_ones_the_change_had_and_clear_deletes_them() {
    let w = world("variants-replace");
    w.run(&[
        "variants",
        "1",
        "--board",
        "A",
        "--selector",
        ".fields",
        "--props",
        TWO,
    ])
    .unwrap();
    // A later commit, named with --at, draws B with one set of props.
    let first = w.head();
    std::fs::write(w.dir.join("design/mocks/B.dc.html"), "<x-dc>B2</x-dc>").unwrap();
    git(&w.dir, &["commit", "-q", "-a", "-m", "B again"]);
    git(&w.dir, &["checkout", "-q", &first]);
    w.run(&[
        "variants",
        "1",
        "--board",
        "design/mocks/B.dc.html",
        "--selector",
        "p",
        "--props",
        "[{}]",
        "--at",
        "main",
    ])
    .unwrap();
    let variants = w.variants().unwrap();
    assert_eq!(variants.board, "design/mocks/B.dc.html");
    assert_eq!(variants.theme, Theme::Dark, "dark unless asked");
    assert_ne!(variants.tip, first);
    assert_eq!(variants.props, vec![Props::new()]);
    assert_eq!(label(&variants.props[0]), "No props");

    assert!(
        w.run(&["variants", "1", "--clear"])
            .unwrap()
            .contains("deleted the variants of design change #1")
    );
    assert_eq!(w.variants(), None);
    assert!(
        w.run(&["variants", "1", "--clear"])
            .unwrap()
            .contains("has no variants")
    );
    assert!(
        message(w.run(&["variants", "1", "--clear", "--board", "A"])).contains("takes no --board")
    );
}

#[test]
fn variants_refuse_a_board_that_is_no_place_or_no_file_bad_props_and_a_closed_change() {
    let w = world("variants-refuse");
    let run = |board: &str, props: &str| {
        message(w.run(&[
            "variants",
            "1",
            "--board",
            board,
            "--selector",
            ".x",
            "--props",
            props,
        ]))
    };
    assert!(run("Nope", TWO).contains("is no file at commit"));
    assert!(run("../A", TWO).contains("is no board"));
    for props in ["{}", "[1]", "nope"] {
        assert!(
            run("A", props).contains("--props is a JSON array of objects"),
            "{props}"
        );
    }
    assert!(run("A", "[]").contains("gives no props"));
    let blank = message(w.run(&[
        "variants",
        "1",
        "--board",
        "A",
        "--selector",
        " ",
        "--props",
        TWO,
    ]));
    assert!(blank.contains("--selector is empty"), "{blank}");
    assert!(
        message(w.run(&[
            "variants",
            "1",
            "--board",
            "A",
            "--selector",
            ".x",
            "--props",
            TWO,
            "--at",
            "nope"
        ]))
        .contains("find commit nope")
    );
    assert_eq!(w.variants(), None, "nothing was kept");
    w.store
        .write(&stamp(NOON), "design drop 1", |db| {
            crate::lists::design::drop(db, Id(1), "not needed", &stamp(NOON))
        })
        .unwrap();
    assert!(run("A", TWO).contains("dropped design change #1"));
    assert!(message(w.run(&["variants", "1", "--clear"])).contains("dropped design change #1"));
}

#[test]
fn a_board_that_is_no_board_place_of_the_repo_is_refused() {
    let w = world_of("variants-no-place", "design/mocks/mac.*.dc.html");
    let refused = message(w.run(&[
        "variants",
        "1",
        "--board",
        "A",
        "--selector",
        ".x",
        "--props",
        TWO,
    ]));
    assert!(
        refused.contains("design/mocks/A.dc.html is no board place of repo bae's design places"),
        "{refused}"
    );
}

#[test]
fn landing_or_dropping_a_change_deletes_its_variants_and_an_amend_leaves_them() {
    for close in ["landed", "dropped"] {
        let w = world("variants-close");
        w.run(&[
            "variants",
            "1",
            "--board",
            "A",
            "--selector",
            ".x",
            "--props",
            TWO,
        ])
        .unwrap();
        // What an amend writes of the change leaves its variants as they are.
        w.store
            .write(&stamp(NOON), "design amend 1", |db| {
                let change = store::read(db)?.design(Id(1)).unwrap().clone();
                crate::lists::design::save(db, &change)
            })
            .unwrap();
        assert!(w.variants().is_some(), "{close}");
        let head = w.head();
        w.store
            .write(&stamp(NOON), "design close 1", |db| match close {
                "landed" => crate::lists::design::landed(
                    db,
                    Id(1),
                    "bae",
                    std::slice::from_ref(&head),
                    &stamp(NOON),
                )
                .map(|_| ()),
                _ => crate::lists::design::drop(db, Id(1), "not needed", &stamp(NOON)),
            })
            .unwrap();
        let db = crate::db::open_reader(&w.store.database())
            .unwrap()
            .unwrap();
        let rows: i64 = db
            .query_row(
                "SELECT (SELECT COUNT(*) FROM design_variants) + (SELECT COUNT(*) FROM design_variant)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "{close}");
        let lists = w.store.load().unwrap();
        let crate::lists::Record::Design(closed) = &lists.closed_record(Id(1)).unwrap().record
        else {
            panic!("a design change")
        };
        assert_eq!(closed.variants, None, "{close}");
    }
}

#[test]
fn every_named_constraint_of_the_variants_tables_refuses_its_row() {
    let db = crate::lists::fixtures::db();
    let id = crate::lists::fixtures::design(&db);
    let mut refused = Refusals::new(include_str!("design_variants.sql"));
    let variants = [
        ("design_id", v(id.0)),
        ("board", v("design/mocks/A.dc.html")),
        ("selector", v(".x")),
        ("theme", v("dark")),
        ("tip", v("a".repeat(40))),
    ];
    for board in [
        "design/mocks/A",
        "design/mocks/A.dc.html#x",
        "design/mocks/",
    ] {
        refused.check(
            "design_variants_board_is_a_board_file",
            plant(
                &db,
                "design_variants",
                &row(&variants, &[("board", v(board))]),
            ),
        );
    }
    refused.check(
        "design_variants_selector_says_something",
        plant(
            &db,
            "design_variants",
            &row(&variants, &[("selector", v(" "))]),
        ),
    );
    refused.check(
        "design_variants_theme_known",
        plant(
            &db,
            "design_variants",
            &row(&variants, &[("theme", v("dim"))]),
        ),
    );
    for tip in ["abc1234".to_string(), "g".repeat(40)] {
        refused.check(
            "design_variants_tip_is_a_commit",
            plant(&db, "design_variants", &row(&variants, &[("tip", v(tip))])),
        );
    }
    plant(&db, "design_variants", &variants).unwrap();
    let variant = [
        ("design_id", v(id.0)),
        ("position", v(0)),
        ("props", v("{}")),
    ];
    refused.check(
        "design_variant_position_not_negative",
        plant(
            &db,
            "design_variant",
            &row(&variant, &[("position", v(-1))]),
        ),
    );
    for props in ["[]", "nope", "1"] {
        refused.check(
            "design_variant_props_are_an_object",
            plant(
                &db,
                "design_variant",
                &row(&variant, &[("props", v(props))]),
            ),
        );
    }
    refused.check_as(
        "design_variant_is_of_the_variants",
        "FOREIGN KEY",
        plant(
            &db,
            "design_variant",
            &row(&variant, &[("design_id", v(id.0 + 1))]),
        ),
    );
    plant(&db, "design_variant", &variant).unwrap();
    refused.done();
}
