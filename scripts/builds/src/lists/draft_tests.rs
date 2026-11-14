use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::draft;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

#[test]
fn a_brief_must_be_a_file() {
    let scratch = crate::testing::Scratch::new("brief");
    std::fs::create_dir_all(scratch.dir.join("plans/briefs")).unwrap();
    std::fs::write(scratch.dir.join("plans/briefs/x.md"), "# Section\n").unwrap();
    assert!(require_brief(&scratch.dir, "plans/briefs/x.md").is_ok());
    assert!(require_brief(&scratch.dir, "plans/briefs/x.md#Section").is_ok());
    assert!(refusal(require_brief(&scratch.dir, "plans/briefs/y.md")).contains("does not exist"));
    assert!(refusal(require_brief(&scratch.dir, "#Section")).contains("no file"));
    let mut d = draft("One two three four five");
    assert!(d.require_brief(&scratch.dir).is_ok());
    d.brief = Some("missing.md".into());
    assert!(d.require_brief(&scratch.dir).is_err());
}

#[test]
fn only_a_relative_brief_needs_a_checkout() {
    let scratch = crate::testing::Scratch::new("brief-from");
    let file = scratch.dir.join("b.md");
    std::fs::write(&file, "brief").unwrap();
    assert!(require_brief_from(&scratch.dir, &file.display().to_string()).is_ok());
    assert!(refusal(require_brief_from(&scratch.dir, "b.md")).contains("in no git checkout"));
    let mut d = draft("One two three four five");
    assert!(require_draft_briefs(&scratch.dir, std::slice::from_ref(&d)).is_ok());
    d.brief = Some("b.md".into());
    assert!(require_draft_briefs(&scratch.dir, &[d]).is_err());
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
