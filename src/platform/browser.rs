//! The Browser platform: reads a [`PageRead`] into a snapshot.
//!
//! An engine reports what a page holds as nodes in paint order, each with the
//! words the accessibility tree gives it and the box the layout gives it. This
//! module decides which of those a person could act on, says what each is in
//! the same terms the phone readers use, and hands back the [`Snapshot`] the
//! rest of the crate already knows how to offer to a model.
//!
//! Nothing here touches a browser, so a page read recorded once reads the same
//! way every time.

use super::page::{Checked, NodeStates, PageBox, PageNode, PageRead};
use super::{HierarchyError, Platform};
use crate::act::Operation;
use crate::snapshot::{Bounds, Dialog, DialogKind, Element, MAX_ELEMENTS, NodeKey, Snapshot, Tab};
use std::collections::HashMap;

/// How far beyond the viewport a row is still offered, in viewport heights.
///
/// A single constant so the band can be measured before it is settled: a
/// page is read whole, and rows a scroll away are reachable without one, but
/// every row offered is a row the model can pick wrongly.
pub const BAND_VIEWPORTS: f64 = 1.0;

/// Roles a person acts on directly.
const ACTIONABLE_ROLES: &[&str] = &[
    "link",
    "button",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "slider",
    "spinbutton",
    "treeitem",
];

/// Roles whose nodes can stand in for a checkbox or radio that is hidden.
const CHOICE_ROLES: &[&str] = &["checkbox", "radio", "switch"];

/// Roles of the text a page says that is not a control.
const TEXT_ROLES: &[&str] = &["statictext", "heading"];

/// Roles that group a row with the words around it.
const CONTAINER_ROLES: &[&str] = &["listitem", "row", "article", "group"];

/// The longest piece of surrounding text added to a row as context.
const CONTEXT_CHARS: usize = 80;

/// How many look-alikes still get context. Beyond this they are page
/// furniture, which [`Catalog`](crate::act::Catalog) already drops.
const MOST_LOOK_ALIKES: usize = 3;

/// A web browser, driven through the Chrome DevTools Protocol or whatever
/// else produces a [`PageRead`].
#[derive(Debug, Clone, Copy)]
pub struct Browser {
    band: f64,
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

/// A browser has no home screen, app switcher, keyboard to put away, or
/// system gestures beyond Back, so none is offered: a choice the engine
/// cannot carry out is a choice a model should never be handed. What it has
/// instead is addresses, tabs and JavaScript dialogs; the catalog offers each
/// of those only when the page gives it something to act on.
const OPERATIONS: &[Operation] = &[
    Operation::Tap,
    Operation::TypeText,
    Operation::Submit,
    Operation::ScrollUp,
    Operation::ScrollDown,
    Operation::Back,
    Operation::Navigate,
    Operation::SwitchTab,
    Operation::AcceptDialog,
    Operation::DismissDialog,
    Operation::Wait,
    Operation::Done,
    Operation::Blocked,
];

impl Browser {
    /// A browser that offers rows up to [`BAND_VIEWPORTS`] beyond the screen.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_band(BAND_VIEWPORTS)
    }

    /// A browser that offers rows up to `band` viewport heights above and
    /// below the screen. Zero offers only what is on screen.
    #[must_use]
    pub const fn with_band(band: f64) -> Self {
        Self { band }
    }

    /// Build the snapshot a page read describes.
    ///
    /// The same reading [`Platform::parse_hierarchy`] does, without the round
    /// trip through JSON. Quiet time is the engine's to add afterwards, with
    /// [`Snapshot::quiet_for`].
    #[must_use]
    pub fn snapshot_from(&self, read: &PageRead) -> Snapshot {
        let page = Page::new(read, self.band);
        let candidates = page.candidates();
        let (offered, refused) = page.split(candidates);
        let (kept, left_out) = Page::truncate(offered);
        let kept = page.with_context(kept);

        let notices = page.notices(&kept, &refused, left_out);
        let unavailable = refused.iter().map(|c| c.label.as_str().into()).collect();
        let mut elements = Vec::with_capacity(kept.len());
        let mut keys = Vec::with_capacity(kept.len());
        let mut field_only = Vec::with_capacity(kept.len());
        let mut protected = Vec::with_capacity(kept.len());
        for row in kept {
            elements.push(Element {
                label: row.label.into(),
                detail: row.detail.map(Into::into),
                editable: row.editable,
                bounds: bounds_of(row.bounds),
            });
            keys.push(Some(row.key.clone()));
            field_only.push(row.field_only);
            protected.push(row.node.protected);
        }

        // Cut to `MAX_ELEMENTS` above, so the snapshot cannot refuse.
        let snapshot = Snapshot::new(elements).unwrap_or_else(|_| empty_snapshot());
        snapshot
            .in_app(Some(origin_of(&read.url).into()))
            .at_url(Some(read.url.clone()))
            .saying(notices)
            .offering_but_refusing(unavailable)
            .with_tabs(other_tabs(read))
            .with_dialog(read.dialog.clone())
            .with_node_keys(keys)
            .with_field_only(field_only)
            .with_protected(protected)
    }
}

impl Platform for Browser {
    fn name(&self) -> &'static str {
        "Browser"
    }

    fn operations(&self) -> &'static [Operation] {
        OPERATIONS
    }

    fn parse_hierarchy(&self, raw: &str) -> Result<Snapshot, HierarchyError> {
        let read: PageRead =
            serde_json::from_str(raw).map_err(|error| HierarchyError::Malformed {
                detail: error.to_string().into_boxed_str(),
            })?;
        Ok(self.snapshot_from(&read))
    }
}

fn empty_snapshot() -> Snapshot {
    // An empty list is under every limit.
    Snapshot::new(Vec::new()).unwrap_or_else(|_| unreachable!("no rows is within the limit"))
}

/// Which part of the page a row is in, nearest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Zone {
    /// Inside the viewport.
    Screen,
    /// Outside it, within the band.
    Band,
}

/// A node that could be a row, or a refused control, with its words worked out.
///
/// Independent facts about one node, not facets of one state.
#[allow(clippy::struct_excessive_bools)]
struct Candidate<'a> {
    /// Where it comes in paint order.
    index: usize,
    node: &'a PageNode,
    role: &'a str,
    bounds: PageBox,
    /// Whether it is a clickable that is not a control by role.
    wrapper: bool,
    disabled: bool,
    label: String,
    detail: Option<String>,
    editable: bool,
    field_only: bool,
    key: &'a NodeKey,
    zone: Option<Zone>,
    /// How far outside the viewport it is, in CSS pixels.
    gap: f64,
}

/// The page read with the geometry it is judged against.
struct Page<'a> {
    read: &'a PageRead,
    band: f64,
    /// The topmost modal dialog, by position in paint order and box.
    modal: Option<(usize, Option<PageBox>)>,
}

impl<'a> Page<'a> {
    fn new(read: &'a PageRead, band: f64) -> Self {
        let modal = read
            .nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, node)| {
                node.states.modal && matches!(lower(&node.role).as_str(), "dialog" | "alertdialog")
            })
            .map(|(index, node)| (index, node.bounds));
        Self { read, band, modal }
    }

    /// Every node that is a control a person could act on, in paint order.
    fn candidates(&self) -> Vec<Candidate<'a>> {
        let nodes = &self.read.nodes;
        let mut found = Vec::new();
        let mut at = 0;
        while at < nodes.len() {
            let node = &nodes[at];
            let control = self.hidden_control_of(node, at + 1);
            if let Some(candidate) = self.candidate(at, node, control) {
                found.push(candidate);
            }
            at += if control.is_some() { 2 } else { 1 };
        }
        collapse_wrappers(found)
    }

    /// The checkbox or radio a label wraps when the control itself is hidden.
    ///
    /// Such a control has no box and cannot be clicked; the label drawn around
    /// it can, and toggles it. The control follows its label in the tree.
    fn hidden_control_of(&self, node: &PageNode, next: usize) -> Option<&'a PageNode> {
        if !matches!(lower(&node.role).as_str(), "labeltext" | "label") {
            return None;
        }
        let control = self.read.nodes.get(next)?;
        let hidden = control
            .bounds
            .is_none_or(|b| b.width <= 0.0 || b.height <= 0.0);
        (hidden
            && control.frame == node.frame
            && CHOICE_ROLES.contains(&lower(&control.role).as_str()))
        .then_some(control)
    }

    fn candidate(
        &self,
        index: usize,
        node: &'a PageNode,
        control: Option<&'a PageNode>,
    ) -> Option<Candidate<'a>> {
        let bounds = node.bounds.filter(|b| b.width > 0.0 && b.height > 0.0)?;
        // A promoted label is the control, as far as anything reading it goes.
        let (role_node, name) = match control {
            Some(control) if node.name.trim().is_empty() => (control, &*control.name),
            Some(control) => (control, &*node.name),
            None => (node, &*node.name),
        };
        let role = role_node.role.as_ref();
        let states = role_node.states;
        let actionable = ACTIONABLE_ROLES.contains(&lower(role).as_str());
        let named = !name.trim().is_empty();
        if !(actionable || node.clickable || (states.focusable && named)) {
            return None;
        }
        let words = Words::of(role_node, name, states)?;
        let (zone, gap) = self.zone_of(bounds);
        Some(Candidate {
            index,
            node,
            role,
            bounds,
            wrapper: !actionable && node.clickable,
            disabled: states.disabled,
            label: words.label,
            detail: words.detail,
            editable: words.editable,
            field_only: words.field_only,
            key: &node.key,
            zone,
            gap,
        })
    }

    /// Where a box is against the viewport, and how far outside it.
    fn zone_of(&self, bounds: PageBox) -> (Option<Zone>, f64) {
        let view = self.read.viewport;
        let across = bounds.x < view.width && bounds.x + bounds.width > 0.0;
        if !across {
            return (None, f64::INFINITY);
        }
        let bottom = bounds.y + bounds.height;
        if bottom > 0.0 && bounds.y < view.height {
            return (Some(Zone::Screen), 0.0);
        }
        let gap = if bottom <= 0.0 {
            -bottom
        } else {
            bounds.y - view.height
        };
        let reach = self.band * view.height;
        if reach > 0.0 && gap <= reach {
            (Some(Zone::Band), gap)
        } else {
            (None, gap)
        }
    }

    /// Whether the box belongs to the modal, when there is one, or to the page
    /// when there is not.
    fn in_scope(&self, index: usize, bounds: PageBox) -> bool {
        let Some((at, frame)) = self.modal else {
            return true;
        };
        if index <= at {
            return false;
        }
        frame.is_none_or(|frame| {
            let (x, y) = (
                bounds.x + bounds.width / 2.0,
                bounds.y + bounds.height / 2.0,
            );
            x >= frame.x
                && x <= frame.x + frame.width
                && y >= frame.y
                && y <= frame.y + frame.height
        })
    }

    /// Divide candidates into rows that can be acted on and controls refused.
    ///
    /// A refused control is only worth naming while it is on screen.
    fn split(&self, candidates: Vec<Candidate<'a>>) -> (Vec<Candidate<'a>>, Vec<Candidate<'a>>) {
        let mut offered = Vec::new();
        let mut refused = Vec::new();
        for candidate in candidates {
            if !self.in_scope(candidate.index, candidate.bounds) {
                continue;
            }
            if candidate.disabled {
                if candidate.zone == Some(Zone::Screen) {
                    refused.push(candidate);
                }
            } else if candidate.zone.is_some() {
                offered.push(candidate);
            }
        }
        (offered, refused)
    }

    /// Keep what fits in one judgment: the screen's rows, then the band's,
    /// nearest first. Returns what was kept in paint order, and how many were
    /// left out.
    fn truncate(mut offered: Vec<Candidate<'a>>) -> (Vec<Candidate<'a>>, usize) {
        let left_out = offered.len().saturating_sub(MAX_ELEMENTS);
        if left_out > 0 {
            offered.sort_by(|a, b| {
                a.zone
                    .cmp(&b.zone)
                    .then(a.gap.total_cmp(&b.gap))
                    .then(a.index.cmp(&b.index))
            });
            offered.truncate(MAX_ELEMENTS);
            offered.sort_by_key(|c| c.index);
        }
        (offered, left_out)
    }

    /// Mark the band's rows, and tell two or three look-alikes apart.
    ///
    /// Four or more are left identical on purpose: giving twenty "About this
    /// result" rows distinct context would put them all back into the choice,
    /// when the catalog drops them as page furniture.
    fn with_context(&self, mut kept: Vec<Candidate<'a>>) -> Vec<Candidate<'a>> {
        let mut groups: HashMap<(String, String), usize> = HashMap::new();
        for row in &kept {
            *groups
                .entry((lower(row.role), row.label.clone()))
                .or_default() += 1;
        }
        for row in &mut kept {
            let alike = groups[&(lower(row.role), row.label.clone())];
            if (2..=MOST_LOOK_ALIKES).contains(&alike)
                && let Some(context) = self.context_of(row)
            {
                row.detail = Some(match row.detail.take() {
                    Some(detail) => format!("{detail}, in {context}"),
                    None => format!("in {context}"),
                });
            }
            if row.zone == Some(Zone::Band) {
                row.detail = Some(match row.detail.take() {
                    Some(detail) => format!("{detail}, off screen"),
                    None => "off screen".to_owned(),
                });
            }
        }
        kept
    }

    /// The nearest heading or landmark the row sits under, or else the words
    /// of the smallest list item, row or group around it.
    fn context_of(&self, row: &Candidate<'_>) -> Option<String> {
        if let Some(context) = row
            .node
            .context
            .as_deref()
            .map(squash)
            .filter(|c| !c.is_empty())
        {
            return Some(clip(&context));
        }
        let around = row.bounds;
        self.read
            .nodes
            .iter()
            .filter(|node| node.key != *row.key)
            .filter(|node| CONTAINER_ROLES.contains(&lower(&node.role).as_str()))
            .filter(|node| !node.name.trim().is_empty())
            .filter_map(|node| node.bounds.map(|b| (node, b)))
            .filter(|(_, b)| contains(*b, around))
            .min_by(|(_, a), (_, b)| (a.width * a.height).total_cmp(&(b.width * b.height)))
            .map(|(node, _)| clip(&squash(&node.name)))
    }

    /// What the screen says: the words in view that no row already carries, an
    /// open dialog, an alert that was accepted, and any rows left out.
    fn notices(
        &self,
        kept: &[Candidate<'_>],
        refused: &[Candidate<'_>],
        left_out: usize,
    ) -> Vec<Box<str>> {
        let spoken: Vec<&str> = kept
            .iter()
            .chain(refused)
            .map(|row| row.label.as_str())
            .collect();
        let mut said: Vec<(PageBox, String)> = self
            .read
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| TEXT_ROLES.contains(&lower(&node.role).as_str()))
            .filter_map(|(index, node)| Some((index, node.bounds?, squash(&node.name))))
            .filter(|(index, bounds, text)| {
                !text.is_empty()
                    && self.zone_of(*bounds).0 == Some(Zone::Screen)
                    && self.in_scope(*index, *bounds)
                    && !spoken.iter().any(|label| label.contains(text.as_str()))
            })
            .map(|(_, bounds, text)| (bounds, text))
            .collect();
        said.sort_by(|(a, _), (b, _)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));

        let mut notices: Vec<Box<str>> = said.into_iter().map(|(_, text)| text.into()).collect();
        if let Some(dialog) = &self.read.dialog {
            notices.push(describe_dialog(dialog).into());
        }
        match self.read.unreadable_frames {
            0 => {}
            1 => notices.push("A frame from another site on this page cannot be read".into()),
            n => notices
                .push(format!("{n} frames from other sites on this page cannot be read").into()),
        }
        if let Some(alert) = &self.read.accepted_alert {
            notices.push(format!("An alert was accepted: \"{alert}\"").into());
        }
        if left_out > 0 {
            notices.push(format!("{left_out} more rows were left out").into());
        }
        notices
    }
}

/// A clickable that only wraps one other row is that row, seen from outside.
///
/// A card with a click listener around a single link would otherwise be two
/// rows that do the same thing, and the outer one is drawn over the inner.
///
/// A label around a field is the same case, told apart differently. Chrome
/// reports it as a clickable node of its own, drawn under the field, and the
/// field's box is often a pixel or two taller than the label's text, so the
/// label does not contain it, and something else (a sticky header) may cross
/// it too. But the field takes its name from the label, so the label is the
/// field it overlaps that is called what it is called.
fn collapse_wrappers(candidates: Vec<Candidate<'_>>) -> Vec<Candidate<'_>> {
    let controls: Vec<&Candidate<'_>> = candidates.iter().filter(|c| !c.wrapper).collect();
    let wrapped = |c: &Candidate<'_>| {
        if matches!(lower(c.role).as_str(), "labeltext" | "label") {
            controls.iter().any(|inner| {
                inner.node.frame == c.node.frame
                    && inner.label == c.label
                    && overlaps(c.bounds, inner.bounds)
            })
        } else {
            controls
                .iter()
                .filter(|inner| contains(c.bounds, inner.bounds))
                .count()
                == 1
        }
    };
    let keep: Vec<bool> = candidates
        .iter()
        .map(|c| !c.wrapper || !wrapped(c))
        .collect();
    candidates
        .into_iter()
        .zip(keep)
        .filter_map(|(c, keep)| keep.then_some(c))
        .collect()
}

/// Whether two boxes share any area.
fn overlaps(a: PageBox, b: PageBox) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Whether `outer` holds `inner`, give or take a pixel of rounding.
fn contains(outer: PageBox, inner: PageBox) -> bool {
    const SLACK: f64 = 1.0;
    inner.x >= outer.x - SLACK
        && inner.y >= outer.y - SLACK
        && inner.x + inner.width <= outer.x + outer.width + SLACK
        && inner.y + inner.height <= outer.y + outer.height + SLACK
}

/// What a row is called and says about itself.
struct Words {
    label: String,
    detail: Option<String>,
    editable: bool,
    field_only: bool,
}

impl Words {
    /// `None` for a control that has nothing to be called.
    fn of(node: &PageNode, name: &str, states: NodeStates) -> Option<Self> {
        let role = lower(&node.role);
        let name = squash(name);
        let description = node
            .description
            .as_deref()
            .map(squash)
            .filter(|d| !d.is_empty());
        let select = role == "combobox" && !states.editable && !node.options.is_empty();
        let editable =
            select || states.editable || matches!(role.as_str(), "textbox" | "searchbox");

        if !editable {
            let label = if name.is_empty() {
                description.clone()?
            } else {
                name
            };
            let detail = toggle_state(&role, states)
                .map(str::to_owned)
                .or_else(|| description.filter(|d| *d != label));
            return Some(Self {
                label,
                detail,
                editable: false,
                field_only: false,
            });
        }

        // Chrome names a field with no label by its placeholder, so such a
        // field is called by it and says it holds nothing, as the Android
        // reader does: a run that had typed once took a form that had reset
        // for one already filled in. A description is never a placeholder.
        let (label, nameless) = if name.is_empty() {
            (
                if select { "drop-down" } else { "text field" }.to_owned(),
                true,
            )
        } else {
            (name, false)
        };
        let value = node.value.as_deref().map(squash).filter(|v| !v.is_empty());
        let detail = if select {
            let current = value.unwrap_or_else(|| "nothing selected".to_owned());
            let options: Vec<&str> = node.options.iter().map(AsRef::as_ref).collect();
            Some(format!("{current} (options: {})", options.join(", ")))
        } else if node.protected {
            // The read never carries what a password field holds, so whether
            // it is empty is not known either; saying "empty" would invite
            // typing it again.
            Some("contents not shown".to_owned())
        } else {
            match value {
                Some(value) => Some(value),
                None if nameless => None,
                None => Some("empty".to_owned()),
            }
        };
        Some(Self {
            label,
            detail,
            editable: true,
            field_only: select,
        })
    }
}

/// A toggle's state in words, when the node has one.
fn toggle_state(role: &str, states: NodeStates) -> Option<&'static str> {
    match states.checked {
        Some(Checked::Checked) => return Some("checked"),
        Some(Checked::Unchecked) => return Some("not checked"),
        Some(Checked::Mixed) => return Some("mixed"),
        None => {}
    }
    match states.expanded {
        Some(true) => return Some("expanded"),
        Some(false) => return Some("collapsed"),
        None => {}
    }
    (states.selected && matches!(role, "tab" | "option" | "treeitem" | "menuitem"))
        .then_some("selected")
}

fn describe_dialog(dialog: &Dialog) -> String {
    let kind = match dialog.kind {
        DialogKind::Confirm => "confirm",
        DialogKind::Prompt => "prompt",
        DialogKind::BeforeUnload => "leave-page",
    };
    format!("A {kind} dialog is open: \"{}\"", dialog.message)
}

/// The tabs the run owns other than the one this read came from.
///
/// The engine lists every tab it owns, the driven one included, and only the
/// others can be switched to. The driven one is named by its key.
fn other_tabs(read: &PageRead) -> Vec<Tab> {
    read.tabs
        .iter()
        .filter(|tab| read.current.as_ref() != Some(&tab.key))
        .map(|tab| Tab {
            key: tab.key.clone(),
            title: tab.title.clone(),
            url: tab.url.clone(),
        })
        .collect()
}

/// The scheme, host and port of an address, which is what a site is.
///
/// Credentials and everything after the host are left out, and an address
/// with no host (`about:blank`, `data:…`) is named by its scheme alone so
/// nothing a page put in it travels with the snapshot.
fn origin_of(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or(authority);
        return format!("{scheme}://{host}");
    }
    let scheme = url.split(':').next().unwrap_or_default();
    format!("{scheme}:")
}

fn bounds_of(bounds: PageBox) -> Bounds {
    Bounds {
        left: pixel(bounds.x),
        top: pixel(bounds.y),
        right: pixel(bounds.x + bounds.width),
        bottom: pixel(bounds.y + bounds.height),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn pixel(value: f64) -> i32 {
    // Clamped first, so a page's absurd box cannot wrap.
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn lower(text: &str) -> String {
    text.to_ascii_lowercase()
}

/// Whitespace collapsed to single spaces, as names arrive with the line breaks
/// and non-breaking spaces of the markup they came from.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str) -> String {
    if text.chars().count() <= CONTEXT_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().take(CONTEXT_CHARS).collect();
    format!("{}…", kept.trim_end())
}
