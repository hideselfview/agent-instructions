use super::*;
use crate::testing::{Scratch, age, fill};

#[test]
fn test_temps_go_after_a_day_and_scratch_entries_after_three() {
    let scratch = Scratch::new("prune-ages");
    let mut tool = scratch.tool(0);
    let tmpdir = scratch.dir.join("T");
    tool.places.tmpdir = Some(tmpdir.clone());
    for (name, hours) in [(".tmpOld", 25), (".tmpNew", 23), ("other", 48)] {
        fill(&tmpdir.join(name).join("f"), 1);
        age(&tmpdir.join(name), hours);
    }
    let pad = tool.places.scratch_base().join("project/session/scratchpad");
    for (name, hours) in [("old", 73), ("new", 71)] {
        fill(&pad.join(name), 1);
        age(&pad.join(name), hours);
    }
    prune_test_temps(&tool, Run::Real).unwrap();
    prune_scratchpads(&tool, Run::Real).unwrap();
    assert!(!tmpdir.join(".tmpOld").exists(), "a test temp folder a day old was kept");
    assert!(tmpdir.join(".tmpNew").exists() && tmpdir.join("other").exists());
    assert!(!pad.join("old").exists(), "a scratch entry three days old was kept");
    assert!(pad.join("new").exists());
}
