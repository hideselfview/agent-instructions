//! Reading a rendered board the way the shots' platforms read theirs: Chrome's
//! accessibility tree becomes an element list in reading order, in the shape
//! the shot harnesses write (`role`, `native`, `label`, `value`, `bounds`,
//! `enabled`, `selected`, `text`, `parent`, `focused`, `placeholder`).
//!
//! The tree is read in two parts. [`build`] turns raw nodes into elements and
//! needs no browser: roles, words, folding a control's text into its name,
//! skipping ignored nodes. [`elements`] gathers the raw nodes from a page:
//! the accessibility tree, and for each node its box, from its DOM node (a text
//! node's box is the range of its words, not its parent's), and for text its
//! computed font.

use crate::names::Platform;
use crate::shots::{Bounds, Element, Role, TextStyle, words};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::accessibility::{
    AxNode, AxNodeId, AxPropertyName, EnableParams, GetFullAxTreeParams,
};
use chromiumoxide::cdp::browser_protocol::dom::ResolveNodeParams;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("Chrome failed: {0}")]
    Protocol(String),
    #[error("the accessibility tree has no node {0}")]
    NoNode(String),
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

/// Chrome roles that only hold other nodes and say nothing of their own.
const STRUCTURE: [&str; 6] = [
    "generic",
    "none",
    "presentation",
    "RootWebArea",
    "InlineTextBox",
    "LineBreak",
];

/// What the browser holds about one node of the accessibility tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Raw {
    pub(crate) id: String,
    /// The DOM node the accessibility node describes.
    pub(crate) backend: Option<i64>,
    pub(crate) children: Vec<String>,
    pub(crate) ignored: bool,
    pub(crate) chrome_role: String,
    pub(crate) name: Option<String>,
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
}

fn nonempty(text: Option<&str>) -> Option<String> {
    text.map(words).filter(|w| !w.is_empty())
}

/// The elements of `raw`, in reading order (the tree's order from its first
/// node). Ignored nodes (hidden, `aria-hidden`, presentational) are left out,
/// their visible children kept; a text inside a named control is the
/// control's name, not an element of its own.
pub(crate) fn build(raw: &[Raw], platform: Platform) -> Result<Vec<Element>, ReadError> {
    let by_id: HashMap<&str, &Raw> = raw.iter().map(|n| (n.id.as_str(), n)).collect();
    let Some(first) = raw.first() else {
        return Ok(Vec::new());
    };
    let mut elements = Vec::new();
    // (node, index of the nearest kept ancestor, a named control above it)
    let mut pending = vec![(first, None, false)];
    while let Some((node, parent, folded)) = pending.pop() {
        let role = Role::for_node(node, platform);
        let label = nonempty(node.name.as_deref()).filter(|_| role != Role::Other);
        let kept = !node.ignored && !STRUCTURE.contains(&node.chrome_role.as_str());
        let inside_control = folded;
        let mut next_parent = parent;
        let mut next_folded = folded;
        if kept && !(role == Role::Text && inside_control) {
            let Some(bounds) = node.bounds else {
                return Err(ReadError::NoBox {
                    role: node.chrome_role.clone(),
                    name: node.name.clone().unwrap_or_default(),
                    why: node.no_box.unwrap_or("Chrome gave it none"),
                });
            };
            next_parent = Some(elements.len());
            elements.push(element(node, role, label.clone(), bounds, parent));
        }
        if kept && role.is_control() && label.is_some() {
            next_folded = true;
        }
        for child in node.children.iter().rev() {
            let child = by_id
                .get(child.as_str())
                .ok_or_else(|| ReadError::NoNode(child.clone()))?;
            pending.push((child, next_parent, next_folded));
        }
    }
    Ok(elements)
}

fn element(
    node: &Raw,
    role: Role,
    label: Option<String>,
    bounds: Bounds,
    parent: Option<usize>,
) -> Element {
    let value = match role {
        Role::Toggle => node.checked.as_deref().map(|c| match c {
            "true" => "on".to_string(),
            "false" => "off".to_string(),
            other => other.to_string(),
        }),
        Role::Field => Some(node.value.clone().unwrap_or_default()),
        Role::Popup | Role::Slider | Role::Progress => node.value.clone(),
        _ => None,
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

/// What a DOM node reports about its box and its font.
#[derive(Debug, Deserialize)]
struct Geometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
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
    /// The design-system component that draws the node: the nearest one
    /// above it in React's tree.
    component: Option<String>,
}

const GEOMETRY: &str = r#"function() {
  // The design system's components by their values: each function or
  // memo/forwardRef object that a global the page's scripts added holds
  // (`Bae.MacButton` is MacButton). React's globals, and the double-underscore
  // ones of the runtime and of this program, hold none.
  const components = () => {
    const shots = window.__buildsShots;
    if (shots.components) return shots.components;
    const found = new Map();
    for (const name of Object.getOwnPropertyNames(window)) {
      if (shots.globals.has(name) || name.startsWith('__')
        || name === 'React' || name === 'ReactDOM') continue;
      let holder;
      try { holder = window[name]; } catch (e) { continue; }
      if (!holder || typeof holder !== 'object') continue;
      for (const [key, value] of Object.entries(holder)) {
        const component = typeof value === 'function'
          || (value && typeof value === 'object' && value.$$typeof
            && ('render' in value || 'type' in value));
        if (component && !found.has(value)) found.set(value, key);
      }
    }
    shots.components = found;
    return found;
  };
  // The nearest component above a DOM node in React's tree: the first fiber
  // up from the node's own (or its nearest ancestor's) whose type is one.
  const componentOf = (node) => {
    for (let dom = node; dom; dom = dom.parentNode) {
      const key = Object.keys(dom).find((k) => k.startsWith('__reactFiber$'));
      if (!key) continue;
      const known = components();
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
  const element = isText ? this.parentElement : this;
  const style = element ? getComputedStyle(element) : null;
  return {
    x: rect.x, y: rect.y, width: rect.width, height: rect.height,
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
    component: componentOf(this),
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

fn string(
    value: &Option<chromiumoxide::cdp::browser_protocol::accessibility::AxValue>,
) -> Option<String> {
    match value.as_ref()?.value.as_ref()? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
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
        name: string(&node.name),
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

/// The page's elements in reading order.
pub(crate) async fn elements(page: &Page, platform: Platform) -> Result<Vec<Element>, ReadError> {
    build(&gather(page).await?, platform)
}

/// The boxes of the parts of a board that the app draws no counterpart for,
/// such as an on-screen keyboard: the elements each of <selectors> (the
/// app's `shots-unshot` line) matches. Such a part is hidden from the
/// accessibility tree, so the picture comparison leaves its area out.
pub(crate) async fn unshot(page: &Page, selectors: &[String]) -> Result<Vec<Bounds>, ReadError> {
    if selectors.is_empty() {
        return Ok(Vec::new());
    }
    let selector = serde_json::to_string(&selectors.join(", ")).expect("a string serializes");
    let boxes: Vec<Bounds> = page
        .evaluate(format!(
            "[...document.querySelectorAll({selector})].map(e => {{ \
               const r = e.getBoundingClientRect(); \
               return {{x: r.x, y: r.y, width: r.width, height: r.height}}; }})"
        ))
        .await
        .map_err(protocol)?
        .into_value()
        .map_err(protocol)?;
    Ok(boxes)
}

/// The elements under the DOM node `backend`, in reading order, with boxes
/// measured from `origin`, the node's top-left corner.
pub(crate) async fn elements_in(
    page: &Page,
    backend: i64,
    origin: (f64, f64),
    platform: Platform,
) -> Result<Vec<Element>, ReadError> {
    let raw = gather(page).await?;
    let mut inside = subtree(&raw, backend)?;
    for node in &mut inside {
        if let Some(bounds) = &mut node.bounds {
            bounds.x -= origin.0;
            bounds.y -= origin.1;
        }
    }
    build(&inside, platform)
}

/// The node describing DOM node `backend` and everything under it, the node
/// first.
fn subtree(raw: &[Raw], backend: i64) -> Result<Vec<Raw>, ReadError> {
    let by_id: HashMap<&str, &Raw> = raw.iter().map(|n| (n.id.as_str(), n)).collect();
    let root = raw
        .iter()
        .find(|n| n.backend == Some(backend))
        .ok_or_else(|| ReadError::NoNode(format!("DOM node {backend}")))?;
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        found.push(node.clone());
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

async fn gather(page: &Page) -> Result<Vec<Raw>, ReadError> {
    page.execute(EnableParams::default())
        .await
        .map_err(protocol)?;
    let tree = page
        .execute(GetFullAxTreeParams::default())
        .await
        .map_err(protocol)?;
    let mut nodes = Vec::new();
    for node in &tree.result.nodes {
        let mut raw = raw(node);
        if !raw.ignored && !STRUCTURE.contains(&raw.chrome_role.as_str()) {
            match measure(page, node).await? {
                Ok(geometry) => {
                    raw.bounds = Some(Bounds {
                        x: geometry.x,
                        y: geometry.y,
                        width: geometry.width,
                        height: geometry.height,
                    });
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
                    if geometry.current && raw.selected.is_none() {
                        raw.selected = Some(true);
                    }
                    // Chrome gives an empty field its placeholder as its
                    // accessibility value; the DOM holds what is typed.
                    if matches!(raw.chrome_role.as_str(), "textbox" | "searchbox")
                        && let Some(value) = geometry.field_value
                    {
                        raw.value = Some(value);
                    }
                }
                Err(why) => raw.no_box = Some(why),
            }
        }
        nodes.push(raw);
    }
    Ok(nodes)
}

/// The box and style of the DOM node <node> describes, or why Chrome gave
/// none.
async fn measure(page: &Page, node: &AxNode) -> Result<Result<Geometry, &'static str>, ReadError> {
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
    call.return_by_value = Some(true);
    let called = page.execute(call).await.map_err(protocol)?;
    match called.result.result.value {
        Some(value) if !value.is_null() => Ok(Ok(serde_json::from_value(value).map_err(protocol)?)),
        _ => Ok(Err("its DOM node has no box to measure")),
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
