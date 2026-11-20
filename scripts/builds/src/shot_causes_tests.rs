use super::*;
use serde_json::{Value, json};

/// A run's findings.json with these records, of a run that checked their
/// scenes and the <clean> scenes, which have none.
fn run(records: &[Value], clean: &[&str]) -> Run {
    let mut scenes: Vec<&str> = records
        .iter()
        .filter_map(|record| record["scene"].as_str())
        .chain(clean.iter().copied())
        .collect();
    scenes.sort();
    scenes.dedup();
    parse(
        &json!({"version": "v1", "commit": "abc", "scenes": scenes, "findings": records})
            .to_string(),
    )
    .unwrap()
}

fn element() -> Value {
    json!({"role": "text", "words": "Library"})
}

/// A record of <kind> in <scene>, drawn by <component> when one is given.
fn record(scene: &str, component: Option<&str>, kind: &str, fields: Value) -> Value {
    let mut record = json!({"scene": scene, "kind": kind, "sentence": "s"});
    if let Some(component) = component {
        record["component"] = json!(component);
    }
    for (key, value) in fields.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

fn colour(scene: &str, component: Option<&str>, part: &str, board: &str, shot: &str) -> Value {
    record(
        scene,
        component,
        "colour",
        json!({"element": element(), "part": part, "board": board, "shot": shot, "delta": 4.0}),
    )
}

fn moved(scene: &str, component: Option<&str>) -> Value {
    record(
        scene,
        component,
        "moved",
        json!({"element": element(), "dx": 1.0, "dy": 0.0}),
    )
}

fn frame(scene: &str, dx: f64, dy: f64) -> Value {
    record(scene, None, "frame-offset", json!({"dx": dx, "dy": dy}))
}

fn region(scene: &str) -> Value {
    record(
        scene,
        None,
        "region",
        json!({"x": 0.0, "y": 16.0, "width": 32.0, "height": 16.0, "delta": 9.5}),
    )
}

fn group(cause: &str, scenes: &[(&str, usize)], kinds: &[(&str, usize)]) -> Group {
    Group {
        cause: cause.to_string(),
        findings: scenes.iter().map(|(_, n)| n).sum(),
        scenes: scenes.iter().map(|(s, n)| (s.to_string(), *n)).collect(),
        kinds: kinds.iter().map(|(k, n)| (k.to_string(), *n)).collect(),
    }
}

#[test]
fn components_rank_by_the_scenes_they_break_with_their_kinds_and_scenes() {
    let run = run(
        &[
            moved("mac.a", Some("MacRow")),
            moved("mac.a", Some("MacRow")),
            moved("mac.a", Some("MacRow")),
            moved("mac.a", Some("MacIconButton")),
            colour(
                "mac.b",
                Some("MacIconButton"),
                "background",
                "#222230",
                "#1E1E1E",
            ),
            colour(
                "mac.c",
                Some("MacIconButton"),
                "background",
                "#222230",
                "#1E1E1E",
            ),
            frame("mac.a", 8.0, 8.0),
            frame("mac.b", 7.5, 8.1),
            frame("mac.c", -4.5, 0.0),
            region("mac.c"),
        ],
        &[],
    );
    let causes = causes(&run);
    assert_eq!(
        causes.components,
        vec![
            group(
                "MacIconButton",
                &[("mac.a", 1), ("mac.b", 1), ("mac.c", 1)],
                &[
                    ("an element has another background colour in the shot", 2),
                    ("an element moved in the shot", 1)
                ]
            ),
            group(
                "MacRow",
                &[("mac.a", 3)],
                &[("an element moved in the shot", 3)]
            ),
        ]
    );
    assert_eq!(
        causes.remainder,
        vec![
            group(
                "the shot's frame sits away from the board's",
                &[("mac.a", 1), ("mac.b", 1), ("mac.c", 1)],
                &[]
            ),
            group("the pictures differ in an area", &[("mac.c", 1)], &[]),
        ]
    );
    assert_eq!(
        causes.colours,
        vec![group(
            "#222230 on the board is #1E1E1E in the shot",
            &[("mac.b", 1), ("mac.c", 1)],
            &[("background", 2)]
        )]
    );
    assert_eq!(
        causes.frames,
        vec![
            group(
                "the shot's frame sits 8 pt right and 8 pt down from the board's, to the nearest point",
                &[("mac.a", 1), ("mac.b", 1)],
                &[]
            ),
            group(
                "the shot's frame sits 5 pt left and 0 pt down from the board's, to the nearest point",
                &[("mac.c", 1)],
                &[]
            ),
        ]
    );
}

#[test]
fn the_text_names_each_cause_with_its_scenes_findings_and_kinds_under_its_grouping() {
    let run = run(
        &[
            moved("mac.a", Some("MacRow")),
            colour("mac.b", Some("MacRow"), "text", "#6A6A7A", "#FFFFFF"),
            frame("mac.a", 8.0, 8.0),
            frame("mac.b", 8.0, 8.0),
        ],
        &["mac.c"],
    );
    assert_eq!(
        text(&run),
        "4 findings in 2 of the 3 scenes the run checked, grouped by cause, largest first.\n\
         \n\
         By component:\n\
         \x20 MacRow: 2 scenes, 2 findings\n\
         \x20   kinds: 1 a text has another colour in the shot; 1 an element moved in the shot\n\
         \x20   scenes: mac.a (1), mac.b (1)\n\
         \n\
         Findings that no component draws, by kind:\n\
         \x20 the shot's frame sits away from the board's: 2 scenes, 2 findings\n\
         \x20   scenes: mac.a (1), mac.b (1)\n\
         \n\
         By colour pair:\n\
         \x20 and 1 cause with one finding each\n\
         \n\
         By frame offset:\n\
         \x20 the shot's frame sits 8 pt right and 8 pt down from the board's, to the nearest point: 2 scenes, 2 findings\n\
         \x20   scenes: mac.a (1), mac.b (1)\n"
    );
}

#[test]
fn a_cause_names_its_ten_largest_scenes_and_causes_of_one_finding_are_counted() {
    let mut records: Vec<Value> = (0..12)
        .map(|n| frame(&format!("mac.s{n:02}"), 8.0, 8.0))
        .collect();
    records.push(frame("mac.s00", 8.0, 8.0));
    records.push(frame("mac.t", 1.0, 30.0));
    records.push(frame("mac.u", 0.0, -30.0));
    let text = text(&run(&records, &[]));
    assert!(
        text.contains(
            "By frame offset:\n  the shot's frame sits 8 pt right and 8 pt down from the board's, to the nearest point: 12 scenes, 13 findings\n    scenes: mac.s00 (2), mac.s01 (1), mac.s02 (1), mac.s03 (1), mac.s04 (1), mac.s05 (1), mac.s06 (1), mac.s07 (1), mac.s08 (1), mac.s09 (1), and 2 more\n  and 2 causes with one finding each\n"
        ),
        "{text}"
    );
}

#[test]
fn a_run_without_findings_says_each_grouping_has_none() {
    assert_eq!(
        text(&run(&[], &[])),
        "0 findings in 0 of the 0 scenes the run checked, grouped by cause, largest first.\n\nBy component: none\n\nFindings that no component draws, by kind: none\n\nBy colour pair: none\n\nBy frame offset: none\n"
    );
}

#[test]
fn every_kind_the_shots_program_writes_is_read() {
    let element = json!({"role": "radio", "words": "On"});
    let mut records = vec![
        record(
            "mac.a",
            Some("MacRadio"),
            "missing",
            json!({"element": element}),
        ),
        record("mac.a", None, "extra", json!({"element": element})),
        record(
            "mac.a",
            Some("MacRadio"),
            "out-of-order",
            json!({"element": element}),
        ),
        record(
            "mac.a",
            Some("MacRadio"),
            "role-differs",
            json!({"element": element, "shot_role": "button"}),
        ),
        record(
            "mac.a",
            Some("MacRadio"),
            "state-differs",
            json!({"element": element, "aspect": "selected", "board": "selected", "shot": "not selected"}),
        ),
        record(
            "mac.a",
            Some("MacRadio"),
            "resized",
            json!({"element": element, "dw": 1.0, "dh": -2.5}),
        ),
        record(
            "mac.a",
            Some("MacRadio"),
            "font-size",
            json!({"element": element, "board": 13.0, "shot": 12.0}),
        ),
        record(
            "mac.a",
            Some("MacRadio"),
            "font-weight",
            json!({"element": element, "board": 400, "shot": 600}),
        ),
    ];
    records.push(region("mac.a"));
    let causes = causes(&run(&records, &[]));
    assert_eq!(causes.components.len(), 1);
    assert_eq!(causes.components[0].kinds.len(), 7);
    assert_eq!(causes.remainder.len(), 2);
}

#[test]
fn a_file_the_shots_program_did_not_write_is_refused_with_its_reason() {
    let error = parse("{\"findings\": [{\"scene\": \"mac.a\", \"kind\": \"glow\"}]}")
        .expect_err("refused")
        .text();
    assert!(error.contains("glow"), "{error}");
}

#[test]
fn a_report_folder_without_the_file_is_refused_naming_it() {
    let scratch = crate::testing::Scratch::new("shot-causes");
    let error = read(&scratch.dir.join("report"))
        .expect_err("refused")
        .text();
    assert!(error.contains(FILE), "{error}");
}
