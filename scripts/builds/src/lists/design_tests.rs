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
        if matches!(args.first().map(String::as_str), Some("show" | "list")) {
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

fn base_request(world: &World, places: serde_json::Value) -> serde_json::Value {
    json!({ "title": "New cover grid", "details": "the boards", "worktree": world.worktree(), "range": "base..HEAD", "places": places })
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
    assert!(w.run("list").unwrap().contains("New cover grid (repo bae, 3 places, 1 item waiting)"));
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
        let refused = message(w.request(base_request(&w, places)));
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
        crate::lists::draft::close(&mut state, id, How::Dropped("gone".into()), &stamp(NOON)).unwrap();
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
fn amend_merges_places_over_a_new_range() {
    let w = world("design-amend");
    w.commit(&["design/mocks/A.dc.html"]);
    w.request(base_request(&w, json!({ "design/mocks/A.dc.html": { "matches": "x" } }))).unwrap();
    let first = w.sha("HEAD");
    w.commit(&["design/mocks/B.dc.html", "design/system/components/Sym/a.html"]);
    let before = w.bytes();
    let mut request = json!({ "worktree": w.worktree(), "range": format!("{first}..HEAD"),
                              "places": { "design/mocks/B.dc.html": { "new": draft("Build the grid the board shows") } } });
    let refused = message(w.amend(1, request.clone()));
    assert!(refused.contains("design/system/components/Sym/"), "{refused}");
    assert_eq!(w.bytes(), before);
    request["places"]["design/system/components/Sym/"] = json!({ "matches": "y" });
    let out = w.amend(1, request).unwrap();
    let design = &w.state().designs[0];
    assert_eq!(design.places.len(), 3);
    assert_eq!(design.places["design/mocks/A.dc.html"], Resolution::Matches("x".into()));
    assert!(out.contains("wait on it"), "{out}");
    assert_eq!(w.state().items[0].waits, [Wait::Design(design.id)]);
    let mut stale = json!({ "worktree": w.worktree(), "range": "base..HEAD", "places": {} });
    stale["places"] = json!({});
    w.amend(1, stale).unwrap();
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
    assert_eq!(design.landed[0].commits, [sha.clone()]);
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
