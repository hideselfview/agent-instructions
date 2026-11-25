//! The design-system components a drawn page holds: each fiber of React's
//! tree whose type is one of the design system's components (the watch
//! script's `components()`, board.rs), with its name, the props it was drawn
//! with that can be part of a state (states.rs says which are), and which of
//! its string props it draws as words.

use crate::render::board;
use crate::render::chrome::DrawPage;
use serde::Deserialize;
use std::collections::BTreeMap;

/// One component drawn on a page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Instance {
    /// The component's name in the design system, such as `MacButton`.
    pub(crate) component: String,
    /// The props whose value is a boolean, a finite number or a string. A
    /// prop that holds `null` or `undefined` is unset and left out, and so
    /// are `children` and every prop that holds a function, a React element,
    /// an array or an object.
    pub(crate) props: BTreeMap<String, serde_json::Value>,
    /// The string props whose value the instance draws as words: the whole
    /// text of an element it draws or of one of that element's text nodes,
    /// or an element's accessible name, title, placeholder, alt text or
    /// value.
    pub(crate) words: Vec<String>,
    /// On a preview page, whether the instance draws inside one of the
    /// page's rows; on a preview page with no rows, which draws one state,
    /// every instance does. False on a board.
    pub(crate) in_row: bool,
    /// Whether the instance draws a node: an element, or a text node that a
    /// component returns as a bare string. A component can draw nothing, as
    /// one that returns null for a state it has no picture for.
    pub(crate) draws: bool,
    /// Whether the page sets the instance's props: no component draws it, or
    /// the outermost component that draws it received it from the page, as
    /// a child or in a prop, possibly copied with props of its own added (then
    /// `props` holds only the page's). False for a component that another component draws
    /// with props of its own, which are the design system's code.
    pub(crate) by_page: bool,
}

/// The script that reads the instances. It walks each React root of the
/// page from its current host fiber, child before sibling, so the instances
/// come in the order React draws them. <rows> is the selector of a preview's
/// rows, `null` on a board.
fn script(rows: Option<&str>) -> String {
    let rows = match rows {
        Some(selector) => serde_json::to_string(selector).expect("a string serializes"),
        None => "null".to_string(),
    };
    format!(
        r#"(() => {{
  const known = window.__buildsShots.components();
  const rowSelector = {rows};
  const pageHasRows = rowSelector !== null && document.querySelector(rowSelector) !== null;
  // The host nodes a fiber draws, elements and text nodes: the first node
  // down each branch, in document order.
  const nodesOf = (fiber) => {{
    const out = [];
    const pending = fiber.child ? [fiber.child] : [];
    while (pending.length) {{
      const next = pending.pop();
      if (next.sibling) pending.push(next.sibling);
      if (next.stateNode instanceof Element || next.stateNode instanceof Text) out.push(next.stateNode);
      else if (next.child) pending.push(next.child);
    }}
    return out;
  }};
  const inRow = (nodes) => {{
    if (rowSelector === null) return false;
    if (!pageHasRows) return true;
    if (nodes.length === 0) return false;
    const first = nodes[0] instanceof Element ? nodes[0] : nodes[0].parentElement;
    return first !== null && first.closest(rowSelector) !== null;
  }};
  // The props of the React element of <type> that <value>, a prop's value,
  // holds and that <actual>, the props a fiber drew with, contain: the
  // element itself, or one inside an array, an object or another element's
  // props. A component that copies the element with props of its own added
  // (cloneElement) leaves the page's props among <actual>; null when none.
  const held = (value, type, actual, seen) => {{
    if (value === null || typeof value !== 'object' || seen.has(value)) return null;
    seen.add(value);
    if (value.$$typeof !== undefined && value.type === type && value.props
        && Object.keys(value.props).every((key) => Object.is(value.props[key], actual[key]))) {{
      return value.props;
    }}
    const inside = value.$$typeof !== undefined ? [value.props] : Object.values(value);
    for (const inner of inside) {{
      const found = held(inner, type, actual, seen);
      if (found !== null) return found;
    }}
    return null;
  }};
  const plain = (text) => text.replace(/\s+/g, ' ').trim();
  const ATTRIBUTES = ['aria-label', 'title', 'placeholder', 'alt', 'value'];
  // The words the elements show: each element's whole text and each of its
  // text nodes', and the attributes that name or fill an element.
  const wordsOf = (nodes) => {{
    const words = new Set();
    for (const top of nodes) {{
      if (top instanceof Text) {{
        words.add(plain(top.textContent));
        continue;
      }}
      for (const element of [top, ...top.querySelectorAll('*')]) {{
        words.add(plain(element.textContent || ''));
        for (const name of ATTRIBUTES) {{
          const value = element.getAttribute(name);
          if (value) words.add(plain(value));
        }}
        if (typeof element.value === 'string') words.add(plain(element.value));
        for (const child of element.childNodes) {{
          if (child.nodeType === 3) words.add(plain(child.textContent));
        }}
      }}
    }}
    return words;
  }};
  const kept = (props) => {{
    const out = {{}};
    for (const [key, value] of Object.entries(props || {{}})) {{
      if (key === 'children') continue;
      const type = typeof value;
      if (type === 'boolean' || type === 'string' || (type === 'number' && isFinite(value))) {{
        out[key] = value;
      }}
    }}
    return out;
  }};
  const found = [];
  for (const node of document.querySelectorAll('*')) {{
    const key = Object.keys(node).find((k) => k.startsWith('__reactContainer$'));
    if (!key || !node[key] || !node[key].stateNode) continue;
    // Each fiber with the fibers of the components that draw it, nearest
    // first.
    const pending = [[node[key].stateNode.current, []]];
    while (pending.length) {{
      const [fiber, drawers] = pending.pop();
      const component = known.get(fiber.elementType) || known.get(fiber.type);
      if (component) {{
        // The page hands its elements to the outermost component that
        // holds them; elements a component makes for itself are in no
        // props of that component. Props the component added to a copy
        // of the page's element are its own, not the page's.
        const outermost = drawers[drawers.length - 1];
        const actual = fiber.memoizedProps || {{}};
        let given = null;
        if (outermost !== undefined) {{
          for (const v of Object.values(outermost.memoizedProps || {{}})) {{
            given = held(v, fiber.type, actual, new Set());
            if (given !== null) break;
          }}
        }}
        const by_page = outermost === undefined || given !== null;
        const props = kept(given === null ? actual : given);
        const nodes = nodesOf(fiber);
        const strings = Object.entries(props).filter(([, v]) => typeof v === 'string' && plain(v));
        const shown = strings.length ? wordsOf(nodes) : new Set();
        const words = strings.filter(([, v]) => shown.has(plain(v))).map(([k]) => k);
        found.push({{ component, props, words, in_row: inRow(nodes), draws: nodes.length > 0, by_page }});
      }}
      const below = component ? [fiber, ...drawers] : drawers;
      if (fiber.sibling) pending.push([fiber.sibling, drawers]);
      if (fiber.child) pending.push([fiber.child, below]);
    }}
  }}
  return found;
}})()"#
    )
}

/// The design-system components the drawn page holds, in the order React
/// draws them. <rows> is the selector of a preview page's rows, None on a
/// board.
pub(crate) async fn instances(
    page: &DrawPage,
    rows: Option<&str>,
) -> Result<Vec<Instance>, board::BoardError> {
    page.evaluate(script(rows))
        .await
        .map_err(board::protocol)?
        .into_value()
        .map_err(board::protocol)
}

#[cfg(test)]
#[path = "instances_tests.rs"]
mod tests;
