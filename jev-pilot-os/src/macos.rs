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

/// A process's pid version, and its original parent's pid version when it
/// was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PidVersions {
    /// The process's pid version, which changes on every fork and exec.
    pub version: i32,
    /// Its original parent's pid version when it was started, which does
    /// not change, even if it is re-parented.
    pub parent_at_start: i32,
}

/// `struct proc_uniqidentifierinfo`, from xnu's `sys/proc_info_private.h`,
/// which marks the structure as API.
#[repr(C)]
#[derive(Default)]
struct UniqIdentifierInfo {
    uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    id_version: i32,
    original_parent_id_version: i32,
    reserved: [u64; 2],
}

/// `PROC_PIDUNIQIDENTIFIERINFO`, from the same header.
const PROC_PIDUNIQIDENTIFIERINFO: i32 = 17;

/// The pid versions of process `pid`, from `proc_pidinfo`.
///
/// A parent that has run another program in place since it started a child
/// has a newer version than the one the child recorded.
///
/// # Errors
///
/// When there is no such process, or it may not be asked about.
#[allow(unsafe_code)]
pub fn pid_versions(pid: u32) -> io::Result<PidVersions> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    let mut info = UniqIdentifierInfo::default();
    let size = i32::try_from(size_of::<UniqIdentifierInfo>()).map_err(io::Error::other)?;
    // SAFETY: `info` is a `proc_uniqidentifierinfo` of `size` bytes, which
    // the call fills for this flavour; it takes no other pointer.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDUNIQIDENTIFIERINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if written != size {
        return Err(io::Error::last_os_error());
    }
    Ok(PidVersions {
        version: info.id_version,
        parent_at_start: info.original_parent_id_version,
    })
}
