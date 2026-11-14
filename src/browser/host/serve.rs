//! The host's threads: one reading Chrome on stdin, one accepting runs on
//! the local channel and one per run, all telling the [`Relay`] what
//! happened, and the relay's answers carried out on the calling thread.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime};

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{Listener, ListenerOptions, Stream};

use super::channel::Place;
use super::{Input, Output, Relay, RunId, Started, utc};
use crate::browser::bridge::{self, ByteOrder, FrameError, Message, Nonce};
use crate::peer::{Expected, PeerCheck, Verdict, decide, recheck};

/// How long a run has to echo the nonce.
const ECHO_WAIT: Duration = Duration::from_secs(10);

/// How often the host looks at the installed version.
const INSTALL_CHECK: Duration = Duration::from_secs(30);

/// The audit log's size before it is rotated, and how many old ones are
/// kept.
const AUDIT_LIMIT: u64 = 4 << 20;
const AUDIT_KEPT: u32 = 3;

/// What a host needs to serve.
pub struct Serve {
    /// Where the channel is.
    pub place: Place,
    /// What a run must be: the installed jev-pilot.
    pub expected: Expected,
    /// How the OS is asked about a run.
    pub check: Arc<dyn PeerCheck + Send + Sync>,
    /// The audit log's directory.
    pub audit_dir: PathBuf,
    /// The host's own version.
    pub version: String,
    /// The version of the extension's files on disk.
    pub extension_on_disk: Option<String>,
    /// The installed jev-pilot's version, read again from time to time.
    pub installed_version: Box<dyn Fn() -> Option<String> + Send>,
}

impl core::fmt::Debug for Serve {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Serve")
            .field("place", &self.place)
            .field("expected", &self.expected)
            .field("audit_dir", &self.audit_dir)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// What the threads tell the relay's thread.
enum Event {
    /// A connection, to write to.
    Joined(RunId, Arc<Stream>),
    /// Something for the relay.
    Input(Input),
}

/// Serve the extension on `stdin` and `stdout` until Chrome closes stdin,
/// relaying one verified run at a time from the channel at `place`.
///
/// When a verified host already serves at `place`, the extension is told
/// [`Message::Elsewhere`] and nothing more is done. A socket left by a host
/// that has gone, or one that fails the check, is replaced.
///
/// # Errors
/// When the channel cannot be listened on.
pub fn serve(
    config: Serve,
    stdin: impl Read + Send + 'static,
    mut stdout: impl Write,
) -> io::Result<()> {
    if another_serves(&config) {
        send_to_chrome(&mut stdout, &Message::Elsewhere)?;
        return Ok(());
    }
    if let Place::Socket(path) = &config.place {
        let _ = std::fs::remove_file(path);
    }
    let listener = listen(&config.place)?;
    let (events, heard) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));

    read_chrome(stdin, events.clone());
    accept_runs(
        listener,
        config.check.clone(),
        config.expected.clone(),
        events.clone(),
        stop.clone(),
    );
    watch_install(config.installed_version, events);

    let mut relay = Relay::new(&config.version, config.extension_on_disk);
    let mut runs: HashMap<RunId, Arc<Stream>> = HashMap::new();
    'serving: for event in heard {
        let input = match event {
            Event::Joined(run, stream) => {
                runs.insert(run, stream);
                continue;
            }
            Event::Input(input) => input,
        };
        if let Input::RunGone { run } = &input {
            runs.remove(run);
        }
        for output in relay.handle(input) {
            match output {
                Output::ToExtension(message) => {
                    if send_to_chrome(&mut stdout, &message).is_err() {
                        break 'serving;
                    }
                }
                Output::ToRun { run, message } => {
                    if let Some(stream) = runs.get(&run) {
                        let _ = send_to_run(stream, &message);
                    }
                }
                Output::Close { run } => {
                    if let Some(stream) = runs.remove(&run) {
                        hang_up(&stream);
                    }
                }
                Output::Audit(audit) => {
                    let _ = append_audit(
                        &config.audit_dir,
                        &audit.line(&utc(SystemTime::now())),
                        AUDIT_LIMIT,
                    );
                }
                Output::Exit => break 'serving,
            }
        }
    }

    stop.store(true, Ordering::SeqCst);
    for stream in runs.values() {
        hang_up(stream);
    }
    // Wake the accepting thread, so it sees the stop and lets go of the
    // listener.
    if let Ok(name) = config.place.name() {
        let _ = Stream::connect(name);
    }
    if let Place::Socket(path) = &config.place {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Whether a host that passes the check already answers at the place.
fn another_serves(config: &Serve) -> bool {
    let Ok(name) = config.place.name() else {
        return false;
    };
    let Ok(stream) = Stream::connect(name) else {
        return false;
    };
    let facts = config.check.facts(&stream, &config.expected);
    matches!(decide(&facts, &config.expected), Verdict::Accept)
}

fn listen(place: &Place) -> io::Result<Listener> {
    let options = ListenerOptions::new().name(place.name()?);
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
        use std::os::unix::fs::PermissionsExt as _;
        // Set before bind where the system allows (Linux); elsewhere
        // (macOS) right after. Either way the directory, the owner's alone,
        // is what keeps others out.
        match options.mode(0o600).create_sync() {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                let listener = ListenerOptions::new().name(place.name()?).create_sync()?;
                if let Place::Socket(path) = place {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                }
                Ok(listener)
            }
            other => other,
        }
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt as _;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        let sid = crate::peer::own_sid()?;
        let sddl = widestring::U16CString::from_str(super::channel::pipe_sddl(&sid))
            .map_err(io::Error::other)?;
        options
            .security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
            .create_sync()
    }
    #[cfg(not(any(unix, windows)))]
    options.create_sync()
}

/// Read Chrome's frames from `stdin` until it closes.
fn read_chrome(mut stdin: impl Read + Send + 'static, events: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        loop {
            let input = match bridge::decode(&mut stdin, ByteOrder::Native) {
                Ok(Some(frame)) => match serde_json::from_slice(&frame) {
                    Ok(message) => Input::Extension(message),
                    // A message this host does not know is passed over.
                    Err(_) => continue,
                },
                Err(FrameError::TooLargeToRead { .. }) => {
                    let _ = events.send(Event::Input(Input::ExtensionTooLarge));
                    return;
                }
                Ok(None) | Err(_) => {
                    let _ = events.send(Event::Input(Input::StdinClosed));
                    return;
                }
            };
            if events.send(Event::Input(input)).is_err() {
                return;
            }
        }
    });
}

/// Accept runs until told to stop, each on a thread of its own.
fn accept_runs(
    listener: Listener,
    check: Arc<dyn PeerCheck + Send + Sync>,
    expected: Expected,
    events: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let mut next: RunId = 0;
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let Ok(stream) = stream else {
                continue;
            };
            next += 1;
            let (run, check, expected, events) =
                (next, check.clone(), expected.clone(), events.clone());
            std::thread::spawn(move || serve_run(run, stream, &*check, &expected, &events));
        }
    });
}

/// One connection: checked before anything is read from it, given a
/// nonce, checked again after its echo, then relayed until it closes.
fn serve_run(
    run: RunId,
    stream: Stream,
    check: &dyn PeerCheck,
    expected: &Expected,
    events: &mpsc::Sender<Event>,
) {
    let stream = Arc::new(stream);
    let send = |input| {
        let _ = events.send(Event::Input(input));
    };
    let first = check.facts(&stream, expected);
    if events.send(Event::Joined(run, stream.clone())).is_err() {
        return;
    }
    let refuse = |check: String| {
        send(Input::RunRefused {
            run,
            pid: first.pid,
            check,
        });
    };
    if let Verdict::Refuse(refusal) = decide(&first, expected) {
        refuse(refusal.to_string());
        return;
    }
    let nonce = Nonce::fresh();
    if send_to_run(&stream, &nonce.message()).is_err() {
        send(Input::RunGone { run });
        return;
    }
    let _ = stream.set_recv_timeout(Some(ECHO_WAIT));
    // Gone before echoing: a host checking whether this one serves, or a
    // run that gave up. Nothing to audit.
    let Some(echo) = read_run(&stream) else {
        send(Input::RunGone { run });
        return;
    };
    if !nonce.is_echoed_by(&echo) {
        refuse("nonce: not echoed".into());
        return;
    }
    // The kernel may name the socket's last user, not the process that
    // connected: whoever wrote the echo must still pass.
    let again = check.facts(&stream, expected);
    if let Verdict::Refuse(refusal) = recheck(&first, &again, expected) {
        refuse(refusal.to_string());
        return;
    }
    let _ = stream.set_recv_timeout(None);
    let Message::Echo {
        goal,
        sites,
        parent_pid,
        parent_exe,
        ..
    } = echo
    else {
        return;
    };
    send(Input::RunVerified {
        run,
        started: Started {
            pid: again.pid.unwrap_or_default(),
            goal,
            sites,
            parent_pid,
            parent_exe,
        },
    });
    while let Some(message) = read_run(&stream) {
        send(Input::Run { run, message });
    }
    send(Input::RunGone { run });
}

/// Look at the installed version from time to time.
fn watch_install(installed: Box<dyn Fn() -> Option<String> + Send>, events: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(INSTALL_CHECK);
            if let Some(version) = installed()
                && events
                    .send(Event::Input(Input::Installed { version }))
                    .is_err()
            {
                return;
            }
        }
    });
}

/// The next message from a run, or `None` when it has closed or sent
/// something that is not a frame of the bridge.
fn read_run(mut stream: &Stream) -> Option<Message> {
    let frame = bridge::decode(&mut stream, ByteOrder::Little).ok()??;
    serde_json::from_slice(&frame).ok()
}

fn send_to_run(mut stream: &Stream, message: &Message) -> io::Result<()> {
    let text = serde_json::to_vec(message)?;
    stream.write_all(&bridge::encode(&text, ByteOrder::Little))?;
    stream.flush()
}

fn send_to_chrome(stdout: &mut impl Write, message: &Message) -> io::Result<()> {
    let text = serde_json::to_vec(message)?;
    let frame = bridge::encode_for_chrome(&text, ByteOrder::Native).map_err(io::Error::other)?;
    stdout.write_all(&frame)?;
    stdout.flush()
}

/// End a connection now, so its reader sees it close.
fn hang_up(stream: &Stream) {
    #[cfg(unix)]
    {
        let Stream::UdSocket(socket) = stream;
        let _ = socket.inner().shutdown(std::net::Shutdown::Both);
    }
    #[cfg(not(unix))]
    let _ = stream;
}

/// Append `line` to `dir`'s `audit.log`, first rotating it to `audit.log.1`
/// (and so on, keeping three) when it has reached `limit` bytes.
///
/// # Errors
/// When the log cannot be written.
pub fn append_audit(dir: &Path, line: &str, limit: u64) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let log = dir.join("audit.log");
    if std::fs::metadata(&log).is_ok_and(|m| m.len() >= limit) {
        for n in (1..AUDIT_KEPT).rev() {
            let _ = std::fs::rename(
                dir.join(format!("audit.log.{n}")),
                dir.join(format!("audit.log.{}", n + 1)),
            );
        }
        std::fs::rename(&log, dir.join("audit.log.1"))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o664);
    }
    options.open(&log)?.write_all(line.as_bytes())
}
