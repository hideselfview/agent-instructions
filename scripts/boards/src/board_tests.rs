use super::*;
use serde_json::json;

#[test]
fn a_board_is_named_by_its_name_or_its_path_and_never_leaves_its_folder() {
    assert_eq!(
        board_name("mac.welcome.open-failed").unwrap(),
        "mac.welcome.open-failed"
    );
    assert_eq!(
        board_name("design/mocks/mac.welcome.open-failed.dc.html").unwrap(),
        "mac.welcome.open-failed"
    );
    for bad in ["", "../x", ".hidden", "a/b", "design/mocks/a b.dc.html"] {
        let said = board_name(bad).unwrap_err().text();
        assert!(said.contains("is no board"), "{bad:?}: {said}");
    }
    assert_eq!(board_path("A"), "design/mocks/A.dc.html");
}

#[test]
fn the_boards_a_board_mounts_are_its_dc_imports_in_order() {
    let body =
        r#"<div><dc-import name="B" word="x"></dc-import><dc-import name="C"></dc-import></div>"#;
    assert_eq!(imports(body), ["B", "C"]);
    assert!(imports("<p>no parts</p>").is_empty());
}

#[test]
fn a_boards_size_comes_from_the_canvas_index() {
    let canvas = json!({"boards": {"A.dc.html": {"w": 400, "h": 300}, "B.dc.html": {"w": 10}}});
    assert_eq!(canvas_size(&canvas, "A.dc.html").unwrap(), (400, 300));
    assert_eq!(
        canvas_size(&canvas, "B.dc.html").unwrap_err().text(),
        "design/mocks/canvas.json gives no width and height for B.dc.html"
    );
}

#[test]
fn a_boards_place_comes_from_the_canvas_index() {
    let canvas = json!({"boards": {"A.dc.html": {"x": 500, "y": 20.5}, "B.dc.html": {"x": 10}}});
    assert_eq!(canvas_place(&canvas, "A.dc.html").unwrap(), (500.0, 20.5));
    assert_eq!(
        canvas_place(&canvas, "B.dc.html").unwrap_err().text(),
        "design/mocks/canvas.json gives no x and y for B.dc.html"
    );
}

#[test]
fn the_design_systems_namespace_is_the_one_the_canvas_index_names() {
    let named = |namespace: &str| json!({"designSystems": [{"namespace": namespace}]});
    assert_eq!(design_namespace(&named("bae")).unwrap(), "bae");
    assert!(
        design_namespace(&named("Bae"))
            .unwrap_err()
            .text()
            .contains("no folder name")
    );
    assert!(
        design_namespace(&json!({}))
            .unwrap_err()
            .text()
            .contains("names no design system")
    );
}
