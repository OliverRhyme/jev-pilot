use std::io;

/// Sets the real and effective group ids, with `setregid`.
///
/// For a process that is not root, macOS also sets the saved group id to the
/// new effective one whenever neither new id equals the old saved id, so
/// `set_real_and_effective_gid(r, r)` gives up a setgid group for good.
///
/// # Errors
///
/// When the kernel refuses the change.
#[allow(unsafe_code)]
pub fn set_real_and_effective_gid(real: u32, effective: u32) -> io::Result<()> {
    // SAFETY: setregid takes two integers and no pointers.
    if unsafe { libc::setregid(real, effective) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
