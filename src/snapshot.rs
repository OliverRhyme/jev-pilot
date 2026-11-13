//! One observation of a screen, and references that cannot outlive it.
//!
//! Every snapshot is stamped with a fresh [`Generation`]. An [`ElementRef`]
//! carries the stamp of the snapshot that produced it, so a reference taken
//! before an action cannot be resolved against the screen that follows it. The
//! UI may look identical and still have rebound every row underneath; a stale
//! tap is how a loop ends up acting on unrelated application state.
//!
//! Pixel geometry lives here and travels no further. A device adapter needs a
//! point to tap; the model never sees one, so it cannot invent one.
//!
//! Nothing in this module is platform-specific. Producing [`Element`]s from a
//! particular UI hierarchy is the job of [`crate::platform`].

use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use serde::{Deserialize, Serialize};

/// Identifies which observation a reference belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Generation(u32);

impl Generation {
    fn next() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// A point on the screen, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    /// Distance from the left edge.
    pub x: i32,
    /// Distance from the top edge.
    pub y: i32,
}

/// Where an element sits on screen, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounds {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge.
    pub right: i32,
    /// Bottom edge.
    pub bottom: i32,
}

impl Bounds {
    /// Build bounds from an origin and a size, as iOS hierarchies report them.
    #[must_use]
    pub const fn from_origin_size(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            left: x,
            top: y,
            right: x.saturating_add(width),
            bottom: y.saturating_add(height),
        }
    }

    /// The point an adapter taps to hit this element.
    ///
    /// `midpoint` rather than `(a + b) / 2`: bounds come from a hierarchy the
    /// worker did not author, and the naive average overflows inside `i32`.
    #[must_use]
    pub const fn center(self) -> Point {
        Point {
            x: i32::midpoint(self.left, self.right),
            y: i32::midpoint(self.top, self.bottom),
        }
    }
}

/// A handle to one element of one snapshot.
///
/// Deliberately carries no geometry: holding a reference lets you ask the
/// snapshot that issued it for a tap target, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementRef {
    generation: Generation,
    index: u8,
}

/// An engine's own handle for one row of a page.
///
/// Opaque on purpose, and for the same reason as [`TabKey`]: it is kept in the
/// snapshot so a device can find the element again, never described to the
/// model, and never compared by anything but the engine that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeKey(Box<str>);

impl NodeKey {
    /// Wrap an engine's node identifier.
    #[must_use]
    pub fn new(key: impl Into<Box<str>>) -> Self {
        Self(key.into())
    }

    /// The identifier as the engine spelled it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An engine's own name for a browser tab.
///
/// Opaque on purpose. It is kept in the snapshot so a device can find the tab
/// again, never described to the model, and never compared by anything but the
/// engine that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabKey(Box<str>);

impl TabKey {
    /// Wrap an engine's tab identifier.
    #[must_use]
    pub fn new(key: impl Into<Box<str>>) -> Self {
        Self(key.into())
    }

    /// The identifier as the engine spelled it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A browser tab the run owns, other than the one being driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    /// The engine's name for the tab.
    pub key: TabKey,
    /// The title the tab shows.
    pub title: Box<str>,
    /// The address the tab shows.
    pub url: Box<str>,
}

/// Which kind of JavaScript dialog is open.
///
/// An alert is not here: it has one button, so it is accepted as it appears
/// and only its message is carried on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DialogKind {
    /// A question answered by accepting or dismissing.
    Confirm,
    /// A question answered with words, or dismissed.
    Prompt,
    /// The "leave this site?" question asked as a page is closed or left.
    BeforeUnload,
}

/// A JavaScript dialog that is open and blocking its page.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dialog {
    /// Which kind of dialog it is.
    pub kind: DialogKind,
    /// What it says.
    pub message: Box<str>,
    /// The words a prompt would hold if accepted untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_prompt: Option<Box<str>>,
}

/// A handle to one tab of one snapshot.
///
/// Stamped with the snapshot's [`Generation`] exactly like an [`ElementRef`], so
/// a tab chosen from an old read cannot be acted on once the list has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabRef {
    generation: Generation,
    index: u8,
}

/// One thing on screen that a person could act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// What the element calls itself.
    pub label: Box<str>,
    /// Supporting text shown alongside the title, when there is any.
    pub detail: Option<Box<str>>,
    /// Whether text can be typed into it.
    pub editable: bool,
    /// Where it sits on screen.
    pub bounds: Bounds,
}

impl Element {
    /// How this element should be described to a model choosing among rows.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{} — {}", self.label, detail),
            None => self.label.to_string(),
        }
    }
}

/// The element is there, but nothing of it can be touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obscured;

impl fmt::Display for Obscured {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the element is completely covered by what is drawn over it"
        )
    }
}

impl core::error::Error for Obscured {}

/// A reference that does not belong to the snapshot it was offered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleRef {
    /// The snapshot the reference was offered to.
    pub expected: Generation,
    /// The snapshot that issued the reference.
    pub found: Generation,
}

impl fmt::Display for StaleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "reference belongs to snapshot {:?} but was resolved against {:?}",
            self.found, self.expected
        )
    }
}

impl core::error::Error for StaleRef {}

/// The screen holds more actionable elements than one judgment can address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyElements {
    /// How many were found.
    pub found: usize,
    /// The most that can be addressed.
    pub limit: usize,
}

impl fmt::Display for TooManyElements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} actionable elements exceeds the {} that can be addressed",
            self.found, self.limit
        )
    }
}

impl core::error::Error for TooManyElements {}

/// The most elements a single snapshot can address.
///
/// Bounded by [`crate::judgment::MAX_OPTIONS`]: an element that cannot be
/// offered as a choice cannot be acted on, so addressing it would be a lie.
///
/// Five slots are held back for the actions a catalog adds beyond the screen
/// itself — at most three system gestures plus two terminal verdicts. Reserving
/// them here means a snapshot that parsed can always be turned into a catalog,
/// so building one cannot fail.
pub const MAX_ELEMENTS: usize = crate::judgment::MAX_OPTIONS - RESERVED_ACTIONS;

/// Catalog slots not drawn from the screen: system gestures plus the verdicts.
pub(crate) const RESERVED_ACTIONS: usize = 5;

/// A screen that parses must always fit in one Choice. Checked at compile time
/// rather than tested, so changing either limit in a way that breaks the other
/// fails the build instead of a test run.
const _: () = assert!(MAX_ELEMENTS + RESERVED_ACTIONS <= crate::judgment::MAX_OPTIONS);

/// Why an element could not be turned into a point to tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TapError {
    /// The reference came from a different observation.
    Stale(StaleRef),
    /// Nothing of the element is left uncovered.
    Obscured(Obscured),
}

impl fmt::Display for TapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale(inner) => inner.fmt(f),
            Self::Obscured(inner) => inner.fmt(f),
        }
    }
}

impl core::error::Error for TapError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Stale(inner) => Some(inner),
            Self::Obscured(inner) => Some(inner),
        }
    }
}

/// Whether `over` hides part of the band between `top` and `bottom`.
const fn over_covers(over: Bounds, top: i32, bottom: i32) -> bool {
    over.top < bottom && over.bottom > top
}

/// One observation of a screen.
#[derive(Debug)]
pub struct Snapshot {
    generation: Generation,
    keyboard_open: bool,
    app: Option<Box<str>>,
    notices: Box<[Box<str>]>,
    unavailable: Box<[Box<str>]>,
    quiet_for_ms: Option<u32>,
    url: Option<Box<str>>,
    tabs: Box<[Tab]>,
    dialog: Option<Dialog>,
    // Both run alongside `elements`, row for row, and are empty on a screen
    // whose reader has no such thing to say.
    node_keys: Box<[Option<NodeKey>]>,
    field_only: Box<[bool]>,
    elements: Box<[Element]>,
}

impl Snapshot {
    /// Take an observation of the given elements, in the order they appear.
    ///
    /// # Errors
    /// Returns [`TooManyElements`] when the screen holds more than
    /// [`MAX_ELEMENTS`] actionable elements.
    pub fn new(elements: Vec<Element>) -> Result<Self, TooManyElements> {
        if elements.len() > MAX_ELEMENTS {
            return Err(TooManyElements {
                found: elements.len(),
                limit: MAX_ELEMENTS,
            });
        }
        Ok(Self {
            generation: Generation::next(),
            keyboard_open: false,
            app: None,
            notices: Box::default(),
            unavailable: Box::default(),
            quiet_for_ms: None,
            url: None,
            tabs: Box::default(),
            dialog: None,
            node_keys: Box::default(),
            field_only: Box::default(),
            elements: elements.into_boxed_slice(),
        })
    }

    /// Note that a soft keyboard is covering part of this screen.
    ///
    /// Worth carrying rather than inferring: it is the difference between a
    /// form that is being filled and one that is not, and the alternative is
    /// for a reader to guess it from a pile of single-letter rows — which is
    /// exactly what this crate stops offering.
    #[must_use]
    pub fn with_keyboard_open(mut self, open: bool) -> Self {
        self.keyboard_open = open;
        self
    }

    /// Whether a soft keyboard is covering part of this screen.
    #[must_use]
    pub const fn keyboard_open(&self) -> bool {
        self.keyboard_open
    }

    /// Note which application this screen belongs to.
    #[must_use]
    pub fn in_app(mut self, app: Option<Box<str>>) -> Self {
        self.app = app;
        self
    }

    /// Note what the screen says, beyond what it offers to act on.
    #[must_use]
    pub fn saying(mut self, notices: Vec<Box<str>>) -> Self {
        self.notices = notices.into_boxed_slice();
        self
    }

    /// What the screen says, in the order it says it.
    ///
    /// Static text: headings, captions, totals, progress through a step. None
    /// of it can be acted on, which is exactly why it is kept apart from the
    /// rows — offering it as a target would hand the model choices that do
    /// nothing. It is here because a catalog of buttons alone is no account of
    /// the state those buttons act on, and "is the goal met?" is a question
    /// about the state.
    pub fn notices(&self) -> impl Iterator<Item = &str> {
        self.notices.iter().map(|notice| &**notice)
    }

    /// How many rows this screen offers.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.elements.len()
    }

    /// Note how long the reader had seen the screen unchanged.
    #[must_use]
    pub const fn quiet_for(mut self, ms: Option<u32>) -> Self {
        self.quiet_for_ms = ms;
        self
    }

    /// How long the screen had been quiet when it was read, in milliseconds.
    ///
    /// `None` from a reader that cannot say, which is not the same as zero: a
    /// `uiautomator` dump carries no such thing, and treating its silence as
    /// "just changed" would make every shell-read screen look mid-transition.
    ///
    /// A screen still being drawn emits accessibility events and a settled
    /// one does not, so this is the device's own answer to whether it has
    /// finished — exactly what polling the tree from outside cannot work out.
    #[must_use]
    pub const fn quiet_for_ms(&self) -> Option<u32> {
        self.quiet_for_ms
    }

    /// Note the controls that are present but cannot be used.
    #[must_use]
    pub fn offering_but_refusing(mut self, controls: Vec<Box<str>>) -> Self {
        self.unavailable = controls.into_boxed_slice();
        self
    }

    /// Controls the screen shows and will not let anything act on.
    ///
    /// A greyed-out button keeps its place on screen and drops the flags that
    /// would make it a row, so it is absent from the catalog rather than
    /// offered and refused. That absence reads as a screen with no way on,
    /// when in truth there is a way on and something has to happen first.
    ///
    /// Apart from [`Self::notices`] deliberately: this is a fact about what
    /// can be done, not a line of prose, and among the screen's words the
    /// only thing marking it out is how it happens to be phrased.
    pub fn unavailable(&self) -> impl Iterator<Item = &str> {
        self.unavailable.iter().map(|control| &**control)
    }

    /// Note the address this screen was read from.
    #[must_use]
    pub fn at_url(mut self, url: Option<Box<str>>) -> Self {
        self.url = url;
        self
    }

    /// The address of the page this screen was read from, when it has one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Note the other tabs the run owns and could switch to.
    ///
    /// At most 256 are kept, which is what a [`TabRef`] can address.
    #[must_use]
    pub fn with_tabs(mut self, tabs: Vec<Tab>) -> Self {
        let mut tabs = tabs;
        tabs.truncate(usize::from(u8::MAX) + 1);
        self.tabs = tabs.into_boxed_slice();
        self
    }

    /// Note the reader's own handle for each row, in the rows' order.
    ///
    /// Rows beyond the end of `keys` have none.
    #[must_use]
    pub fn with_node_keys(mut self, mut keys: Vec<Option<NodeKey>>) -> Self {
        keys.truncate(self.elements.len());
        self.node_keys = keys.into_boxed_slice();
        self
    }

    /// Note which rows are fields to type into and never targets to tap, in
    /// the rows' order.
    ///
    /// A native drop-down is the case: tapping it opens a pop-up nothing can
    /// read, so its words are the option to choose and nothing else is
    /// offered. Rows beyond the end of `marks` are not field-only.
    #[must_use]
    pub fn with_field_only(mut self, mut marks: Vec<bool>) -> Self {
        marks.truncate(self.elements.len());
        self.field_only = marks.into_boxed_slice();
        self
    }

    /// The reader's own handle for the row, when it gave one.
    ///
    /// # Errors
    /// Returns [`StaleRef`] when the reference was issued by a different
    /// observation.
    pub fn node_key(&self, handle: ElementRef) -> Result<Option<&NodeKey>, StaleRef> {
        self.resolve(handle)?;
        Ok(self
            .node_keys
            .get(usize::from(handle.index))
            .and_then(Option::as_ref))
    }

    /// Whether the row is a field only, and never a target to tap.
    ///
    /// # Errors
    /// Returns [`StaleRef`] when the reference was issued by a different
    /// observation.
    pub fn is_field_only(&self, handle: ElementRef) -> Result<bool, StaleRef> {
        self.resolve(handle)?;
        Ok(self
            .field_only
            .get(usize::from(handle.index))
            .copied()
            .unwrap_or(false))
    }

    /// Note the dialog that is open over this screen, if there is one.
    #[must_use]
    pub fn with_dialog(mut self, dialog: Option<Dialog>) -> Self {
        self.dialog = dialog;
        self
    }

    /// The dialog that is open over this screen, if there is one.
    #[must_use]
    pub const fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Every other tab the run owns, paired with the reference that addresses it.
    pub fn tabs(&self) -> impl Iterator<Item = (TabRef, &Tab)> {
        self.tabs.iter().enumerate().map(|(index, tab)| {
            let handle = TabRef {
                generation: self.generation,
                // The length was bounded by `with_tabs`.
                index: u8::try_from(index).unwrap_or(u8::MAX),
            };
            (handle, tab)
        })
    }

    /// Exchange a tab reference for the tab it names.
    ///
    /// # Errors
    /// Returns [`StaleRef`] when the reference was issued by a different
    /// observation, which means the tab list has since been read again.
    #[must_use = "a stale reference must not be ignored"]
    pub fn resolve_tab(&self, handle: TabRef) -> Result<&Tab, StaleRef> {
        if handle.generation == self.generation
            && let Some(tab) = self.tabs.get(usize::from(handle.index))
        {
            return Ok(tab);
        }
        Err(StaleRef {
            expected: self.generation,
            found: handle.generation,
        })
    }

    /// Which application this screen belongs to, when the reader could say.
    ///
    /// The identifier the platform uses — a package name on Android, a bundle
    /// identifier on iOS — not anything a person would read. It exists so a
    /// run can tell that it is no longer where it started: a launcher, a
    /// browser opened by a link, or another app entirely presents rows that
    /// look exactly as legitimate as the ones it was asked about.
    #[must_use]
    pub fn app(&self) -> Option<&str> {
        self.app.as_deref()
    }

    /// Which screen this is, as against what state it is in.
    ///
    /// The rows it offers, which of them can be reached, the words it says, the
    /// controls it refuses and whether a keyboard is up — and deliberately not
    /// where any of it sits, nor what has been typed into its fields.
    /// A form re-entered after a detour is the same place with its rows at
    /// slightly different offsets, and a spinner moving a pixel is the same
    /// place too.
    ///
    /// Apart from [`Self::fingerprint`] because they answer different
    /// questions. Settling asks "did anything change?", where a pixel counts.
    /// Revisiting asks "have I been here before?", where it must not —
    /// measured on three laps of one transfer flow, whose identical screens
    /// went unrecognised because the geometry had shifted underneath.
    #[must_use]
    pub fn place(&self) -> u64 {
        use core::hash::{Hash as _, Hasher as _};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.keyboard_open.hash(&mut hasher);
        self.notices.hash(&mut hasher);
        self.unavailable.hash(&mut hasher);
        self.tab_identity().hash(&mut hasher);
        self.dialog.hash(&mut hasher);
        for (handle, element) in self.refs() {
            // A field's words are what was typed into it, and a form is the
            // same place whatever it holds. Counted, a form that reformats its
            // amount from "50" to "50.00" while a quote is on the way looked
            // like somewhere new, and its Continue was pressed three times.
            if !element.editable {
                element.label.hash(&mut hasher);
            }
            element.detail.hash(&mut hasher);
            element.editable.hash(&mut hasher);
            // Not where a row is, but whether it can be reached. A web page
            // is read whole, so a scroll moves every row and changes none of
            // their words; which rows it leaves within reach is the only
            // sign that it went anywhere.
            self.tap_point(handle).is_ok().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// What is on this screen, as one number.
    ///
    /// Two reads of an unchanged screen give the same value; a screen that has
    /// moved on gives a different one. That is what lets a run tell the result
    /// of its action from the screen it acted on — and a loop that cannot tell
    /// those apart re-chooses the same action against what looks like an
    /// unchanged screen.
    ///
    /// Deliberately not the [`Generation`]: every read makes a new one, so
    /// comparing those would call every screen different.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        use core::hash::{Hash as _, Hasher as _};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // The keyboard changes what can be done without changing the rows
        // behind it, so it belongs in the identity of the screen.
        self.keyboard_open.hash(&mut hasher);
        // A screen whose only change is in its words has still changed. A PIN
        // pad is identical after every digit but for the count it reports, and
        // a run entering one is otherwise stopped by the guard against
        // repeating itself.
        //
        // Hashed as slices, not as a run of strings: a slice writes its own
        // length first, so a control moving between these two lists is a
        // different screen. Fed in one after the other they are the same
        // stream either way — and "Continue" moving from unavailable to
        // available is exactly the change worth noticing, since it is the
        // moment a form has a way on.
        self.notices.hash(&mut hasher);
        self.unavailable.hash(&mut hasher);
        // A tab appearing, or a dialog opening, changes what can be done
        // without touching a row.
        self.tab_identity().hash(&mut hasher);
        self.dialog.hash(&mut hasher);
        for element in &self.elements {
            element.label.hash(&mut hasher);
            element.detail.hash(&mut hasher);
            element.editable.hash(&mut hasher);
            element.bounds.left.hash(&mut hasher);
            element.bounds.top.hash(&mut hasher);
            element.bounds.right.hash(&mut hasher);
            element.bounds.bottom.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// What the other tabs are called and where they are, without the engine's
    /// own keys, which say nothing about what the tab shows.
    fn tab_identity(&self) -> Vec<(&str, &str)> {
        self.tabs
            .iter()
            .map(|tab| (&*tab.title, &*tab.url))
            .collect()
    }

    /// Whether this screen offers anything to act on.
    ///
    /// A screen caught between two others parses to nothing: the old view is
    /// gone and the new one has not been laid out. That is not a decision
    /// anyone can make — there is no row to choose, and no operation that
    /// helps — so a reader looks again rather than handing it to a model.
    ///
    /// An open dialog is the exception: the page behind it cannot be used, and
    /// the dialog's own answer is the decision to make, rows or no rows.
    #[must_use]
    pub fn worth_acting_on(&self) -> bool {
        !self.elements.is_empty() || self.dialog.is_some()
    }

    /// Every element, paired with the reference that addresses it.
    pub fn refs(&self) -> impl Iterator<Item = (ElementRef, &Element)> {
        self.elements.iter().enumerate().map(|(index, element)| {
            let handle = ElementRef {
                generation: self.generation,
                // The length was bounded by `new`.
                index: u8::try_from(index).unwrap_or(u8::MAX),
            };
            (handle, element)
        })
    }

    /// Exchange a reference for the element it names.
    ///
    /// # Errors
    /// Returns [`StaleRef`] when the reference was issued by a different
    /// observation, which means the screen has since been replaced.
    #[must_use = "a stale reference must not be ignored"]
    pub fn resolve(&self, handle: ElementRef) -> Result<&Element, StaleRef> {
        if handle.generation == self.generation
            && let Some(element) = self.elements.get(usize::from(handle.index))
        {
            return Ok(element);
        }
        Err(StaleRef {
            expected: self.generation,
            found: handle.generation,
        })
    }

    /// Where to tap to hit this element.
    ///
    /// Not simply the middle of its box. A hierarchy reports an element at its
    /// full size even when most of it lies behind something drawn later, so the
    /// middle of a list row that runs under a navigation bar is a point inside
    /// the navigation bar — and a tap there opens whatever is on top.
    ///
    /// The box is narrowed to the tallest band no later element covers, and the
    /// middle of that band is used instead.
    ///
    /// # Errors
    /// Returns [`StaleRef`] when the reference came from another observation,
    /// and [`Obscured`] when nothing of the element is left to touch.
    pub fn tap_point(&self, handle: ElementRef) -> Result<Point, TapError> {
        let target = self.resolve(handle).map_err(TapError::Stale)?.bounds;
        let x = i32::midpoint(target.left, target.right);

        // Only what is drawn after this element can cover it, and only where it
        // actually crosses the column being tapped.
        let (mut top, mut bottom) = (target.top, target.bottom);
        for over in self
            .elements
            .iter()
            .skip(usize::from(handle.index) + 1)
            .map(|element| element.bounds)
            .filter(|over| over.left <= x && x < over.right)
        {
            if over_covers(over, top, bottom) {
                if over.top <= top {
                    top = top.max(over.bottom);
                } else {
                    bottom = bottom.min(over.top);
                }
            }
        }

        if bottom <= top {
            return Err(TapError::Obscured(Obscured));
        }
        Ok(Point {
            x,
            y: i32::midpoint(top, bottom),
        })
    }

    /// Whether the screen offered nothing to act on.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// How many actionable elements the screen offered.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.elements.len()
    }
}
