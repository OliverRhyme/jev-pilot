//! The local channel between a run and the host: a Unix socket in a
//! directory only the owner can enter, or a named pipe only the owner can
//! open. The kernel keeps other users off it; which program of the owner's is
//! at the other end is the business of [`crate::peer`].

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
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
        Os::Linux | Os::Windows => match env("XDG_RUNTIME_DIR").map(PathBuf::from) {
            Some(runtime) if runtime.is_absolute() => runtime.join("jev-pilot"),
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

/// The run's end of the channel, kept in step with its frames, whose
/// little-endian length prefixes it reads as they pass.
///
/// Between frames, a read waits only as long as its [`Pacer`] allows, and
/// says "nothing yet" ([`io::ErrorKind::WouldBlock`]) when nothing came.
/// Once a frame has started, reads wait for the rest of it however long it
/// takes, because giving up part way would leave the stream out of step.
#[derive(Debug)]
pub struct RunStream {
    stream: Stream,
    pacer: Pacer,
    /// The length prefix of the frame being read, as far as it has come.
    prefix: [u8; 4],
    /// How many of its bytes have come.
    prefix_read: usize,
    /// How many bytes of the frame's message are still to come.
    message_left: usize,
    /// Whole frames read already and put back, to be read again first.
    again: io::Cursor<Vec<u8>>,
}

impl RunStream {
    /// `stream`, kept in step, and the pacer that says how long to wait
    /// between frames.
    #[must_use]
    pub fn new(stream: Stream) -> (Self, Pacer) {
        let pacer = Pacer::new(HANDSHAKE_WAIT);
        let run = Self {
            stream,
            pacer: pacer.clone(),
            prefix: [0; 4],
            prefix_read: 0,
            message_left: 0,
            again: io::Cursor::new(Vec::new()),
        };
        (run, pacer)
    }

    /// Read `frame`, a whole frame read before, again before anything new.
    fn put_back(&mut self, frame: Vec<u8>) {
        self.again = io::Cursor::new(frame);
    }

    fn between_frames(&self) -> bool {
        self.prefix_read == 0 && self.message_left == 0
    }
}

impl Read for RunStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if usize::try_from(self.again.position()).unwrap_or(usize::MAX) < self.again.get_ref().len()
        {
            return self.again.read(buf);
        }
        let between = self.between_frames();
        let wait = between.then(|| self.pacer.wait().max(Duration::from_millis(1)));
        // A socket the other end has closed may refuse a timeout (macOS
        // says EINVAL); the read below then reports the close.
        let _ = self.stream.set_recv_timeout(wait);
        // Read no further than the part of the frame being read, so the
        // prefix and the message are each counted.
        let limit = if self.message_left > 0 {
            self.message_left
        } else {
            4 - self.prefix_read
        };
        let limit = limit.min(buf.len());
        let n = match self.stream.read(&mut buf[..limit]) {
            Ok(n) => n,
            Err(e)
                if between
                    && matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            Err(e) => return Err(e),
        };
        if self.message_left > 0 {
            self.message_left -= n;
        } else {
            self.prefix[self.prefix_read..self.prefix_read + n].copy_from_slice(&buf[..n]);
            self.prefix_read += n;
            if self.prefix_read == 4 {
                self.prefix_read = 0;
                self.message_left = u32::from_le_bytes(self.prefix) as usize;
            }
        }
        Ok(n)
    }
}

impl Write for RunStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
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
