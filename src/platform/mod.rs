//! What actually differs between one mobile platform and another.
//!
//! Two things, and only two: how a UI hierarchy is spelled, and which system
//! gestures exist. Everything else — snapshots, catalogs, judgments, the shape
//! of a step — is shared, so adding a platform means implementing [`Platform`]
//! and nothing more.
//!
//! The capability half matters more than it looks. Android has a system Back
//! button and iOS does not, so a catalog built for iOS must never offer one:
//! a model cannot be blamed for choosing an action it was told was available.

use crate::act::Operation;
use crate::snapshot::{Snapshot, TooManyElements};
use core::fmt;

#[cfg(feature = "android")]
mod android;
#[cfg(feature = "ios")]
mod ios;

#[cfg(feature = "android")]
#[cfg_attr(docsrs, doc(cfg(feature = "android")))]
pub use android::Android;
#[cfg(feature = "ios")]
#[cfg_attr(docsrs, doc(cfg(feature = "ios")))]
pub use ios::Ios;

/// A UI hierarchy could not be read.
///
/// Deliberately free of any parser type: which XML library reads a hierarchy is
/// an implementation detail, and leaking it would pin the public API to it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HierarchyError {
    /// The hierarchy was not well-formed.
    Malformed {
        /// What the reader objected to.
        detail: Box<str>,
    },
    /// The screen held more actionable elements than can be addressed.
    TooMany(TooManyElements),
}

impl fmt::Display for HierarchyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { detail } => write!(f, "malformed UI hierarchy: {detail}"),
            Self::TooMany(inner) => write!(f, "{inner}"),
        }
    }
}

impl core::error::Error for HierarchyError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Malformed { .. } => None,
            Self::TooMany(inner) => Some(inner),
        }
    }
}

impl From<TooManyElements> for HierarchyError {
    fn from(inner: TooManyElements) -> Self {
        Self::TooMany(inner)
    }
}

/// A mobile platform the worker can drive.
///
/// Object-safe on purpose: a tool that handles whichever device is plugged in
/// holds a `&dyn Platform` chosen at runtime.
pub trait Platform: Send + Sync {
    /// How this platform is named in logs and prompts.
    fn name(&self) -> &'static str;

    /// The operations this platform supports.
    ///
    /// A catalog offers exactly these, so a platform that lacks an operation
    /// never presents it as a choice. iOS has no system back gesture, for
    /// instance, and offering one would invite a decision the device cannot
    /// carry out.
    fn operations(&self) -> &'static [Operation];

    /// Read a platform-native UI hierarchy into an observation.
    ///
    /// # Errors
    /// Returns [`HierarchyError`] when the hierarchy is unreadable, or holds
    /// more actionable elements than a single judgment can address.
    fn parse_hierarchy(&self, raw: &str) -> Result<Snapshot, HierarchyError>;
}
