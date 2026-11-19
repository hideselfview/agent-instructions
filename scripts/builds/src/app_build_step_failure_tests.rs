use super::*;

#[test]
fn a_step_read_keeps_the_excerpt_and_the_shots_that_fail_the_run() {
    let read = StepRead::default();
    let each = read.each_line();
    for line in [
        "Compiling shots",
        "builds shots: 2 pair(s) fail the run: mac.b.one, mac.a.two",
        "report at shots/index.html",
    ] {
        each(line);
    }
    let failure = read.failure();
    assert_eq!(failure.shots, vec!["mac.a.two", "mac.b.one"]);
    assert!(
        failure
            .excerpt
            .as_deref()
            .is_some_and(|text| text.starts_with("Compiling shots\n")),
        "{failure:?}"
    );
    assert_eq!(StepRead::default().failure(), StepFailure::default());
}
