//! Linux: the peer's credentials as the kernel recorded them at connect, and
//! for a development install, the executable the peer was started from.

use std::fs::File;
use std::os::fd::AsFd;
use std::path::Path;

use interprocess::local_socket::Stream;
use interprocess::local_socket::traits::StreamCommon;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::socket::{getsockopt, sockopt};

use super::{Code, Expected, Facts, LinuxPin, sha256_file};

pub(super) fn facts(stream: &Stream, expected: &Expected) -> Facts {
    // SO_PEERCRED: the effective ids at connect (or listen, for a client
    // asking about the server). An exec after connecting does not change
    // them, so there is no race in reading them later.
    let creds = stream.peer_creds().ok();
    let mut facts = Facts {
        uid: creds.and_then(|c| c.euid()),
        egid: creds.and_then(|c| c.egid()),
        pid: creds
            .and_then(|c| c.pid())
            .and_then(|pid| u32::try_from(pid).ok()),
        ..Facts::default()
    };
    // A setgid install's peer is not dumpable, so its executable cannot be
    // opened by the same user; its egid is what vouches for it. Only a
    // development install is hashed.
    if let (
        Expected::Linux {
            pin: LinuxPin::DevelopmentSha256(_),
            ..
        },
        Some(pid),
    ) = (expected, facts.pid)
    {
        let Stream::UdSocket(socket) = stream;
        // Linux 6.5 and later: a pidfd for the peer, which shows whether the
        // process the pid named has gone, and so whether the pid may since
        // name another.
        let pidfd = getsockopt(&socket.as_fd(), sockopt::PeerPidfd).ok();
        let exe = format!("/proc/{pid}/exe");
        let path = std::fs::read_link(&exe).ok();
        let hash = File::open(Path::new(&exe)).and_then(sha256_file).ok();
        let gone = pidfd.is_some_and(|pidfd| {
            let mut fds = [PollFd::new(pidfd.as_fd(), PollFlags::POLLIN)];
            // Readable means the process has exited; an error means it
            // cannot be told, which counts the same.
            !matches!(poll(&mut fds, PollTimeout::ZERO), Ok(0))
        });
        if !gone {
            facts.path = path;
            facts.code = hash.map(Code::ImageSha256);
        }
    }
    facts
}

/// Makes this process not dumpable, so another program running as the same
/// user can neither ptrace it nor read its memory through `/proc`.
pub(super) fn set_not_dumpable() -> std::io::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .map_err(std::io::Error::from)
}

/// Sets the real, effective and saved gid all to the real gid, and checks
/// that they held.
pub(super) fn drop_install_group() -> std::io::Result<()> {
    use nix::unistd::{getgid, getresgid, setresgid};
    let real = getgid();
    // libc's setresgid applies to every thread of the process, not only the
    // calling one.
    setresgid(real, real, real).map_err(std::io::Error::from)?;
    let now = getresgid().map_err(std::io::Error::from)?;
    if (now.real, now.effective, now.saved) == (real, real, real) {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "the install group is still held after setresgid",
        ))
    }
}
