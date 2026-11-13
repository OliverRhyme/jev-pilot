//! The input a command sends, and the page scripts it runs. Pure.
//!
//! Coordinates are CSS pixels relative to the main frame's viewport, which is
//! what `Input.dispatchMouseEvent` takes.

use crate::act::Direction;
use crate::platform::page::Viewport;
use crate::snapshot::Point;
use serde_json::{Value, json};

/// How far one scroll goes, as a share of the viewport's height: most of a
/// view, so the next read overlaps the last one by a few rows.
pub const SCROLL_SHARE: f64 = 0.8;

/// Whether a point lies outside the viewport, as an off-screen row's does.
#[must_use]
pub fn off_screen(at: Point, viewport: Viewport) -> bool {
    let (x, y) = (f64::from(at.x), f64::from(at.y));
    x < 0.0 || y < 0.0 || x >= viewport.width || y >= viewport.height
}

/// The middle of a node's content box, from a `DOM.getBoxModel` reply.
#[must_use]
pub fn centre(box_model: &Value) -> Option<(f64, f64)> {
    let quad: Vec<f64> = box_model
        .pointer("/model/content")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    if quad.len() < 8 {
        return None;
    }
    let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
    let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
    Some((x, y))
}

/// The three mouse events of one left click: move there, press, release.
///
/// Moving first lets hover-only menus and listeners see the pointer arrive,
/// as they would for a person.
#[must_use]
pub fn click(x: f64, y: f64) -> [Value; 3] {
    [
        json!({"type": "mouseMoved", "x": x, "y": y}),
        json!({"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1}),
        json!({"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1}),
    ]
}

/// Select everything in the focused field.
///
/// Through the editing command, never a ⌘A or Ctrl+A chord: which modifier
/// selects all depends on the platform, and on macOS a synthetic ⌘A does not.
#[must_use]
pub fn select_all() -> [Value; 2] {
    [
        json!({"type": "rawKeyDown", "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "commands": ["selectAll"]}),
        json!({"type": "keyUp", "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65}),
    ]
}

/// Press and release Enter, which submits the focused form.
#[must_use]
pub fn enter() -> [Value; 2] {
    [
        json!({"type": "keyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": "\r"}),
        json!({"type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13}),
    ]
}

/// A mouse wheel at the viewport's centre, of [`SCROLL_SHARE`] of its height.
#[must_use]
pub fn wheel(viewport: Viewport, direction: Direction) -> Value {
    let delta = (viewport.height * SCROLL_SHARE).round();
    let delta = match direction {
        Direction::Down => delta,
        Direction::Up => -delta,
    };
    json!({
        "type": "mouseWheel",
        "x": (viewport.width / 2.0).round(),
        "y": (viewport.height / 2.0).round(),
        "deltaX": 0,
        "deltaY": delta,
    })
}

/// Run on the row's node with the hit node as its argument: whether the hit
/// node is the row or inside it, and if not, a few words on what it is.
pub const CONTAINS: &str = r"function (hit) {
  const inside = this === hit || (typeof this.contains === 'function' && this.contains(hit));
  const el = hit && (hit.nodeType === 1 ? hit : hit.parentElement);
  if (!el) return { inside, what: 'nothing' };
  const words = (el.getAttribute('aria-label') || el.innerText || el.textContent || '')
    .trim().replace(/\s+/g, ' ').slice(0, 60);
  const tag = el.tagName.toLowerCase();
  return { inside, what: words ? tag + ' “' + words + '”' : tag };
}";

/// Run on a native drop-down with the words as its argument: choose the one
/// option whose label is the words exactly, or else the one that contains
/// them ignoring case, and fire `input` and `change` as a person's choice
/// would. With no single match it changes nothing and lists the options.
pub const SELECT_OPTION: &str = r"function (words) {
  const options = Array.from(this.options);
  const labels = options.map(o => (o.label || o.textContent || '').trim());
  let found = labels.map((l, i) => l === words ? i : -1).filter(i => i >= 0);
  if (found.length !== 1) {
    const w = words.toLowerCase();
    found = labels.map((l, i) => l.toLowerCase().includes(w) ? i : -1).filter(i => i >= 0);
  }
  if (found.length !== 1) return { set: false, options: labels };
  this.selectedIndex = found[0];
  this.dispatchEvent(new Event('input', { bubbles: true }));
  this.dispatchEvent(new Event('change', { bubbles: true }));
  return { set: true, chosen: labels[found[0]], options: labels };
}";

/// Run on a field after typing: what it now holds, and whether it is a
/// password field whose value must not be repeated.
pub const READ_VALUE: &str = r"function () {
  const password = this.type === 'password';
  const value = 'value' in this ? String(this.value) : (this.textContent || '');
  return { password, value: password ? '' : value };
}";

/// The page-script watch for DOM changes: one `MutationObserver` per
/// document, installed in an isolated world so the page's own scripts cannot
/// see or disturb it.
const WATCH: &str = r"(globalThis.__jevPilotWatch ??= (() => {
  const state = { last: performance.now(), count: 0 };
  new MutationObserver(() => { state.last = performance.now(); state.count += 1; })
    .observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  return state;
})())";

/// How long the page has gone without a DOM change, and how many changes the
/// watch has seen, now.
#[must_use]
pub fn quiet_now() -> String {
    format!(
        "(() => {{ const w = {WATCH}; return {{ quiet: Math.round(performance.now() - w.last), mutations: w.count }}; }})()"
    )
}

/// Resolve once the page has gone `window_ms` without a DOM change, or after
/// `cap_ms` whatever it is doing, with the same answer as [`quiet_now`].
#[must_use]
pub fn quiet_within(window_ms: u64, cap_ms: u64) -> String {
    format!(
        "new Promise(resolve => {{ const w = {WATCH}; const start = performance.now(); \
         const tick = () => {{ const now = performance.now(); const quiet = now - w.last; \
         if (quiet >= {window_ms} || now - start >= {cap_ms}) resolve({{ quiet: Math.round(quiet), mutations: w.count }}); \
         else setTimeout(tick, Math.max(10, {window_ms} - quiet)); }}; tick(); }})"
    )
}

/// The outcome line for a drop-down choice, from [`SELECT_OPTION`]'s answer.
#[must_use]
pub fn select_outcome(answer: &Value, words: &str) -> Box<str> {
    if answer.get("set").and_then(Value::as_bool) == Some(true) {
        let chosen = answer
            .get("chosen")
            .and_then(Value::as_str)
            .unwrap_or(words);
        return format!("the field now holds \u{201c}{chosen}\u{201d}").into();
    }
    let options: Vec<&str> = answer
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    format!(
        "nothing was chosen: no single option matches \u{201c}{words}\u{201d}; the options are {}",
        options.join(", ")
    )
    .into()
}
