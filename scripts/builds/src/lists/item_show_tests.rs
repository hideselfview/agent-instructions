use super::*;
use crate::lists::item_command::cmd_item;
use crate::lists::item_fixtures::{demo, refusal, words};

fn add(demo: &crate::lists::item_fixtures::Demo, title: &str, area: &str, extra: &[&str]) {
    let details = demo.brief("details.txt");
    let mut args = vec!["add", "--title", title, "--area", area, "--details-file", &details];
    args.extend_from_slice(extra);
    cmd_item(&demo.tool, &words(&args)).unwrap();
}

#[test]
fn render_item_shows_an_open_and_a_closed_item() {
    let demo = demo("show-render");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    add(&demo, "Do the second thing now", "Beta", &["--wait", "item:1"]);
    let lists = demo.store.load().unwrap();
    let one = render_item(&lists, Id(1)).unwrap();
    assert!(one.contains("#1 Do the first thing now [ready]") && one.contains("area: Alpha; for: coordinator"), "{one}");
    let two = render_item(&lists, Id(2)).unwrap();
    assert!(two.contains("[waiting]") && two.contains("waits on item #1 \"Do the first thing now\" (not met)"), "{two}");
    cmd_item(&demo.tool, &words(&["drop", "2", "--why", "gone"])).unwrap();
    let closed = render_item(&demo.store.load().unwrap(), Id(2)).unwrap();
    assert!(closed.contains("[closed, dropped: gone by tester"), "{closed}");
    assert!(refusal(render_item(&lists, Id(9))).contains("not a record"));
}

#[test]
fn next_order_is_first_then_area_order_and_leaves_out_waiting_running_and_design_items() {
    let demo = demo("show-next");
    add(&demo, "Do the beta thing now", "Beta", &[]);
    add(&demo, "Do the alpha thing now", "Alpha", &[]);
    add(&demo, "Do the urgent beta thing", "Beta", &["--priority", "first", "--why", "person"]);
    add(&demo, "Do the waiting thing now", "Alpha", &["--wait", "condition:later"]);
    add(&demo, "Do the design sessions thing", "Alpha", &["--for", "design-session"]);
    let lists = demo.store.load().unwrap();
    let order: Vec<u32> = derive::dispatch_order(&lists).iter().map(|i| i.id.0).collect();
    assert_eq!(order, [3, 2, 1]);
    next(&demo.tool, &words(&[]), &demo.main).unwrap();
}

#[test]
fn list_running_prints_the_marker_with_the_agent_from_the_claim() {
    let demo = demo("show-running");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    let brief = demo.brief("b.md");
    cmd_item(&demo.tool, &words(&["take", "1", "--worktree", side.to_str().unwrap(), "--branch", "work", "--brief", &brief])).unwrap();
    let lists = demo.store.load().unwrap();
    let line = marker(lists.items[0].running.first().unwrap()).unwrap();
    assert_eq!(line, format!("Running: agent A, demo side-1 work, brief {brief}"));
    cmd_item(&demo.tool, &words(&["list", "--running"])).unwrap();
    cmd_item(&demo.tool, &words(&["list", "--ready", "--area", "Alpha", "--for", "coordinator"])).unwrap();
    assert!(refusal(cmd_item(&demo.tool, &words(&["list", "--area", "Nope"]))).contains("does not exist"));
}

#[test]
fn a_closed_item_prints_no_running_marker() {
    let demo = demo("show-closed-running");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    let stamp = crate::lists::fixtures::stamp(crate::lists::fixtures::NOON);
    let running = crate::lists::running::Running {
        worktree: "/d/demo-worktrees/side-1".into(),
        branch: "work".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp.clone(),
    };
    demo.store
        .write(&stamp, "item take 1", |db| crate::lists::running::start(db, &[Id(1)], &[running]))
        .unwrap();
    assert!(render_item(&demo.store.load().unwrap(), Id(1)).unwrap().contains("Running:"));
    demo.store
        .write(&stamp, "item landed", |db| {
            crate::lists::running::end(db, Id(1))?;
            crate::lists::record::close(db, Id(1), How::Landed(vec!["a".repeat(40)]), &stamp)
        })
        .unwrap();
    let text = render_item(&demo.store.load().unwrap(), Id(1)).unwrap();
    assert!(text.contains("[closed, landed") && !text.contains("Running:") && !text.contains("running since"), "{text}");
}
