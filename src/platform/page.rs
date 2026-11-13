//! What a browser engine reports about a page, whichever engine it is.
//!
//! The page read is the contract between an engine and the Browser platform:
//! the platform and every engine depend on it, and not on each other. It
//! serialises to JSON, and that JSON is what platform fixtures hold.
//!
//! Boxes are in CSS pixels, relative to the viewport. They stay inside the
//! snapshot the platform builds from this, as Android bounds do.

use crate::snapshot::{Dialog, NodeKey, TabKey};
use serde::{Deserialize, Serialize};

/// The size of the visible part of the page, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Where a node sits in the viewport, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageBox {
    /// Distance from the left edge of the viewport.
    pub x: f64,
    /// Distance from the top edge of the viewport.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// A browser tab the run owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTab {
    /// The engine's name for the tab.
    pub key: TabKey,
    /// The title the tab shows.
    pub title: Box<str>,
    /// The address the tab shows.
    pub url: Box<str>,
    /// Whether the run opened this tab, as against starting on it.
    #[serde(default)]
    pub opened_by_run: bool,
}

/// Whether a checkable node is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Checked {
    /// Not checked.
    Unchecked,
    /// Checked.
    Checked,
    /// Some of what it controls is checked and some is not.
    Mixed,
}

/// What state a node is in.
///
/// A flat record of independent flags, as the accessibility tree reports them;
/// they are not facets of one state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeStates {
    /// Whether it can take the keyboard focus.
    pub focusable: bool,
    /// Whether text can be typed into it.
    pub editable: bool,
    /// Whether it is present but cannot be used.
    pub disabled: bool,
    /// Whether it is checked, for the nodes that can be.
    pub checked: Option<Checked>,
    /// Whether it is expanded, for the nodes that can be.
    pub expanded: Option<bool>,
    /// Whether it is the selected one of its group.
    pub selected: bool,
    /// Whether it holds the keyboard focus now.
    pub focused: bool,
    /// Whether it is a modal that blocks the page behind it.
    pub modal: bool,
}

/// One node of the page, from the accessibility tree joined with the layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageNode {
    /// Which frame the node is in.
    pub frame: Box<str>,
    /// What the node is, in the accessibility tree's words.
    pub role: Box<str>,
    /// What a screen reader would call it.
    #[serde(default)]
    pub name: Box<str>,
    /// What it holds, for a field. Never set for a password field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Box<str>>,
    /// Whether it is a password field, whose value is never read.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub protected: bool,
    /// Supporting text beyond its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Box<str>>,
    /// What state it is in.
    #[serde(default)]
    pub states: NodeStates,
    /// Whether the layout reports it as something a click does anything to.
    #[serde(default)]
    pub clickable: bool,
    /// The choices of a native drop-down, by the words they show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Box<str>>,
    /// The nearest heading or landmark it sits under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Box<str>>,
    /// Where it sits in the viewport, when it has a box at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<PageBox>,
    /// The engine's own handle for it.
    pub key: NodeKey,
}

/// Everything an engine read from the driven tab at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageRead {
    /// The address of the page.
    pub url: Box<str>,
    /// The title of the page.
    pub title: Box<str>,
    /// The visible part of the page.
    pub viewport: Viewport,
    /// A JavaScript dialog that is open over the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<Dialog>,
    /// The message of an alert that was accepted since the last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_alert: Option<Box<str>>,
    /// The tabs the run owns, the driven one among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<PageTab>,
    /// Which of `tabs` this read came from. By key, because two tabs can
    /// show the same address and title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<TabKey>,
    /// The page's nodes, in paint order: later ones are drawn over earlier ones.
    #[serde(default)]
    pub nodes: Vec<PageNode>,
}
