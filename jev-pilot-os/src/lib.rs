//! The Windows calls jev-pilot needs that no safe crate offers, behind safe
//! functions.
//!
//! jev-pilot itself forbids `unsafe`. Asking Windows which image another
//! process runs (`QueryFullProcessImageNameW`), whose it is, who signed that
//! image (`WinVerifyTrust`), and cutting a wait on a named pipe short
//! (`CancelIoEx`) have no safe Rust API, so those calls live here, each in a
//! function that allows `unsafe` for itself alone.
//!
//! On every other target this crate is empty.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    KnownFolder, Process, Signer, authenticode_signer, current_session, current_user_sid,
    final_path, hang_up_pipe, known_folder, parent_process_id, process_image,
};
