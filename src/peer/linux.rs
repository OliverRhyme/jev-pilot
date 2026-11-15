//! Linux: the peer's credentials as the kernel recorded them at connect, and
//! the executable the peer was started from.

use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use interprocess::local_socket::Stream;
use interprocess::local_socket::traits::StreamCommon;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::socket::{getsockopt, sockopt};

use super::{Code, Expected, Facts, sha256_file};

pub(super) fn facts(stream: &Stream, expected: &Expected) -> Facts {
    // SO_PEERCRED: the effective ids at connect (or listen, for a client
    // asking about the server). An exec after connecting does not change
    // them, so there is no race in reading them later.
    let creds = stream.peer_creds().ok();
    let mut facts = Facts {
        uid: creds.and_then(|c| c.euid()),
        pid: creds
            .and_then(|c| c.pid())
            .and_then(|pid| u32::try_from(pid).ok()),
        ..Facts::default()
    };
    if let (Expected::Linux { .. }, Some(pid)) = (expected, facts.pid) {
        let Stream::UdSocket(socket) = stream;
        // Linux 6.5 and later: a pidfd for the peer, which shows whether the
        // process the pid named has gone, and so whether the pid may since
        // name another.
        let pidfd = getsockopt(&socket.as_fd(), sockopt::PeerPidfd).ok();
        let exe = format!("/proc/{pid}/exe");
        let path = std::fs::read_link(&exe).ok();
        let hash = File::open(Path::new(&exe)).and_then(sha256_file).ok();
        if still_running(pidfd.as_ref()) {
            facts.path = path;
            facts.code = hash.map(Code::ImageSha256);
        }
    }
    facts
}

/// Whether the process `pidfd` refers to is still running, so its pid still
/// names it.
///
/// Without a pidfd (Linux before 6.5) nothing can show that, so the peer
/// counts as gone and its executable is not used.
fn still_running(pidfd: Option<&OwnedFd>) -> bool {
    let Some(pidfd) = pidfd else {
        return false;
    };
    let mut fds = [PollFd::new(pidfd.as_fd(), PollFlags::POLLIN)];
    // Readable means the process has exited; an error means it cannot be
    // told, which counts the same.
    matches!(poll(&mut fds, PollTimeout::ZERO), Ok(0))
}
