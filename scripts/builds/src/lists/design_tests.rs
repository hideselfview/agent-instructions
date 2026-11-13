use super::*;
use crate::fail::Fail;
use crate::lists::design_command::run_to_text;
use crate::lists::fixtures::{NOON, lists, stamp};
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
    git(&dir, &["tag", "base"]);
    let tool = scratch.tool(0);
    let mut state = lists();
    state.design_places.insert("bae".into(), vec!["design/mocks/*.dc.html".into(), "design/system/components/*/".into()]);
    state.design_sources.insert(
        "bae".into(),
        vec!["design/system/components/src/".into(), "design/system/tokens.json".into(), "design/system/tokens.css".into()],
    );
    state.spec_files.insert("bae".into(), vec!["spec/*.md".into()]);
    state.version = 0;
    state.log.clear();
    let store = Store::named(&tool.places, "demo").unwrap();
    store.create(state, &stamp(NOON), "lists init demo").unwrap();
    World { scratch, tool, dir, store }
}

impl World {
    fn commit(&self, files: &[&str]) -> String {
        for path in files {
            let file = self.dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, format!("{path} at {}", self.sha("HEAD"))).unwrap();
            git(&self.dir, &["add", path]);
        }
        git(&self.dir, &["commit", "-q", "-m", &format!("change {}", files.join(" "))]);
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
        if matches!(args.first().map(String::as_str), Some("show" | "list" | "approve")) {
            args.extend(["--project".to_string(), "demo".to_string()]);
        }
        run_to_text(&self.tool, &args)
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
        self.run(&format!("approve {id} --by-person")).unwrap();
    }

    fn state(&self) -> Lists {
        self.store.load().unwrap()
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.store.state_path()).unwrap()
    }

    fn worktree(&self) -> String {
        self.dir.display().to_string()
    }
}

fn draft(title: &str) -> serde_json::Value {
    json!({ "title": title, "area": "Alpha", "details": "do it", "for": "coordinator", "waits": [] })
}

/// A review page with one section holding <boards>, each in the dark.
fn review_of(boards: &[&str]) -> serde_json::Value {
    let boards: Vec<serde_json::Value> =
        boards.iter().map(|b| json!({ "board": b, "caption": format!("the {b}"), "theme": "dark" })).collect();
    json!([{ "heading": "What changed", "words": "the words under it", "boards": boards }])
}

/// A request for `base..HEAD` answering <places>, with a canvas link and a place on the review page for each board among them.
fn base_request(world: &World, places: serde_json::Value) -> serde_json::Value {
    let boards: serde_json::Map<String, serde_json::Value> = places
        .as_object()
        .unwrap()
        .keys()
        .filter(|path| !path.ends_with('/') && !path.contains('#'))
        .map(|path| (path.clone(), json!({ "link": format!("https://canvas.example/#{path}") })))
        .collect();
    let review = review_of(&boards.keys().map(String::as_str).collect::<Vec<_>>());
    json!({ "title": "New cover grid", "details": "the boards", "worktree": world.worktree(), "range": "base..HEAD",
            "places": places, "boards": boards, "review": review })
}

#[test]
fn places_prints_each_place_once_and_nothing_when_there_are_none() {
    let w = world("design-places");
    let a = w.commit(&["design/mocks/A.dc.html", "design/system/components/MacRow/a.html", "src/x.rs"]);
    let b = w.commit(&["design/system/components/MacRow/b.html", "design/theme.toml"]);
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
    assert_eq!(w.run(&format!("places {wt} {shared}")).unwrap(), "shared design/theme.toml\n");
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("a palette tweak the boards do not show");
    w.request(request).unwrap();
    w.approve(1);
    assert!(w.run(&format!("check 1 {wt} {shared}")).is_ok());
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    assert!(w.run(&format!("landed 1 {wt} {shared}")).unwrap().contains("landed"));
}

#[test]
fn places_in_a_repo_of_no_project_is_refused() {
    let w = world("design-places-noproject");
    let other = w.scratch.dir.join("other");
    repo(&other, "target 1\n");
    let sha = w.sha("HEAD");
    let _ = sha;
    let head = crate::lists::design_paths::full_sha(&other, "HEAD").unwrap();
    assert!(message(w.run(&format!("places {} {head}", other.display()))).contains("is in no project"));
}

#[test]
fn record_refuses_each_unresolved_place_by_name_and_changes_nothing() {
    let w = world("design-unresolved");
    w.commit(&["design/mocks/A.dc.html", "design/mocks/B.dc.html", "design/system/components/MacRow/a.html"]);
    let before = w.bytes();
    let refused = message(w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "the app shows it" } }))));
    assert!(refused.contains("design/mocks/B.dc.html") && refused.contains("design/system/components/MacRow/"), "{refused}");
    assert!(!refused.contains("A.dc.html,") && !refused.contains("mocks/A.dc.html;"), "{refused}");
    assert_eq!(w.bytes(), before);
}

#[test]
fn record_resolves_places_by_item_new_item_and_matches() {
    let w = world("design-record");
    let existing = {
        let mut state = w.state();
        let id = crate::lists::fixtures::item(&mut state, vec![]);
        std::fs::write(w.store.state_path(), serde_json::to_string(&state).unwrap()).unwrap();
        id
    };
    w.commit(&["design/mocks/A.dc.html", "design/mocks/B.dc.html", "design/system/components/MacRow/a.html"]);
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
    let new = state.items.iter().find(|i| i.title == "Build the grid the board shows").unwrap();
    assert_eq!(new.waits, [Wait::Design(design.id)]);
    assert_eq!(new.from, Some(design.id));
    assert_eq!(design.places["design/mocks/B.dc.html"], Resolution::Item(new.id));
    assert_eq!(design.places["design/mocks/A.dc.html"], Resolution::Item(existing));
    assert!(out.contains(&format!("new items {} wait on it", new.id)), "{out}");
    assert_eq!(derive::item_state(&state, new), ItemState::Waiting);
    let shown = w.run(&format!("show {}", design.id.0)).unwrap();
    assert!(shown.contains("design/mocks/B.dc.html") && shown.contains("compared the component"), "{shown}");
    assert!(w.run("list").unwrap().contains("New cover grid (waiting on review, repo bae, 3 places, 1 item waiting)"));
}

#[test]
fn record_refuses_bad_answers_and_changes_nothing() {
    let w = world("design-bad-answers");
    w.commit(&["design/mocks/A.dc.html"]);
    let before = w.bytes();
    let board = "design/mocks/A.dc.html";
    let cases = [
        (json!({ board: { "matches": "  " } }), "no how it was checked"),
        (json!({ board: { "item": 99 } }), "no record has"),
        (json!({ board: { "matches": "x" }, "design/mocks/Z.dc.html": { "matches": "x" } }), "does not change"),
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
    let mut bad = base_request(&w, json!({ board: { "new": draft("Build the grid the board shows") } }));
    bad["places"][board]["new"]["brief"] = json!("plans/missing.md");
    assert!(message(w.request(bad)).contains("does not exist"));
    assert_eq!(w.bytes(), before);
}

#[test]
fn record_refuses_an_item_that_is_closed() {
    let w = world("design-closed-item");
    let id = {
        let mut state = w.state();
        let id = crate::lists::fixtures::item(&mut state, vec![]);
        crate::lists::record::close(&mut state, id, How::Dropped("gone".into()), &stamp(NOON)).unwrap();
        std::fs::write(w.store.state_path(), serde_json::to_string(&state).unwrap()).unwrap();
        id
    };
    w.commit(&["design/mocks/A.dc.html"]);
    let refused = message(w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "item": id.0 } }))));
    assert!(refused.contains("closed"), "{refused}");
}

#[test]
fn shared_files_alone_need_no_app_place_and_it_is_refused_beside_places() {
    let w = world("design-shared");
    w.commit(&["design/theme.toml"]);
    let refused = message(w.request(base_request(&w, json!({}))));
    assert!(refused.contains("design/theme.toml") && refused.contains("no_app_place"), "{refused}");
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("   ");
    assert!(message(w.request(request)).contains("no_app_place is empty"));
    let mut request = base_request(&w, json!({}));
    request["no_app_place"] = json!("a palette tweak the boards do not show");
    w.request(request).unwrap();
    assert_eq!(w.state().designs[0].no_app_place.as_deref(), Some("a palette tweak the boards do not show"));

    let w = world("design-shared-with-place");
    w.commit(&["design/theme.toml", "design/mocks/A.dc.html"]);
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request["no_app_place"] = json!("why");
    assert!(message(w.request(request)).contains("only for a range that changes shared design files"));
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
fn amend_holds_a_change_to_what_its_new_range_changes_so_a_board_renamed_before_landing_drops_out() {
    let w = world("design-amend-rename");
    let (old, new) = ("design/mocks/mac.old.dc.html", "design/mocks/all.new.dc.html");
    w.commit(&[old]);
    let mut request = base_request(&w, json!({ old: { "matches": "x" } }));
    request["review"] = review_of(&[old]);
    w.request(request).unwrap();
    // The board is renamed before landing and the history rewritten, so the range changes only the new name.
    git(&w.dir, &["reset", "-q", "--hard", "base"]);
    w.commit(&[new, "design/mocks/other.dc.html"]);
    let other = "design/mocks/other.dc.html";
    let mut amend = base_request(&w, json!({ new: { "matches": "y" }, other: { "matches": "z" } }));
    amend["review"] = review_of(&[new, other]);
    w.amend(1, amend).unwrap();
    let design = &w.state().designs[0];
    let places: Vec<&str> = design.places.keys().map(String::as_str).collect();
    assert_eq!(places, [new, other], "the renamed-away place is gone");
    assert!(!design.boards.contains_key(old), "with its board");
    assert!(design.review.iter().flat_map(|s| &s.boards).all(|b| b.board != old), "and its place on the page");

    // A place the range does change is still refused until it is answered.
    w.commit(&["design/mocks/third.dc.html"]);
    let refused = message(w.amend(1, json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} })));
    assert!(refused.contains("unresolved") && refused.contains("third.dc.html"), "{refused}");
}

#[test]
fn amend_answers_the_places_of_a_new_range_and_drops_those_it_no_longer_changes() {
    let w = world("design-amend");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    let first = w.sha("HEAD");
    w.commit(&["design/mocks/B.dc.html", "design/system/components/Sym/a.html"]);
    let before = w.bytes();
    let mut request = json!({ "worktree": w.worktree(), "range": format!("{first}..HEAD"),
                              "places": { "design/mocks/B.dc.html": { "new": draft("Build the grid the board shows") } },
                              "boards": { "design/mocks/B.dc.html": { "link": "https://canvas.example/#B" } },
                              "review": review_of(&["design/mocks/A.dc.html", "design/mocks/B.dc.html"]) });
    let refused = message(w.amend(1, request.clone()));
    assert!(refused.contains("design/system/components/Sym/"), "{refused}");
    assert_eq!(w.bytes(), before);
    request["places"]["design/system/components/Sym/"] = json!({ "matches": "y" });
    let out = w.amend(1, request).unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places.len(), 2, "A is not in the range first..HEAD");
    assert!(!design.places.contains_key("design/mocks/A.dc.html") && !design.boards.contains_key("design/mocks/A.dc.html"));
    assert!(out.contains("wait on it"), "{out}");
    assert_eq!(w.state().items[0].waits, [Wait::Design(design.id)]);
    // A range that changes A again needs A answered again: its earlier answer went with it.
    let stale = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    assert!(message(w.amend(1, stale)).contains("unresolved: design/mocks/A.dc.html"));
    let again = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": { "design/mocks/A.dc.html": { "matches": "again" } },
                        "boards": { "design/mocks/A.dc.html": { "link": "https://canvas.example/#A" } } });
    w.amend(1, again).unwrap();
}

#[test]
fn check_compares_by_path_so_a_rebased_commit_still_checks() {
    let w = world("design-check");
    let original = w.commit(&["design/mocks/A.dc.html", "design/system/components/MacRow/a.html"]);
    w.request(base_request(
        &w,
        json!({ "design/mocks/A.dc.html": { "matches": "x" }, "design/system/components/MacRow/": { "matches": "y" } }),
    ))
    .unwrap();
    w.approve(1);
    let check = format!("check 1 {}", w.worktree());
    assert!(w.run(&format!("{check} {original}")).unwrap().contains("resolves every place"));
    // The same change as another commit: cherry-picked onto another base.
    git(&w.dir, &["checkout", "-q", "-b", "other", "base"]);
    w.commit(&["unrelated.txt"]);
    git(&w.dir, &["cherry-pick", &original]);
    let picked = w.sha("HEAD");
    assert_ne!(picked, original);
    assert!(w.run(&format!("{check} {picked}")).is_ok());
}

#[test]
fn check_names_what_is_unresolved_and_refuses_a_closed_or_missing_change() {
    let w = world("design-check-refused");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    w.approve(1);
    let later = w.commit(&["design/mocks/B.dc.html", "design/theme.toml"]);
    let wt = w.worktree();
    let refused = message(w.run(&format!("check 1 {wt} {later}")));
    assert!(refused.contains("design/mocks/B.dc.html") && !refused.contains("mocks/A"), "{refused}");
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
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    w.run(&format!("landed 1 {} {sha}", w.worktree())).unwrap();
    assert!(message(w.run(&format!("check 1 {} {sha}", w.worktree()))).contains("already landed"));
}

#[test]
fn landed_closes_the_change_frees_its_items_and_is_idempotent() {
    let w = world("design-landed");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "new": draft("Build the grid the board shows") } }))).unwrap();
    let sha = w.sha("HEAD");
    let wt = w.worktree();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "base"]);
    assert!(message(w.run(&format!("landed 1 {wt} {sha}"))).contains("is not on origin/main"));
    assert_eq!(w.state().designs.len(), 1);
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = w.run(&format!("landed 1 {wt} {sha}")).unwrap();
    assert!(out.contains("design change #1 landed") && out.contains("item #2") && out.contains("is ready"), "{out}");
    let state = w.state();
    assert!(state.designs.is_empty());
    let closed = state.closed_record(Id(1)).unwrap();
    assert_eq!(closed.how, How::Landed(vec![sha.clone()]));
    let Record::Design(design) = &closed.record else { panic!("a design change") };
    assert_eq!(design.landed[0].commits, std::slice::from_ref(&sha));
    assert_eq!(derive::item_state(&state, &state.items[0]), ItemState::Ready);

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
fn a_design_change_waited_on_by_an_item_that_also_waits_elsewhere_stays_waiting() {
    let w = world("design-landed-waits");
    w.commit(&["design/mocks/A.dc.html"]);
    let mut grid = draft("Build the grid the board shows");
    grid["waits"] = json!([{ "condition": "a quiet machine" }]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "new": grid } }))).unwrap();
    git(&w.dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = w.run(&format!("landed 1 {} {}", w.worktree(), w.sha("HEAD"))).unwrap();
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
    let out = w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    assert!(out.contains("waits on the person's review"), "{out}");
    assert!(w.state().designs[0].approved.is_none());
    assert!(w.run("list").unwrap().contains("waiting on review"));
    let wt = w.worktree();
    let refused = message(w.run(&format!("check 1 {wt} {sha}")));
    assert!(refused.contains("waits on the person's review") && refused.contains("approve 1 --by-person"), "{refused}");

    let before = w.bytes();
    assert!(message(w.run("approve 1")).contains("--by-person"));
    assert_eq!(w.bytes(), before);
    assert!(message(w.run("approve 9 --by-person")).contains("not a record"));
    assert!(w.run("approve 1 --by-person").unwrap().contains("approved"));
    assert!(w.state().designs[0].approved.is_some());
    assert!(w.run(&format!("check 1 {wt} {sha}")).unwrap().contains("and is approved"));
    assert!(w.run("show 1").unwrap().contains("approved"));
    assert!(w.run("list").unwrap().contains("(approved,"));

    // The same approval again writes nothing.
    let once = w.bytes();
    assert!(w.run("approve 1 --by-person").unwrap().contains("already approved"));
    assert_eq!(w.bytes(), once);
}

#[test]
fn an_amend_clears_the_approval_and_says_so() {
    let w = world("design-amend-clears");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    w.approve(1);
    let amended = w.amend(1, json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} })).unwrap();
    assert!(amended.contains("approval is cleared"), "{amended}");
    assert!(w.state().designs[0].approved.is_none());
    // Nothing to clear the second time.
    let again = w.amend(1, json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} })).unwrap();
    assert!(!again.contains("approval is cleared"), "{again}");
}

#[test]
fn every_board_needs_a_canvas_link() {
    let w = world("design-boards");
    w.commit(&["design/mocks/A.dc.html"]);
    let before = w.bytes();
    let board = "design/mocks/A.dc.html";
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["boards"] = json!({});
    let refused = message(w.request(request));
    assert!(refused.contains("boards without a canvas link") && refused.contains(board), "{refused}");
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["boards"] = json!({ board: { "link": "  " } });
    assert!(message(w.request(request)).contains("empty canvas link"));
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["boards"] = json!({ board: { "link": "https://c/#A" }, "design/mocks/Z.dc.html": { "link": "https://c/#Z" } });
    assert!(message(w.request(request)).contains("is no place of this design change"));
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["boards"] = json!({ board: { "link": "https://c/#A" }, "design/system/components/MacRow/": { "link": "https://c/#R" } });
    assert!(message(w.request(request)).contains("is no board file"));
    // A render, an image of the board, is gone: the board is drawn from the worktree.
    let mut request = base_request(&w, json!({ board: { "matches": "x" } }));
    request["boards"] = json!({ board: { "link": "https://c/#A", "render": "/tmp/A.png" } });
    assert!(message(w.request(request)).contains("unknown field `render`"));
    assert_eq!(w.bytes(), before);

    w.request(base_request(&w, json!({ board: { "matches": "x" } }))).unwrap();
    assert_eq!(w.state().designs[0].boards[board].link, "https://canvas.example/#design/mocks/A.dc.html");
}

#[test]
fn a_design_change_needs_a_review_page_with_every_board_of_the_change_on_it() {
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
    assert!(message(w.request(with(json!([])))).contains("has no section"));
    assert!(message(w.request(with(json!([{ "heading": " ", "boards": [] }])))).contains("empty heading"));
    let refused = message(w.request(with(review_of(&[a]))));
    assert!(refused.contains("missing from the review page") && refused.contains(b) && !refused.contains(&format!("{a},")), "{refused}");
    assert!(message(w.request(with(review_of(&[a, b, "design/mocks/Gone.dc.html"])))).contains("is no file in"));
    assert!(message(w.request(with(review_of(&[a, b, "docs/x.html"])))).contains("no board place of the repo's design places"));
    assert!(message(w.request(with(review_of(&[a, b, "design/system/components/MacRow/"])))).contains("is no board file"));
    let mut bare = review_of(&[a, b]);
    bare[0]["boards"][0]["caption"] = json!(" ");
    assert!(message(w.request(with(bare))).contains("empty caption"));
    let mut bad_theme = review_of(&[a, b]);
    bad_theme[0]["boards"][0]["theme"] = json!("sepia");
    assert!(message(w.request(with(bad_theme))).contains("sepia"));
    assert_eq!(w.bytes(), before);

    // Sections keep their order; the record keeps the worktree the boards render from.
    let review = json!([
        { "heading": "Words only", "words": "just words" },
        { "heading": "The boards", "boards": [{ "board": b, "caption": "B", "theme": "light" }, { "board": a, "caption": "A", "theme": "dark" }] },
    ]);
    w.request(with(review)).unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.review.len(), 2);
    assert_eq!((design.review[0].heading.as_str(), design.review[0].boards.len()), ("Words only", 0));
    assert_eq!(design.review[1].boards[0].board, b);
    assert!(std::path::Path::new(&design.worktree).join("design/mocks/A.dc.html").is_file());
    let shown = w.run("show 1").unwrap();
    assert!(shown.contains("review: The boards") && shown.contains("(light) B"), "{shown}");

    // An amend that gives a review replaces the page; one that doesn't keeps it.
    let kept = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    w.amend(1, kept).unwrap();
    assert_eq!(w.state().designs[0].review.len(), 2);
    let replaced = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {}, "review": review_of(&[a, b]) });
    w.amend(1, replaced).unwrap();
    assert_eq!(w.state().designs[0].review.len(), 1);
}

#[test]
fn a_changed_shared_source_lists_the_components_whose_drawing_changes() {
    let w = world("design-sources");
    w.commit(&["design/system/components/src/10-mac.js", "design/system/components/MacRow/preview.html"]);
    w.commit(&["design/system/components/MacButton/preview.html"]);
    let sources = w.commit(&["design/system/components/src/20-android.js", "design/system/tokens.json"]);
    let wt = w.worktree();
    assert_eq!(
        w.run(&format!("places {wt} {sources}")).unwrap(),
        "source design/system/components/src/20-android.js\nsource design/system/tokens.json\n"
    );
    // Only a source changed in this commit: it needs the components or no_app_place.
    let mut request = base_request(&w, json!({}));
    request["range"] = json!("HEAD~1..HEAD");
    let refused = message(w.request(request.clone()));
    assert!(refused.contains("shared design-system sources") && refused.contains("no_app_place"), "{refused}");
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
    assert!(refused.contains("only when it changes shared design-system sources"), "{refused}");
    assert_eq!(w.bytes(), before);
}

#[test]
fn no_app_place_says_a_source_changed_no_drawing_and_is_refused_beside_listed_components() {
    let w = world("design-sources-no-app");
    w.commit(&["design/system/components/MacRow/preview.html"]);
    w.commit(&["design/system/components/src/10-mac.js", "design/system/README.md"]);
    let mut request = base_request(&w, json!({}));
    request["range"] = json!("HEAD~1..HEAD");
    request["no_app_place"] = json!("a comment in the source");
    w.request(request.clone()).unwrap();
    w.approve(1);
    assert!(w.run(&format!("check 1 {} {}", w.worktree(), w.sha("HEAD"))).is_ok());

    let w = world("design-sources-no-app-listed");
    w.commit(&["design/system/components/MacRow/preview.html"]);
    w.commit(&["design/system/components/src/10-mac.js"]);
    let mut request = base_request(&w, json!({ "design/system/components/MacRow/": { "matches": "x" } }));
    request["range"] = json!("HEAD~1..HEAD");
    request["no_app_place"] = json!("none");
    assert!(message(w.request(request)).contains("lists no component whose drawing changes"));
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
    assert_eq!(places, "place design/mocks/A.dc.html\nspec spec/bae.md#Sync\nspec spec/bae.md#bae\n");
    let mut request = base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }));
    request["range"] = json!("specced..HEAD");
    let refused = message(w.request(request.clone()));
    assert!(refused.contains("spec/bae.md#Sync"), "{refused}");
    request["places"]["spec/bae.md#Sync"] = json!({ "new": draft("Write the sync rule into the app") });
    request["places"]["spec/bae.md#bae"] = json!({ "matches": "the title line, no behaviour" });
    w.request(request).unwrap();
    w.approve(1);
    assert!(w.run(&format!("check 1 {wt} {sha}")).is_ok());
    // A commit that changes only the spec is no design change.
    let spec_only = w.commit(&["spec/words.md"]);
    assert_eq!(w.run(&format!("places {wt} {spec_only}")).unwrap(), "");
}

#[test]
fn only_succeeds_for_commits_that_are_all_design_and_names_the_rest() {
    let w = world("design-only");
    let design = w.commit(&["design/mocks/A.dc.html", "design/system/tokens.json", "spec/bae.md", "design/theme.toml"]);
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
    let system = w.dir.join("design/system");
    std::fs::create_dir_all(system.join("components")).unwrap();
    std::fs::create_dir_all(w.dir.join("design/mocks")).unwrap();
    std::fs::write(system.join("tokens.css"), ":root{}").unwrap();
    std::fs::write(system.join("components/bundle.css"), "").unwrap();
    std::fs::write(system.join("components/bundle.js"), "window.Bae = {};").unwrap();
    std::fs::write(w.dir.join("design/mocks/mac.a.dc.html"), "<x-dc><p>hi</p></x-dc>").unwrap();
    let wt = w.worktree();
    let dark = w.run(&format!("render {wt} mac.a")).unwrap();
    assert!(dark.contains("data-theme=\"dark\"") && dark.contains("<p>hi</p>"), "{dark}");
    let light = w.run(&format!("render {wt} design/mocks/mac.a.dc.html --theme light")).unwrap();
    assert!(light.contains("data-theme=\"light\""));
    assert!(message(w.run(&format!("render {wt} mac.a --theme sepia"))).contains("light or dark"));
    assert!(message(w.run(&format!("render {wt} mac.none"))).contains("mac.none.dc.html is not there"));
    assert!(message(w.run(&format!("render {wt}"))).contains("usage"));
}
