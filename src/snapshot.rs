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

/// One observation of a screen.
#[derive(Debug)]
pub struct Snapshot {
    generation: Generation,
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
            elements: elements.into_boxed_slice(),
        })
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
