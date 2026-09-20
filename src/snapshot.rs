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

    /// Whether this screen offers anything to act on.
    ///
    /// A screen caught between two others parses to nothing: the old view is
    /// gone and the new one has not been laid out. That is not a decision
    /// anyone can make — there is no row to choose, and no operation that
    /// helps — so a reader looks again rather than handing it to a model.
    #[must_use]
    pub fn worth_acting_on(&self) -> bool {
        !self.elements.is_empty()
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
