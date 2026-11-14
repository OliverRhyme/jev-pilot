//! The local channel between a run and the host: a Unix socket in a
//! directory only the owner can enter, or a named pipe only the owner can
//! open. The kernel keeps other users off it; which program of the owner's is
//! at the other end is the business of [`crate::peer`].

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, Name, Stream, ToFsName as _};

use crate::browser::bridge::{self, ByteOrder, Parent};
use crate::browser::discover::Os;
use crate::browser::extension::{ConnectError, ExtensionWire, Pacer};
use crate::peer::{Expected, PeerCheck, Refusal, Verdict, decide, recheck};

/// The socket's file name, in [`socket_dir`].
pub const SOCKET_NAME: &str = "bridge.sock";

/// Where the socket lives: a directory of the owner's, never `$TMPDIR`, which
/// an SSH session on macOS does not set.
///
/// - macOS: `~/Library/Application Support/jev-pilot/run`;
/// - Linux: `$XDG_RUNTIME_DIR/jev-pilot` when that is an absolute path, else
///   `~/.local/state/jev-pilot/run`.
#[must_use]
pub fn socket_dir(os: Os, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match os {
        Os::MacOs => home.join("Library/Application Support/jev-pilot/run"),
        // Absolute as Linux sees it, whatever system this runs on.
        Os::Linux | Os::Windows => match env("XDG_RUNTIME_DIR") {
            Some(runtime) if runtime.starts_with('/') => PathBuf::from(runtime).join("jev-pilot"),
            _ => home.join(".local/state/jev-pilot/run"),
        },
    }
}

/// The socket in `dir`.
#[must_use]
pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join(SOCKET_NAME)
}

/// What the OS says about the socket's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirFacts {
    /// Whether it is a directory (not a link to one).
    pub is_dir: bool,
    /// Its owner's uid.
    pub owner: u32,
    /// Its permission bits.
    pub mode: u32,
}

/// Why the channel cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelError {
    /// The socket's directory is not a directory.
    NotADirectory {
        /// The directory.
        dir: PathBuf,
    },
    /// The socket's directory belongs to another user.
    NotYours {
        /// The directory.
        dir: PathBuf,
        /// Its owner.
        owner: u32,
    },
    /// The socket's directory lets others in.
    OpenToOthers {
        /// The directory.
        dir: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// The socket's path is longer than the system allows.
    PathTooLong {
        /// The path.
        path: PathBuf,
    },
}

impl core::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotADirectory { dir } => {
                write!(
                    f,
                    "the bridge's directory {} is not a directory",
                    dir.display()
                )
            }
            Self::NotYours { dir, owner } => write!(
                f,
                "the bridge's directory {} is owned by uid {owner}, not you",
                dir.display()
            ),
            Self::OpenToOthers { dir, mode } => write!(
                f,
                "the bridge's directory {} is open to others (mode {mode:o})",
                dir.display()
            ),
            Self::PathTooLong { path } => write!(
                f,
                "the bridge's socket path {} is too long for this system",
                path.display()
            ),
        }
    }
}

impl core::error::Error for ChannelError {}

/// Whether `dir` may hold the socket: a directory, `uid`'s own, closed to
/// group and others.
///
/// # Errors
/// The first of these that does not hold.
pub fn check_dir(dir: &Path, facts: DirFacts, uid: u32) -> Result<(), ChannelError> {
    let dir = dir.to_owned();
    if !facts.is_dir {
        return Err(ChannelError::NotADirectory { dir });
    }
    if facts.owner != uid {
        return Err(ChannelError::NotYours {
            dir,
            owner: facts.owner,
        });
    }
    if facts.mode & 0o077 != 0 {
        return Err(ChannelError::OpenToOthers {
            dir,
            mode: facts.mode & 0o777,
        });
    }
    Ok(())
}

/// Whether a socket at `path` fits in `sockaddr_un`: 104 bytes on macOS and
/// 108 on Linux, the terminating NUL included.
///
/// # Errors
/// [`ChannelError::PathTooLong`], naming the path.
pub fn check_path(os: Os, path: &Path) -> Result<(), ChannelError> {
    let limit = match os {
        Os::MacOs => 104,
        Os::Linux | Os::Windows => 108,
    };
    if path.as_os_str().len() < limit {
        Ok(())
    } else {
        Err(ChannelError::PathTooLong {
            path: path.to_owned(),
        })
    }
}

/// The named pipe for the user whose SID is `sid`.
#[must_use]
pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\jev-pilot-{sid}")
}

/// The pipe's access rights for its user: `FILE_GENERIC_READ` (0x120089)
/// and `FILE_GENERIC_WRITE` (0x120116). Write includes
/// `FILE_CREATE_PIPE_INSTANCE` (`FILE_APPEND_DATA`, 0x4), which the host
/// needs to make an instance of the pipe for each run. A program running as
/// the user can then add an instance of its own and take a connection; each
/// run checks the host before it sends anything, which is what defeats it.
const PIPE_USER_RIGHTS: u32 = 0x0012_0089 | 0x0012_0116;

/// The pipe's security descriptor, as SDDL.
///
/// A protected DACL (`P`), with no inherited entries:
/// - network (`NU`) and anonymous (`AN`) logons denied everything, first,
///   because an access check stops at the first entry that decides;
/// - SYSTEM allowed everything;
/// - the user allowed read and write (`0x12019f`).
///
/// The default descriptor would give Everyone read access, and an empty one
/// (`SecurityDescriptor::new()`) gives everyone everything.
#[must_use]
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(D;;GA;;;NU)(D;;GA;;;AN)(A;;GA;;;SY)(A;;{PIPE_USER_RIGHTS:#x};;;{sid})")
}

/// Where the channel is: a socket's path, or a pipe's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A Unix socket on a filesystem path.
    Socket(PathBuf),
    /// A Windows named pipe, `\\.\pipe\…`.
    Pipe(String),
}

impl Place {
    /// The place as `interprocess` names it.
    ///
    /// # Errors
    /// When the path or name cannot be a local socket's name here.
    pub fn name(&self) -> io::Result<Name<'_>> {
        match self {
            Self::Socket(path) => path.as_path().to_fs_name::<GenericFilePath>(),
            Self::Pipe(name) => name.as_str().to_fs_name::<GenericFilePath>(),
        }
    }
}

/// How long the handshake may wait for each of the host's frames.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);

/// End a connection now, from any thread, so that a read or write waiting
/// on it gives up: the socket is shut down, or on Windows, where a named
/// pipe takes no timeouts, the host's end (`server`) forces the run's
/// closed and every wait on the pipe is cancelled.
///
/// This, not a socket timeout, is what cuts a wait short on every system.
pub fn hang_up(stream: &Stream, server: bool) {
    #[cfg(unix)]
    {
        let Stream::UdSocket(socket) = stream;
        let _ = socket.inner().shutdown(std::net::Shutdown::Both);
        let _ = server;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle as _;
        let Stream::NamedPipe(pipe) = stream;
        jev_pilot_os::hang_up_pipe(pipe.as_handle(), server);
    }
}

/// The run's end of the channel, read a whole frame at a time.
///
/// A thread of its own reads the host's frames whole, as they come, and
/// hands them over, reading at most a couple ahead, so a run that stops
/// reading still stops the host's writes. Between frames, a read waits for
/// the next only as long as its [`Pacer`] allows, and says "nothing yet"
/// ([`io::ErrorKind::WouldBlock`]) when none has begun; one that has begun
/// is waited for however long it takes, and never torn. Named pipes take no
/// read timeouts, so the wait is on the hand-over, never on the channel.
#[derive(Debug)]
pub struct RunStream {
    stream: Arc<Stream>,
    pacer: Pacer,
    /// Whole frames, length prefix and all, or why reading stopped. It
    /// ends when the host closes the channel.
    frames: mpsc::Receiver<io::Result<Vec<u8>>>,
    /// How many frames the reader has begun.
    begun: Arc<AtomicU64>,
    /// How many have been handed over.
    taken: u64,
    /// The frame being read, as far as it has been.
    current: io::Cursor<Vec<u8>>,
    /// Told when the run lets go, so the reader stops.
    closing: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

/// How many whole frames the reader may hold ahead of the run.
const READ_AHEAD: usize = 2;

impl RunStream {
    /// `stream`, read a whole frame at a time, and the pacer that says how
    /// long to wait between frames.
    #[must_use]
    pub fn new(stream: Stream) -> (Self, Pacer) {
        let pacer = Pacer::new(HANDSHAKE_WAIT);
        let stream = Arc::new(stream);
        let closing = Arc::new(AtomicBool::new(false));
        let begun = Arc::new(AtomicU64::new(0));
        let (frames, receive) = mpsc::sync_channel(READ_AHEAD);
        let reader = {
            let (stream, closing, begun) = (stream.clone(), closing.clone(), begun.clone());
            std::thread::spawn(move || read_frames(&stream, &closing, &begun, &frames))
        };
        let run = Self {
            stream,
            pacer: pacer.clone(),
            frames: receive,
            begun,
            taken: 0,
            current: io::Cursor::new(Vec::new()),
            closing,
            reader: Some(reader),
        };
        (run, pacer)
    }

    /// Read `frame`, a whole frame read before, again before anything new.
    fn put_back(&mut self, frame: Vec<u8>) {
        let mut again = frame;
        let position = usize::try_from(self.current.position()).unwrap_or(usize::MAX);
        again.extend_from_slice(self.current.get_ref().get(position..).unwrap_or_default());
        self.current = io::Cursor::new(again);
    }

    fn take(&mut self, next: io::Result<Vec<u8>>, buf: &mut [u8]) -> io::Result<usize> {
        self.taken += 1;
        self.current = io::Cursor::new(next?);
        self.current.read(buf)
    }
}

/// Hand over the host's frames whole, counting each as it begins, until it
/// closes, reading fails, or the run lets go.
fn read_frames(
    stream: &Stream,
    closing: &AtomicBool,
    begun: &AtomicU64,
    frames: &mpsc::SyncSender<io::Result<Vec<u8>>>,
) {
    let mut stream = stream;
    while !closing.load(Ordering::SeqCst) {
        let mut prefix = [0u8; 4];
        match stream.read(&mut prefix[..1]) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = frames.send(Err(e));
                return;
            }
        }
        begun.fetch_add(1, Ordering::SeqCst);
        let (whole, cut_short) = match read_rest(&mut stream, prefix) {
            Ok(read) => read,
            Err(e) => {
                let _ = frames.send(Err(e));
                return;
            }
        };
        // A frame the host cut short is handed over as far as it came, and
        // nothing after it: the run then sees the close in mid-message.
        if frames.send(Ok(whole)).is_err() || cut_short {
            return;
        }
    }
}

/// The rest of a frame whose first byte is in `prefix`: the frame, and
/// whether the channel closed before it was whole.
fn read_rest(stream: &mut &Stream, mut prefix: [u8; 4]) -> io::Result<(Vec<u8>, bool)> {
    let mut filled = 1;
    while filled < 4 {
        match stream.read(&mut prefix[filled..]) {
            Ok(0) => return Ok((prefix[..filled].to_vec(), true)),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let length = u32::from_le_bytes(prefix) as usize;
    if length > bridge::READ_LIMIT {
        // Handed over as it is: the run's own reading refuses it.
        return Ok((prefix.to_vec(), true));
    }
    let mut whole = vec![0u8; 4 + length];
    whole[..4].copy_from_slice(&prefix);
    let mut filled = 4;
    while filled < whole.len() {
        match stream.read(&mut whole[filled..]) {
            Ok(0) => {
                whole.truncate(filled);
                return Ok((whole, true));
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((whole, false))
}

impl Read for RunStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let position = usize::try_from(self.current.position()).unwrap_or(usize::MAX);
        if position < self.current.get_ref().len() {
            return self.current.read(buf);
        }
        let wait = self.pacer.wait().max(Duration::from_millis(1));
        match self.frames.recv_timeout(wait) {
            Ok(next) => self.take(next, buf),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // A frame that has begun is waited for whole.
                if self.begun.load(Ordering::SeqCst) > self.taken {
                    match self.frames.recv() {
                        Ok(next) => self.take(next, buf),
                        Err(_) => Ok(0),
                    }
                } else {
                    Err(io::ErrorKind::WouldBlock.into())
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(0),
        }
    }
}

impl Drop for RunStream {
    /// Let go of the channel, and stop the reader: a pipe's wait is
    /// cancelled only while it waits, so the run asks until the reader has
    /// gone.
    fn drop(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        let Some(reader) = self.reader.take() else {
            return;
        };
        for _ in 0..200 {
            hang_up(&self.stream, false);
            // Frames still coming are not wanted.
            while self.frames.try_recv().is_ok() {}
            if reader.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
}

impl Write for RunStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.stream).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.stream).flush()
    }
}

/// Why a run could not reach the host.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    /// Nothing is listening: Chrome is not running with the extension.
    NoHost,
    /// What answered failed the run's check of the host.
    NotTheHost(Refusal),
    /// The host found the run is not the installed jev-pilot.
    Refused {
        /// The installed jev-pilot, the only one the bridge serves.
        installed: PathBuf,
        /// The check the run failed.
        check: String,
    },
    /// The host turned the run away, or the handshake failed.
    Connect(ConnectError),
}

impl core::fmt::Display for RunError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoHost => ConnectError::Absent.fmt(f),
            Self::NotTheHost(refusal) => ConnectError::NotTheHost {
                why: refusal.to_string().into(),
            }
            .fmt(f),
            Self::Refused { installed, check } => write!(
                f,
                "the bridge only serves the installed jev-pilot at {} ({check})",
                installed.display()
            ),
            Self::Connect(e) => e.fmt(f),
        }
    }
}

impl core::error::Error for RunError {}

/// Whether a host that passes `check` answers at `place`. Nothing is sent:
/// the bridge is not taken, and the extension is not told.
#[must_use]
pub fn host_answers(place: &Place, expected: &Expected, check: &dyn PeerCheck) -> bool {
    let Ok(name) = place.name() else {
        return false;
    };
    let Ok(stream) = Stream::connect(name) else {
        return false;
    };
    matches!(
        decide(&check.facts(&stream, expected), expected),
        Verdict::Accept
    )
}

/// Connect a run to the host at `place`: check with `check` that the host
/// is the installed jev-pilot before anything is sent, then answer its
/// nonce with the run's goal, sites and parent.
///
/// # Errors
/// [`RunError`]: no host, a host that fails the check, or one that turns
/// the run away.
pub fn connect_run(
    place: &Place,
    expected: &Expected,
    check: &dyn PeerCheck,
    goal: &str,
    sites: &[String],
    parent: &Parent,
) -> Result<ExtensionWire<RunStream>, RunError> {
    let name = place.name().map_err(|_| RunError::NoHost)?;
    let stream = Stream::connect(name).map_err(|_| RunError::NoHost)?;
    let first = check.facts(&stream, expected);
    if let Verdict::Refuse(refusal) = decide(&first, expected) {
        return Err(RunError::NotTheHost(refusal));
    }
    let (mut stream, pacer) = RunStream::new(stream);
    // On macOS the kernel names the socket's peer by whoever used it last,
    // when asked: the host is checked again once its nonce has been read,
    // before the run says anything, so a process it handed the socket to is
    // told nothing.
    let nonce = bridge::decode(&mut stream, ByteOrder::Little)
        .map_err(|e| RunError::Connect(ConnectError::Frame(e)))?
        .ok_or(RunError::Connect(ConnectError::Closed))?;
    let again = check.facts(&stream.stream, expected);
    if let Verdict::Refuse(refusal) = recheck(&first, &again, expected) {
        return Err(RunError::NotTheHost(refusal));
    }
    stream.put_back(bridge::encode(&nonce, ByteOrder::Little));
    let wire = ExtensionWire::connect_as(stream, goal, sites, parent).map_err(|e| match e {
        ConnectError::Refused { check } => RunError::Refused {
            installed: expected.path().to_owned(),
            check,
        },
        other => RunError::Connect(other),
    })?;
    Ok(wire.paced_by(pacer))
}
