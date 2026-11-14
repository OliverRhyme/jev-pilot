//! Turning a tab's raw DevTools replies into a page read. Pure.
//!
//! A read joins two views of the page by backend node id: the accessibility
//! tree, which says what each node is (role, name, states), and the layout
//! snapshot, which says where it is drawn, in what order, and whether it has a
//! click listener. Boxes come out of the layout in document coordinates and
//! leave here in viewport CSS pixels, with each same-process frame placed by
//! its iframe's content box.
//!
//! The methods behind these replies are experimental in the protocol, so a
//! Chrome update could change their shape; every shape this crate relies on is
//! read in this one module, against fixtures recorded from a real browser.

use crate::platform::page::{Checked, NodeStates, PageBox, PageNode, PageRead, PageTab, Viewport};
use crate::snapshot::{NodeKey, TabKey};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// The replies one read is made from.
#[derive(Debug, Clone, Default)]
pub struct RawRead {
    /// `Page.getFrameTree`.
    pub frame_tree: Value,
    /// `Accessibility.getFullAXTree`, one per same-process frame, with the
    /// frame's id; the main frame's first.
    pub ax_trees: Vec<(Box<str>, Value)>,
    /// `DOMSnapshot.captureSnapshot` with paint order and DOM rects.
    pub snapshot: Value,
    /// `Page.getLayoutMetrics`.
    pub layout_metrics: Value,
    /// `Target.getTargets`.
    pub targets: Value,
}

/// The replies did not have the shape this crate reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(Box<str>);

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "the page could not be read: {}", self.0)
    }
}

impl core::error::Error for ReadError {}

/// The tabs a run owns: the one it started on, the ones it opened, and the
/// ones those opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnedTabs {
    tabs: Vec<(Box<str>, bool)>,
    /// Tabs that were open before the run attached: the owner's, never the
    /// run's, whatever opened them.
    existing: Vec<Box<str>>,
}

impl OwnedTabs {
    /// Own this tab, which the run either opened (`true`) or started on.
    pub fn start_on(&mut self, target: &str, opened_by_run: bool) {
        if !self.owns(target) {
            self.tabs.push((target.into(), opened_by_run));
        }
    }

    /// Note a tab that was open before the run attached, so it is never
    /// adopted.
    pub fn existed(&mut self, target: &str) {
        if !self.existing.iter().any(|t| &**t == target) {
            self.existing.push(target.into());
        }
    }

    /// Own a newly created tab if a tab the run owns opened it, and say
    /// whether the run owns it.
    ///
    /// The protocol sets the opener even for a `noopener` link, so a tab the
    /// run caused is never missed this way. A tab that was open before the run
    /// attached is never adopted: the protocol reports an opener for those
    /// too, and an owner's tab opened from the start tab yesterday is not the
    /// run's.
    pub fn adopt(&mut self, target: &str, opener: Option<&str>) -> bool {
        if self.existing.iter().any(|t| &**t == target) {
            return self.owns(target);
        }
        match opener {
            Some(opener) if self.owns(opener) => {
                self.start_on(target, true);
                true
            }
            _ => self.owns(target),
        }
    }

    /// Forget a tab that has closed.
    pub fn forget(&mut self, target: &str) {
        self.tabs.retain(|(id, _)| &**id != target);
    }

    /// Whether the run owns this tab.
    #[must_use]
    pub fn owns(&self, target: &str) -> bool {
        self.tabs.iter().any(|(id, _)| &**id == target)
    }

    /// Whether the run opened this tab, as against starting on it.
    #[must_use]
    pub fn opened_by_run(&self, target: &str) -> bool {
        self.tabs
            .iter()
            .any(|(id, opened)| &**id == target && *opened)
    }
}

/// Numbers the documents a run has read, so a node key can say which document
/// its node came from without carrying the protocol's long loader ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaders {
    seen: Vec<(Box<str>, Box<str>)>,
}

impl Loaders {
    /// The number of this frame's document, given one the first time it is
    /// seen. Numbers start at one.
    pub fn number(&mut self, frame: &str, loader: &str) -> usize {
        if let Some(i) = self
            .seen
            .iter()
            .position(|(f, l)| &**f == frame && &**l == loader)
        {
            return i + 1;
        }
        self.seen.push((frame.into(), loader.into()));
        self.seen.len()
    }

    /// The frame and loader a number stands for.
    #[must_use]
    pub fn get(&self, number: usize) -> Option<(&str, &str)> {
        let (frame, loader) = self.seen.get(number.checked_sub(1)?)?;
        Some((frame, loader))
    }
}

/// What a node key says: the backend node id, and the number of the document
/// it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyParts {
    /// The node's backend id.
    pub backend: i64,
    /// The document's number in [`Loaders`].
    pub document: usize,
}

impl KeyParts {
    /// The key for this node.
    #[must_use]
    pub fn key(self) -> NodeKey {
        NodeKey::new(format!("{}:{}", self.backend, self.document))
    }

    /// Read a key this engine issued.
    #[must_use]
    pub fn parse(key: &NodeKey) -> Option<Self> {
        let (backend, document) = key.as_str().split_once(':')?;
        Some(Self {
            backend: backend.parse().ok()?,
            document: document.parse().ok()?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct AxValue {
    #[serde(default)]
    value: Value,
}

#[derive(Debug, Deserialize)]
struct AxProperty {
    name: String,
    value: AxValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AxNode {
    node_id: String,
    #[serde(default)]
    ignored: bool,
    role: Option<AxValue>,
    name: Option<AxValue>,
    description: Option<AxValue>,
    value: Option<AxValue>,
    #[serde(default)]
    properties: Vec<AxProperty>,
    #[serde(default)]
    child_ids: Vec<String>,
    parent_id: Option<String>,
    #[serde(rename = "backendDOMNodeId")]
    backend: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AxTree {
    nodes: Vec<AxNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RareIndex {
    index: Vec<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RareInteger {
    index: Vec<usize>,
    value: Vec<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SnapshotNodes {
    parent_index: Vec<i64>,
    node_name: Vec<i64>,
    backend_node_id: Vec<i64>,
    attributes: Vec<Vec<i64>>,
    is_clickable: RareIndex,
    content_document_index: RareInteger,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SnapshotLayout {
    node_index: Vec<usize>,
    bounds: Vec<Vec<f64>>,
    paint_orders: Vec<i64>,
    client_rects: Vec<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDocument {
    #[serde(default = "minus_one")]
    title: i64,
    nodes: SnapshotNodes,
    layout: SnapshotLayout,
    #[serde(default)]
    scroll_offset_x: f64,
    #[serde(default)]
    scroll_offset_y: f64,
}

fn minus_one() -> i64 {
    -1
}

#[derive(Debug, Deserialize)]
struct DomSnapshot {
    documents: Vec<SnapshotDocument>,
    strings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo {
    target_id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
}

/// Where a node sits in the layout snapshot.
#[derive(Debug, Clone)]
struct Dom {
    doc: usize,
    index: usize,
}

/// One document of the snapshot, with what is derived from it.
struct Layout<'a> {
    snapshot: &'a DomSnapshot,
    /// Each document's layout entry for each node.
    entries: Vec<HashMap<usize, usize>>,
    /// Where each document's origin is in the viewport.
    offsets: Vec<(f64, f64)>,
    /// Each node's place in the document order, frames spliced in at their
    /// iframe.
    order: HashMap<(usize, usize), usize>,
    /// Which iframe holds each document but the first.
    owner: HashMap<usize, (usize, usize)>,
    by_backend: HashMap<i64, Dom>,
}

impl<'a> Layout<'a> {
    fn new(snapshot: &'a DomSnapshot) -> Self {
        let entries: Vec<HashMap<usize, usize>> = snapshot
            .documents
            .iter()
            .map(|d| {
                d.layout
                    .node_index
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (*n, i))
                    .collect()
            })
            .collect();
        let mut owner = HashMap::new();
        for (doc, d) in snapshot.documents.iter().enumerate() {
            let rare = &d.nodes.content_document_index;
            for (node, child) in rare.index.iter().zip(&rare.value) {
                owner.insert(*child, (doc, *node));
            }
        }
        let mut by_backend = HashMap::new();
        for (doc, d) in snapshot.documents.iter().enumerate() {
            for (index, backend) in d.nodes.backend_node_id.iter().enumerate() {
                by_backend.entry(*backend).or_insert(Dom { doc, index });
            }
        }
        let mut this = Self {
            snapshot,
            entries,
            offsets: Vec::new(),
            order: HashMap::new(),
            owner,
            by_backend,
        };
        this.offsets = (0..snapshot.documents.len())
            .map(|doc| this.offset(doc, 0))
            .collect();
        let mut next = 0;
        if !snapshot.documents.is_empty() {
            this.number(0, &mut next);
        }
        this
    }

    /// Where a document's origin is in the main viewport.
    fn offset(&self, doc: usize, depth: usize) -> (f64, f64) {
        let d = &self.snapshot.documents[doc];
        let own = (-d.scroll_offset_x, -d.scroll_offset_y);
        let Some(&(parent, node)) = self.owner.get(&doc) else {
            return own;
        };
        if depth > 16 {
            return own;
        }
        let (px, py) = self.offset(parent, depth + 1);
        let Some(&layout) = self.entries[parent].get(&node) else {
            return own;
        };
        let p = &self.snapshot.documents[parent].layout;
        let bounds = p.bounds.get(layout).map_or(&[][..], Vec::as_slice);
        let client = p.client_rects.get(layout).map_or(&[][..], Vec::as_slice);
        let at = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);
        (
            px + at(bounds, 0) + at(client, 0) + own.0,
            py + at(bounds, 1) + at(client, 1) + own.1,
        )
    }

    /// Number a document's nodes in order, splicing child documents in after
    /// the iframe that holds them.
    fn number(&mut self, doc: usize, next: &mut usize) {
        let children: HashMap<usize, usize> = self
            .owner
            .iter()
            .filter(|(_, (d, _))| *d == doc)
            .map(|(child, (_, node))| (*node, *child))
            .collect();
        let count = self.snapshot.documents[doc].nodes.backend_node_id.len();
        for index in 0..count {
            self.order.insert((doc, index), *next);
            *next += 1;
            if let Some(&child) = children.get(&index)
                && !self.order.contains_key(&(child, 0))
            {
                self.number(child, next);
            }
        }
    }

    fn string(&self, i: i64) -> &str {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.snapshot.strings.get(i))
            .map_or("", String::as_str)
    }

    fn node_name(&self, dom: &Dom) -> &str {
        let d = &self.snapshot.documents[dom.doc];
        self.string(d.nodes.node_name.get(dom.index).copied().unwrap_or(-1))
    }

    fn attribute(&self, dom: &Dom, name: &str) -> Option<&str> {
        let d = &self.snapshot.documents[dom.doc];
        let attrs = d.nodes.attributes.get(dom.index)?;
        attrs
            .chunks(2)
            .find(|pair| self.string(pair[0]).eq_ignore_ascii_case(name))
            .map(|pair| self.string(pair.get(1).copied().unwrap_or(-1)))
    }

    fn clickable(&self, dom: &Dom) -> bool {
        self.snapshot.documents[dom.doc]
            .nodes
            .is_clickable
            .index
            .contains(&dom.index)
    }

    fn bounds(&self, dom: &Dom) -> Option<PageBox> {
        let layout = *self.entries[dom.doc].get(&dom.index)?;
        let bounds = self.snapshot.documents[dom.doc].layout.bounds.get(layout)?;
        let (offset_x, offset_y) = self.offsets[dom.doc];
        match bounds.as_slice() {
            [left, top, width, height, ..] if *width > 0.0 && *height > 0.0 => Some(PageBox {
                x: left + offset_x,
                y: top + offset_y,
                width: *width,
                height: *height,
            }),
            _ => None,
        }
    }

    /// The node's paint order, or its nearest drawn ancestor's.
    fn paint(&self, dom: &Dom) -> i64 {
        let d = &self.snapshot.documents[dom.doc];
        let mut index = dom.index;
        for _ in 0..512 {
            if let Some(&layout) = self.entries[dom.doc].get(&index)
                && let Some(&paint) = d.layout.paint_orders.get(layout)
            {
                return paint;
            }
            match d.nodes.parent_index.get(index).copied() {
                Some(parent) if parent >= 0 => index = usize::try_from(parent).unwrap_or(0),
                _ => break,
            }
        }
        0
    }

    fn order(&self, dom: &Dom) -> usize {
        self.order
            .get(&(dom.doc, dom.index))
            .copied()
            .unwrap_or(usize::MAX)
    }

    /// The iframe that holds this node's document, if it is a frame's.
    fn iframe_of(&self, dom: &Dom) -> Option<Dom> {
        let &(doc, index) = self.owner.get(&dom.doc)?;
        Some(Dom { doc, index })
    }
}

/// Roles a node is kept for even with no name: a control, or a container the
/// reader needs to see.
const KEPT_ROLES: &[&str] = &[
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
    "dialog",
    "alertdialog",
    "Iframe",
];

/// Roles that are never a node of their own.
const DROPPED_ROLES: &[&str] = &[
    "RootWebArea",
    "InlineTextBox",
    "LineBreak",
    "MenuListPopup",
    "none",
];

const LANDMARKS: &[&str] = &[
    "banner",
    "complementary",
    "contentinfo",
    "form",
    "main",
    "navigation",
    "region",
    "search",
    "dialog",
    "alertdialog",
];

const DIALOGS: &[&str] = &["dialog", "alertdialog"];

fn text(value: Option<&AxValue>) -> Box<str> {
    let text = match value.map(|v| &v.value) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    text.into()
}

fn states(node: &AxNode) -> NodeStates {
    let mut states = NodeStates::default();
    for property in &node.properties {
        let v = &property.value.value;
        let on = v.as_bool().unwrap_or(false);
        match property.name.as_str() {
            "focusable" => states.focusable = on,
            "editable" => states.editable = v.as_str().is_some_and(|s| !s.is_empty()),
            "disabled" => states.disabled = on,
            "checked" => {
                states.checked = match v.as_str() {
                    Some("true") => Some(Checked::Checked),
                    Some("false") => Some(Checked::Unchecked),
                    Some("mixed") => Some(Checked::Mixed),
                    _ => None,
                }
            }
            "expanded" => states.expanded = v.as_bool(),
            "selected" => states.selected = on,
            "focused" => states.focused = on,
            "modal" => states.modal = on,
            _ => {}
        }
    }
    states
}

/// One accessibility tree, walked in document order.
struct Walk<'t> {
    by_id: HashMap<&'t str, &'t AxNode>,
}

impl<'t> Walk<'t> {
    fn new(tree: &'t AxTree) -> Self {
        Self {
            by_id: tree.nodes.iter().map(|n| (n.node_id.as_str(), n)).collect(),
        }
    }

    fn root(&self, tree: &'t AxTree) -> Option<&'t AxNode> {
        tree.nodes.iter().find(|n| {
            n.parent_id
                .as_ref()
                .is_none_or(|p| !self.by_id.contains_key(p.as_str()))
        })
    }

    fn children<'n>(&'n self, node: &'n AxNode) -> impl Iterator<Item = &'t AxNode> + 'n {
        node.child_ids
            .iter()
            .filter_map(|id| self.by_id.get(id.as_str()).copied())
    }

    /// The text of every static text below a node, joined, leaving out what
    /// fields hold: that is a value, and a password's must never be read.
    fn words(&self, node: &AxNode) -> String {
        let mut words = Vec::new();
        let mut stack: Vec<&AxNode> = self.children(node).collect();
        stack.reverse();
        while let Some(n) = stack.pop() {
            if states(n).editable {
                continue;
            }
            if role(n) == "StaticText" {
                let t = text(n.name.as_ref());
                if !t.is_empty() {
                    words.push(t);
                }
                continue;
            }
            let mut children: Vec<&AxNode> = self.children(n).collect();
            children.reverse();
            stack.extend(children);
        }
        words.join(" ")
    }

    /// The option names below a native drop-down.
    fn options(&self, node: &AxNode) -> Vec<Box<str>> {
        let mut options = Vec::new();
        let mut stack: Vec<&AxNode> = self.children(node).collect();
        stack.reverse();
        while let Some(n) = stack.pop() {
            if role(n) == "option" {
                options.push(text(n.name.as_ref()));
                continue;
            }
            let mut children: Vec<&AxNode> = self.children(n).collect();
            children.reverse();
            stack.extend(children);
        }
        options
    }
}

fn role(node: &AxNode) -> &str {
    node.role
        .as_ref()
        .and_then(|r| r.value.as_str())
        .unwrap_or_default()
}

/// A node on its way into the read, with what it is sorted by.
struct Placed {
    node: PageNode,
    /// The paint order and document order of each anchor above it (the
    /// iframe that holds its frame, the dialog it sits in) and then its own,
    /// so that it sorts directly after its anchors.
    sort: Vec<(i64, usize)>,
}

struct Ancestor {
    role: Box<str>,
    name: Box<str>,
    /// Whether text can be typed into it, which makes the text below it the
    /// field's value rather than words of the page.
    editable: bool,
    position: usize,
    /// The sort path of the dialog this is, when it is one.
    dialog: Option<Vec<(i64, usize)>>,
}

/// Translate one read's raw replies into a page read.
///
/// `current` is the target id of the driven tab, `owned` the tabs the run
/// owns, and `loaders` the numbers given to the documents read so far, which
/// node keys refer to.
///
/// # Errors
/// Returns [`ReadError`] when a reply does not have the shape this reads.
pub fn translate(
    raw: &RawRead,
    current: &str,
    owned: &OwnedTabs,
    loaders: &mut Loaders,
) -> Result<PageRead, ReadError> {
    let snapshot: DomSnapshot = serde_json::from_value(raw.snapshot.clone())
        .map_err(|e| ReadError(format!("layout snapshot: {e}").into()))?;
    let layout = Layout::new(&snapshot);
    let frames = frame_loaders(&raw.frame_tree);
    let targets: Vec<TargetInfo> = raw
        .targets
        .get("targetInfos")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| ReadError(format!("targets: {e}").into()))?
        .unwrap_or_default();

    let mut placed = Vec::new();
    for (frame, tree) in &raw.ax_trees {
        let tree: AxTree = serde_json::from_value(tree.clone())
            .map_err(|e| ReadError(format!("accessibility tree: {e}").into()))?;
        let loader = frames
            .iter()
            .find(|(f, _)| f.as_str() == &**frame)
            .map_or("", |(_, l)| l.as_str());
        let document = loaders.number(frame, loader);
        read_frame(&tree, frame, document, &layout, &mut placed);
    }
    placed.sort_by(|a, b| a.sort.cmp(&b.sort));

    let main = raw.frame_tree.pointer("/frameTree/frame");
    let url = main.map_or_else(String::new, |f| {
        let url = f.get("url").and_then(Value::as_str).unwrap_or_default();
        let fragment = f
            .get("urlFragment")
            .and_then(Value::as_str)
            .unwrap_or_default();
        format!("{url}{fragment}")
    });
    let current_info = targets.iter().find(|t| t.target_id == current);
    let title = snapshot
        .documents
        .first()
        .map(|d| layout.string(d.title))
        .filter(|t| !t.is_empty())
        .or(current_info.map(|t| t.title.as_str()))
        .unwrap_or_default();

    let metrics = &raw.layout_metrics;
    let viewport = ["cssVisualViewport", "cssLayoutViewport", "layoutViewport"]
        .iter()
        .find_map(|k| metrics.get(*k))
        .map(|v| Viewport {
            width: v.get("clientWidth").and_then(Value::as_f64).unwrap_or(0.0),
            height: v.get("clientHeight").and_then(Value::as_f64).unwrap_or(0.0),
        })
        .ok_or_else(|| ReadError("layout metrics have no viewport".into()))?;

    Ok(PageRead {
        url: url.into(),
        title: title.into(),
        viewport,
        dialog: None,
        accepted_alert: None,
        tabs: tabs(&targets, owned),
        current: Some(TabKey::new(current)),
        unreadable_frames: 0,
        nodes: placed.into_iter().map(|p| p.node).collect(),
    })
}

/// The run-owned tabs among the targets, in the browser's order.
fn tabs(targets: &[TargetInfo], owned: &OwnedTabs) -> Vec<PageTab> {
    targets
        .iter()
        .filter(|t| t.kind == "page" && owned.owns(&t.target_id))
        .map(|t| PageTab {
            key: TabKey::new(t.target_id.as_str()),
            title: t.title.as_str().into(),
            url: t.url.as_str().into(),
            opened_by_run: owned.opened_by_run(&t.target_id),
        })
        .collect()
}

/// The run-owned tabs in a `Target.getTargets` reply.
#[must_use]
pub fn owned_tabs(targets: &Value, owned: &OwnedTabs) -> Vec<PageTab> {
    let infos: Vec<TargetInfo> = targets
        .get("targetInfos")
        .cloned()
        .and_then(|t| serde_json::from_value(t).ok())
        .unwrap_or_default();
    tabs(&infos, owned)
}

/// Every frame's current loader id, from a frame tree.
#[must_use]
pub fn frame_loaders(frame_tree: &Value) -> Vec<(String, String)> {
    fn walk(node: &Value, out: &mut Vec<(String, String)>) {
        if let Some(frame) = node.get("frame") {
            let id = frame.get("id").and_then(Value::as_str).unwrap_or_default();
            let loader = frame
                .get("loaderId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            out.push((id.to_owned(), loader.to_owned()));
        }
        for child in node
            .get("childFrames")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    if let Some(tree) = frame_tree.get("frameTree") {
        walk(tree, &mut out);
    }
    out
}

/// The ids of the main frame's same-process child frames, from a frame tree.
#[must_use]
pub fn child_frames(frame_tree: &Value) -> Vec<String> {
    frame_loaders(frame_tree)
        .into_iter()
        .skip(1)
        .map(|(id, _)| id)
        .collect()
}

fn read_frame(
    tree: &AxTree,
    frame: &str,
    document: usize,
    layout: &Layout<'_>,
    placed: &mut Vec<Placed>,
) {
    let walk = Walk::new(tree);
    let Some(root) = walk.root(tree) else {
        return;
    };
    // The frame's nodes sort under the iframe that holds it.
    let base: Vec<(i64, usize)> = root
        .backend
        .and_then(|b| layout.by_backend.get(&b))
        .and_then(|dom| layout.iframe_of(dom))
        .map_or_else(Vec::new, |iframe| sort_path(layout, &iframe));

    let mut ancestors: Vec<Ancestor> = Vec::new();
    let mut heading: Option<(usize, Box<str>)> = None;
    let mut position = 0usize;
    // (node, depth): depth is how many ancestors it has.
    let mut stack: Vec<(&AxNode, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        ancestors.truncate(depth);
        position += 1;
        let role_name = role(node);
        let mut name = text(node.name.as_ref());

        if !node.ignored
            && let Some(p) = consider(
                node,
                &walk,
                &ancestors,
                frame,
                document,
                layout,
                &base,
                heading.as_ref(),
            )
        {
            // A name filled in from the node's words counts as its name for
            // the text below it, which is then not repeated.
            name.clone_from(&p.node.name);
            placed.push(p);
        }
        if role_name == "heading" && !name.is_empty() {
            heading = Some((position, name.clone()));
        }
        let dialog = (DIALOGS.contains(&role_name) && !node.ignored).then(|| {
            let own = node
                .backend
                .and_then(|b| layout.by_backend.get(&b))
                .map_or((0, usize::MAX), |dom| {
                    (layout.paint(dom), layout.order(dom))
                });
            let mut path = enclosing_dialog(&ancestors).unwrap_or_else(|| base.clone());
            path.push(own);
            path
        });
        ancestors.push(Ancestor {
            role: role_name.into(),
            name,
            editable: states(node).editable,
            position,
            dialog,
        });
        let mut children: Vec<&AxNode> = walk.children(node).collect();
        children.reverse();
        stack.extend(children.into_iter().map(|c| (c, depth + 1)));
    }
}

fn enclosing_dialog(ancestors: &[Ancestor]) -> Option<Vec<(i64, usize)>> {
    ancestors.iter().rev().find_map(|a| a.dialog.clone())
}

/// The sort path of a node in the snapshot: its iframes' and then its own.
fn sort_path(layout: &Layout<'_>, dom: &Dom) -> Vec<(i64, usize)> {
    let mut path = Vec::new();
    let mut at = Some(dom.clone());
    for _ in 0..16 {
        let Some(dom) = at else { break };
        path.push((layout.paint(&dom), layout.order(&dom)));
        at = layout.iframe_of(&dom);
    }
    path.reverse();
    path
}

#[allow(clippy::too_many_arguments)]
fn consider(
    node: &AxNode,
    walk: &Walk<'_>,
    ancestors: &[Ancestor],
    frame: &str,
    document: usize,
    layout: &Layout<'_>,
    base: &[(i64, usize)],
    heading: Option<&(usize, Box<str>)>,
) -> Option<Placed> {
    let role_name = role(node);
    if DROPPED_ROLES.contains(&role_name) {
        return None;
    }
    let backend = node.backend?;
    let dom = layout.by_backend.get(&backend)?;
    let clickable = layout.clickable(dom);
    let states = states(node);
    let mut name = text(node.name.as_ref());

    if role_name == "StaticText" {
        if name.is_empty()
            || ancestors
                .iter()
                .any(|a| a.editable || a.name.contains(&*name))
        {
            return None;
        }
    } else if name.is_empty() && clickable && !states.editable {
        name = walk.words(node).into();
    }
    if name.is_empty() && !clickable && !states.focusable && !KEPT_ROLES.contains(&role_name) {
        return None;
    }

    let native_select = layout.node_name(dom).eq_ignore_ascii_case("SELECT");
    let protected = layout.node_name(dom).eq_ignore_ascii_case("INPUT")
        && layout
            .attribute(dom, "type")
            .is_some_and(|t| t.eq_ignore_ascii_case("password"));
    let value = Some(text(node.value.as_ref())).filter(|v| !v.is_empty() && !protected);
    let description = Some(text(node.description.as_ref())).filter(|d| !d.is_empty());

    let landmark = ancestors
        .iter()
        .rev()
        .find(|a| LANDMARKS.contains(&&*a.role) && !a.name.is_empty());
    let context = match (heading, landmark) {
        (Some((at, h)), Some(l)) if *at > l.position => Some(h.clone()),
        (_, Some(l)) => Some(l.name.clone()),
        (Some((_, h)), None) => Some(h.clone()),
        (None, None) => None,
    };

    let own = (layout.paint(dom), layout.order(dom));
    let mut sort = enclosing_dialog(ancestors).unwrap_or_else(|| base.to_vec());
    sort.push(own);

    Some(Placed {
        node: PageNode {
            frame: frame.into(),
            role: role_name.into(),
            name,
            value,
            protected,
            description,
            states,
            clickable,
            options: if native_select {
                walk.options(node)
            } else {
                Vec::new()
            },
            context,
            bounds: layout.bounds(dom),
            key: KeyParts { backend, document }.key(),
        },
        sort,
    })
}
