use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::draft;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
    }
}

fn repos(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// Writes <text> to <relative> in the main checkout folder of <repo>.
fn plan(places: &Places, repo: &str, relative: &str) -> std::path::PathBuf {
    let file = places.home.join(clone_path(repo)).join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# Section\n").unwrap();
    file
}

#[test]
fn a_relative_brief_is_found_in_the_main_checkout_of_any_repo_of_the_project() {
    let scratch = crate::testing::Scratch::new("brief-any-repo");
    let places = scratch.places();
    let file = plan(&places, "bae", "plans/briefs/x.md");
    let both = repos(&["agent-instructions", "bae"]);
    assert_eq!(
        find_brief(&places, &both, "plans/briefs/x.md").unwrap(),
        file
    );
    assert_eq!(
        find_brief(&places, &both, "plans/briefs/x.md#Section").unwrap(),
        file
    );
}

#[test]
fn a_brief_that_two_main_checkouts_hold_is_refused() {
    let scratch = crate::testing::Scratch::new("brief-ambiguous");
    let places = scratch.places();
    plan(&places, "bae", "plans/briefs/x.md");
    plan(&places, "coven", "plans/briefs/x.md");
    let said = refusal(find_brief(
        &places,
        &repos(&["bae", "coven"]),
        "plans/briefs/x.md",
    ));
    assert!(said.contains("is ambiguous"), "{said}");
    assert!(said.contains("dev/bae/plans/briefs/x.md"), "{said}");
    assert!(said.contains("dev/coven/plans/briefs/x.md"), "{said}");
}

#[test]
fn a_brief_that_no_main_checkout_holds_is_refused() {
    let scratch = crate::testing::Scratch::new("brief-missing");
    let places = scratch.places();
    plan(&places, "bae", "plans/briefs/x.md");
    let said = refusal(find_brief(
        &places,
        &repos(&["bae", "coven"]),
        "plans/briefs/y.md",
    ));
    assert!(said.contains("does not exist"), "{said}");
    assert!(said.contains("dev/coven/plans/briefs/y.md"), "{said}");
    assert!(refusal(find_brief(&places, &repos(&["bae"]), "#Section")).contains("no file"));
}

#[test]
fn an_absolute_brief_is_itself_and_must_exist() {
    let scratch = crate::testing::Scratch::new("brief-absolute");
    let places = scratch.places();
    let file = scratch.dir.join("b.md");
    std::fs::write(&file, "brief").unwrap();
    let path = file.display().to_string();
    assert_eq!(find_brief(&places, &[], &path).unwrap(), file);
    let missing = scratch.dir.join("none.md").display().to_string();
    assert!(refusal(find_brief(&places, &[], &missing)).contains("does not exist"));
    let mut d = draft("One two three four five");
    assert!(require_draft_briefs(&places, &[], std::slice::from_ref(&d)).is_ok());
    d.brief = Some("b.md".into());
    assert!(require_draft_briefs(&places, &[], &[d]).is_err());
}

#[test]
fn work_or_no_work_is_one_of_them() {
    let one = vec![draft("One two three four five")];
    assert!(refusal(work_or_why(Some(one.clone()), Some("x".into()))).contains("not both"));
    assert!(refusal(work_or_why(None, None)).contains("`work`"));
    assert!(refusal(work_or_why(None, Some(" ".into()))).contains("reason"));
    assert_eq!(
        work_or_why(None, Some("nothing follows".into())).unwrap(),
        vec![]
    );
    assert_eq!(work_or_why(Some(one.clone()), None).unwrap(), one);
}
