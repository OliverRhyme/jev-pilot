//! The host's threads: one reading Chrome on stdin, one accepting runs on
//! the local channel and one per run, all telling the [`Relay`] what
//! happened, and the relay's answers carried out on the calling thread.

use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::{self, Read, Seek as _, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime};

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{Listener, ListenerOptions, Stream};

use super::channel::Place;
use super::{Input, Output, Relay, RunId, Started, Throttle, utc};
use crate::browser::bridge::{self, ByteOrder, FrameError, Message, Nonce};
use crate::peer::{Expected, PeerCheck, Verdict, decide, recheck};

/// How long a run has to echo the nonce.
pub const ECHO_WAIT: Duration = Duration::from_secs(10);

/// How long a write to a run may wait before the run is taken as gone.
const SEND_WAIT: Duration = Duration::from_secs(3);

/// The first wait after a failed accept, and the longest.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
const ACCEPT_BACKOFF_MOST: Duration = Duration::from_secs(5);

/// How many events may wait for the relay.
const EVENTS_QUEUED: usize = 256;

/// How often the host looks at the installed version.
const INSTALL_CHECK: Duration = Duration::from_secs(30);

/// The audit log's size before it is rotated.
pub const AUDIT_LIMIT: u64 = 4 << 20;

/// How many old audit logs are kept.
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
    /// Where the host notes what went wrong, apart from the audit log.
    pub diagnostics: Option<Diagnostics>,
    /// How long a run has to echo the nonce: [`ECHO_WAIT`].
    pub echo_wait: Duration,
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
    let log = config.audit_dir.join("audit.log");
    if !log.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "the audit log {} is missing: run the install again",
                log.display()
            ),
        ));
    }
    if another_serves(&config) {
        send_to_chrome(&mut stdout, &Message::Elsewhere)?;
        return Ok(());
    }
    if let Place::Socket(path) = &config.place {
        let _ = std::fs::remove_file(path);
    }
    let listener = listen(&config.place)?;
    // Bounded, so a host that falls behind holds Chrome back rather than
    // queueing without end.
    let (events, heard) = mpsc::sync_channel(EVENTS_QUEUED);
    let stop = Arc::new(AtomicBool::new(false));

    read_chrome(stdin, events.clone());
    accept_runs(
        listener,
        config.check.clone(),
        config.expected.clone(),
        events.clone(),
        stop.clone(),
        config.diagnostics.clone(),
        config.echo_wait,
    );
    watch_install(config.installed_version, events);

    let mut relay = Relay::new(&config.version, config.extension_on_disk);
    let mut runs: HashMap<RunId, Arc<Stream>> = HashMap::new();
    let mut pending: VecDeque<Input> = VecDeque::new();
    let mut throttle = Throttle::default();
    'serving: for event in heard {
        match event {
            Event::Joined(run, stream) => {
                let _ = stream.set_send_timeout(Some(SEND_WAIT));
                runs.insert(run, stream);
                continue;
            }
            Event::Input(input) => pending.push_back(input),
        }
        while let Some(input) = pending.pop_front() {
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
                        // A run that stops reading is taken as gone once a
                        // write has waited SEND_WAIT, so it cannot hold up the
                        // extension or the other runs.
                        if let Some(stream) = runs.get(&run)
                            && send_to_run(stream, &message).is_err()
                        {
                            hang_up(stream);
                            pending.push_back(Input::RunGone { run });
                        }
                    }
                    Output::Close { run } => {
                        if let Some(stream) = runs.remove(&run) {
                            hang_up(&stream);
                        }
                    }
                    Output::Audit(audit) => {
                        for audit in throttle.admit(audit, std::time::Instant::now()) {
                            let _ = append_audit(
                                &config.audit_dir,
                                &audit.line(&utc(SystemTime::now())),
                                AUDIT_LIMIT,
                            );
                        }
                    }
                    Output::Exit => break 'serving,
                }
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
fn read_chrome(mut stdin: impl Read + Send + 'static, events: mpsc::SyncSender<Event>) {
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
    events: mpsc::SyncSender<Event>,
    stop: Arc<AtomicBool>,
    diagnostics: Option<Diagnostics>,
    echo_wait: Duration,
) {
    std::thread::spawn(move || {
        let mut next: RunId = 0;
        let mut backoff = ACCEPT_BACKOFF;
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let stream = match stream {
                Ok(stream) => {
                    backoff = ACCEPT_BACKOFF;
                    stream
                }
                // Noted, and tried again more slowly each time, rather than
                // spun on: on Windows each run needs a new instance of the
                // pipe, and making one can fail.
                Err(e) => {
                    if let Some(diagnostics) = &diagnostics {
                        diagnostics.note(&format!("accepting a run failed: {e}"));
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(ACCEPT_BACKOFF_MOST);
                    continue;
                }
            };
            next += 1;
            let (run, check, expected, events) =
                (next, check.clone(), expected.clone(), events.clone());
            std::thread::spawn(move || {
                serve_run(run, stream, &*check, &expected, &events, echo_wait);
            });
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
    events: &mpsc::SyncSender<Event>,
    echo_wait: Duration,
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
    let _ = stream.set_recv_timeout(Some(echo_wait));
    // Gone before echoing: a host checking whether this one serves, or a
    // run that gave up. Nothing to audit.
    let echo = match bridge::decode(&mut &*stream, ByteOrder::Little) {
        Ok(Some(frame)) => serde_json::from_slice(&frame).unwrap_or(Message::Absent),
        // Silent past the wait: refused, and audited.
        Err(FrameError::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            refuse("nonce: not echoed in time".into());
            return;
        }
        // Not a frame of the bridge: refused, and audited.
        Err(e @ (FrameError::Truncated | FrameError::TooLargeToRead { .. })) => {
            refuse(format!("nonce: echo malformed: {e}"));
            return;
        }
        // Closed before echoing: a check that a host answers, or a run that
        // gave up. Nothing to audit.
        Ok(None) | Err(_) => {
            send(Input::RunGone { run });
            return;
        }
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
fn watch_install(
    installed: Box<dyn Fn() -> Option<String> + Send>,
    events: mpsc::SyncSender<Event>,
) {
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

/// Append `line` to `dir`'s `audit.log`, first rotating it into
/// `audit.log.1` (and so on, keeping three) when it has reached `limit`
/// bytes.
///
/// The install makes all four files, root's with the `jev-pilot` group and
/// mode 0664, so only a member of the group (the setgid host) may write
/// them, and the owner, not root, cannot remove or replace them. They are
/// never made here, since a file the host made would be the owner's: a
/// missing one is an error. Rotation copies within them and truncates, so
/// they keep their owner and mode.
///
/// Two hosts must not rotate under each other, so each holds a lock through
/// the rotation and the write. The lock is `audit.lock`, which the install
/// makes root's with the group and mode 0660, not the log: the owner may
/// read the log, and so could hold it locked and stall the host, but cannot
/// open the lock.
///
/// # Errors
/// When a file is missing or cannot be written.
pub fn append_audit(dir: &Path, line: &str, limit: u64) -> io::Result<()> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("audit.lock"))?;
    // Let go when the file closes.
    lock.lock()?;
    let log = dir.join("audit.log");
    // Opened to write, not to append: Windows will not shorten a file
    // opened only to append. The lock keeps every writer at the end.
    let mut file = OpenOptions::new().read(true).write(true).open(&log)?;
    if file.metadata()?.len() >= limit {
        for n in (1..AUDIT_KEPT).rev() {
            let from = std::fs::read(dir.join(format!("audit.log.{n}")))?;
            overwrite(&dir.join(format!("audit.log.{}", n + 1)), &from)?;
        }
        overwrite(&dir.join("audit.log.1"), &std::fs::read(&log)?)?;
        file.set_len(0)?;
    }
    file.seek(io::SeekFrom::End(0))?;
    file.write_all(line.as_bytes())
}

/// Replace what an existing file holds.
fn overwrite(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(contents)
}

/// The host's diagnostics: a file beside the socket, apart from the audit
/// log, that never grows past its bound. When full, it starts again.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    file: PathBuf,
    limit: u64,
}

impl Diagnostics {
    /// Diagnostics in `file`, kept under about `limit` bytes.
    #[must_use]
    pub fn new(file: PathBuf, limit: u64) -> Self {
        Self { file, limit }
    }

    /// Note `what`, stamped with the time. Best effort: a note that cannot
    /// be written is dropped.
    pub fn note(&self, what: &str) {
        let _ = self.try_note(what);
    }

    /// Opened without following a link and never truncated on opening;
    /// written only when it is a regular file of this user's with one name,
    /// and emptied through the open file when full. The host's group stays
    /// out of it: it is the owner's file.
    fn try_note(&self, what: &str) -> io::Result<()> {
        // Opened to write, not to append: Windows will not shorten a file
        // opened only to append.
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(nix::libc::O_NOFOLLOW).mode(0o600);
        }
        let mut file = options.open(&self.file)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if meta.uid() != nix::unistd::getuid().as_raw() || meta.nlink() != 1 {
                return Err(io::Error::other("not this user's own file"));
            }
        }
        if meta.len() >= self.limit {
            file.set_len(0)?;
        }
        file.seek(io::SeekFrom::End(0))?;
        writeln!(file, "{} {what}", utc(SystemTime::now()))
    }
}
