//! macOS: the peer's audit token, then its running code checked against the
//! pinned requirement by the Security framework.

use std::os::fd::AsFd;

use core_foundation::base::TCFType;
use core_foundation::data::CFData;
use interprocess::local_socket::Stream;
use interprocess::local_socket::traits::StreamCommon;
use nix::sys::socket::{getsockopt, sockopt};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};

use super::{Code, Expected, Facts, MacosPin};

/// `errSecCSReqFailed`: the code is valid but does not satisfy the
/// requirement. Any other failure means the check could not be made.
const REQUIREMENT_FAILED: i32 = -67050;

/// Sets the real, effective and saved gid to the real gid, and checks that
/// they held.
pub(super) fn drop_install_group() -> std::io::Result<()> {
    use nix::unistd::{getegid, geteuid, getgid, setegid};
    let real = getgid();
    let held = getegid();
    jev_pilot_os::set_real_and_effective_gid(real.as_raw(), real.as_raw())?;
    if getgid() != real || getegid() != real {
        return Err(std::io::Error::other(
            "the install group is still held after setregid",
        ));
    }
    // macOS has no getresgid. A process that is not root may set its
    // effective gid only to its real or its saved gid, so if the gid it held
    // can be taken back, the saved gid still holds it.
    if held != real && !geteuid().is_root() && setegid(held).is_ok() {
        let _ = setegid(real);
        return Err(std::io::Error::other(
            "the install group is still held as the saved gid",
        ));
    }
    Ok(())
}

pub(super) fn facts(stream: &Stream, expected: &Expected) -> Facts {
    let Stream::UdSocket(socket) = stream;
    let fd = socket.as_fd();

    // The kernel names the peer by the last process to use its socket, at the
    // moment it is asked. The pid and the token are both read that way, so
    // they describe the same process; the uid was recorded at connect.
    let uid = stream.peer_creds().ok().and_then(|creds| creds.euid());
    let pid = getsockopt(&fd, sockopt::LocalPeerPid)
        .ok()
        .and_then(|pid| u32::try_from(pid).ok());
    let token = getsockopt(&fd, sockopt::LocalPeerToken).ok().map(|t| t.val);

    let mut facts = Facts {
        uid,
        pid,
        token,
        ..Facts::default()
    };
    let Some(token) = token else {
        return facts;
    };
    // The token, not the pid, names the code: a pid can be reused, and the
    // token's pid version changes on every fork and exec.
    let Some(code) = guest(&token) else {
        return facts;
    };
    facts.path = code.path(Flags::NONE).ok().and_then(|url| url.to_path());
    if let Expected::Macos { pin, .. } = expected {
        facts.code = satisfies(&code, pin);
    }
    facts
}

/// The executable of process `pid`, as the Security framework finds it.
pub(super) fn path_of(pid: u32) -> Option<std::path::PathBuf> {
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(i32::try_from(pid).ok()?);
    SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .ok()?
        .path(Flags::NONE)
        .ok()?
        .to_path()
}

fn guest(token: &[u32; 8]) -> Option<SecCode> {
    let bytes: Vec<u8> = token.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let data = CFData::from_buffer(&bytes);
    let mut attributes = GuestAttributes::new();
    // `data` outlives the call that reads it.
    attributes.set_audit_token(data.as_concrete_TypeRef());
    SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE).ok()
}

fn satisfies(code: &SecCode, pin: &MacosPin) -> Option<Code> {
    let parsed: SecRequirement = pin.requirement().parse().ok()?;
    let satisfied = match code.check_validity(Flags::NONE, &parsed) {
        Ok(()) => true,
        Err(error) if error.code() == REQUIREMENT_FAILED => false,
        Err(_) => return None,
    };
    Some(Code::Requirement {
        pin: pin.clone(),
        satisfied,
    })
}
