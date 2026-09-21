//! Drive a mobile UI with [TypeSafe Jev] as the decision provider.
//!
//! The loop inverts the usual arrangement. Rather than asking a model what to
//! do and parsing an action out of prose, this crate **enumerates every action
//! the current screen allows** and asks the model to pick one. The model's
//! whole vocabulary is a list of option ids, so it cannot invent a coordinate,
//! name an element that is not on screen, or reference a screen that has since
//! been replaced.
//!
//! # Shape of a step
//!
//! ```text
//! observe -> Snapshot -> Catalog -> StepQuestions -> [Jev] -> StepAnswers
//!                           |                                      |
//!                           +---------- decide ---------------------+
//!                                          |
//!                                     Act -> Command -> device
//! ```
//!
//! # Platforms
//!
//! [`platform::Platform`] abstracts the two things that actually differ between
//! Android and iOS: how a UI hierarchy is spelled, and which system gestures
//! exist. Everything else — snapshots, catalogs, judgments — is shared.
//!
//! [TypeSafe Jev]: https://typesafe.ai/

#![forbid(unsafe_code)]
// docs.rs builds with this set, so items behind a feature say so in the docs.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod act;
pub mod cli;
pub mod client;
pub mod credential;
pub mod desk;
pub mod device;
pub mod judgment;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;
pub mod pilot;
pub mod platform;
pub mod snapshot;
pub mod step;

// The handful of names almost every call site touches. The modules remain the
// canonical path; these save an import list per example without hiding where
// anything lives.
pub use act::{Act, Catalog, Operation};
pub use device::Device;
pub use judgment::Confidence;
pub use pilot::{Ending, Escalate, Judge, Pilot};
pub use platform::Platform;
pub use snapshot::Snapshot;
