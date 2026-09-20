//! iOS, read through an XCUITest element hierarchy.
//!
//! # Status
//!
//! **Unverified against a physical device.** The Android reader was built from
//! a dump taken off a real handset, and doing so immediately exposed a
//! structural assumption that was wrong. This reader has had no such contact
//! with reality: it is written to the documented XCUITest attribute shape that
//! XCUITest and WebDriverAgent emit, and the fixture backing its tests was
//! written by hand. Treat it as a starting point to be corrected against a real
//! `source` dump, not as a finished counterpart to [`super::Android`].

use super::{HierarchyError, Platform};
use crate::act::Operation;
use crate::snapshot::{Bounds, Element, Snapshot};

/// iOS, driven through XCUITest and a WebDriverAgent-style session.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ios;

/// iOS has no system Back: returning is a per-screen affordance, either a
/// navigation-bar button (an ordinary element) or an edge swipe. Offering a
/// system Back here would invite a choice the device cannot carry out.
const OPERATIONS: &[Operation] = &[
    Operation::Tap,
    Operation::DoubleTap,
    Operation::LongPress,
    Operation::SwipeLeft,
    Operation::SwipeRight,
    Operation::Peek,
    Operation::ScrollUp,
    Operation::ScrollDown,
    Operation::Home,
    Operation::AppSwitcher,
    Operation::Submit,
    Operation::Wait,
    Operation::Done,
    Operation::Blocked,
];

/// Element types a person can act on directly.
///
/// Mirrors the interactive half of `XCUIElementType`. Notably absent:
/// `StaticText`, which is what a `UILabel` becomes. It carries
/// `isUserInteractionEnabled = false` by default and no trait implying
/// interactivity, so it is a text node rather than a target — and treating it
/// as one stops a cell from absorbing the title and subtitle it contains.
/// Container types (`Other`, `Group`, `Table`, `ScrollView`, `TabBar`,
/// `NavigationBar`, `Image`) are absent for the same reason: their children are
/// the targets, not them.
const HITTABLE: &[&str] = &[
    "Button",
    "Cell",
    "CheckBox",
    "ColorWell",
    "ComboBox",
    "DatePicker",
    "DecrementArrow",
    "DisclosureTriangle",
    "IncrementArrow",
    "Key",
    "Link",
    "MenuBarItem",
    "MenuButton",
    "MenuItem",
    "PageIndicator",
    "Picker",
    "PickerWheel",
    "PopUpButton",
    "RadioButton",
    "SearchField",
    "SecureTextField",
    "SegmentedControl",
    "Slider",
    "Stepper",
    "Switch",
    "Tab",
    "TextField",
    "TextView",
    "Toggle",
    "ToolbarButton",
];

/// Element types that accept typed text.
const EDITABLE: &[&str] = &["SearchField", "SecureTextField", "TextField", "TextView"];

impl Platform for Ios {
    fn name(&self) -> &'static str {
        "iOS"
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
                    editable: kind_of(&node).is_some_and(|kind| EDITABLE.contains(&kind)),
                    bounds: parse_bounds(&node)?,
                })
            })
            .collect();

        Snapshot::new(elements).map_err(HierarchyError::from)
    }
}

/// The element type, with the `XCUIElementType` prefix stripped.
///
/// The type appears as the tag name, and is repeated in a `type` attribute;
/// either spelling is accepted because emitters differ on which they fill.
fn kind_of<'a>(node: &'a roxmltree::Node) -> Option<&'a str> {
    let raw = node
        .attribute("type")
        .unwrap_or_else(|| node.tag_name().name());
    raw.strip_prefix("XCUIElementType").or(Some(raw))
}

/// Offscreen and disabled elements are present in the hierarchy but cannot be
/// acted on, so offering them would be offering a choice that fails.
///
/// `hittable` is the semantically correct signal — it answers whether a real
/// touch would land on this element — but it costs a hit-test per element and
/// so is omitted from a page source unless explicitly requested. When it is
/// absent, `enabled && visible` is the usual approximation: `visible` speaks
/// only to occlusion, not to whether a touch is delivered.
fn is_actionable(node: &roxmltree::Node) -> bool {
    if !node.is_element() {
        return false;
    }
    if !kind_of(node).is_some_and(|kind| HITTABLE.contains(&kind)) {
        return false;
    }
    match node.attribute("hittable") {
        Some(hittable) => hittable == "true",
        None => {
            node.attribute("visible") != Some("false") && node.attribute("enabled") != Some("false")
        }
    }
}

/// Every label under `node` that no nested actionable element has claimed.
///
/// A table cell carries its title and subtitle as child `StaticText` elements,
/// the same shape Android uses, so the same descent applies.
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

/// What this element says to a person.
///
/// `label` is the accessibility label, which is what VoiceOver reads. `value`
/// is the current contents, and is the right fallback because the emitter
/// already folds sensible defaults into it per type — static text falls back to
/// its own label, a text input to its placeholder.
///
/// `name` comes last and reluctantly. It is the developer-set accessibility
/// identifier, meant for lookup rather than display, so it reads like `row-1`
/// rather than anything a person would recognise. It beats discarding an
/// element that has nothing else, and nothing more.
fn label_of(node: &roxmltree::Node) -> Option<Box<str>> {
    ["label", "value", "name"]
        .iter()
        .filter_map(|attr| node.attribute(*attr))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(Box::from)
}

/// XCUITest reports a frame as separate origin and size attributes.
///
/// The values are points and may be fractional, so each is rounded to the
/// nearest whole pixel rather than truncated: truncation biases every tap up
/// and to the left, which matters on a control near the edge of its bounds.
fn parse_bounds(node: &roxmltree::Node) -> Option<Bounds> {
    let read = |name: &str| to_pixels(node.attribute(name)?.trim().parse::<f64>().ok()?);
    Some(Bounds::from_origin_size(
        read("x")?,
        read("y")?,
        read("width")?,
        read("height")?,
    ))
}

/// Round a point measurement to a pixel, rejecting anything `i32` cannot hold.
///
/// A hierarchy is external input: infinity and NaN are not impossible, and a
/// saturating cast would turn them into a plausible-looking tap target.
fn to_pixels(value: f64) -> Option<i32> {
    let rounded = value.round();
    if !rounded.is_finite() {
        return None;
    }
    // Compared as f64 before converting, so the cast below cannot be lossy.
    if rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(rounded as i32)
}
