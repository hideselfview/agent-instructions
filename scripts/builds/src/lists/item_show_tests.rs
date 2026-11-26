use super::*;
use crate::lists::item_command::cmd_item;
use crate::lists::item_fixtures::{demo, refusal, words};

fn add(demo: &crate::lists::item_fixtures::Demo, title: &str, area: &str, extra: &[&str]) {
    let details = demo.brief("details.txt");
    let mut args = vec![
        "add",
        "--title",
        title,
        "--area",
        area,
        "--details-file",
        &details,
    ];
    args.extend_from_slice(extra);
    cmd_item(&demo.tool, &words(&args)).unwrap();
}

#[test]
fn render_item_shows_an_open_and_a_closed_item() {
    let demo = demo("show-render");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    add(
        &demo,
        "Do the second thing now",
        "Beta",
        &["--wait", "item:1"],
    );
    let lists = demo.store.load().unwrap();
    let one = render_item(&lists, Id(1), 0).unwrap();
    assert!(
        one.contains("#1 Do the first thing now [ready]")
            && one.contains("area: Alpha; for: coordinator"),
        "{one}"
    );
    let two = render_item(&lists, Id(2), 0).unwrap();
    assert!(
        two.contains("[waiting]")
            && two.contains("waits on item #1 \"Do the first thing now\" (not met)"),
        "{two}"
    );
    cmd_item(&demo.tool, &words(&["drop", "2", "--why", "gone"])).unwrap();
    let closed = render_item(&demo.store.load().unwrap(), Id(2), 0).unwrap();
    assert!(
        closed.contains("[closed, dropped: gone by tester"),
        "{closed}"
    );
    assert!(refusal(render_item(&lists, Id(9), 0)).contains("not a record"));
}

#[test]
fn next_order_is_first_then_area_order_and_leaves_out_waiting_running_and_design_items() {
    let demo = demo("show-next");
    add(&demo, "Do the beta thing now", "Beta", &[]);
    add(&demo, "Do the alpha thing now", "Alpha", &[]);
    add(
        &demo,
        "Do the urgent beta thing",
        "Beta",
        &["--priority", "first", "--why", "person"],
    );
    add(
        &demo,
        "Do the waiting thing now",
        "Alpha",
        &["--wait", "condition:later", "--until", "agent"],
    );
    add(
        &demo,
        "Do the design sessions thing",
        "Alpha",
        &["--for", "design-session"],
    );
    let lists = demo.store.load().unwrap();
    let order: Vec<u32> = derive::dispatch_order(&lists, Audience::Coordinator)
        .iter()
        .map(|i| i.id.0)
        .collect();
    assert_eq!(order, [3, 2, 1]);
    next(&demo.tool, &words(&[]), &demo.main).unwrap();
}

#[test]
fn list_ready_comes_in_the_queue_order_not_in_id_order() {
    let demo = demo("show-list-ready-order");
    add(&demo, "Do the beta thing now", "Beta", &[]);
    add(&demo, "Do the alpha thing now", "Alpha", &[]);
    add(
        &demo,
        "Do the urgent beta thing",
        "Beta",
        &["--priority", "first", "--why", "person"],
    );
    add(&demo, "Do the later alpha thing", "Alpha", &[]);
    let lists = demo.store.load().unwrap();
    let ids = |ready: bool, area: Option<&str>| -> Vec<u32> {
        chosen(
            &lists,
            &Choice {
                running: false,
                ready,
                backlog: false,
                area,
                audience: None,
                tag: None,
            },
        )
        .unwrap()
        .iter()
        .map(|i| i.id.0)
        .collect()
    };
    assert_eq!(ids(true, None), [3, 2, 4, 1]);
    assert_eq!(ids(true, Some("Beta")), [3, 1]);
    // Without --ready the list stays in id order.
    assert_eq!(ids(false, None), [1, 2, 3, 4]);
}

#[test]
fn list_running_prints_the_marker_with_the_agent_from_the_claim() {
    let demo = demo("show-running");
    let side = demo.add_side(1, "agent A", "work");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    let brief = demo.brief("b.md");
    cmd_item(
        &demo.tool,
        &words(&[
            "take",
            "1",
            "--worktree",
            side.to_str().unwrap(),
            "--branch",
            "work",
            "--brief",
            &brief,
        ]),
    )
    .unwrap();
    let lists = demo.store.load().unwrap();
    let line = marker(lists.items[0].running.first().unwrap()).unwrap();
    assert_eq!(
        line,
        format!("Running: agent A, demo side-1 work, brief {brief}")
    );
    cmd_item(&demo.tool, &words(&["list", "--running"])).unwrap();
    cmd_item(
        &demo.tool,
        &words(&["list", "--ready", "--area", "Alpha", "--for", "coordinator"]),
    )
    .unwrap();
    assert!(
        refusal(cmd_item(&demo.tool, &words(&["list", "--area", "Nope"])))
            .contains("does not exist")
    );
}

#[test]
fn a_running_item_shows_the_tier_of_the_model_it_takes() {
    let demo = demo("show-tier");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    let side = demo.add_side(1, "agent A", "work");
    let brief = demo.brief("b.md");
    cmd_item(
        &demo.tool,
        &words(&[
            "take",
            "1",
            "--worktree",
            side.to_str().unwrap(),
            "--branch",
            "work",
            "--brief",
            &brief,
            "--tier",
            "strongest",
        ]),
    )
    .unwrap();
    let text = render_item(&demo.store.load().unwrap(), Id(1), 0).unwrap();
    assert!(text.contains("\ntier: strongest\n"), "{text}");
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
        tier: crate::model_tier::Tier::Implementation,
    };
    demo.store
        .write(&stamp, "item take 1", |db| {
            crate::lists::running::start(db, &[Id(1)], &[running])
        })
        .unwrap();
    assert!(
        render_item(&demo.store.load().unwrap(), Id(1), 0)
            .unwrap()
            .contains("Running:")
    );
    demo.store
        .write(&stamp, "item landed", |db| {
            crate::lists::running::end(db, Id(1), crate::lists::Ended::Closed, &stamp)?;
            crate::lists::record::close(db, Id(1), How::Landed(vec!["a".repeat(40)]), &stamp)
        })
        .unwrap();
    let text = render_item(&demo.store.load().unwrap(), Id(1), 0).unwrap();
    assert!(
        text.contains("[closed, landed")
            && !text.contains("Running:")
            && !text.contains("running since"),
        "{text}"
    );
}

#[test]
fn times_say_how_long_each_item_and_branch_ran_and_was_held_or_waited() {
    use crate::lists::fixtures::{NOON, stamp};
    use crate::lists::running::{Ended, SpanKind, end, end_span, open_span, start};
    use crate::lists::{Running, record::close};
    let demo = demo("show-times");
    add(&demo, "Do the first thing now", "Alpha", &[]);
    add(&demo, "Do the second thing now", "Alpha", &[]);
    add(&demo, "Do the third thing now", "Alpha", &[]);
    let run = |branch: &str, n: usize, at: i64| Running {
        worktree: demo
            .scratch
            .dir
            .join(format!("demo-worktrees/side-{n}"))
            .display()
            .to_string(),
        branch: branch.into(),
        brief: "b.md".into(),
        since: stamp(at),
        tier: crate::model_tier::Tier::Implementation,
    };
    demo.store
        .write(&stamp(NOON + 3600), "seed", |db| {
            start(db, &[Id(1)], &[run("work", 1, NOON)])?;
            open_span(db, Id(1), SpanKind::Held, NOON + 60)?;
            end_span(db, Id(1), SpanKind::Held, NOON + 660)?;
            end(db, Id(1), Ended::Closed, &stamp(NOON + 3600))?;
            close(
                db,
                Id(1),
                How::Landed(vec!["a".repeat(40)]),
                &stamp(NOON + 3600),
            )?;
            start(db, &[Id(2)], &[run("other", 2, NOON + 1000)])
        })
        .unwrap();
    let lists = demo.store.load().unwrap();
    let at = |t: i64| crate::text::local_minute(NOON + t);
    let first = format!(
        "first started {} and landed {}, 1h00m later; ran 1h00m in 1 run; paused 10m; waited on you 0m",
        at(0),
        at(3600)
    );
    let second = format!(
        "first started {} and runs now; ran 50m in 1 run; paused 0m; waited on you 0m",
        at(1000)
    );
    assert_eq!(
        times_text(&lists, &[], NOON + 4000).unwrap(),
        format!(
            "running\n  #2 Do the second thing now: {second}\n\
             landed in the last 14 days\n  #1 Do the first thing now: {first}\n\
             branches\n  demo other: {second}\n  demo work: {first}\n"
        )
    );
    assert_eq!(
        times_text(&lists, &[Id(3), Id(1)], NOON + 4000).unwrap(),
        format!(
            "#3 Do the third thing now: builds has recorded no run\n#1 Do the first thing now: {first}\n"
        )
    );
    assert!(
        render_item(&lists, Id(1), NOON + 4000)
            .unwrap()
            .contains(&format!("times: {first}\n"))
    );
    demo.store
        .write(&stamp(NOON + 3600), "spans from", |db| {
            crate::lists::running::plant(
                db,
                "spans_recorded",
                &[("singleton", 1.into()), ("from_at", (NOON + 500).into())],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    let lists = demo.store.load().unwrap();
    let from = format!(
        "builds has recorded paused and waiting time only since {}",
        at(500)
    );
    assert_eq!(
        times_text(&lists, &[Id(1)], NOON + 4000).unwrap(),
        format!("#1 Do the first thing now: {first}\n{from}\n")
    );
    assert!(
        render_item(&lists, Id(1), NOON + 4000)
            .unwrap()
            .contains(&format!("times: {first} ({from})\n"))
    );
}

#[test]
fn next_says_how_many_items_each_unblocks_and_takes_the_design_sessions_items() {
    let demo = demo("show-next-unblocks");
    add(&demo, "Do the plain thing now", "Alpha", &[]);
    add(&demo, "Do the blocking thing now", "Beta", &[]);
    add(
        &demo,
        "Do the blocked thing now",
        "Alpha",
        &["--wait", "item:2"],
    );
    add(
        &demo,
        "Draw the new screen now",
        "Alpha",
        &["--for", "design-session"],
    );
    let lists = demo.store.load().unwrap();
    let coordinator = next_text(&lists, Audience::Coordinator);
    let at = |text: &str| coordinator.find(text).unwrap_or(usize::MAX);
    assert!(
        at("2 Do the blocking thing now (Beta, unblocks 1 item)")
            < at("1 Do the plain thing now (Alpha)"),
        "{coordinator}"
    );
    assert!(
        !coordinator.contains("Draw the new screen"),
        "{coordinator}"
    );
    let design = next_text(&lists, Audience::DesignSession);
    assert!(
        design.contains("4 Draw the new screen now (Alpha)"),
        "{design}"
    );
    assert!(!design.contains("plain"), "{design}");
}

#[test]
fn next_without_a_project_lists_the_ready_items_of_every_project() {
    let demo = demo("show-next-every-project");
    add(
        &demo,
        "Draw the demo screen now",
        "Alpha",
        &["--for", "design-session"],
    );
    crate::lists::fixtures::make_store_named(&demo.tool.places, "other", &["other"]);
    let details = demo.brief("details.txt");
    cmd_item(
        &demo.tool,
        &plain(&[
            "add",
            "--project",
            "other",
            "--by",
            "tester",
            "--title",
            "Draw the other screen now",
            "--area",
            "Alpha",
            "--details-file",
            &details,
            "--for",
            "design-session",
        ]),
    )
    .unwrap();
    let both = next_report(&demo.tool, &plain(&["--for", "design-session"]), &demo.main).unwrap();
    let demo_at = both.find("project demo").expect(&both);
    let other_at = both.find("project other").expect(&both);
    assert!(
        both[demo_at..other_at].contains("1 Draw the demo screen now (Alpha)")
            && both[other_at..].contains("1 Draw the other screen now (Alpha)"),
        "{both}"
    );
    let one = next_report(
        &demo.tool,
        &plain(&["--for", "design-session", "--project", "other"]),
        &demo.main,
    )
    .unwrap();
    assert!(
        one.contains("Draw the other screen now") && !one.contains("demo screen"),
        "{one}"
    );
    let none = next_report(&demo.tool, &plain(&[]), &demo.main).unwrap();
    assert_eq!(none, "nothing is ready to dispatch\n");
}

fn plain(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}
