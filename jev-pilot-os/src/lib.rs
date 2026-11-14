//! The operating system calls jev-pilot needs that no safe crate offers,
//! behind safe functions.
//!
//! jev-pilot itself forbids `unsafe`. Asking Windows which image another
//! process runs (`QueryFullProcessImageNameW`), whose it is, and who signed
//! that image (`WinVerifyTrust`) has no safe Rust API, nor do macOS's
//! `setregid` and its processes' pid versions, so those calls live here,
//! each in a function that allows `unsafe` for itself alone.
//!
//! On every other target this crate is empty.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{PidVersions, own_pid_version, pid_versions, set_real_and_effective_gid};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    KnownFolder, Process, Signer, authenticode_signer, current_account, current_session,
    current_user_sid, final_path, is_elevated, known_folder, machine_key_exists, owner_sid,
    parent_process_id, process_image, session_account, session_user_sid,
};
