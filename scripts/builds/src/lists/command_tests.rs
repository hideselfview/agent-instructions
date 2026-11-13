use super::*;
use crate::lists::Id;
use crate::testing::{Scratch, repo};

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

fn refusal(result: Result<()>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
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
    assert_eq!(lists.design_places["bae"], ["design/mocks/*.dc.html", "design/system/components/*/"]);
    assert_eq!((lists.version, lists.log[0].by.as_str()), (1, "tester"));
    assert_eq!(lists.next_id, Id(1));
    run(&tool, "show --project bae").unwrap();
    run(&tool, "show --json --project bae").unwrap();
}

#[test]
fn init_refuses_an_existing_store() {
    let scratch = Scratch::new("lists-cmd-twice");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae --by tester").unwrap();
    let before = std::fs::read(tool.places.lists.join("bae/lists.json")).unwrap();
    assert!(refusal(run(&tool, "init bae --repos bae,coven --by tester")).contains("already exist"));
    assert_eq!(std::fs::read(tool.places.lists.join("bae/lists.json")).unwrap(), before);
}

#[test]
fn init_needs_its_arguments() {
    let scratch = Scratch::new("lists-cmd-args");
    let tool = scratch.tool(0);
    assert!(refusal(run(&tool, "init bae --by me")).contains("--repos"));
    assert!(refusal(run(&tool, "init --repos bae --by me")).contains("usage"));
    assert!(refusal(run(&tool, "init ../x --repos bae --by me")).contains("not a project name"));
    assert!(refusal(run(&tool, "init bae --repos bae --design-places nocolon --by me")).contains("REPO:GLOB"));
    assert!(refusal(run(&tool, "init bae --repos bae --design-places other:x --by me")).contains("not one of the project's"));
    assert!(refusal(run(&tool, "init bae --repos bae --by")).contains("--by needs a value"));
    assert!(refusal(run(&tool, "init bae --repos bae --nope 1 --by me")).contains("--nope"));
    assert!(!tool.places.lists.join("bae").exists());
}

#[test]
fn init_from_a_file_loads_checks_and_advances_the_state() {
    let scratch = Scratch::new("lists-cmd-from");
    let tool = scratch.tool(0);
    let mut state = Lists::empty("bae", vec!["bae".into(), "coven".into()], BTreeMap::new());
    state.next_id = Id(660);
    state.areas = vec!["Alpha".into()];
    let file = scratch.dir.join("bae.json");
    std::fs::write(&file, serde_json::to_string(&state).unwrap()).unwrap();
    let from = file.display().to_string();

    assert!(refusal(run(&tool, &format!("init bae --repos bae --from {from} --by me"))).contains("does not match the repos"));
    assert!(refusal(run(&tool, &format!("init bae --repos bae,coven --design-places bae:x --from {from} --by me"))).contains("design places"));
    assert!(refusal(run(&tool, &format!("init other --from {from} --by me"))).contains("holds the lists of project bae"));
    assert!(!tool.places.lists.join("bae").exists());

    run(&tool, &format!("init bae --repos bae,coven --from {from} --by me")).unwrap();
    let lists = Store::named(&tool.places, "bae").unwrap().load().unwrap();
    assert_eq!((lists.next_id, lists.version, lists.areas.clone()), (Id(660), 1, vec!["Alpha".to_string()]));
    assert!(lists.log[0].words.contains("--from"));
}

#[test]
fn init_from_a_state_failing_the_check_makes_no_store() {
    let scratch = Scratch::new("lists-cmd-from-bad");
    let tool = scratch.tool(0);
    let mut state = Lists::empty("bae", vec!["bae".into()], BTreeMap::new());
    state.areas = vec!["A".into(), "A".into()];
    let file = scratch.dir.join("bad.json");
    std::fs::write(&file, serde_json::to_string(&state).unwrap()).unwrap();
    let message = refusal(run(&tool, &format!("init bae --from {} --by me", file.display())));
    assert!(message.contains("'A' is listed twice"), "{message}");
    assert!(!tool.places.lists.join("bae/lists.json").exists());
    assert!(refusal(run(&tool, "init bae --from /nonexistent/x.json --by me")).contains("reading"));
}

#[test]
fn show_of_a_store_failing_the_check_fails_naming_the_fault() {
    let scratch = Scratch::new("lists-cmd-show-bad");
    let tool = scratch.tool(0);
    run(&tool, "init bae --repos bae --by me").unwrap();
    let file = tool.places.lists.join("bae/lists.json");
    let mut state: Lists = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    state.areas = vec!["Dup".into(), "Dup".into()];
    std::fs::write(&file, serde_json::to_string(&state).unwrap()).unwrap();
    for command in ["show --project bae", "show --json --project bae"] {
        let message = refusal(run(&tool, command));
        assert!(message.contains("fail their check") && message.contains("'Dup' is listed twice"), "{message}");
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
    assert!(refusal(run(&tool, &format!("project-of {}", scratch.dir.display()))).contains("not inside a git checkout"));
    assert!(refusal(run(&tool, "project-of")).contains("usage"));
    run(&tool, "init again --repos bae --by me").unwrap();
    assert!(refusal(run(&tool, &format!("project-of {}", in_project.display()))).contains("two projects"));
}

#[test]
fn an_unknown_command_is_refused() {
    let scratch = Scratch::new("lists-cmd-unknown");
    let tool = scratch.tool(0);
    assert!(refusal(run(&tool, "frobnicate")).contains("unknown lists command frobnicate"));
    assert!(refusal(cmd_lists(&tool, &[])).contains("usage"));
}
