//! Windows: the pipe's peer pid and session, then the image that process
//! runs, its user, and the image's hash or Authenticode signer.

use std::fs::File;

use interprocess::local_socket::Stream;
use jev_pilot_os::{Process, authenticode_signer, process_image};

use super::{Code, Expected, Facts, WindowsPin, sha256_file};

pub(super) fn facts(stream: &Stream, expected: &Expected) -> Facts {
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
    facts.sid = process.user_sid().ok();
    facts.path = process_image(&process).ok();
    if let (Expected::Windows { pin, .. }, Some(path)) = (expected, &facts.path) {
        // Windows refuses to write to the image of a running process, so the
        // file hashed is the file the process runs.
        facts.code = match pin {
            WindowsPin::Sha256(_) => File::open(path)
                .and_then(sha256_file)
                .ok()
                .map(Code::ImageSha256),
            WindowsPin::Signer(_) => authenticode_signer(path).map(|signer| Code::Signer {
                subject: signer.subject,
            }),
        };
    }
    drop(process);
    facts
}
