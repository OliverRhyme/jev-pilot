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

use super::{Code, Expected, Facts};

/// `errSecCSReqFailed`: the code is valid but does not satisfy the
/// requirement. Any other failure means the check could not be made.
const REQUIREMENT_FAILED: i32 = -67050;

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
    if let Expected::Macos { requirement, .. } = expected {
        facts.code = satisfies(&code, requirement);
    }
    facts
}

fn guest(token: &[u32; 8]) -> Option<SecCode> {
    let bytes: Vec<u8> = token.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let data = CFData::from_buffer(&bytes);
    let mut attributes = GuestAttributes::new();
    // `data` outlives the call that reads it.
    attributes.set_audit_token(data.as_concrete_TypeRef());
    SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE).ok()
}

fn satisfies(code: &SecCode, requirement: &str) -> Option<Code> {
    let parsed: SecRequirement = requirement.parse().ok()?;
    let satisfied = match code.check_validity(Flags::NONE, &parsed) {
        Ok(()) => true,
        Err(error) if error.code() == REQUIREMENT_FAILED => false,
        Err(_) => return None,
    };
    Some(Code::Requirement {
        requirement: requirement.to_owned(),
        satisfied,
    })
}
