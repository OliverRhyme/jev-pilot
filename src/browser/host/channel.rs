//! The local channel between a run and the host: a Unix socket in a
//! directory only the owner can enter, or a named pipe only the owner can
//! open. The kernel keeps other users off it; which program of the owner's is
//! at the other end is the business of [`crate::peer`].

use std::path::{Path, PathBuf};

use crate::browser::discover::Os;

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

/// The pipe's access rights for its user: `FILE_GENERIC_READ` (0x120089),
/// and `FILE_GENERIC_WRITE` (0x120116) without `FILE_APPEND_DATA` (0x4),
/// which on a pipe is `FILE_CREATE_PIPE_INSTANCE` and would let any program
/// of the user's add an instance of its own and take connections.
const PIPE_USER_RIGHTS: u32 = 0x0012_0089 | (0x0012_0116 & !0x4);

/// The pipe's security descriptor, as SDDL.
///
/// A protected DACL (`P`), with no inherited entries:
/// - network (`NU`) and anonymous (`AN`) logons denied everything, first,
///   because an access check stops at the first entry that decides;
/// - SYSTEM allowed everything;
/// - the user allowed read and write (`0x12019b`), without the right to
///   create instances.
///
/// The default descriptor would give Everyone read access, and an empty one
/// (`SecurityDescriptor::new()`) gives everyone everything.
#[must_use]
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(D;;GA;;;NU)(D;;GA;;;AN)(A;;GA;;;SY)(A;;{PIPE_USER_RIGHTS:#x};;;{sid})")
}
