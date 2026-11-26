use super::*;
use crate::fail::Fail;
use crate::lists::design_paths::Globs;
use std::rc::Rc;

fn globs() -> Globs {
    Globs {
        places: vec![
            "design/mocks/*.dc.html".into(),
            "design/mocks/canvas.json".into(),
            "design/system/components/*/".into(),
        ],
        sources: vec![
            "design/system/components/src/".into(),
            "design/system/tokens.json".into(),
        ],
        ..Globs::default()
    }
}

fn changed(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| path.to_string()).collect()
}

fn declared(lane: Lane) -> Result<ShotNames> {
    let names: &[&str] = match lane {
        Lane::MAC => &[
            "mac.welcome.add-code",
            "mac.welcome.open-failed",
            "mac.component.row.plain",
            "mac.component.row.selected",
            "mac.component.status-chip.ok",
        ],
        _ => &["android.component.status-chip.ok"],
    };
    Ok(Rc::new(names.iter().map(|name| name.to_string()).collect()))
}

#[test]
fn a_changed_board_picks_its_shot_and_a_changed_component_its_rows() {
    let found = picked(
        &globs(),
        &changed(&[
            "design/mocks/mac.welcome.add-code.dc.html",
            "design/system/components/MacRow/preview.html",
            "design/system/components/MacRow/README.md",
        ]),
        &[Lane::MAC, Lane::ANDROID],
        &mut |lane| declared(lane),
    )
    .unwrap();
    assert_eq!(
        found.prefixes,
        ["mac.component.row.", "mac.welcome.add-code"]
    );
    assert_eq!(found.left_out, Vec::<String>::new());
}

#[test]
fn a_component_every_platform_shares_picks_each_platform_that_declares_its_rows() {
    let found = picked(
        &globs(),
        &changed(&["design/system/components/StatusChip/preview.html"]),
        &[Lane::MAC, Lane::ANDROID],
        &mut |lane| declared(lane),
    )
    .unwrap();
    assert_eq!(
        found.prefixes,
        [
            "android.component.status-chip.",
            "mac.component.status-chip."
        ]
    );
}

#[test]
fn a_change_with_no_declared_shot_is_named_and_left_out() {
    let found = picked(
        &globs(),
        &changed(&[
            "design/mocks/mac.welcome.no-shot.dc.html",
            "design/mocks/canvas.json",
            "design/system/components/MacBare/preview.html",
            "design/system/components/src/10-mac.js",
            "src/main.rs",
        ]),
        &[Lane::MAC, Lane::ANDROID],
        &mut |lane| declared(lane),
    )
    .unwrap();
    assert_eq!(found.prefixes, Vec::<String>::new());
    assert_eq!(found.left_out.len(), 3, "{:?}", found.left_out);
    let said = found.left_out.join("\n");
    assert!(said.contains("mac.welcome.no-shot"), "{said}");
    assert!(said.contains("MacBare"), "{said}");
    assert!(said.contains("shared design-system source"), "{said}");
}

#[test]
fn a_changed_readme_picks_nothing() {
    let found = picked(
        &globs(),
        &changed(&["design/system/components/MacRow/README.md"]),
        &[Lane::MAC, Lane::ANDROID],
        &mut |lane| declared(lane),
    )
    .unwrap();
    assert_eq!(found.prefixes, Vec::<String>::new());
    assert_eq!(found.left_out, Vec::<String>::new());
}

#[test]
fn shots_that_cannot_be_read_fail_the_pick() {
    let refused = picked(
        &globs(),
        &changed(&["design/mocks/mac.welcome.add-code.dc.html"]),
        &[Lane::MAC, Lane::ANDROID],
        &mut |_| Err(Fail::Message("no declared shots".into())),
    );
    assert_eq!(refused.unwrap_err().text(), "no declared shots");
}
