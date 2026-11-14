//! Driving a web browser.
//!
//! The engine seam and its first engine (the Chrome DevTools Protocol), the
//! device that carries out the pilot's commands through them, and the bridge
//! that lets the engine drive tabs through Chrome's jev-pilot extension
//! instead of Chrome's own websocket.

pub mod bridge;
pub mod cdp;
pub mod device;
pub mod discover;
pub mod engine;
pub mod extension;
pub mod host;
pub mod input;
pub mod install;
pub mod launch;
pub mod packaged;
pub mod read;
pub mod transport;
