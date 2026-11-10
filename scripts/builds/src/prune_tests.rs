use super::*;
use crate::testing::{Scratch, age, age_all, fill};

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

#[test]
fn a_scratch_entry_goes_only_when_its_session_ended_and_nothing_inside_changed_for_three_days() {
    let scratch = Scratch::new("prune-scratch");
    let tool = scratch.tool(0);
    let ended = tool.places.scratch_base().join("project/ended/scratchpad");
    let running = tool
        .places
        .scratch_base()
        .join("project/running/scratchpad");
    // A copy refreshed in place: the folder's own time is old, a file inside is new.
    fill(&ended.join("refreshed/inner/f"), 1);
    age_all(&ended.join("refreshed"), 80);
    age(&ended.join("refreshed"), 80);
    fill(&ended.join("refreshed/inner/g"), 1);
    age(&ended.join("refreshed/inner"), 80);
    age(&ended.join("refreshed"), 80);
    fill(&ended.join("idle/f"), 1);
    age_all(&ended.join("idle"), 80);
    age(&ended.join("idle"), 80);
    fill(&running.join("idle/f"), 1);
    age_all(&running.join("idle"), 80);
    age(&running.join("idle"), 80);
    let sessions = tool.places.claude_sessions();
    std::fs::create_dir_all(&sessions).unwrap();
    let me = unsafe { libc::getpid() };
    std::fs::write(
        sessions.join(format!("{me}.json")),
        format!(r#"{{"pid":{me},"sessionId":"running"}}"#),
    )
    .unwrap();
    prune_scratchpads(&tool, Run::Real).unwrap();
    assert!(
        !ended.join("idle").exists(),
        "an ended session's entry idle three days was kept"
    );
    assert!(
        ended.join("refreshed").exists(),
        "an entry written inside lately was deleted"
    );
    assert!(
        running.join("idle").exists(),
        "a running session's entry was deleted"
    );
}
