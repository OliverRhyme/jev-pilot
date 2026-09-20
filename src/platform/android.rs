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
    Operation::Submit,
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

        // The soft keyboard is a window of its own, and its keys are not rows
        // of the screen: they are the rendering of a field the run already
        // decided to type into, and typing goes through the field. Offered as
        // targets they drown everything — 52 of 54 nodes in one measured case
        // — and flatten the judgement across them.
        let elements = document
            .descendants()
            .filter(|node| !within_input_method(node))
            .filter(|node| is_actionable(node))
            .filter_map(|node| {
                let editable = is_editable(&node);
                let mut texts = own_texts(&node).into_iter();
                // An empty field has no text of its own, and is exactly the
                // row a form needs acted on. It is named by its hint, or
                // failing that for what it is, rather than thrown away for
                // having nothing to say. A node that is neither nameable nor
                // typeable is noise and still goes.
                let label = match texts.next() {
                    Some(label) => label,
                    None if editable => hint_of(&node).unwrap_or_else(|| Box::from("text field")),
                    None => return None,
                };
                Some(Element {
                    label,
                    detail: texts.next(),
                    editable,
                    bounds: parse_bounds(node.attribute("bounds")?)?,
                })
            })
            .collect();

        Snapshot::new(elements)
            .map(|snapshot| snapshot.with_keyboard_open(shows_input_method(&document)))
            .map(|snapshot| snapshot.in_app(app_of(&document)))
            .map(|snapshot| snapshot.saying(notices_of(&document)))
            .map_err(HierarchyError::from)
    }
}

/// Whether this node belongs to the soft keyboard's window.
///
/// Only the window's root carries `window-type`, so the question is really
/// about ancestry. `uiautomator` dumps carry no window metadata and contain
/// only the active window, so the keyboard does not arise there.
fn within_input_method(node: &roxmltree::Node) -> bool {
    node.ancestors()
        .any(|ancestor| is_input_method_window(&ancestor))
}

/// Whether a screen has the soft keyboard up.
fn shows_input_method(document: &roxmltree::Document) -> bool {
    document
        .descendants()
        .any(|node| is_input_method_window(&node))
}

fn is_input_method_window(node: &roxmltree::Node) -> bool {
    node.attribute("window-type") == Some("input_method")
}

/// Which application this screen belongs to.
///
/// A helper dump labels its windows, and exactly one of them is the
/// application — so the answer is read rather than guessed, and neither the
/// status bar drawn over every screen nor the keyboard drawn over half of them
/// can claim it.
///
/// `uiautomator` dumps carry no window metadata and hold only the active
/// window, so there the answer is whichever package owns most of the tree,
/// with system chrome set aside for the same reason.
fn app_of(document: &roxmltree::Document) -> Option<Box<str>> {
    if let Some(package) = document
        .descendants()
        .find(|node| node.attribute("window-type") == Some("application"))
        .and_then(|node| node.attribute("package"))
    {
        return Some(package.into());
    }

    let mut tally: Vec<(&str, usize)> = Vec::new();
    for package in document
        .descendants()
        .filter_map(|node| node.attribute("package"))
        .filter(|package| *package != SYSTEM_CHROME)
    {
        match tally.iter_mut().find(|(seen, _)| *seen == package) {
            Some((_, count)) => *count += 1,
            None => tally.push((package, 1)),
        }
    }
    tally
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(package, _)| package.into())
}

/// The status bar, navigation bar and notification shade, which are drawn over
/// every screen and belong to none of them.
const SYSTEM_CHROME: &str = "com.android.systemui";

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

/// Whether text can be typed into this node.
///
/// The `editable` trait is the node's own answer and is what the helper
/// reports, whatever toolkit drew the screen — Flutter builds its
/// accessibility tree itself and does not use Android's widgets. `uiautomator`
/// dumps carry no such attribute, so there the class name is still the only
/// signal there is.
fn is_editable(node: &roxmltree::Node) -> bool {
    match node.attribute("editable") {
        Some(trait_) => trait_ == "true",
        None => node
            .attribute("class")
            .is_some_and(|class| class.contains("EditText")),
    }
}

/// What an empty field is called, as opposed to what it holds.
///
/// A field showing its hint contains nothing, so this is deliberately not part
/// of [`label_of`]: treating a hint as content would tell a run that a form was
/// already filled.
fn hint_of(node: &roxmltree::Node) -> Option<Box<str>> {
    node.attribute("hint")
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(Box::from)
}

/// Every label under `node` that no nested actionable row has already claimed.
///
/// A Settings row is a clickable container holding a title and a subtitle in
/// child views, with no text of its own. Labelling the leaf text instead would
/// produce elements that read well and cannot be tapped. Stopping the descent
/// at a nested actionable node keeps each row's text with that row, so a list
/// item does not absorb the label of a button sitting inside it.
/// What the screen says that is not the name of something to act on.
///
/// Headings, captions, totals, the count of digits entered so far. A node is a
/// notice when nothing about it or above it is actionable, because the text
/// under an actionable node is already that row's label and repeating it would
/// say everything twice.
///
/// The keyboard's own window is left out for the reason its keys are: it is the
/// rendering of a field, not something the screen is saying.
fn notices_of(document: &roxmltree::Document) -> Vec<Box<str>> {
    let mut notices: Vec<Box<str>> = Vec::new();
    for node in document.descendants() {
        if notices.len() >= MAX_NOTICES {
            break;
        }
        if within_input_method(&node) || node.ancestors().any(|a| is_actionable(&a)) {
            continue;
        }
        let Some(label) = label_of(&node) else {
            continue;
        };
        // The same caption often appears on a node and on the wrapper drawn
        // around it. Said once is what a person reads.
        if !notices.contains(&label) {
            notices.push(label);
        }
    }
    notices
}

/// How many notices travel with a screen.
///
/// A bound rather than a judgement about what matters: the state is sent to a
/// model on every step, and a dense screen would otherwise crowd out the rows,
/// which is the mistake the soft keyboard already taught.
const MAX_NOTICES: usize = 24;

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
