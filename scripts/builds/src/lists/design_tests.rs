use super::*;
use crate::fail::Fail;
use crate::lists::design_command::run_in_session;
use crate::lists::fixtures::{NOON, stamp};
use crate::lists::project::NewProject;
use crate::lists::store::Store;
use crate::testing::{Scratch, git, repo};
use crate::tool::Tool;
use serde_json::json;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
    }
}

/// A scratch repo `bae` with a base commit, and the store of project `demo`
/// over repos bae and coven with bae's design places.
struct World {
    scratch: Scratch,
    tool: Tool,
    dir: std::path::PathBuf,
    store: Store,
}

fn world(name: &str) -> World {
    let scratch = Scratch::new(name);
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    // The canvas the boards load a design system from, so the shots checker
    // can draw them.
    std::fs::create_dir_all(dir.join("design/mocks")).unwrap();
    std::fs::write(
        dir.join("design/mocks/canvas.json"),
        r#"{"designSystems": [{"namespace": "demo"}], "boards": {}}"#,
    )
    .unwrap();
    git(&dir, &["add", "design/mocks/canvas.json"]);
    git(&dir, &["commit", "-q", "--amend", "--no-edit"]);
    git(&dir, &["tag", "base"]);
    // The repo's main as its origin has it: what a branch's commits and its
    // rules review are read past.
    git(&dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    let tool = scratch.tool(0);
    let store = Store::named(&tool.places, "demo").unwrap();
    let globs = |globs: &[&str]| {
        std::collections::BTreeMap::from([(
            "bae".to_string(),
            globs.iter().map(|g| g.to_string()).collect(),
        )])
    };
    let new = NewProject {
        name: "demo".into(),
        canvas: true,
        repos: vec!["bae".into(), "coven".into()],
        places: globs(&[
            "design/mocks/*.dc.html",
            "design/mocks/canvas.json",
            "design/system/components/*/",
        ]),
        sources: globs(&[
            "design/system/components/src/",
            "design/system/tokens.json",
            "design/system/tokens.css",
        ]),
        specs: globs(&["spec/*.md"]),
    };
    store.create(&new, &stamp(NOON), "lists init demo").unwrap();
    store
        .write(&stamp(NOON), "area add Alpha", |db| {
            crate::lists::area::add_area(db, "Alpha", None)
        })
        .unwrap();
    store
        .write(&stamp(NOON), "area add Beta", |db| {
            crate::lists::area::add_area(db, "Beta", None)
        })
        .unwrap();
    World {
        scratch,
        tool,
        dir,
        store,
    }
}

impl World {
    fn commit(&self, files: &[&str]) -> String {
        for path in files {
            let file = self.dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, format!("{path} at {}", self.sha("HEAD"))).unwrap();
            git(&self.dir, &["add", path]);
        }
        git(
            &self.dir,
            &["commit", "-q", "-m", &format!("change {}", files.join(" "))],
        );
        self.sha("HEAD")
    }

    fn sha(&self, rev: &str) -> String {
        crate::lists::design_paths::full_sha(&self.dir, rev).unwrap()
    }

    fn run(&self, command: &str) -> Result<String> {
        self.run_args(command.split_whitespace().map(str::to_string).collect())
    }

    fn run_args(&self, mut args: Vec<String>) -> Result<String> {
        args.extend(["--by".to_string(), "tester".to_string()]);
        // show and list find the project from the directory the command runs
        // in, which is no repo of the project here.
        if matches!(
            args.first().map(String::as_str),
            Some("show" | "list" | "approve" | "drop" | "title")
        ) {
            args.extend(["--project".to_string(), "demo".to_string()]);
        }
        // No session's inbox: the test process's own environment may name a
        // live session's, which a test never keeps.
        run_in_session(&self.tool, &args, || Ok(None))
    }

    fn request(&self, body: serde_json::Value) -> Result<String> {
        let file = self.scratch.dir.join("request.json");
        std::fs::write(&file, body.to_string()).unwrap();
        self.run(&format!("record --request {}", file.display()))
    }

    fn amend(&self, id: u32, body: serde_json::Value) -> Result<String> {
        let file = self.scratch.dir.join("amend.json");
        std::fs::write(&file, body.to_string()).unwrap();
        self.run(&format!("amend {id} --request {}", file.display()))
    }

    fn approve(&self, id: u32) {
        self.run(&format!("approve {id}")).unwrap();
    }

    fn state(&self) -> Lists {
        self.store.load().unwrap()
    }

    fn bytes(&self) -> String {
        crate::lists::fixtures::state_of(&self.store)
    }

    fn worktree(&self) -> String {
        self.dir.display().to_string()
    }

    /// Makes the shots checker of this world's builds program say that every
    /// board and component preview asked about draws as <verdict> (`alike` or
    /// `differs`), whatever the files' bytes.
    fn draws(&mut self, verdict: &str) {
        self.tool.program = self.scratch.shots_script("draws", verdict);
    }
}

fn draft(title: &str) -> serde_json::Value {
    json!({ "title": title, "area": "Alpha", "details": "do it", "for": "coordinator", "waits": [] })
}

/// A review page holding <boards>, each in the dark with one circle around
/// its root.
fn review_of(boards: &[&str]) -> serde_json::Value {
    let boards: Vec<serde_json::Value> = boards
        .iter()
        .map(|b| json!({ "board": b, "caption": format!("the {b}"), "theme": "dark", "circles": [{ "selector": "#r" }] }))
        .collect();
    json!({ "heading": "What changed", "boards": boards })
}

/// A request for `base..HEAD` answering <places>, with a place on the review page for each board among them.
fn base_request(world: &World, places: serde_json::Value) -> serde_json::Value {
    let boards: Vec<&str> = places
        .as_object()
        .unwrap()
        .keys()
        .filter(|path| PlaceKind::of(path) == PlaceKind::Board)
        .map(String::as_str)
        .collect();
    let review = review_of(&boards);
    json!({ "title": "New cover grid", "details": "the boards", "worktree": world.worktree(), "range": "base..HEAD",
            "places": places, "review": review })
}

/// Commits boards <paths>, tags the commit `base` where `world` tagged the
/// first, then deletes <gone> in one more commit.
fn delete_boards(w: &World, gone: &[&str]) -> String {
    for path in gone {
        git(&w.dir, &["rm", "-q", path]);
    }
    git(&w.dir, &["commit", "-q", "-m", "delete boards"]);
    w.sha("HEAD")
}

/// `places` gives the same output for a range as for the SHAs of its
/// commits, and it fails on a name that is not a commit instead of reporting
/// every board as deleted.
#[test]
fn places_reads_a_range_as_its_commits() {
    let w = world("design-places-range");
    let board = "design/mocks/A.dc.html";
    w.commit(&[board]);
    let head = w.commit(&[board]);
    let wt = w.worktree();
    let by_sha = w.run(&format!("places {wt} {head}")).unwrap();
    assert_eq!(by_sha, format!("place {board}\n"));
    assert_eq!(w.run(&format!("places {wt} HEAD~1..HEAD")).unwrap(), by_sha);
    assert!(w.run(&format!("places {wt} nothing-here")).is_err());
}

#[test]
fn a_deleted_board_needs_no_answer_no_link_and_no_place_on_the_review_page() {
    let w = world("design-deleted");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a, b]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    delete_boards(&w, &[a]);
    w.commit(&[b]);
    let wt = w.worktree();
    let request = base_request(&w, json!({ b: { "matches": "the app shows it" } }));
    w.request(request).unwrap();
    let state = w.state();
    let design = state.design(Id(1)).unwrap();
    assert_eq!(design.deleted, BTreeSet::from([a.to_string()]));
    assert_eq!(design.places.keys().collect::<Vec<_>>(), vec![b]);
    let shown = w.run("show 1").unwrap();
    assert!(shown.contains(&format!("deleted {a}")), "{shown}");
    w.approve(1);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let head = w.sha("HEAD");
    let first = w.sha("HEAD~1");
    assert!(w.run(&format!("check 1 {wt} {first} {head}")).is_ok());
}

/// A change that deletes a component keeps the person's approval through a
/// rebase, and health and land cover its commits, as for a change that edits
/// one.
#[test]
fn a_change_that_deletes_a_component_keeps_its_approval_through_a_rebase() {
    let w = world("design-component-deleted");
    let (a, gone) = ("design/mocks/A.dc.html", "design/system/components/Gone/");
    w.commit(&[a, "design/system/components/Gone/preview.html"]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&w.dir, &["rm", "-q", "-r", gone]);
    git(&w.dir, &["commit", "-q", "-m", "delete Gone"]);
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    let state = w.state();
    let design = state.design(Id(1)).unwrap();
    assert_eq!(design.deleted, BTreeSet::from([gone.to_string()]));
    assert_eq!(design.places.keys().collect::<Vec<_>>(), vec![a]);
    w.approve(1);
    let approved = w.state().designs[0].approved.clone();
    rebase_onto_moved_main(&w);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "moved"]);
    assert_eq!(health_of(&w, "moved..HEAD"), Vec::<String>::new());
    let request = json!({ "worktree": w.worktree(), "range": "moved..HEAD", "places": {} });
    let amended = w.amend(1, request).unwrap();
    assert!(amended.contains("approval stays"), "{amended}");
    assert_eq!(w.state().designs[0].approved, approved);
}

#[test]
fn a_range_that_only_deletes_boards_records_with_a_review_page_of_words() {
    let w = world("design-deleted-only");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a, b]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let sha = delete_boards(&w, &[a, b]);
    let wt = w.worktree();
    assert!(w.run(&format!("only {wt} {sha}")).is_ok());
    assert_eq!(
        w.run(&format!("places {wt} {sha}")).unwrap(),
        format!("deleted {a}\ndeleted {b}\n")
    );
    let request = base_request(&w, json!({}));
    w.request(request).unwrap();
    let state = w.state();
    let design = state.design(Id(1)).unwrap();
    assert_eq!(
        design.deleted,
        BTreeSet::from([a.to_string(), b.to_string()])
    );
    assert!(design.places.is_empty());
    // An amend over a range that draws one of them again takes it back as a place.
    let back = w.commit(&[a]);
    let mut request = base_request(&w, json!({ a: { "matches": "drawn again" } }));
    request["range"] = json!("base..HEAD");
    w.amend(1, request).unwrap();
    let state = w.state();
    let design = state.design(Id(1)).unwrap();
    assert_eq!(design.deleted, BTreeSet::from([b.to_string()]));
    assert!(design.places.contains_key(a));
    let _ = back;
}

#[test]
fn places_lists_no_board_that_the_range_adds_and_deletes() {
    let w = world("design-places-added-deleted");
    let (kept, added) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[kept]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    w.commit(&[added]);
    let sha = delete_boards(&w, &[added]);
    let wt = w.worktree();
    assert_eq!(w.run(&format!("places {wt} {sha}")).unwrap(), "");
    assert_eq!(
        w.run(&format!("places {wt} origin/main..HEAD")).unwrap(),
        ""
    );
}

#[test]
fn places_prints_each_place_once_and_nothing_when_there_are_none() {
    let w = world("design-places");
    let a = w.commit(&[
        "design/mocks/A.dc.html",
        "design/system/components/MacRow/a.html",
        "src/x.rs",
    ]);
    let b = w.commit(&[
        "design/system/components/MacRow/b.html",
        "design/theme.toml",
    ]);
    let out = w.run(&format!("places {} {a} {b}", w.worktree())).unwrap();
    assert_eq!(
        out,
        "place design/mocks/A.dc.html\nplace design/system/components/MacRow/\nshared design/theme.toml\n"
    );
    let c = w.commit(&["src/y.rs"]);
    assert_eq!(w.run(&format!("places {} {c}", w.worktree())).unwrap(), "");
}

#[test]
fn a_shared_only_commit_is_listed_by_places_then_recorded_with_no_app_place() {
    let w = world("design-places-shared");
    let shared = w.commit(&["design/theme.toml"]);
    let wt = w.worktree();
    assert_eq!(
        w.run(&format!("places {wt} {shared}")).unwrap(),
        "shared design/theme.toml\n"
    );
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("a palette tweak the boards do not show");
    w.request(request).unwrap();
    w.approve(1);
    assert!(w.run(&format!("check 1 {wt} {shared}")).is_ok());
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert!(
        w.run(&format!("landed 1 {wt} {shared}"))
            .unwrap()
            .contains("landed")
    );
}

#[test]
fn places_in_a_repo_of_no_project_is_refused() {
    let w = world("design-places-noproject");
    let other = w.scratch.dir.join("other");
    repo(&other, "target 1\n");
    let sha = w.sha("HEAD");
    let _ = sha;
    let head = crate::lists::design_paths::full_sha(&other, "HEAD").unwrap();
    assert!(
        message(w.run(&format!("places {} {head}", other.display()))).contains("is in no project")
    );
}

/// A change records with no app work for any place: no item exists for it
/// before the person approves it (decision #304).
#[test]
fn record_stores_each_changed_place_with_no_app_work() {
    let w = world("design-record-no-app-work");
    let (a, b, row) = (
        "design/mocks/A.dc.html",
        "design/mocks/B.dc.html",
        "design/system/components/MacRow/",
    );
    w.commit(&[a, b, "design/system/components/MacRow/a.html"]);
    let out = w
        .request(base_request(&w, json!({ a: null, b: null, row: null })))
        .unwrap();
    assert!(out.contains("waits on approval"), "{out}");
    let design = &w.state().designs[0];
    assert_eq!(
        design.places.iter().collect::<Vec<_>>(),
        [
            (&a.to_string(), &None),
            (&b.to_string(), &None),
            (&row.to_string(), &None)
        ]
    );
    let shown = w.run("show 1").unwrap();
    assert!(shown.contains("no app work named yet"), "{shown}");

    // A request that answers one place and gives the other `null` records
    // the other with no app work.
    let w = world("design-record-one-answer");
    w.commit(&[a, b]);
    w.request(base_request(&w, json!({ a: { "matches": "x" }, b: null })))
        .unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places[a], Some(Resolution::Matches("x".into())));
    assert_eq!(design.places[b], None);
}

/// Record and amend run `builds shots states` on the change's commits
/// against the parent of its first, in its worktree, and refuse the change,
/// writing nothing, when it lists a component state that no preview row
/// shows.
#[test]
fn record_and_amend_refuse_a_change_that_adds_a_component_state_no_preview_row_shows() {
    let mut w = world("design-record-states");
    let board = "design/mocks/A.dc.html";
    let first = w.commit(&[board]);
    let tip = w.commit(&["design/mocks/B.dc.html"]);
    let shown = w.tool.program.clone();
    w.tool.program = w.scratch.script(
        "states-added",
        "echo \"asked: $*\" >&2; echo \"in: $(pwd -P)\" >&2; echo 'MacButton tinted: mac.a' >&2; exit 1",
    );
    let before = w.bytes();
    let request = base_request(&w, json!({ board: null, "design/mocks/B.dc.html": null }));
    let refused = message(w.request(request.clone()));
    assert!(
        refused.contains(&format!(
            "asked: shots states --at {tip} --against {first}^"
        )),
        "{refused}"
    );
    let top = std::fs::canonicalize(&w.dir).unwrap();
    assert!(
        refused.contains(&format!("in: {}", top.display())),
        "{refused}"
    );
    assert!(refused.contains("MacButton tinted: mac.a"), "{refused}");
    assert!(refused.contains("add each state as a row"), "{refused}");
    assert_eq!(w.bytes(), before);

    w.tool.program = shown.clone();
    w.request(request.clone()).unwrap();
    let recorded = w.bytes();
    w.tool.program = w.scratch.dir.join("states-added");
    let refused = message(w.amend(1, request));
    assert!(refused.contains("MacButton tinted: mac.a"), "{refused}");
    assert_eq!(w.bytes(), recorded);
}

/// A range that changes only a component's README changes no drawing, so
/// record and amend ask the shots checker nothing; a range that changes a
/// file the component draws from still asks it.
#[test]
fn record_and_amend_skip_the_states_check_for_a_range_that_changes_no_drawing() {
    let mut w = world("design-record-states-readme");
    let component = "design/system/components/MacHairline/";
    w.commit(&["design/system/components/MacHairline/README.md"]);
    w.tool.program = w.scratch.script("states-asked", "echo asked >&2; exit 1");
    let request = base_request(&w, json!({ component: null }));
    w.request(request.clone()).unwrap();
    w.amend(1, request.clone()).unwrap();

    w.commit(&["design/system/components/MacHairline/preview.html"]);
    let refused = message(w.amend(1, request));
    assert!(refused.contains("asked"), "{refused}");
}

/// Landing needs each place to name its app work: the design session gives
/// it after the person approves, and giving it keeps the approval.
#[test]
fn landing_is_refused_until_each_place_names_its_app_work_and_naming_it_keeps_the_approval() {
    let w = world("design-name-after-approval");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    let sha = w.commit(&[a, b]);
    w.request(base_request(&w, json!({ a: null, b: null })))
        .unwrap();
    let wt = w.worktree();
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(refused.contains("waits on approval"), "{refused}");
    w.approve(1);
    let approved = w.state().designs[0].approved.clone();
    assert!(approved.is_some());
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(
        refused.contains("names no app work for")
            && refused.contains(a)
            && refused.contains(b)
            && refused.contains("amend 1"),
        "{refused}"
    );

    let item = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let out = w
        .amend(
            1,
            json!({ "worktree": wt, "range": "base..HEAD", "places": { a: { "item": item.0 } } }),
        )
        .unwrap();
    assert!(out.contains("approval stays"), "{out}");
    assert_eq!(w.state().designs[0].approved, approved);
    let still = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(still.contains(b) && !still.contains(a), "{still}");

    // Naming a place leaves the ones the request leaves out as they are.
    w.amend(
        1,
        json!({ "worktree": wt, "range": "base..HEAD", "places": { b: { "matches": "the app shows it" } } }),
    )
    .unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places[a], Some(Resolution::Item(item)));
    assert_eq!(design.approved, approved);
    let out = w.run(&format!("check 1 {wt} {sha}")).unwrap();
    assert!(out.contains("and is approved"), "{out}");
}

#[test]
fn record_resolves_places_by_item_new_item_and_matches() {
    let w = world("design-record");
    let existing = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    w.commit(&[
        "design/mocks/A.dc.html",
        "design/mocks/B.dc.html",
        "design/system/components/MacRow/a.html",
    ]);
    let out = w
        .request(base_request(
            &w,
            json!({
                "design/mocks/A.dc.html": { "item": existing.0 },
                "design/mocks/B.dc.html": { "new": draft("Build the grid the board shows") },
                "design/system/components/MacRow/": { "matches": "compared the component with MacRow.swift" },
            }),
        ))
        .unwrap();
    let state = w.state();
    let design = &state.designs[0];
    assert_eq!((design.repo.as_str(), design.places.len()), ("bae", 3));
    let new = state
        .items
        .iter()
        .find(|i| i.title == "Build the grid the board shows")
        .unwrap();
    assert_eq!(new.waits, [Wait::Design(design.id)]);
    assert_eq!(new.origin, Some(design.id));
    assert_eq!(
        design.places["design/mocks/B.dc.html"],
        Some(Resolution::Item(new.id))
    );
    assert_eq!(
        design.places["design/mocks/A.dc.html"],
        Some(Resolution::Item(existing))
    );
    assert!(
        out.contains(&format!("new items {} wait on it", new.id)),
        "{out}"
    );
    assert_eq!(derive::item_state(&state, new), ItemState::Waiting);
    let shown = w.run(&format!("show {}", design.id.0)).unwrap();
    assert!(
        shown.contains("design/mocks/B.dc.html") && shown.contains("compared the component"),
        "{shown}"
    );
    assert!(
        w.run("list")
            .unwrap()
            .contains("New cover grid (waiting on review, repo bae, 3 places, 1 item waiting)")
    );
}

#[test]
fn record_refuses_bad_answers_and_changes_nothing() {
    let w = world("design-bad-answers");
    w.commit(&["design/mocks/A.dc.html"]);
    let before = w.bytes();
    let board = "design/mocks/A.dc.html";
    let cases = [
        (
            json!({ board: { "matches": "  " } }),
            "no how it was checked",
        ),
        (json!({ board: { "item": 99 } }), "no record has"),
        (
            json!({ board: { "matches": "x" }, "design/mocks/Z.dc.html": { "matches": "x" } }),
            "does not change",
        ),
        (json!({ board: { "new": draft("too short") } }), "words"),
    ];
    for (places, expect) in cases {
        let mut request = base_request(&w, places);
        // The review page names only the board that is a file: a stray place is refused by its own rule.
        request["review"] = review_of(&[board]);
        let refused = message(w.request(request));
        assert!(refused.contains(expect), "{refused}");
        assert_eq!(w.bytes(), before);
    }
    let mut bad = base_request(&w, json!({ board: { "matches": "x" } }));
    bad["title"] = json!("");
    assert!(message(w.request(bad)).contains("title is empty"));
    let mut bad = base_request(
        &w,
        json!({ board: { "new": draft("Build the grid the board shows") } }),
    );
    bad["places"][board]["new"]["brief"] = json!("plans/missing.md");
    assert!(message(w.request(bad)).contains("does not exist"));
    assert_eq!(w.bytes(), before);
}

/// A design change's title names the part of the app it draws in a few plain
/// words: record and amend take one of two words, refuse only a title that is
/// no one line, and `title` prints it.
#[test]
fn a_design_change_takes_a_title_that_names_the_part_it_draws() {
    let w = world("design-part-title");
    let board = "design/mocks/A.dc.html";
    w.commit(&[board]);
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["title"] = json!("People rows");
    w.request(request).unwrap();
    assert_eq!(w.run("title 1").unwrap().trim(), "People rows");
    let amend = json!({ "title": "AirPlay wait", "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    w.amend(1, amend).unwrap();
    assert_eq!(w.run("title 1").unwrap().trim(), "AirPlay wait");
    let before = w.bytes();
    for bad in ["People rows ", "People\nrows"] {
        let amend =
            json!({ "title": bad, "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
        assert!(message(w.amend(1, amend)).contains("title"), "{bad:?}");
    }
    assert_eq!(w.bytes(), before);
}

#[test]
fn record_refuses_an_item_that_is_closed() {
    let w = world("design-closed-item");
    let id = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            let id = crate::lists::fixtures::item(db, vec![]);
            crate::lists::record::close(db, id, How::Dropped("gone".into()), &stamp(NOON))?;
            Ok(id)
        })
        .unwrap();
    w.commit(&["design/mocks/A.dc.html"]);
    let refused = message(w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "item": id.0 } }),
    )));
    assert!(refused.contains("closed"), "{refused}");
}

#[test]
fn shared_files_alone_need_no_app_place_and_it_is_refused_beside_places() {
    let w = world("design-shared");
    w.commit(&["design/theme.toml"]);
    let refused = message(w.request(base_request(&w, json!({}))));
    assert!(
        refused.contains("design/theme.toml") && refused.contains("no_app_place"),
        "{refused}"
    );
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("   ");
    assert!(message(w.request(request)).contains("no_app_place is empty"));
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("a palette tweak the boards do not show");
    w.request(request).unwrap();
    assert_eq!(
        w.state().designs[0].no_app_place.as_deref(),
        Some("a palette tweak the boards do not show")
    );

    let w = world("design-shared-with-place");
    w.commit(&["design/theme.toml", "design/mocks/A.dc.html"]);
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request["no_app_place"] = json!("why");
    assert!(message(w.request(request)).contains("or shared design files and no board"));
}

#[test]
fn a_range_that_changes_no_design_files_or_no_commits_is_refused() {
    let w = world("design-nothing");
    w.commit(&["src/a.rs"]);
    assert!(message(w.request(base_request(&w, json!({})))).contains("changes no design files"));
    let mut request = base_request(&w, json!({}));
    request["range"] = json!("HEAD..HEAD");
    assert!(message(w.request(request)).contains("holds no commits"));
}

#[test]
fn amend_holds_a_change_to_what_its_new_range_changes_so_a_board_renamed_before_landing_drops_out()
{
    let w = world("design-amend-rename");
    let (old, new) = (
        "design/mocks/mac.old.dc.html",
        "design/mocks/all.new.dc.html",
    );
    w.commit(&[old]);
    let mut request = base_request(&w, json!({ old: { "matches": "x" } }));
    request["review"] = review_of(&[old]);
    w.request(request).unwrap();
    // The board is renamed before landing and the history rewritten, so the range changes only the new name.
    git(&w.dir, &["reset", "-q", "--hard", "base"]);
    w.commit(&[new, "design/mocks/other.dc.html"]);
    let other = "design/mocks/other.dc.html";
    let mut amend = base_request(
        &w,
        json!({ new: { "matches": "y" }, other: { "matches": "z" } }),
    );
    amend["review"] = review_of(&[new, other]);
    w.amend(1, amend).unwrap();
    let design = &w.state().designs[0];
    let places: Vec<&str> = design.places.keys().map(String::as_str).collect();
    assert_eq!(places, [new, other], "the renamed-away place is gone");
    assert!(
        design
            .review
            .iter()
            .flat_map(|s| &s.boards)
            .all(|b| b.named() != old),
        "and its place on the page"
    );
}

#[test]
fn amend_answers_the_places_of_a_new_range_and_drops_those_it_no_longer_changes() {
    let w = world("design-amend");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" } }),
    ))
    .unwrap();
    let first = w.sha("HEAD");
    w.commit(&[
        "design/mocks/B.dc.html",
        "design/system/components/Sym/a.html",
    ]);
    let mut request = json!({ "worktree": w.worktree(), "range": format!("{first}..HEAD"),
                              "places": { "design/mocks/B.dc.html": { "new": draft("Build the grid the board shows") } },
                              "review": review_of(&["design/mocks/A.dc.html", "design/mocks/B.dc.html"]) });
    request["places"]["design/system/components/Sym/"] = json!({ "matches": "y" });
    let out = w.amend(1, request).unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places.len(), 2, "A is not in the range first..HEAD");
    assert!(!design.places.contains_key("design/mocks/A.dc.html"));
    assert!(out.contains("wait on it"), "{out}");
    assert_eq!(w.state().items[0].waits, [Wait::Design(design.id)]);
    // A range that changes A again holds A with no app work: its earlier answer went with it.
    let stale = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    w.amend(1, stale).unwrap();
    assert_eq!(w.state().designs[0].places["design/mocks/A.dc.html"], None);
    let again = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": { "design/mocks/A.dc.html": { "matches": "again" } } });
    w.amend(1, again).unwrap();
}

#[test]
fn check_compares_by_path_so_a_rebased_commit_still_checks() {
    let w = world("design-check");
    let original = w.commit(&[
        "design/mocks/A.dc.html",
        "design/system/components/MacRow/a.html",
    ]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" }, "design/system/components/MacRow/": { "matches": "y" } }),
    ))
    .unwrap();
    w.approve(1);
    let check = format!("check 1 {}", w.worktree());
    assert!(
        w.run(&format!("{check} {original}"))
            .unwrap()
            .contains("resolves every place")
    );
    // The same change as another commit: cherry-picked onto another base.
    git(&w.dir, &["checkout", "-q", "-b", "other", "base"]);
    w.commit(&["unrelated.txt"]);
    git(&w.dir, &["cherry-pick", &original]);
    let picked = w.sha("HEAD");
    assert_ne!(picked, original);
    assert!(w.run(&format!("{check} {picked}")).is_ok());
}

/// Moves main past `base` with a commit of a file outside the design files,
/// then rebases the branch's commits onto it, as an agent rebases a branch
/// before it lands. The moved main is the branch `moved`.
fn rebase_onto_moved_main(w: &World) {
    git(&w.dir, &["checkout", "-q", "-b", "moved", "base"]);
    w.commit(&["src/moved.rs"]);
    git(&w.dir, &["checkout", "-q", "main"]);
    git(&w.dir, &["rebase", "-q", "moved"]);
}

/// The commit that stands for origin/main in <range>: its start.
fn main_of(w: &World, range: &str) -> String {
    let (main, _) = range.split_once("..").expect("a range");
    crate::lists::design_paths::full_sha(&w.dir, main).unwrap()
}

/// What git says of the worktree of <w> whose commits past origin/main are
/// those of <range>.
fn side_read_of(w: &World, range: &str) -> crate::side_git::SideRead {
    let ahead: Vec<crate::side_git::Commit> =
        crate::lists::design_paths::commits_of_range(&w.dir, range)
            .unwrap()
            .into_iter()
            .rev()
            .map(|full| crate::side_git::Commit {
                sha: full[..7].to_string(),
                full,
                time: 1,
                authored: 1,
                subject: "a commit".into(),
            })
            .collect();
    crate::side_git::SideRead {
        head: ahead.first().cloned(),
        ahead,
        main: main_of(w, range),
        uncommitted: 0,
        last_write: None,
    }
}

/// What `builds health` says about the worktree of <w> whose commits past
/// origin/main are those of <range>.
fn health_of(w: &World, range: &str) -> Vec<String> {
    let tree = crate::lists::held::Worktree {
        label: "bae/side-1".into(),
        checkout: crate::checkout::Checkout::at(&w.dir).unwrap().unwrap(),
        branch: None,
        claim: crate::claim::Claim {
            who: "agent".into(),
            since: NOON,
            note: None,
        },
        project: Some("demo".into()),
        git: Ok(side_read_of(w, range)),
        committing: Ok(Vec::new()),
    };
    let loaded = Ok(vec![crate::lists::store::Loaded {
        project: "demo".into(),
        lists: Ok(w.state()),
    }]);
    crate::lists::held::inspect(
        &loaded,
        &[tree],
        crate::lists::design_reads::Coverage::Read(&Default::default()),
    )
    .findings
    .into_iter()
    .map(|advice| advice.text)
    .collect()
}

/// The dashboard's design card for the worktree of <w> whose commits past
/// origin/main are those of <range>, once their rules review takes them.
fn card_of(w: &World, range: &str) -> Option<crate::daemon::cards::ItemWait> {
    let git = side_read_of(w, range);
    let lists = w.state();
    crate::daemon::design_cards::standing(&Default::default(), &lists, "bae", &w.dir, &git)
        .unwrap()
        .and_then(|standing| crate::daemon::design_cards::wait(&standing, true))
}

/// The dashboard's design card and health decide coverage the same way: on
/// a branch rebased onto a moved main, both count the recorded change as
/// covering its commits, and both stop once a later commit draws a board
/// again.
#[test]
fn the_design_card_and_health_agree_on_a_rebased_branch() {
    use crate::daemon::cards::ItemWait;
    let w = world("design-card-rebased");
    let a = "design/mocks/A.dc.html";
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    rebase_onto_moved_main(&w);
    let title = w.state().designs[0].title.clone();
    assert_eq!(
        card_of(&w, "moved..HEAD"),
        Some(ItemWait::Review { id: 1, title })
    );
    assert_eq!(health_of(&w, "moved..HEAD"), Vec::<String>::new());
    w.approve(1);
    assert_eq!(card_of(&w, "moved..HEAD"), None);
    assert_eq!(health_of(&w, "moved..HEAD"), Vec::<String>::new());

    w.commit(&[a]);
    assert_eq!(card_of(&w, "moved..HEAD"), Some(ItemWait::Unrecorded));
    assert_eq!(health_of(&w, "moved..HEAD").len(), 1);
}

/// A design change recorded on commits covers them after the agent rebases
/// the branch onto a main that moved: the rebased commits draw the boards
/// the same, so health raises nothing and land's check passes. Once a later
/// commit draws a board again, the change no longer covers it.
#[test]
fn a_design_branch_rebased_onto_a_moved_main_stays_covered() {
    let w = world("design-rebased");
    let a = "design/mocks/A.dc.html";
    let drawn = w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    w.approve(1);
    rebase_onto_moved_main(&w);
    let rebased = w.sha("HEAD");
    assert_ne!(rebased, drawn);
    assert_eq!(health_of(&w, "moved..HEAD"), Vec::<String>::new());
    let check = format!("check 1 {} moved..HEAD", w.worktree());
    let out = w.run(&check).unwrap();
    assert!(out.contains("resolves every place"), "{out}");

    let redrawn = w.commit(&[a]);
    let found = health_of(&w, "moved..HEAD");
    assert!(
        found.len() == 1 && found[0].contains(&rebased[..10]) && found[0].contains(&redrawn[..10]),
        "{found:?}"
    );
    let refused = message(w.run(&check));
    assert!(
        refused.contains(&format!("design change #1 drew {a}"))
            && refused.contains("builds design amend 1"),
        "{refused}"
    );
}

/// A shared design-system source changes how boards draw, so once a later
/// commit edits a source that the design change drew, the change no longer
/// covers it.
#[test]
fn a_source_changed_after_the_change_drew_it_is_not_covered() {
    let w = world("design-source-redrawn");
    let (board, tokens) = ("design/mocks/A.dc.html", "design/system/tokens.json");
    w.commit(&[board, tokens]);
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["no_app_place"] = json!("the design system has no components");
    w.request(request).unwrap();
    w.approve(1);
    let check = format!("check 1 {} base..HEAD", w.worktree());
    w.run(&check).unwrap();
    w.commit(&[tokens]);
    let refused = message(w.run(&check));
    assert!(
        refused.contains(&format!("design change #1 drew {tokens}")),
        "{refused}"
    );
    assert_eq!(health_of(&w, "base..HEAD").len(), 1);
}

/// Amending a change to the commits of its rebased branch keeps the person's
/// approval when every board comes out byte-identical, and clears it once a
/// board or the review page changes.
#[test]
fn an_amend_whose_boards_come_out_the_same_keeps_the_approval() {
    let w = world("design-amend-same");
    let a = "design/mocks/A.dc.html";
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    w.approve(1);
    let approved = w.state().designs[0].approved.clone();
    rebase_onto_moved_main(&w);
    let request = json!({ "worktree": w.worktree(), "range": "moved..HEAD", "places": {} });
    let amended = w.amend(1, request.clone()).unwrap();
    assert!(
        amended.contains("approval stays") && !amended.contains("waits on approval"),
        "{amended}"
    );
    let design = &w.state().designs[0];
    assert_eq!(design.approved, approved);
    assert_eq!(design.tip, Some(w.sha("HEAD")));

    let mut new_page = request.clone();
    new_page["review"] = json!({ "heading": "Other words", "boards": [{ "board": a, "caption": "A", "theme": "light", "circles": [{ "selector": "#r" }] }] });
    let amended = w.amend(1, new_page).unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
    w.approve(1);

    w.commit(&[a]);
    let amended = w.amend(1, request).unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
    assert!(w.state().designs[0].approved.is_none());
}

/// A rebase that merges unrelated lines into a board changes its bytes and
/// not its drawing: the amend keeps the person's approval. A board that draws
/// differently clears it, whatever its bytes.
#[test]
fn an_amend_keeps_the_approval_when_the_board_files_differ_and_draw_alike() {
    let mut w = world("design-amend-alike");
    let a = "design/mocks/A.dc.html";
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    w.approve(1);
    let approved = w.state().designs[0].approved.clone();
    let request = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });

    w.draws("alike");
    w.commit(&[a]);
    let amended = w.amend(1, request.clone()).unwrap();
    assert!(
        amended.contains("approval stays") && !amended.contains("waits on approval"),
        "{amended}"
    );
    assert_eq!(w.state().designs[0].approved, approved);
    assert_eq!(w.state().designs[0].tip, Some(w.sha("HEAD")));

    w.draws("differs");
    w.commit(&[a]);
    let amended = w.amend(1, request).unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
    assert!(w.state().designs[0].approved.is_none());
}

/// A changed `canvas.json` or design-system source draws nothing alone: the
/// amend keeps the approval when every board of the change draws alike, and
/// clears it when one does not.
#[test]
fn an_amend_judges_a_changed_shared_file_by_what_the_changes_boards_draw() {
    let mut w = world("design-amend-shared");
    let (a, tokens) = ("design/mocks/A.dc.html", "design/system/tokens.json");
    w.commit(&[a, tokens]);
    let mut first = base_request(&w, json!({ a: { "matches": "x" } }));
    first["no_app_place"] = json!("the design system has no components");
    w.request(first).unwrap();
    w.approve(1);
    let request = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });

    w.draws("alike");
    w.commit(&[tokens]);
    let amended = w.amend(1, request.clone()).unwrap();
    assert!(amended.contains("approval stays"), "{amended}");

    w.draws("differs");
    w.commit(&[tokens]);
    let amended = w.amend(1, request).unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
}

/// Health and land's check cover commits whose board files differ from the
/// change's in bytes only, and stop covering them once a board draws
/// differently.
#[test]
fn health_and_check_cover_commits_whose_boards_differ_in_bytes_and_draw_alike() {
    let mut w = world("design-covers-alike");
    let a = "design/mocks/A.dc.html";
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    w.approve(1);
    let check = format!("check 1 {} base..HEAD", w.worktree());

    w.draws("alike");
    w.commit(&[a]);
    let health = |w: &World| {
        let tree = crate::lists::held::Worktree {
            label: "bae/side-1".into(),
            checkout: crate::checkout::Checkout::at(&w.dir).unwrap().unwrap(),
            branch: None,
            claim: crate::claim::Claim {
                who: "agent".into(),
                since: NOON,
                note: None,
            },
            project: Some("demo".into()),
            git: Ok(side_read_of(w, "base..HEAD")),
            committing: Ok(Vec::new()),
        };
        let loaded = Ok(vec![crate::lists::store::Loaded {
            project: "demo".into(),
            lists: Ok(w.state()),
        }]);
        let reads = crate::lists::design_reads::DesignReads::with_program(
            &w.tool.places,
            w.tool.program.clone(),
        );
        crate::lists::held::inspect(
            &loaded,
            &[tree],
            crate::lists::design_reads::Coverage::Read(&reads),
        )
        .findings
        .into_iter()
        .map(|advice| advice.text)
        .collect::<Vec<_>>()
    };
    assert_eq!(health(&w), Vec::<String>::new());
    let out = w.run(&check).unwrap();
    assert!(out.contains("resolves every place"), "{out}");

    // A later commit is another pair of commits: the answer for the first one
    // is kept.
    w.draws("differs");
    w.commit(&[a]);
    assert_eq!(health(&w).len(), 1);
    let refused = message(w.run(&check));
    assert!(
        refused.contains(&format!("design change #1 drew {a}")),
        "{refused}"
    );
}

#[test]
fn check_names_what_is_unresolved_and_refuses_a_closed_or_missing_change() {
    let w = world("design-check-refused");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" } }),
    ))
    .unwrap();
    w.approve(1);
    let later = w.commit(&["design/mocks/B.dc.html", "design/theme.toml"]);
    let wt = w.worktree();
    let refused = message(w.run(&format!("check 1 {wt} {later}")));
    assert!(
        refused.contains("design/mocks/B.dc.html") && !refused.contains("mocks/A"),
        "{refused}"
    );
    assert!(message(w.run(&format!("check 9 {wt} {later}"))).contains("not a record"));
    let shared = w.commit(&["design/theme.toml"]);
    assert!(message(w.run(&format!("check 1 {wt} {shared}"))).contains("no_app_place"));
    let src = w.commit(&["src/a.rs"]);
    assert!(message(w.run(&format!("check 1 {wt} {src}"))).contains("change no design files"));
    assert!(message(w.run(&format!("check 1 {wt}"))).contains("usage"));
}

#[test]
fn check_of_a_change_that_has_landed_is_refused() {
    let w = world("design-check-landed");
    let sha = w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" } }),
    ))
    .unwrap();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    w.run(&format!("landed 1 {} {sha}", w.worktree())).unwrap();
    assert!(message(w.run(&format!("check 1 {} {sha}", w.worktree()))).contains("already landed"));
}

/// Two changes, each drawn on its own commits, land together when the person
/// has approved each and the two together resolve every place the commits
/// change.
#[test]
fn check_of_several_changes_needs_each_approved_and_every_place_in_one_of_them() {
    let w = world("design-check-several");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    w.approve(1);
    git(&w.dir, &["tag", "drew-a"]);
    let drew_b = w.commit(&[b]);
    let mut second = base_request(&w, json!({ b: { "matches": "y" } }));
    second["range"] = json!("drew-a..HEAD");
    w.request(second).unwrap();
    let wt = w.worktree();
    let both = format!("{wt} base..HEAD");
    let refused = message(w.run(&format!("check 1,2 {both}")));
    assert!(
        refused.contains("design change #2 waits on approval"),
        "{refused}"
    );
    w.approve(2);
    let alone = message(w.run(&format!("check 1 {both}")));
    assert!(alone.contains(b) && !alone.contains(a), "{alone}");
    let out = w.run(&format!("check 1,2 {both}")).unwrap();
    assert!(
        out.contains("design changes #1, #2 together resolve every place"),
        "{out}"
    );
    assert!(message(w.run(&format!("check 1,1 {both}"))).contains("#1 is named twice"));
    // A change that resolves nothing the commits change would close with
    // commits that are not its.
    let stray = message(w.run(&format!("check 1,2 {wt} {drew_b}")));
    assert!(
        stray.contains("design change #1 resolves nothing the commits change"),
        "{stray}"
    );

    let c = "design/mocks/C.dc.html";
    w.commit(&[c]);
    let neither = message(w.run(&format!("check 1,2 {both}")));
    assert!(
        neither.contains(&format!("design changes #1, #2 do not resolve {c}"))
            && !neither.contains(a)
            && !neither.contains(b),
        "{neither}"
    );
}

#[test]
fn landed_closes_the_change_frees_its_items_and_is_idempotent() {
    let w = world("design-landed");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "new": draft("Build the grid the board shows") } }),
    ))
    .unwrap();
    let sha = w.sha("HEAD");
    let wt = w.worktree();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    assert!(message(w.run(&format!("landed 1 {wt} {sha}"))).contains("is not on origin/main"));
    assert_eq!(w.state().designs.len(), 1);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = w.run(&format!("landed 1 {wt} {sha}")).unwrap();
    assert!(
        out.contains("design change #1 landed")
            && out.contains("item #2")
            && out.contains("is ready"),
        "{out}"
    );
    let state = w.state();
    assert!(state.designs.is_empty());
    let closed = state.closed_record(Id(1)).unwrap();
    assert_eq!(closed.how, How::Landed(vec![sha.clone()]));
    let Record::Design(design) = &closed.record else {
        panic!("a design change")
    };
    assert_eq!(design.landed[0].commits, std::slice::from_ref(&sha));
    assert_eq!(
        derive::item_state(&state, &state.items[0]),
        ItemState::Ready
    );

    let once = w.bytes();
    let again = w.run(&format!("landed 1 {wt} {sha}")).unwrap();
    assert!(again.contains("already recorded"), "{again}");
    assert_eq!(w.bytes(), once);
    let other = w.commit(&["design/mocks/B.dc.html"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert!(message(w.run(&format!("landed 1 {wt} {other}"))).contains("already landed as"));
    assert!(message(w.run(&format!("landed 7 {wt} {other}"))).contains("not a record"));
}

#[test]
fn landed_records_the_worktrees_claim_wherever_it_runs() {
    let w = world("design-landed-who");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "new": draft("Build the grid the board shows") } }),
    ))
    .unwrap();
    let sha = w.sha("HEAD");
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let checkout = crate::checkout::Checkout::at(&w.dir).unwrap().unwrap();
    crate::claim::Claim {
        who: "design session".into(),
        since: 1_790_000_000,
        note: None,
    }
    .write(&checkout.claim_file())
    .unwrap();
    // No --by, and the test process runs in no claimed worktree, as builds
    // land does when the coordinator runs it from the person's checkout.
    let args: Vec<String> = ["landed", "1", &w.worktree(), &sha]
        .iter()
        .map(|word| word.to_string())
        .collect();
    run_in_session(&w.tool, &args, || Ok(None)).unwrap();
    let state = w.state();
    assert_eq!(
        state.closed_record(Id(1)).unwrap().closed.by,
        "design session"
    );
}

/// Runs `design drop <id> --why <why>`.
fn drop_change(w: &World, id: u32, why: &str) -> Result<String> {
    w.run_args(
        ["drop", &id.to_string(), "--why", why]
            .iter()
            .map(|word| word.to_string())
            .collect(),
    )
}

#[test]
fn drop_closes_an_open_change_keeps_why_and_leaves_the_open_lists() {
    let w = world("design-drop");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "the app draws it" } }),
    ))
    .unwrap();
    let why = "design change 2 covers the whole move";
    let out = drop_change(&w, 1, why).unwrap();
    assert_eq!(out, "closed design change #1; it will not land\n");
    let state = w.state();
    assert!(state.designs.is_empty());
    assert_eq!(
        state.closed_record(Id(1)).unwrap().how,
        How::Dropped(why.to_string())
    );
    assert_eq!(w.run("list").unwrap(), "");
    let view = crate::lists::view::View::build(&state, NOON).unwrap();
    assert!(
        view.designs.is_empty(),
        "a dropped change is not one that landed"
    );
    let board = crate::daemon::board::board(&view);
    assert_eq!(board.waiting, 0);
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains("[dropped ") && shown.contains(&format!("dropped: {why}")),
        "{shown}"
    );
    assert!(
        !shown.contains("waiting on the person's approval"),
        "{shown}"
    );

    let once = w.bytes();
    assert!(message(drop_change(&w, 1, why)).contains("tester dropped design change #1"));
    assert!(message(w.run("approve 1")).contains("tester dropped design change #1"));
    let sha = w.sha("HEAD");
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert!(
        message(w.run(&format!("landed 1 {} {sha}", w.worktree())))
            .contains("tester dropped design change #1")
    );
    assert_eq!(w.bytes(), once);
}

#[test]
fn drop_refuses_a_landed_change_another_record_and_no_why() {
    let w = world("design-drop-refused");
    let sha = w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "the app draws it" } }),
    ))
    .unwrap();
    assert!(message(w.run("drop 1")).contains("--why"));
    assert!(message(drop_change(&w, 1, " ")).contains("--why is empty"));
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    w.run(&format!("landed 1 {} {sha}", w.worktree())).unwrap();
    assert!(message(drop_change(&w, 1, "late")).contains("design change #1 has already landed"));
    assert!(message(drop_change(&w, 9, "none")).contains("not a record"));
    w.store
        .write(&stamp(NOON), "item add", |db| {
            crate::lists::item::add_item(
                db,
                &serde_json::from_value(draft("Build the grid the board shows")).unwrap(),
                None,
                &stamp(NOON),
            )
        })
        .unwrap();
    assert!(message(drop_change(&w, 2, "no")).contains("not a design change"));
}

#[test]
fn drop_refuses_while_an_open_item_waits_on_the_change() {
    let w = world("design-drop-waited");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "new": draft("Build the grid the board shows") } }),
    ))
    .unwrap();
    let once = w.bytes();
    let refused = message(drop_change(&w, 1, "not needed"));
    assert!(
        refused.contains("#2 \"Build the grid the board shows\" waits on #1")
            && refused.contains("builds item wait 2 --off design:1"),
        "{refused}"
    );
    assert_eq!(w.bytes(), once);
    w.store
        .write(&stamp(NOON), "item wait 2 --off design:1", |db| {
            crate::lists::item::remove_wait(db, Id(2), &Wait::Design(Id(1)))
        })
        .unwrap();
    drop_change(&w, 1, "not needed").unwrap();
    assert!(w.state().designs.is_empty());
}

#[test]
fn a_design_change_waited_on_by_an_item_that_also_waits_elsewhere_stays_waiting() {
    let w = world("design-landed-waits");
    w.commit(&["design/mocks/A.dc.html"]);
    let mut grid = draft("Build the grid the board shows");
    grid["waits"] = json!([{ "condition": { "text": "a quiet machine", "until": "coordinator" } }]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "new": grid } }),
    ))
    .unwrap();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = w
        .run(&format!("landed 1 {} {}", w.worktree(), w.sha("HEAD")))
        .unwrap();
    assert!(!out.contains("is ready"), "{out}");
    assert_eq!(w.state().items[0].waits.len(), 2);
}

#[test]
fn show_and_list_refuse_what_is_not_a_design_change() {
    let w = world("design-show");
    assert_eq!(w.run("list").unwrap(), "");
    assert!(message(w.run("show 5")).contains("not a record"));
    assert!(message(w.run("nonsense")).contains("unknown design command"));
    assert!(message(w.run("")).contains("usage"));
}

#[test]
fn a_recorded_change_waits_on_review_and_landing_checks_refuse_until_the_person_approves() {
    let w = world("design-approve");
    let sha = w.commit(&["design/mocks/A.dc.html"]);
    let out = w
        .request(base_request(
            &w,
            json!({ "design/mocks/A.dc.html": { "matches": "x" } }),
        ))
        .unwrap();
    assert!(out.contains("waits on approval"), "{out}");
    assert!(w.state().designs[0].approved.is_none());
    assert!(w.run("list").unwrap().contains("waiting on review"));
    let wt = w.worktree();
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(
        refused.contains("waits on approval") && refused.contains("approve 1"),
        "{refused}"
    );

    let before = w.bytes();
    assert!(message(w.run("approve 9")).contains("not a record"));
    assert_eq!(w.bytes(), before);
    assert!(w.run("approve 1").unwrap().contains("approved"));
    assert!(w.state().designs[0].approved.is_some());
    assert!(
        w.run(&format!("check 1 {wt} {sha}"))
            .unwrap()
            .contains("and is approved")
    );
    assert!(w.run("show 1").unwrap().contains("approved"));
    assert!(w.run("list").unwrap().contains("(approved,"));

    // The same approval again writes nothing.
    let once = w.bytes();
    assert!(w.run("approve 1").unwrap().contains("already approved"));
    assert_eq!(w.bytes(), once);
}

#[test]
fn a_fix_that_changes_no_design_is_approved_when_recorded_and_needs_no_review_page() {
    let w = world("design-fix");
    let sha = w.commit(&["design/mocks/A.dc.html"]);
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request.as_object_mut().unwrap().remove("review");
    request["fix"] = json!("");
    let before = w.bytes();
    let said = message(w.request(request.clone()));
    assert!(
        said.contains("fix is empty") && said.contains("what the app draws today"),
        "{said}"
    );
    assert_eq!(w.bytes(), before);

    request["fix"] = json!("the row heights follow the design system's measures");
    let out = w.request(request).unwrap();
    assert!(out.contains("needs no review"), "{out}");
    let design = &w.state().designs[0];
    assert!(design.approved.is_some());
    assert_eq!(
        design.settlement,
        Settlement::Fix("the row heights follow the design system's measures".into())
    );
    let wt = w.worktree();
    assert!(
        w.run(&format!("check 1 {wt} {sha}"))
            .unwrap()
            .contains("and is approved")
    );
    assert!(
        w.run("show 1")
            .unwrap()
            .contains("a fix that changes no design")
    );
}

#[test]
fn an_amend_without_the_fix_makes_the_change_wait_on_review_again() {
    let w = world("design-fix-amend");
    w.commit(&["design/mocks/A.dc.html"]);
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request["fix"] = json!("a drawing mistake in the last change");
    w.request(request).unwrap();
    // A new commit draws the board again, which the fix said it did not.
    w.commit(&["design/mocks/A.dc.html"]);
    let amended = w
        .amend(
            1,
            json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} }),
        )
        .unwrap();
    assert!(amended.contains("waits on approval"), "{amended}");
    let design = &w.state().designs[0];
    assert!(design.approved.is_none() && design.settlement == Settlement::Review);
}

#[test]
fn an_amend_that_only_names_app_work_keeps_the_approval_of_a_fix() {
    let w = world("design-fix-places-amend");
    let board = "design/mocks/A.dc.html";
    w.commit(&[board]);
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request.as_object_mut().unwrap().remove("review");
    request["fix"] = json!("a drawing mistake in the last change");
    w.request(request).unwrap();
    let amended = w
        .amend(
            1,
            json!({ "worktree": w.worktree(), "range": "base..HEAD",
                    "places": { board: { "matches": "the app shows it" } } }),
        )
        .unwrap();
    assert!(!amended.contains("approval is cleared"), "{amended}");
    let design = &w.state().designs[0];
    assert!(design.approved.is_some(), "the amend cleared the approval");
    assert_eq!(
        design.settlement,
        Settlement::Fix("a drawing mistake in the last change".into())
    );
}

#[test]
fn an_amend_that_only_names_app_work_keeps_the_approval_of_a_decided_change() {
    let w = world("design-decided-places-amend");
    let board = "design/mocks/A.dc.html";
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    w.commit(&[board]);
    w.request(decided_request(&w, board, work, decision))
        .unwrap();
    let id = w.state().designs[0].id.0;
    let amended = w
        .amend(
            id,
            json!({ "worktree": w.worktree(), "range": "base..HEAD",
                    "places": { board: { "matches": "the app shows it" } } }),
        )
        .unwrap();
    assert!(!amended.contains("approval is cleared"), "{amended}");
    let design = &w.state().designs[0];
    assert!(design.approved.is_some());
    assert_eq!(design.settlement, Settlement::Decision(decision));
}

/// Records in <w>'s store a decision the person made, and returns its id.
fn decide(w: &World) -> Id {
    w.store
        .write(&stamp(NOON), "decision add", |db| {
            let new = crate::lists::decision::NewDecision {
                title: "Add a button to the alert".into(),
                by: "person".into(),
                body: "The alert gets a second button.".into(),
                question: None,
                replaces: None,
                plan: None,
            };
            Ok(crate::lists::decision::add_decision(db, &new, &[], &stamp(NOON))?.decision)
        })
        .unwrap()
}

/// A request for `base..HEAD` that records the board edit of a change the
/// person decided in <decision>: the board's place answered by the item that
/// does the app work, its canvas link, and no review page.
fn decided_request(w: &World, board: &str, item: Id, decision: Id) -> serde_json::Value {
    let mut request = base_request(w, json!({ board: { "item": item.0 } }));
    request.as_object_mut().unwrap().remove("review");
    request["decision"] = json!(decision.0);
    request
}

/// An implementing agent edits an existing board in the commit that builds
/// the change the person decided, and records the edit with the decision.
/// builds approves it as it records it, since the decision is the approval:
/// land's check passes, health raises nothing and the worktree's card waits
/// on nothing.
#[test]
fn a_board_edit_for_a_decided_change_is_approved_when_recorded_and_lands() {
    let w = world("design-decided");
    let board = "design/mocks/A.dc.html";
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    let sha = w.commit(&[board, "src/alert.rs"]);
    assert_eq!(health_of(&w, "base..HEAD").len(), 1);

    let out = w
        .request(decided_request(&w, board, work, decision))
        .unwrap();
    assert!(
        out.contains("needs no review") && out.contains(&format!("decision {decision}")),
        "{out}"
    );
    let state = w.state();
    let design = &state.designs[0];
    assert!(design.approved.is_some());
    assert_eq!(design.settlement, Settlement::Decision(decision));
    let wt = w.worktree();
    let id = design.id.0;
    assert!(
        w.run(&format!("check {id} {wt} {sha}"))
            .unwrap()
            .contains("and is approved")
    );
    assert!(
        w.run(&format!("show {id}"))
            .unwrap()
            .contains(&format!("decision {decision}"))
    );
    assert_eq!(health_of(&w, "base..HEAD"), Vec::<String>::new());
    assert_eq!(card_of(&w, "base..HEAD"), None);
}

/// A decided change is approved as it is recorded, so nothing later names its
/// app work: record and amend refuse it with every place that lacks app work
/// named at once, and store nothing.
#[test]
fn record_and_amend_of_a_decided_change_name_every_place_that_lacks_app_work() {
    let w = world("design-decided-lacking");
    let (a, b, c) = (
        "design/mocks/A.dc.html",
        "design/mocks/B.dc.html",
        "design/mocks/C.dc.html",
    );
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    w.commit(&[a, b, c]);
    let before = w.bytes();
    let mut request = decided_request(&w, a, work, decision);
    request["places"] = json!({ a: { "item": work.0 }, b: null, c: null });
    let refused = message(w.request(request.clone()));
    assert!(
        refused.contains(b) && refused.contains(c) && !refused.contains(&format!("{a},")),
        "{refused}"
    );
    assert_eq!(w.bytes(), before);

    request["places"] =
        json!({ a: { "item": work.0 }, b: { "item": work.0 }, c: { "item": work.0 } });
    w.request(request.clone()).unwrap();
    let id = w.state().designs[0].id.0;
    let (d, e) = ("design/mocks/D.dc.html", "design/mocks/E.dc.html");
    w.commit(&[d, e]);
    let mut again = request;
    again["places"] = json!({});
    let refused = message(w.amend(id, again));
    assert!(
        refused.contains(d) && refused.contains(e) && !refused.contains(b),
        "{refused}"
    );
}

/// Lists that hold a decided change without its decision come from a
/// broken store: `design show` and the change's view in the lists JSON fail
/// on them and name the decision, instead of leaving its title out.
#[test]
fn a_decided_change_whose_decision_the_lists_lack_fails_show_and_its_view() {
    let w = world("design-decided-broken");
    let board = "design/mocks/A.dc.html";
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    w.commit(&[board]);
    w.request(decided_request(&w, board, work, decision))
        .unwrap();
    let mut lists = w.state();
    lists.decisions.clear();
    let design = lists.designs[0].clone();
    let shown = message(show(&lists, design.id));
    assert!(
        shown.contains(&format!("decision {decision}")) && shown.contains("broken"),
        "{shown}"
    );
    let viewed = message(crate::lists::view::design_view(&lists, &design, None));
    assert!(viewed.contains("broken"), "{viewed}");
}

/// A decided change names a decision of the log: builds refuses an id no
/// record has, a record that is no decision, and a change that is both a fix
/// and a decided change, and records nothing.
#[test]
fn a_decided_change_must_name_a_decision_of_the_log() {
    let w = world("design-decided-refused");
    let board = "design/mocks/A.dc.html";
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    w.commit(&[board]);
    let before = w.bytes();
    for named in [Id(99), work] {
        let refused = message(w.request(decided_request(&w, board, work, named)));
        assert!(refused.contains("no decision"), "{refused}");
        assert_eq!(w.bytes(), before);
    }
    let mut both = decided_request(&w, board, work, decision);
    both["fix"] = json!("a drawing mistake");
    let refused = message(w.request(both));
    assert!(refused.contains("both"), "{refused}");
    assert_eq!(w.bytes(), before);
}

#[test]
fn an_amend_without_the_decision_makes_the_change_wait_on_review_again() {
    let w = world("design-decided-amend");
    let board = "design/mocks/A.dc.html";
    let work = w
        .store
        .write(&stamp(NOON), "seed", |db| {
            Ok(crate::lists::fixtures::item(db, vec![]))
        })
        .unwrap();
    let decision = decide(&w);
    w.commit(&[board]);
    w.request(decided_request(&w, board, work, decision))
        .unwrap();
    let id = w.state().designs[0].id.0;
    let amended = w
        .amend(
            id,
            json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {},
                    "review": review_of(&[board]) }),
        )
        .unwrap();
    assert!(amended.contains("waits on approval"), "{amended}");
    let design = &w.state().designs[0];
    assert!(design.approved.is_none() && design.settlement == Settlement::Review);
}

#[test]
fn an_amend_clears_the_approval_and_says_so() {
    let w = world("design-amend-clears");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" } }),
    ))
    .unwrap();
    w.approve(1);
    // A new commit draws the board again, so the approval no longer holds.
    w.commit(&["design/mocks/A.dc.html"]);
    let amended = w
        .amend(
            1,
            json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} }),
        )
        .unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
    assert!(w.state().designs[0].approved.is_none());
    // Nothing to clear the second time.
    let again = w
        .amend(
            1,
            json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} }),
        )
        .unwrap();
    assert!(!again.contains("approval is cleared"), "{again}");
}

/// A project that publishes no canvas records and lands a change too.
#[test]
fn a_project_with_no_canvas_records_and_lands_a_change() {
    let w = world("design-no-canvas");
    w.store
        .write(&stamp(NOON), "lists design-globs demo", |db| {
            crate::lists::project::set_canvas(db, false)
        })
        .unwrap();
    let sha = w.commit(&["design/mocks/A.dc.html"]);
    let board = "design/mocks/A.dc.html";
    w.request(base_request(&w, json!({ board: { "matches": "x" } })))
        .unwrap();
    let state = w.state();
    assert_eq!(
        state.designs[0].review.as_ref().unwrap().boards[0].named(),
        board
    );
    w.approve(1);
    let wt = w.worktree();
    w.run(&format!("check 1 {wt} {sha}")).unwrap();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = w.run(&format!("landed 1 {wt} {sha}")).unwrap();
    assert!(out.contains("design change #1 landed"), "{out}");
}

#[test]
fn the_canvas_index_changed_beside_a_new_board_has_no_place_on_the_review_page() {
    let w = world("design-canvas-index");
    let (board, index) = ("design/mocks/A.dc.html", "design/mocks/canvas.json");
    w.commit(&[board, index]);
    let places = json!({ board: { "matches": "x" }, index: { "matches": "places the new board" } });
    w.request(base_request(&w, places)).unwrap();
    let design = &w.state().designs[0];
    assert!(design.places.contains_key(index));
    let on_page: Vec<&str> = design
        .review
        .iter()
        .flat_map(|s| &s.boards)
        .map(|b| b.named())
        .collect();
    assert_eq!(on_page, [board]);
}

/// A record of a board change needs no canvas link, even in a project that
/// publishes a canvas (decision #702): claude.ai is only for publishing.
#[test]
fn a_board_change_records_with_no_canvas_link() {
    let w = world("design-no-link");
    let board = "design/mocks/A.dc.html";
    w.commit(&[board]);
    let before = w.bytes();
    let mut request = base_request(&w, json!({ board: null }));
    request["boards"] = json!({ board: { "link": "https://canvas.example/#A" } });
    let refused = message(w.request(request));
    assert!(refused.contains("unknown field `boards`"), "{refused}");
    assert_eq!(w.bytes(), before);
    w.request(base_request(&w, json!({ board: null }))).unwrap();
    assert!(w.state().designs[0].places.contains_key(board));
    let shown = w.run("show 1").unwrap();
    assert!(!shown.contains("canvas"), "{shown}");
}

/// The recorder picks the boards that show the proposal (decision #369): the
/// page need not hold every board the change changes, nor any board at all.
#[test]
fn a_design_change_needs_a_review_page_that_shows_the_boards_its_recorder_picks() {
    let w = world("design-review");
    w.commit(&["design/mocks/A.dc.html", "design/mocks/B.dc.html"]);
    let before = w.bytes();
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    let places = json!({ a: { "matches": "x" }, b: { "matches": "y" } });
    let with = |review: serde_json::Value| {
        let mut request = base_request(&w, places.clone());
        request["review"] = review;
        request
    };
    let mut request = base_request(&w, places.clone());
    request.as_object_mut().unwrap().remove("review");
    assert!(message(w.request(request)).contains("needs its `review`"));
    assert!(
        message(w.request(with(json!({ "heading": " ", "boards": [] })))).contains("empty heading")
    );
    assert!(
        message(w.request(with(review_of(&[a, b, "design/mocks/Gone.dc.html"]))))
            .contains("is no file at the range")
    );
    assert!(
        message(w.request(with(review_of(&[a, b, "docs/x.html"]))))
            .contains("no board place of the repo's design places")
    );
    assert!(
        message(w.request(with(review_of(&[a, b, "design/system/components/MacRow/"]))))
            .contains("is no board file")
    );
    let mut bare = review_of(&[a, b]);
    bare["boards"][0]["caption"] = json!(" ");
    assert!(message(w.request(with(bare))).contains("empty caption"));
    let mut bad_theme = review_of(&[a, b]);
    bad_theme["boards"][0]["theme"] = json!("sepia");
    assert!(message(w.request(with(bad_theme))).contains("sepia"));
    assert_eq!(w.bytes(), before);

    // The record keeps the worktree the boards render from.
    let mut review = review_of(&[b, a]);
    review["heading"] = json!("The boards");
    w.request(with(review)).unwrap();
    let design = &w.state().designs[0];
    let page = design.review.as_ref().unwrap();
    assert_eq!(
        (page.heading.as_str(), page.boards.len()),
        ("The boards", 2)
    );
    assert_eq!(page.boards[0].named(), b);
    assert!(
        std::path::Path::new(&design.worktree)
            .join("design/mocks/A.dc.html")
            .is_file()
    );
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains("review: The boards") && shown.contains("(dark) the design/mocks/B.dc.html"),
        "{shown}"
    );

    // An amend that gives a review replaces the page; one that doesn't keeps it.
    let kept = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    w.amend(1, kept).unwrap();
    assert_eq!(
        w.state().designs[0].review.as_ref().unwrap().heading,
        "The boards"
    );
    let mut other = review_of(&[a, b]);
    other["heading"] = json!("Other");
    let replaced =
        json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {}, "review": other });
    w.amend(1, replaced).unwrap();
    assert_eq!(
        w.state().designs[0].review.as_ref().unwrap().heading,
        "Other"
    );
}

/// A page that shows one of the change's two boards records, and so does a
/// page that shows none: the recorder picks what shows the proposal
/// (decision #369).
#[test]
fn a_review_page_may_leave_out_boards_the_change_changes() {
    for (name, shown) in [("design-review-some", 1), ("design-review-none", 0)] {
        let w = world(name);
        let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
        w.commit(&[a, b]);
        let mut request = base_request(&w, json!({ a: { "matches": "x" }, b: { "matches": "y" } }));
        request["review"] = review_of(&[a, b][..shown]);
        w.request(request).unwrap();
        let design = &w.state().designs[0];
        assert_eq!(design.review.as_ref().unwrap().boards.len(), shown);
        assert!(design.places.contains_key(a) && design.places.contains_key(b));
    }
}

/// A board on the page shows the proposal alone unless the recorder asks
/// for its now with `show_now` (decision #369). A board that is no file on
/// origin/main has no now to show, so a page that asks for one is refused.
#[test]
fn a_review_board_shows_its_now_only_where_the_recorder_asks() {
    let w = world("design-review-show-now");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    // The change redraws A and adds B.
    w.commit(&[a, b]);
    let places = json!({ a: { "matches": "x" }, b: { "matches": "x" } });
    let with = |now_a: bool, now_b: bool| {
        let mut request = base_request(&w, places.clone());
        request["review"]["boards"][0]["show_now"] = json!(now_a);
        request["review"]["boards"][1]["show_now"] = json!(now_b);
        request
    };
    let before = w.bytes();
    let refused = message(w.request(with(true, true)));
    assert!(
        refused.contains(b) && refused.contains("no now") && !refused.contains(a),
        "{refused}"
    );
    assert_eq!(w.bytes(), before);
    w.request(with(true, false)).unwrap();
    let page = w.state().designs[0].review.clone().unwrap();
    assert_eq!(
        page.boards.iter().map(|b| b.show_now).collect::<Vec<_>>(),
        [true, false]
    );
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains(&format!("    {a} (dark, with its now) the {a}\n"))
            && shown.contains(&format!("    {b} (dark) the {b}\n")),
        "{shown}"
    );
    // A page that leaves `show_now` out shows each board's proposal alone.
    let mut request = base_request(&w, places.clone());
    request.as_object_mut().unwrap().remove("title");
    w.amend(1, request).unwrap();
    let page = w.state().designs[0].review.clone().unwrap();
    assert!(page.boards.iter().all(|b| !b.show_now));
}

/// A review entry names a board, with its theme, or a component, whose
/// preview has no theme; the record refuses a component the design system
/// does not define at the range's last commit, a now for a component the
/// change adds, and an entry that names both, neither, or a theme the entry
/// cannot have.
#[test]
fn a_review_entry_can_be_a_components_preview() {
    let w = world("design-review-component");
    let a = "design/mocks/A.dc.html";
    let row_preview = "design/system/components/MacRow/preview.html";
    w.commit(&[a, row_preview]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    // The change redraws MacRow's preview and adds MacNew's.
    let new_preview = "design/system/components/MacNew/preview.html";
    w.commit(&[row_preview, new_preview]);
    let places = json!({
        "design/system/components/MacRow/": { "matches": "x" },
        "design/system/components/MacNew/": { "matches": "x" },
    });
    let with = |entry: serde_json::Value| {
        let mut request = base_request(&w, places.clone());
        request["review"] = json!({ "heading": "Rows", "boards": [entry] });
        request
    };
    let component = |name: &str| {
        json!({ "component": name, "caption": format!("{name}'s preview"), "show_now": true,
                "circles": [{ "selector": ".row", "label": "the row" }] })
    };
    let before = w.bytes();
    let refused = message(w.request(with(component("MacMissing"))));
    assert!(
        refused.contains("MacMissing") && refused.contains("does not define"),
        "{refused}"
    );
    let refused = message(w.request(with(component("Mac Row"))));
    assert!(refused.contains("no component name"), "{refused}");
    let refused = message(w.request(with(component("MacNew"))));
    assert!(
        refused.contains("the component MacNew") && refused.contains("no now"),
        "{refused}"
    );
    let mut themed = component("MacRow");
    themed["theme"] = json!("dark");
    let refused = message(w.request(with(themed)));
    assert!(refused.contains("takes no `theme`"), "{refused}");
    let refused = message(w.request(with(json!({ "board": a, "caption": "A" }))));
    assert!(refused.contains("needs its `theme`"), "{refused}");
    let mut both = component("MacRow");
    both["board"] = json!(a);
    let refused = message(w.request(with(both)));
    assert!(
        refused.contains("`board` file or the `component`"),
        "{refused}"
    );
    let mut nothing = component("MacRow");
    nothing.as_object_mut().unwrap().remove("component");
    let refused = message(w.request(with(nothing)));
    assert!(
        refused.contains("`board` file or the `component`"),
        "{refused}"
    );
    let mut empty = component("MacRow");
    empty["caption"] = json!(" ");
    let refused = message(w.request(with(empty)));
    assert!(
        refused.contains("the component MacRow on the review page has an empty caption"),
        "{refused}"
    );
    assert_eq!(w.bytes(), before);
    // MacRow exists on origin/main and MacNew's preview is new: the page
    // shows MacRow's now, and MacNew's proposal alone.
    let mut request = base_request(&w, places.clone());
    let mut added = component("MacNew");
    added["show_now"] = json!(false);
    request["review"] = json!({ "heading": "Rows", "boards": [component("MacRow"), added] });
    w.request(request).unwrap();
    let page = w.state().designs[0].review.clone().unwrap();
    assert_eq!(
        page.boards
            .iter()
            .map(|e| (e.shown.clone(), e.show_now))
            .collect::<Vec<_>>(),
        [
            (Shown::Component("MacRow".into()), true),
            (Shown::Component("MacNew".into()), false)
        ]
    );
    assert_eq!(page.boards[0].circles.len(), 1);
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains("    MacRow (component preview, with its now) MacRow's preview\n")
            && shown.contains("    MacNew (component preview) MacNew's preview\n"),
        "{shown}"
    );
}

#[test]
fn a_review_page_is_one_section() {
    let w = world("design-review-short");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a, b]);
    let places = json!({ a: { "matches": "x" }, b: { "matches": "x" } });
    let with = |review: serde_json::Value| {
        let mut request = base_request(&w, places.clone());
        request["review"] = review;
        request
    };
    let before = w.bytes();
    // A list of sections is no review page.
    let refused = message(w.request(with(json!([review_of(&[a, b])]))));
    assert!(
        refused.contains("review") || refused.contains("sequence"),
        "{refused}"
    );
    assert_eq!(w.bytes(), before);
    w.request(with(review_of(&[a, b]))).unwrap();
}

#[test]
fn details_are_at_most_40_words_with_no_sentence_over_15() {
    let w = world("design-details-short");
    let a = "design/mocks/A.dc.html";
    w.commit(&[a]);
    let places = json!({ a: { "matches": "x" } });
    let with = |details: String| {
        let mut request = base_request(&w, places.clone());
        request["details"] = json!(details);
        request
    };
    let before = w.bytes();
    let long = vec!["word"; 41].join(" ");
    let refused = message(w.request(with(long.clone())));
    assert!(
        refused.contains("41 words") && refused.contains("boards and circles"),
        "{refused}"
    );
    let sentence = format!("{}. Short one.", vec!["word"; 16].join(" "));
    let refused = message(w.request(with(sentence.clone())));
    assert!(refused.contains("16 words"), "{refused}");
    assert_eq!(w.bytes(), before);
    // An amend refuses the same text.
    w.request(with("Short one. Another short one.".to_string()))
        .unwrap();
    for bad in [long, sentence] {
        let mut amend = base_request(&w, places.clone());
        amend.as_object_mut().unwrap().remove("title");
        amend["details"] = json!(bad);
        assert!(message(w.amend(1, amend)).contains("details"));
    }
    let ok = format!(
        "{}. {}. {}.",
        vec!["word"; 15].join(" "),
        vec!["word"; 15].join(" "),
        ["word"; 10].join(" ")
    );
    let mut amend = base_request(&w, places.clone());
    amend.as_object_mut().unwrap().remove("title");
    amend["details"] = json!(ok);
    w.amend(1, amend).unwrap();
}

#[test]
fn a_changed_shared_source_lists_the_components_whose_drawing_changes() {
    let w = world("design-sources");
    w.commit(&[
        "design/system/components/src/10-mac.js",
        "design/system/components/MacRow/preview.html",
    ]);
    w.commit(&["design/system/components/MacButton/preview.html"]);
    let sources = w.commit(&[
        "design/system/components/src/20-android.js",
        "design/system/tokens.json",
    ]);
    let wt = w.worktree();
    assert_eq!(
        w.run(&format!("places {wt} {sources}")).unwrap(),
        "source design/system/components/src/20-android.js\nsource design/system/tokens.json\n"
    );
    // Only a source changed in this commit: it needs the components or no_app_place.
    let mut request = base_request(&w, json!({}));
    request["range"] = json!("HEAD~1..HEAD");
    let refused = message(w.request(request.clone()));
    assert!(
        refused.contains("shared design-system sources") && refused.contains("no_app_place"),
        "{refused}"
    );
    // A component that exists but the range does not change, and one that does not exist.
    let mut bad = request.clone();
    bad["places"] = json!({ "design/system/components/Nope/": { "matches": "x" } });
    assert!(message(w.request(bad)).contains("which the range does not change"));
    let mut listed = request.clone();
    listed["places"] = json!({
        "design/system/components/MacRow/": { "matches": "the row reads its colours from the tokens, unchanged" },
        "design/system/components/MacButton/": { "new": draft("Redraw the button the tokens change") },
    });
    w.request(listed).unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places.len(), 2);
    w.approve(1);
    assert!(w.run(&format!("check 1 {wt} {sources}")).is_ok());
}

#[test]
fn a_component_the_range_does_not_change_is_listed_only_when_it_changes_a_shared_source() {
    let w = world("design-affected-needs-sources");
    w.commit(&["design/system/components/MacRow/preview.html"]);
    w.commit(&["design/mocks/A.dc.html"]);
    let mut request = base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" }, "design/system/components/MacRow/": { "matches": "y" } }),
    );
    request["range"] = json!("HEAD~1..HEAD");
    let before = w.bytes();
    let refused = message(w.request(request));
    assert!(
        refused.contains("only when it changes shared design-system sources"),
        "{refused}"
    );
    assert_eq!(w.bytes(), before);
}

#[test]
fn no_app_place_says_a_source_changed_no_drawing_and_is_refused_beside_listed_components() {
    let w = world("design-sources-no-app");
    w.commit(&["design/system/components/MacRow/preview.html"]);
    w.commit(&[
        "design/system/components/src/10-mac.js",
        "design/system/README.md",
    ]);
    let mut request = base_request(&w, json!({}));
    request["range"] = json!("HEAD~1..HEAD");
    request["no_app_place"] = json!("a comment in the source");
    w.request(request.clone()).unwrap();
    w.approve(1);
    assert!(
        w.run(&format!("check 1 {} {}", w.worktree(), w.sha("HEAD")))
            .is_ok()
    );

    let w = world("design-sources-no-app-listed");
    w.commit(&["design/system/components/MacRow/preview.html"]);
    w.commit(&["design/system/components/src/10-mac.js"]);
    let mut request = base_request(
        &w,
        json!({ "design/system/components/MacRow/": { "matches": "x" } }),
    );
    request["range"] = json!("HEAD~1..HEAD");
    request["no_app_place"] = json!("none");
    assert!(
        message(w.request(request))
            .contains("record rejects it when the change lists or changes a component")
    );
}

/// A range that adds a design system with no components beside boards
/// answers the sources with no_app_place and each board with its place; a
/// range that changes a component folder still refuses no_app_place.
#[test]
fn no_app_place_answers_the_sources_beside_changed_boards() {
    let w = world("design-sources-beside-boards");
    let sha = w.commit(&[
        "design/system/tokens.json",
        "design/system/components/src/00-page.css",
        "design/system/build-bundle.py",
        "design/mocks/A.dc.html",
    ]);
    let board = "design/mocks/A.dc.html";
    let mut request = base_request(&w, json!({ board: { "matches": "the page draws it" } }));
    let refused = message(w.request(request.clone()));
    assert!(
        refused.contains("shared design-system sources") && refused.contains("no_app_place"),
        "{refused}"
    );
    request["no_app_place"] = json!("the design system has no components");
    w.request(request).unwrap();
    let state = w.state();
    assert_eq!(
        state.designs[0].no_app_place.as_deref(),
        Some("the design system has no components")
    );
    assert!(state.designs[0].places.contains_key(board));
    w.approve(1);
    w.run(&format!("check 1 {} {sha}", w.worktree())).unwrap();

    let w = world("design-sources-beside-component");
    w.commit(&[
        "design/system/tokens.json",
        "design/system/components/MacRow/preview.html",
        "design/mocks/A.dc.html",
    ]);
    let mut request = base_request(
        &w,
        json!({
            "design/mocks/A.dc.html": { "matches": "x" },
            "design/system/components/MacRow/": { "matches": "x" },
        }),
    );
    request["no_app_place"] = json!("none");
    assert!(message(w.request(request)).contains("whose drawing changes"));
}

#[test]
fn spec_sections_a_design_change_touches_are_places_resolved_like_the_others() {
    let w = world("design-spec");
    w.commit(&["spec/bae.md"]);
    git(&w.dir, &["tag", "specced"]);
    std::fs::write(w.dir.join("spec/bae.md"), "# bae\n\n## Sync\n\nnew rule\n").unwrap();
    git(&w.dir, &["add", "spec/bae.md"]);
    std::fs::create_dir_all(w.dir.join("design/mocks")).unwrap();
    std::fs::write(w.dir.join("design/mocks/A.dc.html"), "board").unwrap();
    git(&w.dir, &["add", "design/mocks/A.dc.html"]);
    git(&w.dir, &["commit", "-q", "-m", "board and spec"]);
    let sha = w.sha("HEAD");
    let wt = w.worktree();
    let places = w.run(&format!("places {wt} {sha}")).unwrap();
    assert_eq!(
        places,
        "place design/mocks/A.dc.html\nspec spec/bae.md#Sync\nspec spec/bae.md#bae\n"
    );
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request["range"] = json!("specced..HEAD");
    w.request(request.clone()).unwrap();
    w.approve(1);
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(refused.contains("spec/bae.md#Sync"), "{refused}");
    request["places"]["spec/bae.md#Sync"] =
        json!({ "new": draft("Write the sync rule into the app") });
    request["places"]["spec/bae.md#bae"] = json!({ "matches": "the title line, no behaviour" });
    w.amend(1, request).unwrap();
    assert!(w.run(&format!("check 1 {wt} {sha}")).is_ok());
    // A commit that changes only the spec is no design change, though its
    // sections are places that a design change may resolve.
    let spec_only = w.commit(&["spec/words.md"]);
    assert_eq!(
        w.run(&format!("places {wt} {spec_only}")).unwrap(),
        "spec spec/words.md#(before the first heading)\n"
    );
}

#[test]
fn a_range_whose_only_places_are_spec_sections_is_recorded_and_checked_like_any_other() {
    let w = world("design-spec-only");
    let sha = w.commit(&["spec/words.md"]);
    let wt = w.worktree();
    let place = "spec/words.md#(before the first heading)";
    let mut request = base_request(&w, json!({}));
    request["review"] = review_of(&[]);
    w.request(request.clone()).unwrap();
    w.approve(1);
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(refused.contains(place), "{refused}");
    request["places"][place] = json!({ "new": draft("Write the words into the app") });
    w.amend(1, request).unwrap();
    assert!(w.run(&format!("check 1 {wt} {sha}")).is_ok());
    // The commit is still no design change, so it needs none to land.
    assert!(message(w.run(&format!("only {wt} {sha}"))).contains("no design files"));
}

#[test]
fn only_succeeds_for_commits_that_are_all_design_and_names_the_rest() {
    let w = world("design-only");
    let design = w.commit(&[
        "design/mocks/A.dc.html",
        "design/system/tokens.json",
        "spec/bae.md",
        "design/theme.toml",
    ]);
    let wt = w.worktree();
    assert_eq!(w.run(&format!("only {wt} {design}")).unwrap(), "");
    let mixed = w.commit(&["design/mocks/B.dc.html", "src/app.rs"]);
    let refused = message(w.run(&format!("only {wt} {design} {mixed}")));
    assert!(refused.contains("src/app.rs"), "{refused}");
    let code = w.commit(&["src/b.rs"]);
    assert!(message(w.run(&format!("only {wt} {code}"))).contains("no design files"));
    let spec = w.commit(&["spec/words.md"]);
    assert!(message(w.run(&format!("only {wt} {spec}"))).contains("no design files"));
    assert!(message(w.run(&format!("only {wt}"))).contains("usage"));
}

#[test]
fn render_prints_a_board_of_a_worktree_as_one_page_in_the_theme_asked() {
    let w = world("design-render");
    crate::testing::design_system(&w.dir, "1", "", "window.Bae = {};");
    std::fs::create_dir_all(w.dir.join("design/mocks")).unwrap();
    std::fs::write(
        w.dir.join("design/mocks/mac.a.dc.html"),
        "<x-dc><p>hi</p></x-dc>",
    )
    .unwrap();
    let wt = w.worktree();
    let dark = w.run(&format!("render {wt} mac.a")).unwrap();
    assert!(
        dark.contains("data-theme=\"dark\"") && dark.contains("<p>hi</p>"),
        "{dark}"
    );
    let light = w
        .run(&format!(
            "render {wt} design/mocks/mac.a.dc.html --theme light"
        ))
        .unwrap();
    assert!(light.contains("data-theme=\"light\""));
    assert!(message(w.run(&format!("render {wt} mac.a --theme sepia"))).contains("light or dark"));
    assert!(
        message(w.run(&format!("render {wt} mac.none"))).contains("mac.none.dc.html is not there")
    );
    assert!(message(w.run(&format!("render {wt}"))).contains("usage"));
}

#[test]
fn render_reads_a_board_at_origin_main_or_at_a_commit_each_with_its_design_system() {
    let w = world("design-render-at");
    let write = |board: &str, bundle: &str| {
        std::fs::create_dir_all(w.dir.join("design/mocks")).unwrap();
        crate::testing::design_system(&w.dir, "1", "", bundle);
        std::fs::write(w.dir.join("design/mocks/mac.a.dc.html"), board).unwrap();
        git(&w.dir, &["add", "design"]);
        git(&w.dir, &["commit", "-q", "-m", "board"]);
        w.sha("HEAD")
    };
    let first = write(
        "<x-dc><p>the old board</p></x-dc>",
        "window.Bae = { OLD: 1 };",
    );
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let second = write(
        "<x-dc><p>the new board</p></x-dc>",
        "window.Bae = { NEW: 1 };",
    );
    let wt = w.worktree();
    let now = w.run(&format!("render {wt} mac.a --now")).unwrap();
    assert!(now.contains("the old board") && now.contains("OLD: 1") && !now.contains("NEW"));
    let at = w.run(&format!("render {wt} mac.a --at {second}")).unwrap();
    assert!(at.contains("the new board") && at.contains("NEW: 1") && !at.contains("OLD"));
    let earlier = w.run(&format!("render {wt} mac.a --at {first}")).unwrap();
    assert!(earlier.contains("the old board"));
    let as_checked_out = w.run(&format!("render {wt} mac.a")).unwrap();
    assert!(as_checked_out.contains("the new board"));
    let both = message(w.run(&format!("render {wt} mac.a --now --at {first}")));
    assert!(both.contains("give one"), "{both}");
    let bad = message(w.run(&format!("render {wt} mac.a --at nope")));
    assert!(bad.contains("find the commit nope"), "{bad}");
}

#[test]
fn every_named_constraint_of_the_design_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, item, row, v};
    use crate::lists::record::open;
    let db = db();
    let good = open(&db, crate::lists::Kind::Design, &stamp(1)).unwrap();
    let spare = open(&db, crate::lists::Kind::Design, &stamp(1)).unwrap();
    let worker = item(&db, vec![]);
    let design = [
        ("id", v(good.0)),
        ("title", v("A design change")),
        ("details", v("d")),
        ("repo", v("bae")),
        ("worktree", v("/d/bae-worktrees/side-2")),
    ];
    plant(&db, "design", &design).unwrap();
    let mut refused = Refusals::new(include_str!("design.sql"));
    let spare_row = |over: &[(&'static str, rusqlite::types::Value)]| {
        row(&row(&design, &[("id", v(spare.0))]), over)
    };
    refused.check(
        "design_is_a_design",
        plant(&db, "design", &spare_row(&[("record_kind", v("item"))])),
    );
    for title in ["", " x", "a\nb"] {
        refused.check(
            "design_title_is_one_line",
            plant(&db, "design", &spare_row(&[("title", v(title))])),
        );
    }
    refused.check(
        "design_details_say_something",
        plant(&db, "design", &spare_row(&[("details", v(" "))])),
    );
    refused.check_as(
        "design_repo_is_the_projects",
        "FOREIGN KEY",
        plant(&db, "design", &spare_row(&[("repo", v("nope"))])),
    );
    for worktree in ["", "d/bae-worktrees/side-2", "~/x"] {
        refused.check(
            "design_worktree_is_absolute",
            plant(&db, "design", &spare_row(&[("worktree", v(worktree))])),
        );
    }
    refused.check(
        "design_no_app_place_says_something",
        plant(&db, "design", &spare_row(&[("no_app_place", v(" "))])),
    );
    refused.check_as(
        "design_is_a_record",
        "FOREIGN KEY",
        plant(&db, "design", &row(&design, &[("id", v(900))])),
    );
    refused.check(
        "design_fix_says_what_it_fixes",
        plant(
            &db,
            "design_fix",
            &[("design_id", v(good.0)), ("fixes", v(" "))],
        ),
    );
    refused.check(
        "design_approval_by_says_something",
        plant(
            &db,
            "design_approval",
            &[("design_id", v(good.0)), ("at", v(1)), ("by", v(" "))],
        ),
    );
    let place = [
        ("design_id", v(good.0)),
        ("path", v("design/mocks/A.dc.html")),
        ("item_id", v(worker.0)),
    ];
    refused.check(
        "design_place_matches_says_something",
        plant(
            &db,
            "design_place",
            &[
                ("design_id", v(good.0)),
                ("path", v("p")),
                ("matches", v(" ")),
            ],
        ),
    );
    refused.check(
        "design_place_is_resolved_one_way",
        plant(&db, "design_place", &row(&place, &[("matches", v("m"))])),
    );
    plant(&db, "design_place", &place).unwrap();
    let deleted = [
        ("design_id", v(good.0)),
        ("path", v("design/mocks/Old.dc.html")),
    ];
    refused.check(
        "design_deleted_path_is_a_place",
        plant(
            &db,
            "design_deleted",
            &row(&deleted, &[("path", v("spec/x.md#Heading"))]),
        ),
    );
    for tip in ["abc", "not-a-sha", "ABCDEF1234"] {
        refused.check(
            "design_tip_is_a_commit",
            plant(
                &db,
                "design_tip",
                &[("design_id", v(good.0)), ("tip", v(tip))],
            ),
        );
    }
    let section = [("design_id", v(good.0)), ("heading", v("H"))];
    refused.check(
        "review_section_heading_says_something",
        plant(
            &db,
            "review_section",
            &row(&section, &[("heading", v(" "))]),
        ),
    );
    plant(&db, "review_section", &section).unwrap();
    let review = [
        ("design_id", v(good.0)),
        ("position", v(0)),
        ("board", v("design/mocks/A.dc.html")),
        ("caption", v("c")),
        ("theme", v("dark")),
        ("show_now", v(0)),
    ];
    refused.check(
        "review_board_position_not_negative",
        plant(&db, "review_board", &row(&review, &[("position", v(-1))])),
    );
    refused.check(
        "review_board_is_a_board_file",
        plant(
            &db,
            "review_board",
            &row(&review, &[("board", v("design/system/components/B/"))]),
        ),
    );
    refused.check(
        "review_board_caption_says_something",
        plant(&db, "review_board", &row(&review, &[("caption", v(" "))])),
    );
    refused.check(
        "review_board_theme_known",
        plant(&db, "review_board", &row(&review, &[("theme", v("sepia"))])),
    );
    refused.check(
        "review_board_component_is_a_name",
        plant(
            &db,
            "review_board",
            &row(
                &review,
                &[
                    ("board", rusqlite::types::Value::Null),
                    ("component", v("Mac Row")),
                    ("theme", rusqlite::types::Value::Null),
                ],
            ),
        ),
    );
    refused.check(
        "review_board_shows_a_board_or_a_component",
        plant(
            &db,
            "review_board",
            &row(&review, &[("component", v("MacRow"))]),
        ),
    );
    refused.check(
        "review_board_has_a_theme_only_on_a_board",
        plant(
            &db,
            "review_board",
            &row(
                &review,
                &[
                    ("board", rusqlite::types::Value::Null),
                    ("component", v("MacRow")),
                ],
            ),
        ),
    );
    refused.check(
        "review_board_show_now_is_a_flag",
        plant(&db, "review_board", &row(&review, &[("show_now", v(2))])),
    );
    refused.check_as(
        "review_board_is_on_a_section",
        "FOREIGN KEY",
        plant(
            &db,
            "review_board",
            &row(&review, &[("design_id", v(spare.0))]),
        ),
    );
    plant(&db, "review_board", &review).unwrap();
    let circle = [
        ("design_id", v(good.0)),
        ("board", v(0)),
        ("position", v(0)),
        ("selector", v("#disc-1")),
    ];
    refused.check(
        "review_circle_position_not_negative",
        plant(&db, "review_circle", &row(&circle, &[("position", v(-1))])),
    );
    refused.check(
        "review_circle_selector_says_something",
        plant(&db, "review_circle", &row(&circle, &[("selector", v(" "))])),
    );
    refused.check(
        "review_circle_label_says_something",
        plant(&db, "review_circle", &row(&circle, &[("label", v(" "))])),
    );
    refused.check_as(
        "review_circle_is_on_a_board",
        "FOREIGN KEY",
        plant(&db, "review_circle", &row(&circle, &[("board", v(1))])),
    );
    plant(&db, "review_circle", &circle).unwrap();
    refused.covered_without_a_name(&["design_repo_is_the_projects"]);
    refused.done();
}

#[test]
fn dropping_a_place_takes_its_board_with_it_and_a_section_its_review_boards_and_their_circles() {
    use crate::lists::fixtures::{db, design_drawn, read};
    let db = db();
    let id = design_drawn(
        &db,
        "/d/bae-worktrees/side-2",
        &["design/mocks/A.dc.html"],
        Some(ReviewSection {
            heading: "H".into(),
            boards: vec![ReviewEntry {
                shown: Shown::Board {
                    path: "design/mocks/A.dc.html".into(),
                    theme: Theme::Dark,
                },
                caption: "c".into(),
                show_now: false,
                circles: vec![Circle {
                    around: Around::Selector("#disc-1".into()),
                    label: None,
                }],
            }],
        }),
    );
    db.execute("DELETE FROM design_place WHERE design_id = ?1", [id])
        .unwrap();
    db.execute("DELETE FROM review_section WHERE design_id = ?1", [id])
        .unwrap();
    let change = read(&db).design(id).unwrap().clone();
    assert!(change.places.is_empty() && change.review.is_none());
    let circles: i64 = db
        .query_row("SELECT count(*) FROM review_circle", [], |r| r.get(0))
        .unwrap();
    assert_eq!(circles, 0);
}

#[test]
fn a_record_keeps_the_last_commit_of_its_range_and_an_amend_moves_it() {
    let w = world("design-tip");
    let a = "design/mocks/A.dc.html";
    let first = w.commit(&[a]);
    w.request(base_request(&w, json!({ a: { "matches": "x" } })))
        .unwrap();
    assert_eq!(w.state().designs[0].tip, Some(first));
    assert!(w.run("show 1").unwrap().contains("drawn at "));
    let second = w.commit(&[a]);
    let mut amend = base_request(&w, json!({ a: { "matches": "x" } }));
    amend.as_object_mut().unwrap().remove("title");
    w.amend(1, amend).unwrap();
    assert_eq!(w.state().designs[0].tip, Some(second));
}

#[test]
fn a_review_board_must_be_a_file_at_the_last_commit_not_only_in_the_worktree() {
    let w = world("design-tip-file");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a]);
    // B is in the worktree but no commit holds it.
    std::fs::write(w.dir.join(b), "uncommitted").unwrap();
    let places = json!({ a: { "matches": "x" } });
    let mut request = base_request(&w, places);
    request["review"] = review_of(&[a, b]);
    let refused = message(w.request(request));
    assert!(
        refused.contains("is no file at the range's last commit"),
        "{refused}"
    );
}

#[test]
fn a_review_board_keeps_the_circles_the_designer_names_and_so_does_a_board_the_change_adds() {
    let w = world("design-review-circles");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    // The change redraws A and adds B.
    w.commit(&[a, b]);
    let places = json!({ a: { "matches": "x" }, b: { "matches": "x" } });
    let with = |on_a: serde_json::Value, on_b: serde_json::Value| {
        let mut request = base_request(&w, places.clone());
        request["review"]["boards"][0]["circles"] = on_a;
        request["review"]["boards"][1]["circles"] = on_b;
        request
    };
    let before = w.bytes();
    let on_b = json!([{ "selector": "#y" }]);
    let refused = message(w.request(with(json!([{ "selector": " " }]), on_b.clone())));
    assert!(refused.contains("empty selector"), "{refused}");
    let refused = message(w.request(with(
        json!([{ "selector": "#x", "label": " " }]),
        on_b.clone(),
    )));
    assert!(refused.contains("empty label"), "{refused}");
    let refused = message(w.request(with(
        json!([{ "selector": "#x", "kind": "moved" }]),
        on_b.clone(),
    )));
    assert!(refused.contains("unknown field `kind`"), "{refused}");
    assert_eq!(w.bytes(), before);

    let circles = json!([
        { "selector": "#disc-1", "label": "The Disc 1 heading goes" },
        { "selector": ".row" }
    ]);
    let on_new = json!([{ "selector": ".empty", "label": "The empty state it adds" }]);
    w.request(with(circles.clone(), on_new.clone())).unwrap();
    let page = w.state().designs[0].review.clone().unwrap();
    assert_eq!(
        page.boards[0].circles,
        vec![
            Circle {
                around: Around::Selector("#disc-1".into()),
                label: Some("The Disc 1 heading goes".into()),
            },
            Circle {
                around: Around::Selector(".row".into()),
                label: None,
            },
        ]
    );
    assert_eq!(
        page.boards[1].circles,
        vec![Circle {
            around: Around::Selector(".empty".into()),
            label: Some("The empty state it adds".into()),
        }],
        "a board that is no file on origin/main takes circles too"
    );
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains("      circles #disc-1: The Disc 1 heading goes\n")
            && shown.contains("      circles .row\n")
            && shown.contains("      circles .empty: The empty state it adds\n"),
        "{shown}"
    );

    // The circles are part of the page the person approved: an amend that
    // keeps them keeps the approval, and one that changes them clears it.
    w.approve(1);
    let amend = |circles: serde_json::Value| {
        let mut request = with(circles, on_new.clone());
        request.as_object_mut().unwrap().remove("title");
        w.amend(1, request).unwrap();
        w.state().designs[0].approved.is_some()
    };
    assert!(amend(circles));
    assert!(!amend(json!([{ "selector": "#disc-1" }])));
}

/// A board takes circles where they help the reviewer, and none where the
/// proposal speaks for itself (decision #369). A circle takes the selector of
/// a containing element or a box on the board, and the page keeps their
/// order.
#[test]
fn a_board_takes_circles_or_none_and_a_circle_takes_a_selector_or_a_box() {
    let w = world("design-circle-areas");
    let (a, b) = ("design/mocks/A.dc.html", "design/mocks/B.dc.html");
    w.commit(&[a]);
    git(&w.dir, &["tag", "-f", "base"]);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    // The change adds B and leaves A as it is.
    w.commit(&[b]);
    let places = json!({ b: { "matches": "x" } });
    let page = |on_a: serde_json::Value, on_b: serde_json::Value| {
        let mut request = base_request(&w, places.clone());
        request["review"]["boards"] = json!([
            { "board": a, "caption": "A as it is", "theme": "dark", "circles": on_a },
            { "board": b, "caption": "B", "theme": "dark", "circles": on_b },
        ]);
        request
    };
    let before = w.bytes();
    for (circle, expect) in [
        (
            json!({ "selector": "#x", "box": { "x": 1, "y": 2, "width": 3, "height": 4 } }),
            "one of them",
        ),
        (json!({ "label": "words" }), "one of them"),
        (
            json!({ "box": { "x": 1, "y": 2, "width": 0, "height": 4 } }),
            "no width or no height",
        ),
        (json!({ "box": { "x": 1, "y": 2, "width": 3 } }), "height"),
        (
            json!({ "box": { "x": 1, "y": 2, "width": 3, "height": 4, "depth": 5 } }),
            "depth",
        ),
    ] {
        let refused = message(w.request(page(json!([]), json!([circle]))));
        assert!(refused.contains(expect), "{refused}");
    }
    assert_eq!(w.bytes(), before);

    let mixed = json!([
        { "box": { "x": 10, "y": 20, "width": 300, "height": 40 }, "label": "The sheet" },
        { "selector": ".sheet" },
        { "box": { "x": 0, "y": 0, "width": 5, "height": 5 } },
    ]);
    w.request(page(json!([]), mixed)).unwrap();
    let held = w.state().designs[0].review.clone().unwrap();
    assert!(held.boards[0].circles.is_empty());
    let around: Vec<Around> = held.boards[1]
        .circles
        .iter()
        .map(|c| c.around.clone())
        .collect();
    assert_eq!(
        around,
        vec![
            Around::Box(BoardBox {
                x: 10,
                y: 20,
                width: 300,
                height: 40
            }),
            Around::Selector(".sheet".into()),
            Around::Box(BoardBox {
                x: 0,
                y: 0,
                width: 5,
                height: 5
            }),
        ]
    );
    assert_eq!(
        held.boards[1].circles[0].label.as_deref(),
        Some("The sheet")
    );
    let shown = w.run("show 1").unwrap();
    assert!(
        shown.contains("      circles box 10,20 300x40: The sheet\n")
            && shown.contains("      circles .sheet\n")
            && shown.contains("      circles box 0,0 5x5\n"),
        "{shown}"
    );
    // The page's JSON gives a box circle its `box`.
    let json = serde_json::to_value(&held.boards[1].circles[0]).unwrap();
    assert_eq!(
        json,
        json!({ "box": { "x": 10, "y": 20, "width": 300, "height": 40 }, "label": "The sheet" })
    );
    // The board the change adds may also go with no circle.
    w.request(page(json!([]), json!([]))).unwrap();
    let held = w.state().designs[1].review.clone().unwrap();
    assert!(held.boards.iter().all(|b| b.circles.is_empty()));
}

#[test]
fn tokens_css_writes_the_stylesheet_of_a_worktree_from_its_tokens() {
    let w = world("design-tokens-css");
    let wt = w.worktree();
    assert!(
        message(w.run(&format!("tokens-css {wt}")))
            .contains("design/system/tokens.json is not there")
    );
    let tokens = crate::testing::tokens_json("1");
    std::fs::create_dir_all(w.dir.join("design/system")).unwrap();
    std::fs::write(w.dir.join("design/system/tokens.json"), &tokens).unwrap();
    let css = w.dir.join("design/system/tokens.css");
    assert_eq!(
        w.run(&format!("tokens-css {wt}")).unwrap(),
        format!("wrote {}\n", css.display())
    );
    assert_eq!(
        std::fs::read_to_string(&css).unwrap(),
        boards::tokens_css(&tokens).unwrap()
    );
    assert!(message(w.run("tokens-css")).contains("usage"));
}
