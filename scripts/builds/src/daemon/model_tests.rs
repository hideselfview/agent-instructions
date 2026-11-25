use super::*;

/// The sessions a rules-review card judges from come from the last reading
/// of the processes, and a failed reading is an error, never an old list.
#[test]
fn the_last_sample_is_the_reading_or_its_error() {
    let mut state = State::default();
    assert!(state.last_sample().unwrap().is_none());
    state.page.sample_error = Some(Arc::new(crate::fail::Fail::Message("no ps".into())));
    let error = state
        .last_sample()
        .err()
        .expect("the reading failed")
        .text();
    assert!(error.contains("could not read processes: no ps"), "{error}");
}
