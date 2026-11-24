//! Reading a rendered board the way the shots' platforms read theirs: Chrome's
//! accessibility tree becomes an element list in reading order, in the shape
//! the shot harnesses write (`role`, `native`, `label`, `value`, `bounds`,
//! `enabled`, `selected`, `text`, `parent`, `focused`, `placeholder`).
//!
//! The tree is read in two parts. [`build`] turns raw nodes into elements and
//! needs no browser: roles, words, folding a control's text into its name,
//! skipping ignored nodes. [`elements`] gathers the raw nodes from a page:
//! the accessibility tree, and for each node its box, from its DOM node (a text
//! node's box is the range of its words, not its parent's), whether any part
//! of the box is visible, and for text its computed font.

use crate::names::Platform;
use crate::shots::{Bounds, Element, Role, TextStyle, words};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::accessibility::{
    AxNode, AxNodeId, AxPropertyName, AxValue, AxValueSourceType, EnableParams, GetFullAxTreeParams,
};
use chromiumoxide::cdp::browser_protocol::dom::ResolveNodeParams;
use chromiumoxide::cdp::js_protocol::runtime::{CallArgument, CallFunctionOnParams};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("Chrome failed: {0}")]
    Protocol(String),
    #[error("the accessibility tree has no node {0}")]
    NoNode(String),
    #[error(
        "Chrome sent the accessibility node {id} of unknown role with the name {name:?} and no sources for it"
    )]
    NoNameSources { id: String, name: String },
    #[error("the board's {role} node {name:?} has no box: {why}")]
    NoBox {
        role: String,
        name: String,
        why: &'static str,
    },
}

fn protocol(error: impl std::fmt::Display) -> ReadError {
    ReadError::Protocol(error.to_string())
}

impl Role {
    /// The shots' word for a Chrome role.
    fn of(chrome: &str) -> Self {
        match chrome {
            "button" => Self::Button,
            "switch" | "checkbox" => Self::Toggle,
            "radio" => Self::Radio,
            "textbox" | "searchbox" => Self::Field,
            "combobox" | "PopUpButton" | "listbox" => Self::Popup,
            "tab" => Self::Tab,
            "link" => Self::Link,
            "img" | "image" => Self::Image,
            "slider" => Self::Slider,
            "progressbar" => Self::Progress,
            "spinbutton" => Self::Stepper,
            "StaticText" => Self::Text,
            _ => Self::Other,
        }
    }

    /// The role of a node: its Chrome role, except that on a Mac board a
    /// button that opens a menu or list is a pop-up (SwiftUI's `Menu` is an
    /// `AXPopUpButton`). Compose cannot say "opens a menu" of a button, so an
    /// Android board's stays a button.
    fn for_node(node: &Raw, platform: Platform) -> Self {
        match Self::of(&node.chrome_role) {
            Self::Button if node.has_popup && platform == Platform::Mac => Self::Popup,
            role => role,
        }
    }
}

/// Chrome roles that only hold other nodes and say nothing of their own. A
/// list item's marker (`ListMarker`, "\u{2022} ") is one: it has no DOM node
/// to measure, so the list item stands for it.
const STRUCTURE: [&str; 7] = [
    "ListMarker",
    "generic",
    "none",
    "presentation",
    "RootWebArea",
    "InlineTextBox",
    "LineBreak",
];

/// A node's accessible name and where it comes from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Name {
    pub(crate) text: String,
    pub(crate) origin: Origin,
}

/// Where Chrome took a node's name from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Origin {
    /// The board gave it (an `aria-label` or `aria-labelledby`).
    Given,
    /// Chrome computed it from the node's content or another default.
    Computed,
    /// Chrome sent the name without its sources, as it does for the text nodes
    /// it makes up, which have no DOM node.
    Unsent,
}

/// What the browser holds about one node of the accessibility tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Raw {
    pub(crate) id: String,
    /// The DOM node the accessibility node describes.
    pub(crate) backend: Option<i64>,
    pub(crate) children: Vec<String>,
    pub(crate) ignored: bool,
    pub(crate) chrome_role: String,
    pub(crate) name: Option<Name>,
    pub(crate) value: Option<String>,
    /// `true`, `false` or `mixed`.
    pub(crate) checked: Option<String>,
    pub(crate) selected: Option<bool>,
    pub(crate) disabled: bool,
    pub(crate) focused: bool,
    /// The control opens a menu or list (`aria-haspopup`).
    pub(crate) has_popup: bool,
    pub(crate) bounds: Option<Bounds>,
    /// Why the node has no bounds, when it is one the board's elements read.
    pub(crate) no_box: Option<&'static str>,
    pub(crate) text: Option<TextStyle>,
    pub(crate) placeholder: Option<String>,
    /// The design-system component that draws the node's DOM node.
    pub(crate) component: Option<String>,
    /// A password field: what is typed shows masked, never as itself.
    pub(crate) secret: bool,
}

fn nonempty(text: Option<&str>) -> Option<String> {
    text.map(words).filter(|w| !w.is_empty())
}

/// A node the walk has yet to read: the node, the index of the nearest kept
/// ancestor, and whether a named control is above it.
type Pending<'a> = (&'a Raw, Option<usize>, bool);

/// Whether a node is a window of its own over the screen under it: an open
/// menu (`role=menu`) on mac and android, and on android also a dialog
/// (`role=dialog`), which TalkBack moves into before the screen. An ignored
/// node is no window: its visible children stay in the page.
fn is_window(node: &Raw, platform: Platform) -> bool {
    !node.ignored
        && match platform {
            Platform::Mac => node.chrome_role == "menu",
            Platform::Android => matches!(node.chrome_role.as_str(), "menu" | "dialog"),
            Platform::Web => false,
        }
}

/// The elements of `raw`, in reading order (the tree's order from its first
/// node). Ignored nodes (hidden, clipped, `aria-hidden`, presentational) are left out,
/// their visible children kept; a text inside a named control is the
/// control's name, and a text inside a field is its value, not an element of
/// its own.
///
/// An open window reads apart from the page it opens over. On a mac the shots
/// read an open menu after the page, in the order the page opens its menus. On
/// android the shots read each window (a menu or a dialog) before the page,
/// as TalkBack moves its focus into a window that opens: the windows come in
/// the tree's order, an outer window before the windows inside it, each
/// without them, and an android window hangs from no node of the page.
pub(crate) fn build(raw: &[Raw], platform: Platform) -> Result<Vec<Element>, ReadError> {
    let by_id: HashMap<&str, &Raw> = raw.iter().map(|n| (n.id.as_str(), n)).collect();
    let Some(first) = raw.first() else {
        return Ok(Vec::new());
    };
    let mut elements = Vec::new();
    let mut windows = VecDeque::new();
    match platform {
        Platform::Android => {
            for window in windows_in_tree_order(first, &by_id, platform)? {
                read(
                    &mut elements,
                    &by_id,
                    platform,
                    (window, None, false),
                    &mut windows,
                )?;
            }
            read(
                &mut elements,
                &by_id,
                platform,
                (first, None, false),
                &mut windows,
            )?;
        }
        Platform::Mac | Platform::Web => {
            read(
                &mut elements,
                &by_id,
                platform,
                (first, None, false),
                &mut windows,
            )?;
            while let Some(window) = windows.pop_front() {
                read(&mut elements, &by_id, platform, window, &mut windows)?;
            }
        }
    }
    Ok(elements)
}

/// The windows under `root`, each before the windows inside it.
fn windows_in_tree_order<'a>(
    root: &'a Raw,
    by_id: &HashMap<&str, &'a Raw>,
    platform: Platform,
) -> Result<Vec<&'a Raw>, ReadError> {
    let mut nodes = pre_order(root, by_id)?;
    nodes.retain(|node| is_window(node, platform));
    Ok(nodes)
}

/// `root` and every node under it, each node before its children, the
/// children in order.
fn pre_order<'a>(root: &'a Raw, by_id: &HashMap<&str, &'a Raw>) -> Result<Vec<&'a Raw>, ReadError> {
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        found.push(node);
        for child in node.children.iter().rev() {
            pending.push(
                by_id
                    .get(child.as_str())
                    .ok_or_else(|| ReadError::NoNode(child.clone()))?,
            );
        }
    }
    Ok(found)
}

/// Reads `start` and what is under it into `elements`. A window under `start`
/// is left for `windows` to hold, to be read apart from `start`.
fn read<'a>(
    elements: &mut Vec<Element>,
    by_id: &HashMap<&str, &'a Raw>,
    platform: Platform,
    start: Pending<'a>,
    windows: &mut VecDeque<Pending<'a>>,
) -> Result<(), ReadError> {
    let start_id = start.0.id.as_str();
    let mut pending = vec![start];
    while let Some((node, parent, folded)) = pending.pop() {
        if node.id != start_id && is_window(node, platform) {
            windows.push_back((node, parent, folded));
            continue;
        }
        let role = Role::for_node(node, platform);
        // A node of no known role is named only by what the board gives it: the
        // shot reports no label for a name Chrome computes from content.
        let kept = !node.ignored && !STRUCTURE.contains(&node.chrome_role.as_str());
        let label = match &node.name {
            Some(name) if kept && role == Role::Other && name.origin == Origin::Unsent => {
                return Err(ReadError::NoNameSources {
                    id: node.id.clone(),
                    name: name.text.clone(),
                });
            }
            Some(name) if role == Role::Other && name.origin != Origin::Given => None,
            Some(name) => nonempty(Some(&name.text)),
            None => None,
        };
        let inside_control = folded;
        let mut next_parent = parent;
        let mut next_folded = folded;
        if kept && !(role == Role::Text && inside_control) {
            let Some(bounds) = node.bounds else {
                return Err(ReadError::NoBox {
                    role: node.chrome_role.clone(),
                    name: node
                        .name
                        .as_ref()
                        .map(|n| n.text.clone())
                        .unwrap_or_default(),
                    why: node.no_box.unwrap_or("Chrome gave it none"),
                });
            };
            next_parent = Some(elements.len());
            elements.push(element(node, role, label.clone(), bounds, parent, platform));
        }
        if kept && role.is_control() && (label.is_some() || role == Role::Field) {
            next_folded = true;
        }
        for child in node.children.iter().rev() {
            let child = by_id
                .get(child.as_str())
                .ok_or_else(|| ReadError::NoNode(child.clone()))?;
            pending.push((child, next_parent, next_folded));
        }
    }
    Ok(())
}

fn element(
    node: &Raw,
    role: Role,
    label: Option<String>,
    bounds: Bounds,
    parent: Option<usize>,
    platform: Platform,
) -> Element {
    let value = match role {
        _ if !role.has_value() => None,
        Role::Toggle => node.checked.as_deref().map(|c| match c {
            "true" => "on".to_string(),
            "false" => "off".to_string(),
            other => other.to_string(),
        }),
        Role::Field => {
            let typed = node.value.clone().unwrap_or_default();
            Some(if node.secret {
                masked(&typed, platform)
            } else {
                typed
            })
        }
        _ => node.value.clone(),
    };
    let selected = match role {
        Role::Radio => node.checked.as_deref().map(|c| c == "true"),
        _ => node.selected,
    };
    Element {
        role,
        native: node.chrome_role.clone(),
        label,
        value,
        bounds,
        enabled: !node.disabled,
        selected,
        text: if role == Role::Text {
            node.text.clone()
        } else {
            None
        },
        parent,
        focused: node.focused.then_some(true),
        component: node.component.clone(),
        placeholder: if role == Role::Field {
            nonempty(node.placeholder.as_deref())
        } else {
            None
        },
    }
}

/// The masked value a board writes for a password field. Mac comparisons
/// keep whether the field is filled because native secure controls report
/// different numbers of bullets. Compose and Chrome use one per character.
fn masked(typed: &str, platform: Platform) -> String {
    match platform {
        Platform::Mac if typed.is_empty() => String::new(),
        Platform::Mac => "\u{F79A}".to_string(),
        Platform::Android | Platform::Web => typed.chars().map(|_| '\u{2022}').collect(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Clipping {
    frame: Bounds,
    body_overflow_applies_to_viewport: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "visibility", rename_all = "snake_case")]
enum Measurement {
    Clipped,
    Visible(Geometry),
}

/// What a visible DOM node reports about its box and its font.
#[derive(Debug, Deserialize)]
struct Geometry {
    bounds: Bounds,
    size: Option<f64>,
    weight: Option<u32>,
    color: Option<String>,
    placeholder: Option<String>,
    /// What a text field holds, from the DOM: an input's value, or a
    /// `role=textbox` element's text without its `aria-hidden` descendants.
    field_value: Option<String>,
    /// The DOM node says it is the current one (`aria-current`, any value but
    /// `false`). Chrome's accessibility tree has no selected state for a
    /// button, where the Mac marks the open row of a sidebar selected.
    current: bool,
    /// The node's own element, or its nearest DOM ancestor with
    /// `aria-selected`, says `true`. Chrome's accessibility tree has no
    /// selected state for a text or an image, where the Mac marks every
    /// element of a selected row selected.
    in_selected: bool,
    /// The design-system component that draws the node: the nearest one
    /// above it in React's tree.
    component: Option<String>,
    /// The node is a password input.
    secret: bool,
}

const GEOMETRY: &str = r#"function({frame, bodyOverflowAppliesToViewport}) {
  // The nearest component above a DOM node in React's tree: the first fiber
  // up from the node's own (or its nearest ancestor's) whose type is one.
  const componentOf = (node) => {
    for (let dom = node; dom; dom = dom.parentNode) {
      const key = Object.keys(dom).find((k) => k.startsWith('__reactFiber$'));
      if (!key) continue;
      const known = window.__buildsShots.components();
      for (let fiber = dom[key]; fiber; fiber = fiber.return) {
        const name = known.get(fiber.elementType) || known.get(fiber.type);
        if (name) return name;
      }
      return null;
    }
    return null;
  };
  // What a role=textbox element holds: its text without the descendants
  // hidden from the accessibility tree (a placeholder drawn inside it).
  const typed = (field) => {
    const copy = field.cloneNode(true);
    copy.querySelectorAll('[aria-hidden="true"]').forEach((n) => n.remove());
    return copy.textContent;
  };
  // The element above a node in the flat tree: a node at the top of a shadow
  // tree (the text of an input) sits under its host.
  const flatParent = (node) =>
    node.parentElement || (node.parentNode instanceof ShadowRoot ? node.parentNode.host : null);
  const clippingParent = (element, style) => {
    // Overflow between a positioned box and its containing block does not clip it.
    if (element instanceof HTMLElement && (style.position === 'absolute' || style.position === 'fixed')) {
      return element.offsetParent;
    }
    return flatParent(element);
  };
  const visible = (rect, ancestor) => {
    let left = Math.max(rect.left, frame.x);
    let top = Math.max(rect.top, frame.y);
    let right = Math.min(rect.right, frame.x + frame.width);
    let bottom = Math.min(rect.bottom, frame.y + frame.height);
    const root = document.documentElement;
    const viewportBody = bodyOverflowAppliesToViewport ? document.body : null;
    while (ancestor && ancestor !== root && left < right && top < bottom) {
      const style = getComputedStyle(ancestor);
      const clips = ancestor !== viewportBody && style.display !== 'inline' && style.display !== 'contents';
      const clipsX = clips && style.overflowX !== 'visible';
      const clipsY = clips && style.overflowY !== 'visible';
      if (clipsX || clipsY) {
        const box = ancestor.getBoundingClientRect();
        const clientLeft = box.left + ancestor.clientLeft;
        const clientTop = box.top + ancestor.clientTop;
        if (clipsX) {
          left = Math.max(left, clientLeft);
          right = Math.min(right, clientLeft + ancestor.clientWidth);
        }
        if (clipsY) {
          top = Math.max(top, clientTop);
          bottom = Math.min(bottom, clientTop + ancestor.clientHeight);
        }
      }
      ancestor = clippingParent(ancestor, style);
    }
    return left < right && top < bottom;
  };
  const isText = this.nodeType === 3;
  let rect;
  if (isText) {
    const range = document.createRange();
    range.selectNodeContents(this);
    rect = range.getBoundingClientRect();
  } else if (this.getBoundingClientRect) {
    rect = this.getBoundingClientRect();
  } else {
    return null;
  }
  const element = isText ? flatParent(this) : this;
  const style = element ? getComputedStyle(element) : null;
  if (!visible(rect, isText ? element : clippingParent(element, style))) {
    return {visibility: 'clipped'};
  }
  return {
    visibility: 'visible',
    bounds: {x: rect.x, y: rect.y, width: rect.width, height: rect.height},
    size: style ? parseFloat(style.fontSize) : null,
    weight: style ? parseInt(style.fontWeight, 10) : null,
    color: style ? style.color : null,
    placeholder: !isText && typeof this.placeholder === 'string' && this.placeholder
      ? this.placeholder : null,
    field_value: isText ? null
      : (this.tagName === 'INPUT' || this.tagName === 'TEXTAREA') ? this.value
      : this.getAttribute('role') === 'textbox' ? typed(this) : null,
    current: !isText && this.hasAttribute('aria-current')
      && this.getAttribute('aria-current') !== 'false',
    in_selected: element !== null
      && element.closest('[aria-selected]')?.getAttribute('aria-selected') === 'true',
    component: componentOf(this),
    secret: !isText && this.tagName === 'INPUT' && this.type === 'password',
  };
}"#;

/// `rgb(r, g, b)` or `rgba(r, g, b, a)` as `#RRGGBBAA`; anything else is not
/// guessed.
fn srgb(css: &str) -> Option<String> {
    let inner = css
        .strip_prefix("rgba(")
        .or_else(|| css.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    let channel = |p: &str| {
        p.parse::<f64>()
            .ok()
            .map(|v| v.round().clamp(0.0, 255.0) as u8)
    };
    let (r, g, b) = (channel(parts[0])?, channel(parts[1])?, channel(parts[2])?);
    let a = match parts.get(3) {
        Some(alpha) => (alpha.parse::<f64>().ok()?.clamp(0.0, 1.0) * 255.0).round() as u8,
        None => 255,
    };
    Some(format!("#{r:02X}{g:02X}{b:02X}{a:02X}"))
}

fn string(value: &Option<AxValue>) -> Option<String> {
    match value.as_ref()?.value.as_ref()? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The node's name and where Chrome took it from: an attribute or a related
/// element it used is the board's, anything else is computed.
fn name(node: &AxNode) -> Option<Name> {
    let text = string(&node.name)?;
    let origin = match node.name.as_ref().and_then(|n| n.sources.as_ref()) {
        None => Origin::Unsent,
        Some(sources)
            if sources.iter().any(|source| {
                matches!(
                    source.r#type,
                    AxValueSourceType::Attribute | AxValueSourceType::RelatedElement
                ) && source.superseded != Some(true)
                    && source.value.is_some()
            }) =>
        {
            Origin::Given
        }
        Some(_) => Origin::Computed,
    };
    Some(Name { text, origin })
}

fn raw(node: &AxNode) -> Raw {
    let property = |name: AxPropertyName| {
        node.properties
            .iter()
            .flatten()
            .find(|p| p.name == name)
            .map(|p| string(&Some(p.value.clone())))
    };
    Raw {
        id: node.node_id.inner().clone(),
        backend: node.backend_dom_node_id.map(|b| *b.inner()),
        children: node
            .child_ids
            .iter()
            .flatten()
            .map(|c: &AxNodeId| c.inner().clone())
            .collect(),
        ignored: node.ignored,
        chrome_role: string(&node.role).unwrap_or_default(),
        name: name(node),
        value: string(&node.value),
        checked: property(AxPropertyName::Checked).flatten(),
        selected: property(AxPropertyName::Selected)
            .flatten()
            .map(|s| s == "true"),
        disabled: property(AxPropertyName::Disabled)
            .flatten()
            .is_some_and(|d| d == "true"),
        focused: property(AxPropertyName::Focused)
            .flatten()
            .is_some_and(|f| f == "true"),
        has_popup: property(AxPropertyName::HasPopup)
            .flatten()
            .is_some_and(|p| p != "false"),
        ..Raw::default()
    }
}

/// The elements visible in `frame`, in reading order.
pub(crate) async fn elements(
    page: &Page,
    frame: Bounds,
    platform: Platform,
) -> Result<Vec<Element>, ReadError> {
    build(&gather(page, frame).await?, platform)
}

fn relative_to(mut bounds: Bounds, origin: (f64, f64)) -> Bounds {
    bounds.x -= origin.0;
    bounds.y -= origin.1;
    bounds
}

/// The boxes of the elements <selector> matches, measured from `origin`.
async fn boxes(page: &Page, selector: &str, origin: (f64, f64)) -> Result<Vec<Bounds>, ReadError> {
    let selector = serde_json::to_string(selector).expect("a string serializes");
    let bounds: Vec<Bounds> = page
        .evaluate(format!(
            "[...document.querySelectorAll({selector})].map(e => {{ \
             const r = e.getBoundingClientRect(); \
             return {{x: r.x, y: r.y, width: r.width, height: r.height}}; }})"
        ))
        .await
        .map_err(protocol)?
        .into_value()
        .map_err(protocol)?;
    Ok(bounds.into_iter().map(|b| relative_to(b, origin)).collect())
}

/// The boxes of the parts of a board that the app draws no counterpart for,
/// such as an on-screen keyboard: the elements each of <selectors> (the
/// app's `shots-unshot` line) matches. Such a part is hidden from the
/// accessibility tree, so the picture comparison leaves its area out. The
/// boxes are measured from `origin`.
pub(crate) async fn unshot(
    page: &Page,
    selectors: &[String],
    origin: (f64, f64),
) -> Result<Vec<Bounds>, ReadError> {
    if selectors.is_empty() {
        return Ok(Vec::new());
    }
    boxes(page, &selectors.join(", "), origin).await
}

/// The class of the dashed stand-in a container's preview holds for its
/// content.
const SLOT_CLASS: &str = "bae-preview-slot";

/// The boxes of the board's preview slots, measured from `origin`. The
/// comparison leaves out what either side draws inside them (compare.rs).
pub(crate) async fn slots(page: &Page, origin: (f64, f64)) -> Result<Vec<Bounds>, ReadError> {
    boxes(page, &format!(".{SLOT_CLASS}"), origin).await
}

/// The elements under the DOM node `backend`, in reading order, with boxes
/// measured from the top-left corner of its screenshot `frame`.
pub(crate) async fn elements_in(
    page: &Page,
    backend: i64,
    frame: Bounds,
    platform: Platform,
) -> Result<Vec<Element>, ReadError> {
    let raw = gather(page, frame).await?;
    build(&subtree(&raw, backend)?, platform)
}

/// The node describing DOM node `backend` and everything under it, the node
/// first.
fn subtree(raw: &[Raw], backend: i64) -> Result<Vec<Raw>, ReadError> {
    let by_id: HashMap<&str, &Raw> = raw.iter().map(|n| (n.id.as_str(), n)).collect();
    let root = raw
        .iter()
        .find(|n| n.backend == Some(backend))
        .ok_or_else(|| ReadError::NoNode(format!("DOM node {backend}")))?;
    Ok(pre_order(root, &by_id)?.into_iter().cloned().collect())
}

/// The id of the style that turns transforms off while the reader measures.
const UNTRANSFORMED: &str = "builds-shots-untransformed";

async fn gather(page: &Page, frame: Bounds) -> Result<Vec<Raw>, ReadError> {
    // The root's overflow applies to the viewport, as does the body's when
    // the root lets overflow through.
    let body_overflow_applies_to_viewport = page
        .evaluate(
            "(() => { const root = getComputedStyle(document.documentElement); \
             return root.overflowX === 'visible' && root.overflowY === 'visible'; })()",
        )
        .await
        .map_err(protocol)?
        .into_value()
        .map_err(protocol)?;
    let clipping = Clipping {
        frame,
        body_overflow_applies_to_viewport,
    };
    let clipping_argument = CallArgument::builder()
        .value(serde_json::to_value(clipping).map_err(protocol)?)
        .build();
    page.execute(EnableParams::default())
        .await
        .map_err(protocol)?;
    let tree = page
        .execute(GetFullAxTreeParams::default())
        .await
        .map_err(protocol)?;
    // The apps report each element's layout box, whatever transform or
    // graphics layer moves its drawing, so the boxes are read with the
    // transforms off. The picture is already taken.
    page.evaluate(format!(
        "(() => {{ const off = document.createElement('style'); off.id = '{UNTRANSFORMED}'; \
         off.textContent = '@layer {{ *, *::before, *::after {{ transform: none !important; \
         translate: none !important; rotate: none !important; scale: none !important; }} }}'; \
         document.head.append(off); }})()"
    ))
    .await
    .map_err(protocol)?;
    let measured: Result<Vec<Raw>, ReadError> = async {
        let mut nodes = Vec::new();
        for node in &tree.result.nodes {
            let mut raw = raw(node);
            if !raw.ignored && !STRUCTURE.contains(&raw.chrome_role.as_str()) {
                match measure(page, node, &clipping_argument).await? {
                    Ok(Measurement::Clipped) => raw.ignored = true,
                    Ok(Measurement::Visible(geometry)) => {
                        raw.bounds = Some(relative_to(geometry.bounds, (frame.x, frame.y)));
                        raw.text = match (geometry.size, geometry.weight, geometry.color) {
                            (Some(size), Some(weight), Some(color)) => {
                                srgb(&color).map(|color| TextStyle {
                                    size: Some(size),
                                    weight: Some(weight),
                                    color: Some(color),
                                })
                            }
                            _ => None,
                        };
                        raw.placeholder = geometry.placeholder;
                        raw.component = geometry.component;
                        if (geometry.current || geometry.in_selected) && raw.selected.is_none() {
                            raw.selected = Some(true);
                        }
                        // Chrome gives an empty field its placeholder as its
                        // accessibility value; the DOM holds what is typed.
                        if matches!(raw.chrome_role.as_str(), "textbox" | "searchbox") {
                            if let Some(value) = geometry.field_value {
                                raw.value = Some(value);
                            }
                            raw.secret = geometry.secret;
                        }
                    }
                    Err(why) => raw.no_box = Some(why),
                }
            }
            nodes.push(raw);
        }
        Ok(nodes)
    }
    .await;
    page.evaluate(format!(
        "document.getElementById('{UNTRANSFORMED}').remove()"
    ))
    .await
    .map_err(protocol)?;
    measured
}

/// Whether the DOM node <node> describes is clipped, its box and style if it
/// is visible, or why Chrome gave it no box.
async fn measure(
    page: &Page,
    node: &AxNode,
    clipping: &CallArgument,
) -> Result<Result<Measurement, &'static str>, ReadError> {
    let Some(backend) = node.backend_dom_node_id else {
        return Ok(Err("the accessibility node describes no DOM node"));
    };
    let resolved = page
        .execute(
            ResolveNodeParams::builder()
                .backend_node_id(backend)
                .build(),
        )
        .await
        .map_err(protocol)?;
    let Some(object_id) = resolved.result.object.object_id else {
        return Ok(Err("Chrome resolved no object for its DOM node"));
    };
    let mut call = CallFunctionOnParams::new(GEOMETRY);
    call.object_id = Some(object_id);
    call.arguments = Some(vec![clipping.clone()]);
    call.return_by_value = Some(true);
    let called = page.execute(call).await.map_err(protocol)?;
    if let Some(thrown) = &called.result.exception_details {
        let detail = thrown
            .exception
            .as_ref()
            .and_then(|e| e.description.clone())
            .unwrap_or_default();
        return Err(ReadError::Protocol(format!(
            "measuring a board node threw: {} {detail}",
            thrown.text
        )));
    }
    match called.result.result.value {
        Some(value) if !value.is_null() => Ok(Ok(serde_json::from_value(value).map_err(protocol)?)),
        _ => Ok(Err("its DOM node has no box to measure")),
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
