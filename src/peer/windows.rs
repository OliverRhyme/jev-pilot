//! Windows: the pipe's peer pid and session, then the image that process
//! runs, its user, and the image's hash or Authenticode signer.

use std::fs::File;
use std::io;
use std::time::SystemTime;

use interprocess::local_socket::Stream;
use jev_pilot_os::{Process, authenticode_signer, final_path, process_image};

use super::{Code, Expected, Facts, WindowsPin, sha256_file};

pub(super) fn facts(stream: &Stream, expected: &Expected) -> Facts {
    // Taken before the pipe names the peer: a process created after this
    // cannot be the peer, only something that reused its pid.
    let asked = SystemTime::now();
    let Stream::NamedPipe(pipe) = stream;
    let pipe = pipe.inner();
    // The server asks about its client, and the client about its server,
    // which is how a pipe taken by another program first is caught.
    let (pid, session) = if pipe.is_server() {
        (pipe.client_process_id(), pipe.client_session_id())
    } else {
        (pipe.server_process_id(), pipe.server_session_id())
    };
    let mut facts = Facts {
        pid: pid.ok(),
        session: session.ok(),
        ..Facts::default()
    };
    let Some(pid) = facts.pid else {
        return facts;
    };
    // Held until every fact is read, so the pid cannot name another process
    // part way through.
    let Ok(process) = Process::open(pid) else {
        return facts;
    };
    if !opened_the_peer(&process.created(), asked) {
        // The peer exited and its pid was given to another process before
        // it was opened: there is nothing to say about the peer.
        facts.pid = None;
        return facts;
    }
    facts.sid = process.user_sid().ok();
    // The image is opened once, and its final path, its hash and its signer
    // all come from that one file. Windows refuses to write to the image of
    // a running process, so it is the file the process runs.
    let image = process_image(&process).and_then(File::open);
    let Ok(image) = image else {
        return facts;
    };
    facts.path = final_path(&image).ok();
    if let (Expected::Windows { pin, .. }, Some(path)) = (expected, &facts.path) {
        facts.code = match pin {
            WindowsPin::Sha256(_) => sha256_file(image).ok().map(Code::ImageSha256),
            WindowsPin::Signer(_) => authenticode_signer(path).map(|signer| Code::Signer {
                subject: signer.subject,
            }),
        };
    }
    drop(process);
    facts
}

/// Whether a process created at `created` can be the one the pipe named at
/// `asked`. It cannot if it was created later, or if its creation is unknown.
fn opened_the_peer(created: &io::Result<SystemTime>, asked: SystemTime) -> bool {
    created.as_ref().is_ok_and(|created| *created <= asked)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::{Duration, SystemTime};

    use super::opened_the_peer;

    #[test]
    fn a_process_created_before_the_pipe_was_asked_is_the_peer() {
        let asked = SystemTime::now();
        assert!(opened_the_peer(&Ok(asked - Duration::from_secs(5)), asked));
        assert!(opened_the_peer(&Ok(asked), asked));
    }

    #[test]
    fn a_process_created_after_is_another_that_reused_the_pid() {
        let asked = SystemTime::now();
        assert!(!opened_the_peer(
            &Ok(asked + Duration::from_millis(1)),
            asked
        ));
    }

    #[test]
    fn a_process_whose_creation_cannot_be_read_is_not_the_peer() {
        let asked = SystemTime::now();
        assert!(!opened_the_peer(&Err(io::Error::other("no")), asked));
    }
}
