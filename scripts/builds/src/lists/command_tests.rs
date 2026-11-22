use super::*;
use crate::fail::Fail;
use crate::testing::{Scratch, repo};

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn refusal(result: Result<()>) -> String {
    match result.unwrap_err() {
        Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
    }
}

fn run(tool: &Tool, command: &str) -> Result<()> {
    cmd_lists(tool, &words(command))
}

#[test]
fn init_makes_the_store_with_repos_and_design_places() {
    let scratch = Scratch::new("lists-cmd-init");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae,coven --design-places bae:design/mocks/*.dc.html,bae:design/system/components/*/ --by tester").unwrap();
    let lists = Store::named(&tool.places, "bae").unwrap().load().unwrap();
    assert_eq!(lists.repos, ["bae", "coven"]);
    assert_eq!(
        lists.design_places["bae"],
        ["design/mocks/*.dc.html", "design/system/components/*/"]
    );
    assert!(lists.design_sources.is_empty() && lists.spec_files.is_empty());
    assert_eq!(lists.version, 1);
    let db = crate::db::open_reader(&Store::named(&tool.places, "bae").unwrap().database())
        .unwrap()
        .unwrap();
    assert_eq!(crate::lists::store::read_log(&db).unwrap()[0].2, "tester");
    run(&tool, "show --project bae").unwrap();
    run(&tool, "show --json --project bae").unwrap();
}

#[test]
fn init_takes_design_sources_and_spec_files() {
    let scratch = Scratch::new("lists-cmd-init-sources");
    let tool = scratch.tool(0);
    run(
        &tool,
        "init bae --repos bae --design-sources bae:design/system/components/src/,bae:design/system/tokens.json --spec-files bae:spec/*.md --by tester",
    )
    .unwrap();
    let lists = Store::named(&tool.places, "bae").unwrap().load().unwrap();
    assert_eq!(
        lists.design_sources["bae"],
        ["design/system/components/src/", "design/system/tokens.json"]
    );
    assert_eq!(lists.spec_files["bae"], ["spec/*.md"]);
}

#[test]
fn init_refuses_an_existing_store() {
    let scratch = Scratch::new("lists-cmd-twice");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae --by tester").unwrap();
    let store = Store::named(&tool.places, "bae").unwrap();
    let before = store.load().unwrap();
    assert!(
        refusal(run(&tool, "init bae --repos bae,coven --by tester")).contains("already exist")
    );
    assert_eq!(store.load().unwrap(), before);
}

#[test]
fn init_needs_its_arguments() {
    let scratch = Scratch::new("lists-cmd-args");
    let tool = scratch.tool(0);
    assert!(refusal(run(&tool, "init bae --by me")).contains("--repos"));
    assert!(refusal(run(&tool, "init --repos bae --by me")).contains("usage"));
    assert!(refusal(run(&tool, "init ../x --repos bae --by me")).contains("not a project name"));
    assert!(
        refusal(run(
            &tool,
            "init bae --repos bae --design-places nocolon --by me"
        ))
        .contains("REPO:GLOB")
    );
    assert!(
        refusal(run(
            &tool,
            "init bae --repos bae --design-places other:x --by me"
        ))
        .contains("not one of the project's")
    );
    assert!(refusal(run(&tool, "init bae --repos bae --by")).contains("--by needs a value"));
    assert!(refusal(run(&tool, "init bae --repos bae --nope 1 --by me")).contains("--nope"));
    assert!(!tool.places.lists.join("bae").exists());
}

#[test]
fn init_no_longer_loads_a_state_from_a_file() {
    let scratch = Scratch::new("lists-cmd-from");
    let tool = scratch.tool(0);
    let file = scratch.dir.join("bae.json");
    std::fs::write(&file, "{}").unwrap();
    assert!(
        refusal(run(
            &tool,
            &format!("init bae --repos bae --from {} --by me", file.display())
        ))
        .contains("--from")
    );
    assert!(!tool.places.lists.join("bae").exists());
}

#[test]
fn show_of_a_store_failing_the_check_fails_naming_the_fault() {
    let scratch = Scratch::new("lists-cmd-show-bad");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae --by me").unwrap();
    // A question that blocks nothing and says nothing of why, written around the check.
    let store = Store::named(&tool.places, "bae").unwrap();
    let db = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
    let q = crate::lists::fixtures::question(&db);
    crate::lists::fixtures::blocks_something(&db, q);
    drop(db);
    for command in ["show --project bae", "show --json --project bae"] {
        let message = refusal(run(&tool, command));
        assert!(
            message.contains("fail their check") && message.contains("blocks no open item"),
            "{message}"
        );
    }
}

#[test]
fn show_of_no_store_says_how_to_make_one() {
    let scratch = Scratch::new("lists-cmd-show-none");
    let tool = scratch.tool(0);
    assert!(refusal(run(&tool, "show --project bae")).contains("builds lists init bae"));
}

#[test]
fn project_of_answers_from_the_repo_of_the_directory() {
    let scratch = Scratch::new("lists-cmd-project-of");
    let tool = scratch.tool(0);
    let in_project = scratch.dir.join("bae");
    repo(&in_project, "target 1\n");
    run(&tool, "init bae --repos bae --by me").unwrap();
    run(&tool, &format!("project-of {}", in_project.display())).unwrap();
    assert!(
        refusal(run(&tool, &format!("project-of {}", scratch.dir.display())))
            .contains("not inside a git checkout")
    );
    assert!(refusal(run(&tool, "project-of")).contains("usage"));
    run(&tool, "init again --repos bae --by me").unwrap();
    assert!(
        refusal(run(&tool, &format!("project-of {}", in_project.display())))
            .contains("two projects")
    );
}

#[test]
fn an_unknown_command_is_refused() {
    let scratch = Scratch::new("lists-cmd-unknown");
    let tool = scratch.tool(0);
    assert!(refusal(run(&tool, "frobnicate")).contains("unknown lists command frobnicate"));
    assert!(refusal(cmd_lists(&tool, &[])).contains("usage"));
}

#[test]
fn add_repo_lets_a_repo_whose_work_a_project_carries_resolve_to_it() {
    let scratch = Scratch::new("lists-cmd-add-repo");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae,coven --by tester").unwrap();
    let carried = scratch.dir.join("agent-instructions");
    repo(&carried, "target 1\n");
    let checkout = Checkout::at(&carried).unwrap().unwrap();
    assert_eq!(project_of(&tool.places, &checkout, None).unwrap(), None);
    run(&tool, "add-repo bae agent-instructions --by tester").unwrap();
    let lists = Store::named(&tool.places, "bae").unwrap().load().unwrap();
    assert_eq!(lists.repos, ["agent-instructions", "bae", "coven"]);
    let db = crate::db::open_reader(&Store::named(&tool.places, "bae").unwrap().database())
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::lists::store::read_log(&db).unwrap().pop().unwrap().3,
        "lists add-repo bae agent-instructions"
    );
    assert_eq!(
        project_of(&tool.places, &checkout, None).unwrap(),
        Some("bae".to_string())
    );
    let none = Args::parse(&[], &[], &[]).unwrap();
    assert_eq!(
        Store::resolve(&tool.places, &none, &carried)
            .unwrap()
            .project,
        "bae"
    );
}

#[test]
fn add_repo_refuses_a_repo_already_in_a_project_and_a_missing_project() {
    let scratch = Scratch::new("lists-cmd-add-repo-refuse");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae,coven --by tester").unwrap();
    run(&tool, "init other --repos web --by tester").unwrap();
    assert!(
        refusal(run(&tool, "add-repo bae coven --by tester"))
            .contains("already one of project bae's")
    );
    // Another project's repo is allowed: two projects may hold one repo.
    run(&tool, "add-repo bae web --by tester").unwrap();
    assert!(
        refusal(run(&tool, "add-repo nope web2 --by tester")).contains("no lists for project nope")
    );
    assert!(
        refusal(run(&tool, "add-repo bae a/b --by tester")).contains("not a repo directory name")
    );
    assert!(refusal(run(&tool, "add-repo bae --by tester")).contains("usage"));
    assert_eq!(
        Store::named(&tool.places, "bae")
            .unwrap()
            .load()
            .unwrap()
            .repos,
        ["bae", "coven", "web"]
    );
}

/// A store made with no design globs gets them later: each kind given
/// replaces that kind's globs for the repos it names, and the rest stay.
/// Health names a repo whose main checkout has a design folder until someone
/// runs design-globs with --design-places for that repo.
#[test]
fn design_globs_sets_the_globs_of_an_existing_store() {
    let scratch = Scratch::new("lists-cmd-design-globs");
    let tool = scratch.tool(0);
    run(
        &tool,
        "init bae --repos bae,coven --spec-files coven:spec/*.md --by tester",
    )
    .unwrap();
    std::fs::create_dir_all(tool.places.home.join("dev/bae/design/mocks")).unwrap();
    let missing = || {
        crate::lists::project::missing_globs_findings(
            &tool.places.home,
            &crate::lists::store::load_all(&tool.places),
        )
    };
    let found = missing();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .text
            .starts_with("lists: repo bae has a design folder")
    );
    assert!(
        found[0]
            .command
            .as_deref()
            .is_some_and(|command| command.starts_with("builds lists design-globs bae ")),
        "{found:?}"
    );

    run(&tool, "design-globs bae --design-places bae:design/mocks/*.html,bae:design/mocks/canvas.json --design-sources bae:design/system/** --by tester").unwrap();
    run(
        &tool,
        "design-globs bae --design-places bae:design/mocks/*.html --by tester",
    )
    .unwrap();
    let lists = Store::named(&tool.places, "bae").unwrap().load().unwrap();
    assert_eq!(lists.design_places["bae"], ["design/mocks/*.html"]);
    assert_eq!(lists.design_sources["bae"], ["design/system/**"]);
    assert_eq!(lists.spec_files["coven"], ["spec/*.md"]);
    assert!(missing().is_empty());

    assert!(
        refusal(run(
            &tool,
            "design-globs bae --design-places forage:design/*.html"
        ))
        .contains("repo forage, which is not one of the project's")
    );
    assert!(refusal(run(&tool, "design-globs bae")).contains("needs --canvas, --design-places"));

    // A project publishes no canvas until someone runs init or design-globs
    // with --canvas yes, and --canvas alone changes only the canvas setting.
    let canvas = || {
        Store::named(&tool.places, "bae")
            .unwrap()
            .load()
            .unwrap()
            .canvas
    };
    assert!(!canvas());
    run(&tool, "design-globs bae --canvas yes --by tester").unwrap();
    assert!(canvas());
    assert_eq!(
        Store::named(&tool.places, "bae")
            .unwrap()
            .load()
            .unwrap()
            .design_places["bae"],
        ["design/mocks/*.html"]
    );
    run(&tool, "design-globs bae --canvas no --by tester").unwrap();
    assert!(!canvas());
    assert!(
        refusal(run(&tool, "design-globs bae --canvas maybe")).contains("--canvas is yes or no")
    );
    run(&tool, "init drawn --repos web --canvas yes --by tester").unwrap();
    assert!(
        Store::named(&tool.places, "drawn")
            .unwrap()
            .load()
            .unwrap()
            .canvas
    );
    assert!(
        refusal(run(&tool, "design-globs nobody --spec-files bae:spec/*.md")).contains("nobody")
    );

    // A repo two projects hold needs design globs in one of them only.
    run(&tool, "init builds --repos tools --by tester").unwrap();
    run(&tool, "add-repo bae tools --by tester").unwrap();
    std::fs::create_dir_all(tool.places.home.join("dev/tools/design/mocks")).unwrap();
    assert_eq!(missing().len(), 2, "{:?}", missing());
    run(
        &tool,
        "design-globs builds --design-places tools:design/mocks/*.dc.html --by tester",
    )
    .unwrap();
    assert!(missing().is_empty(), "{:?}", missing());
}
