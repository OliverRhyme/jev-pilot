//! The boundary between a decision and a device.
//!
//! An [`Act`] names an element; a [`Command`] names a point. Translating one
//! into the other is the only place geometry leaves a [`Snapshot`], and it
//! cannot be done without the snapshot that issued the reference — which is
//! what makes acting on a stale screen a handled error rather than a misfire.

/// Driving an Android device through `adb`.
#[cfg(feature = "android")]
#[cfg_attr(docsrs, doc(cfg(feature = "android")))]
pub mod adb;
pub mod helper;

use crate::act::{Act, Direction, Swipe, SystemAct};
use crate::snapshot::{Point, Snapshot, TapError};

/// A concrete instruction a device adapter can carry out.
///
/// Deliberately **not** `#[non_exhaustive]`, unlike the other growable enums
/// here. Every [`Device`] implementation matches this exhaustively, and that is
/// the point: adding a gesture should fail to compile in every adapter until
/// each one says what it does about it. A wildcard arm would let a new gesture
/// be silently ignored by an adapter that never learned it, which reads to a
/// running loop as an action that worked. Adding a variant is a major release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Tap at a point.
    Tap(Point),
    /// Type text into whatever currently has focus.
    TypeText(Box<str>),
    /// Perform a system gesture.
    System(SystemAct),
    /// Scroll the screen one view in this direction.
    ///
    /// Carries no geometry: how far a view is, and where it is safe to start
    /// the gesture, is a property of the device rather than of the decision.
    Scroll(Direction),
    /// Tap twice in quick succession at a point.
    DoubleTap(Point),
    /// Press and hold at a point.
    LongPress(Point),
    /// Drag sideways from a point.
    SwipeFrom {
        /// Where the drag starts.
        from: Point,
        /// Which way it goes.
        direction: Swipe,
    },
    /// Press firmly at a point to preview.
    Peek(Point),
    /// Let the screen finish whatever it is doing, then look again.
    ///
    /// Carries no duration: how long a screen needs is a property of the
    /// device, not of the decision to wait. A device that can watch for
    /// quiescence should do that instead of sleeping.
    Settle,
}

/// Resolve an action against the screen it was chosen from.
///
/// Returns `None` only for a verdict, which ends the run rather than touching
/// the device.
///
/// # Errors
/// Returns [`TapError`] when the action names an element from another
/// observation, meaning the screen has since been replaced, or one that is
/// entirely covered by whatever is drawn over it.
pub fn command_for(act: &Act, snapshot: &Snapshot) -> Result<Option<Command>, TapError> {
    Ok(match act {
        Act::Tap(handle) => Some(Command::Tap(snapshot.tap_point(*handle)?)),
        Act::DoubleTap(handle) => Some(Command::DoubleTap(snapshot.tap_point(*handle)?)),
        Act::LongPress(handle) => Some(Command::LongPress(snapshot.tap_point(*handle)?)),
        Act::SwipeElement { target, direction } => Some(Command::SwipeFrom {
            from: snapshot.tap_point(*target)?,
            direction: *direction,
        }),
        Act::Peek(handle) => Some(Command::Peek(snapshot.tap_point(*handle)?)),
        Act::TypeText { into, text } => {
            // Resolved for its side effect: typing goes to the focused field,
            // but the field must be proven live and reachable before committing.
            snapshot.tap_point(*into)?;
            Some(Command::TypeText(text.clone()))
        }
        Act::System(gesture) => Some(Command::System(*gesture)),
        Act::Scroll(direction) => Some(Command::Scroll(*direction)),
        Act::Wait => Some(Command::Settle),
        Act::Finish(_) => None,
    })
}

/// A device the worker can observe and drive.
///
/// Implemented by an adapter over `adb`, XCUITest, or a fake used in tests.
pub trait Device {
    /// What can go wrong talking to this device.
    type Error: core::error::Error;

    /// Read the current screen.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the device cannot be reached or its UI
    /// hierarchy cannot be read.
    fn observe(&mut self) -> Result<Snapshot, Self::Error>;

    /// Carry out one command.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the device rejects or fails the command.
    fn perform(&mut self, command: &Command) -> Result<(), Self::Error>;
}
