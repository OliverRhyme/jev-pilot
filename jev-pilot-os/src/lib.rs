//! The Windows calls jev-pilot needs that no safe crate offers, behind safe
//! functions.
//!
//! jev-pilot itself forbids `unsafe`. Asking Windows which image another
//! process runs (`QueryFullProcessImageNameW`), whose it is, and who signed
//! that image (`WinVerifyTrust`) has no safe Rust API, so those calls live
//! here, each in a function that allows `unsafe` for itself alone.
//!
//! On every other target this crate is empty.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    Process, Signer, authenticode_signer, current_session, current_user_sid, final_path,
    process_image,
};
