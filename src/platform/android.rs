//! Android, read through a UIAutomator hierarchy dump.
//!
//! Verified against a dump taken from a physical Pixel 8 Pro; see
//! `tests/fixtures/settings.xml`.

use super::{HierarchyError, Platform};
use crate::act::Operation;
use crate::snapshot::{Bounds, Element, Snapshot};

/// Android, driven through `uiautomator dump` and `adb shell input`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Android;

/// Android exposes all three navigation gestures as system-level actions,
/// whether the device renders them as buttons or as gesture areas.
const OPERATIONS: &[Operation] = &[
    Operation::Tap,
    Operation::DoubleTap,
    Operation::LongPress,
    Operation::SwipeLeft,
    Operation::SwipeRight,
    Operation::ScrollUp,
    Operation::ScrollDown,
    Operation::Back,
    Operation::Home,
    Operation::AppSwitcher,
    Operation::Wait,
    Operation::Done,
    Operation::Blocked,
];

impl Platform for Android {
    fn name(&self) -> &'static str {
        "Android"
    }

    fn operations(&self) -> &'static [Operation] {
        OPERATIONS
    }

    fn parse_hierarchy(&self, raw: &str) -> Result<Snapshot, HierarchyError> {
        let document =
            roxmltree::Document::parse(raw).map_err(|error| HierarchyError::Malformed {
                detail: error.to_string().into_boxed_str(),
            })?;

        let elements = document
            .descendants()
            .filter(is_actionable)
            .filter_map(|node| {
                let mut texts = own_texts(&node).into_iter();
                Some(Element {
                    label: texts.next()?,
                    detail: texts.next(),
                    editable: is_editable(&node),
                    bounds: parse_bounds(node.attribute("bounds")?)?,
                })
            })
            .collect();

        Snapshot::new(elements).map_err(HierarchyError::from)
    }
}

/// A row a person could act on.
///
/// Three flags each map to a distinct gesture, and a control may carry only
/// one: a shortcut target is `long-clickable` without being `clickable`, and a
/// custom toggle can be `checkable` alone. Testing `clickable` by itself drops
/// them from the catalog, which reads to the model as the screen not offering
/// them at all.
///
/// `enabled` is checked separately because it is orthogonal: a greyed-out
/// button keeps `clickable="true"` — the flag says the view handles taps, not
/// that it will act on one. Offering it hands the model a choice that silently
/// does nothing, which a loop then mistakes for a tap that worked.
fn is_actionable(node: &roxmltree::Node) -> bool {
    if !node.has_tag_name("node") || node.attribute("enabled") == Some("false") {
        return false;
    }
    ["clickable", "long-clickable", "checkable"]
        .iter()
        .any(|flag| node.attribute(*flag) == Some("true"))
        || is_editable(node)
}

fn is_editable(node: &roxmltree::Node) -> bool {
    node.attribute("class")
        .is_some_and(|class| class.contains("EditText"))
}

/// Every label under `node` that no nested actionable row has already claimed.
///
/// A Settings row is a clickable container holding a title and a subtitle in
/// child views, with no text of its own. Labelling the leaf text instead would
/// produce elements that read well and cannot be tapped. Stopping the descent
/// at a nested actionable node keeps each row's text with that row, so a list
/// item does not absorb the label of a button sitting inside it.
fn own_texts(node: &roxmltree::Node) -> Vec<Box<str>> {
    let mut texts = Vec::new();
    collect_texts(node, &mut texts, true);
    texts
}

fn collect_texts(node: &roxmltree::Node, out: &mut Vec<Box<str>>, is_root: bool) {
    if !is_root && is_actionable(node) {
        return;
    }
    if let Some(label) = label_of(node) {
        out.push(label);
    }
    for child in node.children() {
        collect_texts(&child, out, false);
    }
}

/// An element speaks through its text, or failing that its accessibility label.
fn label_of(node: &roxmltree::Node) -> Option<Box<str>> {
    ["text", "content-desc"]
        .iter()
        .filter_map(|attr| node.attribute(*attr))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(Box::from)
}

/// UIAutomator writes bounds as `[left,top][right,bottom]`.
fn parse_bounds(raw: &str) -> Option<Bounds> {
    let numbers: Vec<i32> = raw
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match numbers[..] {
        [left, top, right, bottom] => Some(Bounds {
            left,
            top,
            right,
            bottom,
        }),
        _ => None,
    }
}
