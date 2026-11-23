use super::*;
use crate::design::Frame;
use crate::render::board::render;
use crate::render::fixture;

const BOX: Bounds = Bounds {
    x: 0.0,
    y: 0.0,
    width: 10.0,
    height: 10.0,
};

fn node(id: &str, role: &str, children: &[&str]) -> Raw {
    Raw {
        id: id.to_string(),
        chrome_role: role.to_string(),
        children: children.iter().map(|c| c.to_string()).collect(),
        bounds: Some(BOX),
        ..Raw::default()
    }
}

fn named(mut node: Raw, name: &str) -> Raw {
    node.name = Some(Name {
        text: name.to_string(),
        origin: Origin::Computed,
    });
    node
}

fn given(node: Raw, name: &str) -> Raw {
    let mut node = named(node, name);
    node.name.as_mut().unwrap().origin = Origin::Given;
    node
}

#[test]
fn chrome_roles_become_the_shots_words() {
    let words: Vec<Role> = [
        "button",
        "switch",
        "checkbox",
        "radio",
        "textbox",
        "searchbox",
        "combobox",
        "listbox",
        "tab",
        "link",
        "img",
        "slider",
        "progressbar",
        "spinbutton",
        "StaticText",
        "heading",
    ]
    .iter()
    .map(|r| Role::of(r))
    .collect();
    use Role::*;
    assert_eq!(
        words,
        [
            Button, Toggle, Toggle, Radio, Field, Field, Popup, Popup, Tab, Link, Image, Slider,
            Progress, Stepper, Text, Other
        ]
    );
}

#[test]
fn a_controls_text_is_its_name_and_not_an_element() {
    let raw = [
        node("1", "RootWebArea", &["2"]),
        named(node("2", "button", &["3"]), "Add library\u{2026}"),
        named(node("3", "StaticText", &[]), "Add library\u{2026}"),
    ];
    let elements = build(&raw, Platform::Mac).unwrap();
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].role, Role::Button);
    assert_eq!(elements[0].label.as_deref(), Some("Add library\u{2026}"));
}

#[test]
fn ignored_nodes_are_left_out_and_their_children_attach_to_the_next_kept_ancestor() {
    let mut hidden = named(node("3", "StaticText", &[]), "Note");
    hidden.ignored = true;
    let mut wrapper = node("2", "group", &["3", "4"]);
    wrapper.ignored = true;
    let raw = [
        node("1", "list", &["2"]),
        wrapper,
        hidden,
        named(node("4", "StaticText", &[]), "Visible"),
    ];
    let elements = build(&raw, Platform::Mac).unwrap();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0].native, "list");
    assert_eq!(elements[1].label.as_deref(), Some("Visible"));
    assert_eq!(elements[1].parent, Some(0));
}

#[test]
fn an_open_menu_is_read_after_the_page_it_opens_over() {
    let raw = [
        node("1", "list", &["2", "3", "5"]),
        named(node("2", "button", &[]), "Kitchen"),
        node("3", "menu", &["4"]),
        named(node("4", "button", &[]), "Details"),
        named(node("5", "button", &[]), "Den"),
    ];
    let elements = build(&raw, Platform::Android).unwrap();
    let order: Vec<_> = elements
        .iter()
        .map(|e| e.label.as_deref().unwrap_or(&e.native))
        .collect();
    assert_eq!(order, ["list", "Kitchen", "Den", "menu", "Details"]);
    assert_eq!(elements[3].parent, Some(0));
    assert_eq!(elements[4].parent, Some(3));
}

#[test]
fn open_menus_are_read_in_the_order_the_page_opens_them() {
    let mut hidden = node("6", "menu", &["7"]);
    hidden.ignored = true;
    let raw = [
        node("1", "list", &["2", "4", "6", "8"]),
        node("2", "menu", &["3"]),
        named(node("3", "button", &[]), "First"),
        node("4", "menu", &["5"]),
        named(node("5", "button", &[]), "Second"),
        hidden,
        named(node("7", "button", &[]), "Inside a hidden menu"),
        named(node("8", "button", &[]), "Page"),
    ];
    let elements = build(&raw, Platform::Mac).unwrap();
    let order: Vec<_> = elements
        .iter()
        .map(|e| e.label.as_deref().unwrap_or(&e.native))
        .collect();
    assert_eq!(
        order,
        [
            "list",
            "Inside a hidden menu",
            "Page",
            "menu",
            "First",
            "menu",
            "Second"
        ],
        "an ignored menu is no window: its visible children stay in the page"
    );
}

#[test]
fn a_button_that_opens_a_menu_is_a_popup() {
    let mut menu = named(node("2", "button", &[]), "Never");
    menu.has_popup = true;
    let raw = [
        node("1", "list", &["2", "3"]),
        menu,
        named(node("3", "button", &[]), "Set up"),
    ];
    let elements = build(&raw, Platform::Mac).unwrap();
    assert_eq!(elements[1].role, Role::Popup);
    assert_eq!(elements[2].role, Role::Button);
}

#[test]
fn an_android_board_keeps_a_button_that_opens_a_menu_a_button() {
    let mut menu = named(node("2", "button", &[]), "More");
    menu.has_popup = true;
    let mut combo = named(node("3", "combobox", &[]), "Sort");
    combo.has_popup = true;
    let raw = [node("1", "list", &["2", "3"]), menu, combo];
    let elements = build(&raw, Platform::Android).unwrap();
    assert_eq!(elements[1].role, Role::Button);
    assert_eq!(elements[2].role, Role::Popup);
}

#[test]
fn a_node_of_role_other_keeps_a_label_its_board_gives_and_not_one_chrome_computes() {
    let raw = [
        node("1", "list", &["2", "3", "4"]),
        given(node("2", "menuitem", &[]), "5 of 5 stars"),
        named(node("3", "heading", &[]), "Shuffle"),
        node("4", "group", &[]),
    ];
    let elements = build(&raw, Platform::Android).unwrap();
    assert_eq!(elements[1].role, Role::Other);
    assert_eq!(elements[1].label.as_deref(), Some("5 of 5 stars"));
    assert_eq!(elements[2].label, None, "a name from content is no label");
    assert_eq!(elements[3].label, None, "a node with no name has no label");
}

#[test]
fn a_named_node_of_role_other_with_no_sources_is_an_error() {
    let mut unsent = named(node("2", "menuitem", &[]), "Play next");
    unsent.name.as_mut().unwrap().origin = Origin::Unsent;
    let raw = [node("1", "list", &["2"]), unsent];
    assert!(matches!(
        build(&raw, Platform::Android),
        Err(ReadError::NoNameSources { id, .. }) if id == "2"
    ));
}

#[test]
fn a_child_the_tree_does_not_hold_is_an_error() {
    let raw = [node("1", "list", &["2"])];
    assert!(matches!(build(&raw, Platform::Mac), Err(ReadError::NoNode(id)) if id == "2"));
}

#[test]
fn colours_are_srgb_with_alpha_or_absent() {
    assert_eq!(srgb("rgb(27, 27, 34)").as_deref(), Some("#1B1B22FF"));
    assert_eq!(srgb("rgba(255, 0, 128, 0.5)").as_deref(), Some("#FF008080"));
    assert_eq!(srgb("color(srgb 1 0 0)"), None);
}

const BOARD: &str = r##"<x-dc><helmet><style>body{margin:0;font:13px sans-serif}</style></helmet>
<div style="width:400px"><span id="short" style="font-weight:600">Short</span></div>
<button>Add <b>library</b>&hellip;</button>
<div role="switch" aria-checked="true" aria-label="Sync" tabindex="0">x</div>
<div role="switch" aria-checked="false" aria-label="Offline" tabindex="0">x</div>
<div role="radio" aria-checked="true" aria-label="Dark" tabindex="0">x</div>
<input type="text" aria-label="Name" placeholder="Library name" value="Odile">
<input type="text" aria-label="Restore" placeholder="Restore code">
<input type="password" aria-label="Secret" value="abc">
<input type="password" aria-label="Secret empty" value="">
<div role="textbox" aria-label="Code empty"><span aria-hidden="true">Code empty</span></div>
<div role="textbox" aria-label="Code typed">CVR1 ABCD</div>
<select aria-label="Mode"><option>Never</option><option selected>Always</option></select>
<div role="tab" aria-selected="true" tabindex="0">Library</div>
<a href="#x">More</a>
<button aria-current="page">Open pane</button>
<button aria-current="false">Closed pane</button>
<button>Plain pane</button>
<img alt="Cover" width="10" height="10" src="data:image/gif;base64,R0lGODlhAQABAAAAACw=">
<div role="slider" aria-valuenow="5" aria-valuemin="0" aria-valuemax="10" aria-label="Volume" tabindex="0">x</div>
<div role="progressbar" aria-valuenow="40" aria-valuemin="0" aria-valuemax="100" aria-label="Import">x</div>
<span role="spinbutton" aria-label="Number of CD" tabindex="0">1</span>
<p aria-hidden="true">Note for the builder</p>
<p>Words   with
   <span>spans</span> mixed</p>
<button disabled>Off</button>
<ul style="padding-left:16px"><li><span>First address</span><button aria-label="Copy first">c</button></li><li><span>Second address</span></li></ul>
<button aria-haspopup="menu">Never</button>
<div class="demo-keyboard" aria-hidden="true" style="position:absolute;left:0;top:400px;width:300px;height:150px">k</div>
</x-dc>"##;

async fn read_fixture() -> Vec<Element> {
    let root = fixture::tree(&[("fixture.roles", BOARD)]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.roles");
        let rendered = render(&page, "fixture.roles", &url, Frame { w: 500, h: 600 }, 1.0)
            .await
            .unwrap();
        elements(&page, rendered.frame, Platform::Mac).await
    })
    .await;
    elements.unwrap()
}

fn find<'a>(elements: &'a [Element], role: Role, label: &str) -> &'a Element {
    elements
        .iter()
        .find(|e| e.role == role && e.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {role:?} {label:?} in {elements:#?}"))
}

#[tokio::test]
async fn a_board_reads_elements_with_area_inside_its_frame_and_overflow_clips() {
    let root = fixture::tree(&[(
        "fixture.clips",
        r#"<x-dc><helmet><style>body{margin:0;font:16px/20px sans-serif}</style></helmet>
<div style="height:30px;overflow:auto">
  <div style="height:60px;overflow:hidden">
    <div>Inside</div><div>Partly inside</div><div>Below the scroll area</div>
  </div>
</div>
<div style="width:100px;overflow-x:clip;white-space:nowrap">
  <div style="margin-left:90px">Partly across</div>
  <div style="margin-left:100px">Past the right edge</div>
</div>
<div style="height:0;overflow:hidden">
  <div>Inside a hidden box</div>
  <button style="position:absolute;top:80px">Outside the clip</button>
</div>
<div style="position:absolute;top:110px">Partly in the window</div>
<div style="position:absolute;top:130px">Below the window</div>
</x-dc>"#,
    )]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.clips");
        let rendered = render(&page, "fixture.clips", &url, Frame { w: 300, h: 120 }, 1.0)
            .await
            .unwrap();
        elements(&page, rendered.frame, Platform::Mac)
            .await
            .unwrap()
    })
    .await;
    let labels: Vec<_> = elements.iter().filter_map(|e| e.label.as_deref()).collect();
    assert_eq!(
        labels,
        [
            "Inside",
            "Partly inside",
            "Partly across",
            "Outside the clip",
            "Partly in the window",
        ]
    );
    let partial = find(&elements, Role::Text, "Partly across");
    assert!(partial.bounds.x + partial.bounds.width > 100.0);
}

#[tokio::test]
async fn a_boards_on_screen_keyboard_is_found_and_not_an_element() {
    let root = fixture::tree(&[("fixture.roles", BOARD)]);
    let found = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.roles");
        let rendered = render(&page, "fixture.roles", &url, Frame { w: 500, h: 600 }, 1.0)
            .await
            .unwrap();
        (
            unshot(&page, &[".demo-keyboard".to_string()])
                .await
                .unwrap(),
            elements(&page, rendered.frame, Platform::Android)
                .await
                .unwrap(),
        )
    })
    .await;
    let (boxes, elements) = found;
    assert_eq!(boxes.len(), 1);
    assert_eq!((boxes[0].width, boxes[0].height), (300.0, 150.0));
    assert!(!elements.iter().any(|e| e.label.as_deref() == Some("k")));
}

#[tokio::test]
async fn a_board_reads_as_the_shots_do() {
    let elements = read_fixture().await;

    let button = find(&elements, Role::Button, "Add library\u{2026}");
    assert!(button.enabled);
    assert!(
        !elements
            .iter()
            .any(|e| e.role == Role::Text
                && e.label.as_deref().is_some_and(|l| l.starts_with("Add"))),
        "the button's text is its name only"
    );
    assert!(!find(&elements, Role::Button, "Off").enabled);
    find(&elements, Role::Popup, "Never");

    assert_eq!(
        find(&elements, Role::Toggle, "Sync").value.as_deref(),
        Some("on")
    );
    assert_eq!(
        find(&elements, Role::Toggle, "Offline").value.as_deref(),
        Some("off")
    );
    assert_eq!(find(&elements, Role::Radio, "Dark").selected, Some(true));

    let field = find(&elements, Role::Field, "Name");
    assert_eq!(field.value.as_deref(), Some("Odile"));
    assert_eq!(field.placeholder.as_deref(), Some("Library name"));
    let empty = find(&elements, Role::Field, "Restore");
    assert_eq!(
        empty.value.as_deref(),
        Some(""),
        "an empty field's value is not its placeholder"
    );
    assert_eq!(empty.placeholder.as_deref(), Some("Restore code"));
    assert_eq!(
        find(&elements, Role::Field, "Secret").value.as_deref(),
        Some("\u{F79A}"),
        "a password field reads masked, as a Mac's secure field does"
    );
    assert_eq!(
        find(&elements, Role::Field, "Secret empty")
            .value
            .as_deref(),
        Some("")
    );
    assert_eq!(
        find(&elements, Role::Field, "Code empty").value.as_deref(),
        Some("")
    );
    assert_eq!(
        find(&elements, Role::Field, "Code typed").value.as_deref(),
        Some("CVR1 ABCD")
    );

    assert_eq!(
        find(&elements, Role::Popup, "Mode").value.as_deref(),
        Some("Always")
    );
    assert_eq!(find(&elements, Role::Tab, "Library").selected, Some(true));
    find(&elements, Role::Link, "More");
    assert_eq!(
        find(&elements, Role::Button, "Open pane").selected,
        Some(true)
    );
    assert_ne!(
        find(&elements, Role::Button, "Closed pane").selected,
        Some(true)
    );
    assert_ne!(
        find(&elements, Role::Button, "Plain pane").selected,
        Some(true)
    );
    find(&elements, Role::Image, "Cover");
    find(&elements, Role::Stepper, "Number of CD");
    assert_eq!(
        find(&elements, Role::Slider, "Volume").value.as_deref(),
        Some("5")
    );
    assert_eq!(
        find(&elements, Role::Progress, "Import").value.as_deref(),
        Some("40")
    );

    assert!(
        !elements
            .iter()
            .any(|e| e.label.as_deref().is_some_and(|l| l.contains("builder"))),
        "an aria-hidden note is left out"
    );
    find(&elements, Role::Text, "Words with");
    find(&elements, Role::Text, "spans");
    find(&elements, Role::Text, "mixed");
}

#[tokio::test]
async fn a_board_element_is_read_where_layout_puts_it_before_any_transform() {
    let root = fixture::tree(&[(
        "fixture.transforms",
        r#"<x-dc><helmet><style>body{margin:0;font:16px/20px sans-serif}
.raised{position:absolute;left:20px;top:50px;transform:translateY(-10px)}
.spun{position:absolute;left:100px;top:50px;width:40px;height:20px;rotate:45deg;scale:2}
.moved{position:absolute;left:200px;top:50px;translate:30px 30px}</style></helmet>
<button class="raised">Raised</button>
<button class="spun">Spun</button>
<div class="moved">Moved words</div>
</x-dc>"#,
    )]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.transforms");
        let rendered = render(
            &page,
            "fixture.transforms",
            &url,
            Frame { w: 300, h: 150 },
            1.0,
        )
        .await
        .unwrap();
        let read = elements(&page, rendered.frame, Platform::Mac)
            .await
            .unwrap();
        // Reading leaves the page as it drew it.
        let still: String = page
            .evaluate("getComputedStyle(document.querySelector('.raised')).transform")
            .await
            .unwrap()
            .into_value()
            .unwrap();
        assert_eq!(still, "matrix(1, 0, 0, 1, 0, -10)");
        read
    })
    .await;
    let raised = find(&elements, Role::Button, "Raised");
    assert_eq!(raised.bounds.y, 50.0, "{:?}", raised.bounds);
    let spun = find(&elements, Role::Button, "Spun");
    assert_eq!(
        (
            spun.bounds.x,
            spun.bounds.y,
            spun.bounds.width,
            spun.bounds.height
        ),
        (100.0, 50.0, 40.0, 20.0)
    );
    let moved = find(&elements, Role::Text, "Moved words");
    // The words' box sits inside the 20 pt line the layout gives them.
    assert_eq!(moved.bounds.x, 200.0);
    assert!((50.0..52.0).contains(&moved.bounds.y), "{:?}", moved.bounds);
}

#[tokio::test]
async fn a_texts_box_is_its_words_not_its_container() {
    let elements = read_fixture().await;
    let short = find(&elements, Role::Text, "Short");
    assert!(short.bounds.width < 80.0, "{:?}", short.bounds);
    assert!(
        short.bounds.height > 10.0 && short.bounds.height < 30.0,
        "{:?}",
        short.bounds
    );
    let style = short.text.as_ref().unwrap();
    assert_eq!(style.size, Some(13.0));
    assert_eq!(style.weight, Some(600));
    assert_eq!(style.color.as_ref().map(String::len), Some(9));
}

#[tokio::test]
async fn each_board_element_names_the_nearest_component_that_draws_it() {
    let root = fixture::tree(&[(
        "fixture.components",
        "<x-dc><p>Loose words</p><x-import component-from-global-scope=\"Demo.Card\"></x-import></x-dc>",
    )]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.components");
        let rendered = render(
            &page,
            "fixture.components",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
        .unwrap();
        elements(&page, rendered.frame, Platform::Mac).await
    })
    .await
    .unwrap();
    let component = |label: &str| find(&elements, Role::Text, label).component.clone();
    assert_eq!(component("hello").as_deref(), Some("Box"));
    assert_eq!(component("Card title").as_deref(), Some("Card"));
    assert_eq!(component("Loose words"), None);
}

const THROWING_GLOBAL: &str = "<script>Object.defineProperty(window, 'Broken', \
    { get() { throw new Error('no reading this'); } });</script>";

#[tokio::test]
async fn a_page_global_that_throws_when_read_fails_the_read() {
    let root = fixture::tree(&[(
        "fixture.throws",
        &format!("{THROWING_GLOBAL}<x-dc><p>Words</p></x-dc>"),
    )]);
    let read = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.throws");
        let rendered = render(&page, "fixture.throws", &url, Frame { w: 300, h: 200 }, 1.0)
            .await
            .unwrap();
        elements(&page, rendered.frame, Platform::Mac).await
    })
    .await;
    let error = read.expect_err("the read fails").to_string();
    assert!(error.contains("no reading this"), "{error}");
}

#[tokio::test]
async fn the_reader_leaves_out_a_clipped_node_without_reading_its_component() {
    let root = fixture::tree(&[(
        "fixture.clipped",
        &format!(
            "{THROWING_GLOBAL}<x-dc><div style='height:0;overflow:hidden'><p>Words</p></div></x-dc>"
        ),
    )]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.clipped");
        let rendered = render(
            &page,
            "fixture.clipped",
            &url,
            Frame { w: 300, h: 200 },
            1.0,
        )
        .await
        .unwrap();
        elements(&page, rendered.frame, Platform::Mac)
            .await
            .unwrap()
    })
    .await;
    assert!(elements.is_empty());
}

#[test]
fn a_kept_node_with_no_box_fails_the_read_and_says_why() {
    let mut button = named(node("2", "button", &[]), "Add");
    button.bounds = None;
    button.no_box = Some("its DOM node has no box to measure");
    let mut hidden = node("3", "button", &[]);
    hidden.bounds = None;
    hidden.ignored = true;
    let root = node("1", "generic", &["3"]);
    assert!(build(&[root, hidden.clone()], Platform::Mac).is_ok());
    let root = node("1", "generic", &["2", "3"]);
    let error = build(&[root, button, hidden], Platform::Mac)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "the board's button node \"Add\" has no box: its DOM node has no box to measure"
    );
}

#[tokio::test]
async fn a_list_items_marker_is_no_element_of_its_own() {
    let elements = read_fixture().await;
    find(&elements, Role::Text, "First address");
    assert!(
        !elements.iter().any(|e| e.native == "ListMarker"),
        "a marker has no element of its own"
    );
    let items: Vec<&Element> = elements.iter().filter(|e| e.native == "listitem").collect();
    assert_eq!(items.len(), 2);
}

#[test]
fn a_password_field_reads_masked_as_each_platform_masks_it() {
    let mut secret = named(node("2", "textbox", &[]), "Secret");
    secret.value = Some("abcd".to_string());
    secret.secret = true;
    let raw = [node("1", "RootWebArea", &["2"]), secret];
    let value = |platform| {
        build(&raw, platform)
            .unwrap()
            .into_iter()
            .find(|e| e.role == Role::Field)
            .unwrap()
            .value
    };
    assert_eq!(value(Platform::Mac).as_deref(), Some("\u{F79A}"));
    assert_eq!(
        value(Platform::Android).as_deref(),
        Some("\u{2022}\u{2022}\u{2022}\u{2022}")
    );
    assert_eq!(
        value(Platform::Web).as_deref(),
        Some("\u{2022}\u{2022}\u{2022}\u{2022}")
    );
}

#[test]
fn mac_masking_uses_empty_and_filled_states_only_for_passwords() {
    let mut empty = named(node("2", "textbox", &[]), "Empty password");
    empty.secret = true;
    empty.value = Some(String::new());
    let mut plain = named(node("3", "textbox", &[]), "Ordinary field");
    plain.value = Some("\u{F79A}\u{F79A}".to_string());
    let raw = [node("1", "RootWebArea", &["2", "3"]), empty, plain];
    let elements = build(&raw, Platform::Mac).unwrap();
    assert_eq!(
        find(&elements, Role::Field, "Empty password")
            .value
            .as_deref(),
        Some("")
    );
    assert_eq!(
        find(&elements, Role::Field, "Ordinary field")
            .value
            .as_deref(),
        Some("\u{F79A}\u{F79A}")
    );
}

#[tokio::test]
async fn chrome_says_whether_the_board_gave_a_name_or_computed_it_from_content() {
    let root = fixture::tree(&[(
        "fixture.names",
        r#"<x-dc><helmet><style>body{margin:0;font:13px sans-serif}</style></helmet>
<div role="menuitem" aria-label="5 of 5 stars" tabindex="0">x</div>
<h2>Shuffle</h2>
<div role="menuitem" tabindex="0">Play next</div>
<div role="group" aria-labelledby="t"><span id="t">Devices</span></div>
</x-dc>"#,
    )]);
    let elements = fixture::with_browser(root.path(), async |server, chrome| {
        let page = chrome.new_page().await.unwrap();
        let url = server.board_url("fixture.names");
        let rendered = render(&page, "fixture.names", &url, Frame { w: 300, h: 200 }, 1.0)
            .await
            .unwrap();
        elements(&page, rendered.frame, Platform::Android)
            .await
            .unwrap()
    })
    .await;
    let other = |native: &str, nth: usize| {
        elements
            .iter()
            .filter(|e| e.role == Role::Other && e.native == native)
            .nth(nth)
            .unwrap_or_else(|| panic!("no {native} {nth} in {elements:#?}"))
            .label
            .as_deref()
    };
    assert_eq!(other("menuitem", 0), Some("5 of 5 stars"));
    assert_eq!(other("heading", 0), None);
    assert_eq!(other("menuitem", 1), None);
    assert_eq!(other("group", 0), Some("Devices"));
}
