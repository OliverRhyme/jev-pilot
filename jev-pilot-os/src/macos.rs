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

// macOS documents no way to learn which program a process's parent was
// running when it started that process: Endpoint Security knows, but needs an
// entitlement and root. The kernel's `proc_pidinfo` flavour
// `PROC_PIDUNIQIDENTIFIERINFO` records it, as the parent's pid version then,
// but its number and structure are in xnu's `sys/proc_info_private.h`, not
// the SDK, so Apple may change them. They are therefore checked on every
// read: the record must come back whole and filled in, and this process's
// own pid version in it must equal the one its audit token carries, through
// the documented `task_info(TASK_AUDIT_TOKEN)` and libbsm's
// `audit_token_to_pidversion`. Anything else is an error, and the caller
// refuses.

/// `struct proc_uniqidentifierinfo`, from xnu's `sys/proc_info_private.h`.
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

/// The size xnu gives `struct proc_uniqidentifierinfo`.
const RECORD_SIZE: i32 = 56;

const _: () = assert!(size_of::<UniqIdentifierInfo>() == RECORD_SIZE as usize);

/// `PROC_PIDUNIQIDENTIFIERINFO`, from the same header.
const PROC_PIDUNIQIDENTIFIERINFO: i32 = 17;

/// `TASK_AUDIT_TOKEN`, from the SDK's `mach/task_info.h`.
const TASK_AUDIT_TOKEN: u32 = 15;

/// `audit_token_t`, from the SDK's `mach/message.h`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AuditToken {
    val: [u32; 8],
}

#[allow(unsafe_code)]
#[link(name = "bsm")]
unsafe extern "C" {
    /// From the SDK's `bsm/libbsm.h`.
    fn audit_token_to_pidversion(token: AuditToken) -> libc::c_int;
    /// From the SDK's `mach/mach_init.h`: this process's own task port.
    static mach_task_self_: libc::mach_port_t;
}

/// This process's pid version, from its audit token.
///
/// # Errors
///
/// When the kernel will not give the token.
#[allow(unsafe_code)]
pub fn own_pid_version() -> io::Result<i32> {
    let mut token = AuditToken::default();
    let mut count: libc::mach_msg_type_number_t = 8;
    // SAFETY: `mach_task_self_` is the process's own task port, set before
    // `main` and never changed; `token` is an `audit_token_t`, eight
    // naturals, and `count` says so.
    let result = unsafe {
        libc::task_info(
            mach_task_self_,
            TASK_AUDIT_TOKEN,
            (&raw mut token).cast(),
            &raw mut count,
        )
    };
    if result != libc::KERN_SUCCESS || count != 8 {
        return Err(io::Error::other(format!(
            "task_info(TASK_AUDIT_TOKEN) failed: {result}"
        )));
    }
    // SAFETY: takes the token by value and reads nothing else.
    Ok(unsafe { audit_token_to_pidversion(token) })
}

/// The pid versions of process `pid`, from `proc_pidinfo`.
///
/// A parent that has run another program in place since it started a child
/// has a newer version than the one the child recorded.
///
/// # Errors
///
/// When there is no such process, it may not be asked about, or the
/// kernel's record does not look as expected.
pub fn pid_versions(pid: u32) -> io::Result<PidVersions> {
    let own = read(std::process::id())?;
    if own.version != own_pid_version()? {
        return Err(io::Error::other(
            "the kernel's process record is not laid out as expected",
        ));
    }
    read(pid)
}

#[allow(unsafe_code)]
fn read(pid: u32) -> io::Result<PidVersions> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    let mut info = UniqIdentifierInfo::default();
    // SAFETY: `info` is a `proc_uniqidentifierinfo` of `RECORD_SIZE` bytes,
    // which the call fills for this flavour; it takes no other pointer.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDUNIQIDENTIFIERINFO,
            0,
            (&raw mut info).cast(),
            RECORD_SIZE,
        )
    };
    if written <= 0 {
        return Err(io::Error::last_os_error());
    }
    checked(written, &info)
}

/// The versions in a record of `written` bytes, when it is whole and every
/// field read is filled in.
fn checked(written: i32, info: &UniqIdentifierInfo) -> io::Result<PidVersions> {
    if written != RECORD_SIZE
        || info.unique_id == 0
        || info.id_version == 0
        || info.original_parent_id_version == 0
    {
        return Err(io::Error::other(
            "the kernel's process record is not as expected",
        ));
    }
    Ok(PidVersions {
        version: info.id_version,
        parent_at_start: info.original_parent_id_version,
    })
}

#[cfg(test)]
mod tests {
    use super::{UniqIdentifierInfo, checked};

    fn record() -> UniqIdentifierInfo {
        UniqIdentifierInfo {
            unique_id: 7,
            parent_unique_id: 3,
            id_version: 41,
            original_parent_id_version: 40,
            ..UniqIdentifierInfo::default()
        }
    }

    #[test]
    fn a_whole_record_gives_its_versions() {
        let versions = checked(56, &record()).expect("a whole record");
        assert_eq!((versions.version, versions.parent_at_start), (41, 40));
    }

    #[test]
    fn a_short_or_empty_record_is_refused() {
        assert!(checked(48, &record()).is_err(), "short");
        assert!(checked(0, &record()).is_err(), "nothing");
        let no_id = UniqIdentifierInfo {
            unique_id: 0,
            ..record()
        };
        assert!(checked(56, &no_id).is_err(), "no unique id");
        let no_version = UniqIdentifierInfo {
            id_version: 0,
            ..record()
        };
        assert!(checked(56, &no_version).is_err(), "no version");
        let no_parent = UniqIdentifierInfo {
            original_parent_id_version: 0,
            ..record()
        };
        assert!(checked(56, &no_parent).is_err(), "no parent version");
    }
}
