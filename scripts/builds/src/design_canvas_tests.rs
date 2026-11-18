use super::*;
use serde_json::json;

fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().text()
}

/// The titles of the demo repo's canvases and its design system.
pub(crate) const ONE: &str = "demo: one";
pub(crate) const TWO: &str = "demo: two and three";
pub(crate) const DS: &str = "demo";

/// The canvases of the demo repo: page `one` on the first canvas, pages `two`
/// and `three` on the second, and its design system.
pub(crate) fn canvases_toml() -> String {
    format!(
        "# The canvases.\n\n[design_system]\ntitle = \"{DS}\"\n\n[[canvas]]\ntitle = \"{ONE}\"\nlaunch = \"one\"\npages = [\"one\"]\n\n\
         [[canvas]]\ntitle = \"{TWO}\"\nlaunch = \"three\"\npages = [\"two\", \"three\"]\n"
    )
}

/// The demo repo's canvas index: board `a` on page one mounts `c`, which has
/// no page and so is on the first page; `b` is on page two and `d` on page
/// three. Note `n1` is on page two and `n2` has no page.
pub(crate) fn index() -> Value {
    json!({
        "v": 3,
        "title": "demo mocks",
        "attachments": {},
        "createdOnFiles": {"v": 1, "at": "2026-11-07T03:22:02Z"},
        "designSystems": [{"namespace": "demo", "version": null, "copiedAt": "2026-11-07T03:22:02Z"}],
        "launch": {"view": "canvas", "page": "two"},
        "pages": [{"id": "one", "name": "One"}, {"id": "two", "name": "Two"}, {"id": "three", "name": "Three"}],
        "boards": {
            "a.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "one"},
            "b.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "two"},
            "c.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10},
            "d.dc.html": {"x": 0, "y": 0, "w": 10, "h": 10, "page": "three"}
        },
        "order": ["d.dc.html", "c.dc.html", "b.dc.html", "a.dc.html"],
        "notes": {
            "n1": {"x": 0, "y": 0, "text": "on two", "page": "two"},
            "n2": {"x": 0, "y": 0, "text": "on the first page"}
        },
        "noteOrder": ["n2", "n1"]
    })
}

/// The files of the demo repo's design/mocks, by name.
pub(crate) fn boards() -> BTreeMap<String, String> {
    [
        (
            "a.dc.html",
            "<x-dc>A <dc-import name=\"c\"></dc-import></x-dc>",
        ),
        ("b.dc.html", "<x-dc>B</x-dc>"),
        ("c.dc.html", "<x-dc>C</x-dc>"),
        ("d.dc.html", "<x-dc>D</x-dc>"),
    ]
    .into_iter()
    .map(|(name, text)| (name.to_string(), text.to_string()))
    .collect()
}

/// The canvases parsed from <text>.
fn parsed(text: &str) -> Vec<Canvas> {
    parse_canvases(text, CANVASES).unwrap().canvases
}

#[test]
fn a_split_that_leaves_a_page_out_or_gives_it_twice_is_refused() {
    let split_of = |text: &str| {
        let text = format!("[design_system]\ntitle = \"{DS}\"\n{text}");
        message(split(&index(), &parsed(&text), &boards()))
    };
    let canvas = |title: &str, launch: &str, pages: &str| {
        format!("[[canvas]]\ntitle = \"{title}\"\nlaunch = \"{launch}\"\npages = [{pages}]\n")
    };
    let refused = split_of(&canvas(ONE, "one", "\"one\", \"two\""));
    assert!(refused.contains("page three is on no canvas"), "{refused}");
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "one", "\"one\", \"two\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(refused.contains("page two is on two canvases"), "{refused}");
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "one", "\"one\", \"four\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(
        refused.contains(&format!("page four, which {CANVAS} does not list")),
        "{refused}"
    );
    let refused = split_of(&format!(
        "{}{}",
        canvas(ONE, "two", "\"one\""),
        canvas(TWO, "two", "\"two\", \"three\"")
    ));
    assert!(
        refused.contains(&format!(
            "{ONE:?} launches on page two, which is not one of its pages"
        )),
        "{refused}"
    );
}

#[test]
fn a_board_that_mounts_a_board_of_another_canvas_is_refused() {
    let mut files = boards();
    files.insert(
        "b.dc.html".to_string(),
        "<x-dc>B <dc-import name=\"a\"></dc-import></x-dc>".to_string(),
    );
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains(&format!(
            "b.dc.html on {TWO:?} mounts a.dc.html, which is on {ONE:?}"
        )),
        "{refused}"
    );
}

#[test]
fn boards_and_the_index_must_name_the_same_files() {
    let mut files = boards();
    files.insert("e.dc.html".to_string(), "<x-dc>E</x-dc>".to_string());
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains(&format!(
            "e.dc.html is a board file that {CANVAS} does not place"
        )),
        "{refused}"
    );
    let mut files = boards();
    files.remove("d.dc.html");
    let refused = message(split(&index(), &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains("canvas.json places d.dc.html, which is no board file"),
        "{refused}"
    );
}

#[test]
fn a_canvas_over_the_file_limit_is_refused() {
    let mut index = index();
    let mut files = boards();
    for n in 0..MAX_FILES {
        let name = format!("many-{n}.dc.html");
        index["boards"][&name] = json!({"x": 0, "y": 0, "w": 10, "h": 10, "page": "two"});
        files.insert(name, "<x-dc>M</x-dc>".to_string());
    }
    let refused = message(split(&index, &parsed(&canvases_toml()), &files));
    assert!(
        refused.contains(&format!("{TWO:?} would hold")) && refused.contains("at most 512"),
        "{refused}"
    );
}

/// Each canvas and the design system is named by its title, never by a
/// link, which belongs to one account; no two canvases share a title, and the
/// file names the design system, so that no publish leaves it out.
#[test]
fn the_canvases_file_names_each_artifact_by_its_title() {
    let canvas = |title: &str| {
        format!("[[canvas]]\ntitle = \"{title}\"\nlaunch = \"one\"\npages = [\"one\"]\n")
    };
    let system = |title: &str| format!("[design_system]\ntitle = \"{title}\"\n");
    let refused = message(parse_canvases(
        &format!("{}{}{}", system(DS), canvas(ONE), canvas(ONE)),
        CANVASES,
    ));
    assert!(
        refused.contains(&format!("gives two canvases the title {ONE:?}")),
        "{refused}"
    );
    for title in ["", " demo"] {
        let refused = message(parse_canvases(
            &format!("{}{}", system(title), canvas(ONE)),
            CANVASES,
        ));
        assert!(refused.contains("is no title"), "{refused}");
    }
    let refused = message(parse_canvases(
        &format!(
            "[design_system]\nurl = \"https://claude.ai/artifact/Ds\"\n{}",
            canvas(ONE)
        ),
        CANVASES,
    ));
    assert!(refused.contains("unknown field `url`"), "{refused}");
    let refused = message(parse_canvases(&canvas(ONE), CANVASES));
    assert!(refused.contains("names no design system"), "{refused}");
    let refused = message(parse_canvases("[[canvas]]\nurl = \"x\"\n", CANVASES));
    assert!(refused.contains(CANVASES), "{refused}");
    assert!(message(parse_canvases(&system(DS), CANVASES)).contains("names no canvas"));
    let published = parse_canvases(&canvases_toml(), CANVASES).unwrap();
    assert_eq!(published.design_system, DS);
    assert_eq!(published.canvases.len(), 2);
}

/// The design system's entry in the repo's index gives neither its title nor
/// its artifact: the publish sets both from what it found on the account.
#[test]
fn the_publish_names_the_design_system_it_found() {
    let mut named = index();
    name_design_system(&mut named, DS, "https://claude.ai/artifact/Found").unwrap();
    assert_eq!(
        named["designSystems"],
        json!([{"namespace": "demo", "version": null, "copiedAt": "2026-11-07T03:22:02Z", "title": DS, "artifact": "https://claude.ai/artifact/Found"}])
    );
    for key in ["title", "artifact"] {
        let mut given = index();
        given["designSystems"][0][key] = json!("x");
        let refused = message(name_design_system(
            &mut given,
            DS,
            "https://claude.ai/artifact/F",
        ));
        assert!(refused.contains(&format!("gives its {key}")), "{refused}");
    }
    let mut none = index();
    none["designSystems"] = json!([]);
    assert!(message(name_design_system(&mut none, DS, "u")).contains("names no design system"));
}

/// A change a test makes to the demo index.
type Change = dyn Fn(&mut Value);

/// Each part of the index that is not the shape the canvas reads is refused,
/// naming the file and the entry, never read as empty or left out.
#[test]
fn an_index_entry_of_the_wrong_shape_is_refused_naming_it() {
    let canvases = parsed(&canvases_toml());
    let refused = |change: &Change| {
        let mut index = index();
        change(&mut index);
        message(split(&index, &canvases, &boards()))
    };
    let cases: [(&Change, String); 7] = [
        (
            &|index| index["pages"][1] = json!({"name": "Two"}),
            format!("{CANVAS}: page 2 of `pages` ({{\"name\":\"Two\"}}) has no id"),
        ),
        (
            &|index| index["notes"] = json!(["n1"]),
            format!("{CANVAS}: `notes` is not an object of notes"),
        ),
        (
            &|index| index["boards"] = json!([]),
            format!("{CANVAS}: `boards` is not an object of board files"),
        ),
        (
            &|index| index["boards"]["b.dc.html"]["page"] = json!("four"),
            format!("{CANVAS}: board b.dc.html is on page four, which {CANVAS} does not list"),
        ),
        (
            &|index| index["notes"]["n1"]["page"] = json!("four"),
            format!("{CANVAS}: note n1 is on page four, which {CANVAS} does not list"),
        ),
        (
            &|index| index["notes"]["n1"]["page"] = json!(2),
            format!("{CANVAS}: note n1 gives the page 2, which is no page id"),
        ),
        (
            &|index| index["noteOrder"] = json!("n1"),
            format!("{CANVAS}: `noteOrder` is not a list"),
        ),
    ];
    for (change, expected) in cases {
        let said = refused(change);
        assert!(said.contains(&expected), "{said}");
    }
}
