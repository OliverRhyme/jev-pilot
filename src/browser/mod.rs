//! Driving a web browser.
//!
//! The engine seam and its first engine (the Chrome DevTools Protocol), and the
//! device that carries out the pilot's commands through them.

pub mod bridge;
pub mod cdp;
pub mod device;
pub mod discover;
pub mod engine;
pub mod input;
pub mod launch;
pub mod read;
pub mod transport;
