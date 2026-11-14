//! Which program is at the other end of the extension bridge's local socket
//! or named pipe, as the operating system tells it.
//!
//! The kernel already keeps other users off the channel. What it does not do
//! is tell two programs of the same user apart, so each end asks the OS about
//! the other and compares the answer with what the install pinned:
//!
//! - **macOS:** the peer's audit token (`LOCAL_PEERTOKEN`), then its code
//!   checked by the Security framework against a pinned code requirement;
//! - **Linux:** `SO_PEERCRED`, whose effective gid the peer can only have by
//!   executing the root-owned setgid install;
//! - **Windows:** the pipe's client or server pid and session, the image that
//!   process runs, and that image's pinned SHA-256 or Authenticode signer.
//!
//! Gathering those [`Facts`] is the per-OS part, behind [`PeerCheck`].
//! Judging them is [`decide`], a pure function, so the rules of every OS are
//! tested on every OS from plain values. A fact the OS did not give is a
//! refusal, never an assumption.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use interprocess::local_socket::Stream;
use serde::{Deserialize, Serialize};

/// Asks the OS about the program at the other end of a connected channel.
///
/// It works in either direction: the host asks about the run that
/// connected, and the run asks about the host it connected to, which is what
/// defeats a program that took the socket's path or the pipe's name first.
pub trait PeerCheck {
    /// What the OS reports about the peer of `stream`. `expected` says which
    /// code identity to ask for: the requirement to check on macOS, or
    /// whether to hash the image or read its signer.
    fn facts(&self, stream: &Stream, expected: &Expected) -> Facts;

    /// Gathers the facts and [`decide`]s on them.
    fn check(&self, stream: &Stream, expected: &Expected) -> Verdict {
        decide(&self.facts(stream, expected), expected)
    }
}

/// The [`PeerCheck`] of the OS this was built for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Native;

impl PeerCheck for Native {
    fn facts(&self, stream: &Stream, expected: &Expected) -> Facts {
        #[cfg(target_os = "macos")]
        return macos::facts(stream, expected);
        #[cfg(target_os = "linux")]
        return linux::facts(stream, expected);
        #[cfg(windows)]
        return windows::facts(stream, expected);
        // An OS this module has no rules for reports nothing, so every check
        // refuses.
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = (stream, expected);
            Facts::default()
        }
    }
}

/// Makes this process not dumpable (`PR_SET_DUMPABLE` 0), so another program
/// running as the same user can neither ptrace it nor read its memory or
/// open its executable through `/proc`.
///
/// A setgid install is not dumpable from exec; a development install calls
/// this first thing in `main`, which leaves a short window after exec. Note
/// the consequence for the other end: the same user then cannot open this
/// process's `/proc/<pid>/exe` either, so a development install that calls
/// this cannot have its executable hashed by its peer.
///
/// # Errors
///
/// When the kernel refuses the `prctl`.
#[cfg(target_os = "linux")]
#[cfg_attr(docsrs, doc(cfg(target_os = "linux")))]
pub fn set_not_dumpable() -> std::io::Result<()> {
    linux::set_not_dumpable()
}

/// The string form of the SID of the user this process runs as: what a
/// Windows install pins, and what its pipe's security descriptor names.
///
/// # Errors
///
/// When this process's token cannot be read.
#[cfg(windows)]
#[cfg_attr(docsrs, doc(cfg(windows)))]
pub fn own_sid() -> std::io::Result<String> {
    jev_pilot_os::current_user_sid()
}

/// The logon session this process runs in, which a Windows peer must share:
/// see [`Expected::with_session`].
///
/// # Errors
///
/// When Windows cannot report it.
#[cfg(windows)]
#[cfg_attr(docsrs, doc(cfg(windows)))]
pub fn own_session() -> std::io::Result<u32> {
    jev_pilot_os::current_session()
}

/// The SHA-256 of everything `file` holds from where it is.
#[cfg(any(target_os = "linux", windows))]
fn sha256_file(mut file: std::fs::File) -> std::io::Result<Sha256Digest> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            n => hasher.update(&buffer[..n]),
        }
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

/// What the OS reported about the program at the other end of a channel.
///
/// Every field is optional because every one can fail to be read: the peer
/// may have exited, the OS may not offer it, or a call may be refused. A
/// missing fact that a check needs makes [`decide`] refuse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// The peer's effective uid (macOS and Linux).
    pub uid: Option<u32>,
    /// The string form of the peer's user SID (Windows).
    pub sid: Option<String>,
    /// The peer's process id.
    pub pid: Option<u32>,
    /// The peer's logon session (Windows).
    pub session: Option<u32>,
    /// The peer's effective gid when it connected (Linux).
    pub egid: Option<u32>,
    /// The executable the peer runs.
    pub path: Option<PathBuf>,
    /// The peer's audit token (macOS). It names one process image: its pid
    /// version changes on every fork and exec, so [`recheck`] compares it.
    pub token: Option<[u32; 8]>,
    /// What the OS's code identity check found.
    pub code: Option<Code>,
}

/// The result of an OS's check of which code the peer runs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Code {
    /// macOS: the Security framework checked the peer's running code against
    /// `requirement`, and it was satisfied or not.
    Requirement {
        /// The requirement the code was checked against, in the text form
        /// `csreq` reads.
        requirement: String,
        /// Whether the running code satisfied it.
        satisfied: bool,
    },
    /// The SHA-256 of the image file the peer was started from (Windows
    /// without a signature, and a Linux development install).
    ImageSha256(Sha256Digest),
    /// Windows: Authenticode verified the image, and this is its leaf
    /// signer's subject.
    Signer {
        /// The leaf certificate's subject.
        subject: String,
    },
}

/// What the install pinned about the program that is allowed at the other
/// end. It is written to, and read from, `install.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Expected {
    /// macOS: the uid, the installed copy, and its code requirement.
    Macos {
        /// The owner's uid.
        uid: u32,
        /// Where the installed copy lives.
        path: PathBuf,
        /// `cdhash H"…"` for an ad hoc signed copy, or the Developer ID form
        /// `anchor apple generic and identifier "…" and certificate
        /// leaf[subject.OU] = "…"`.
        requirement: String,
    },
    /// Linux: the uid, the installed copy, and what vouches for it.
    Linux {
        /// The owner's uid.
        uid: u32,
        /// Where the installed copy lives.
        path: PathBuf,
        /// The setgid group, or a development install's hash.
        pin: LinuxPin,
    },
    /// Windows: the SID, the installed image, and what vouches for it.
    Windows {
        /// The string form of the owner's SID.
        sid: String,
        /// Where the installed image lives, under Program Files.
        path: PathBuf,
        /// The image's hash, or its signer.
        pin: WindowsPin,
        /// The session the peer must be in: this process's own. A session
        /// lasts one logon, so it is never pinned; set it with
        /// [`Expected::with_session`] before deciding.
        #[serde(skip)]
        session: Option<u32>,
    },
}

/// What vouches for the program on Linux.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinuxPin {
    /// The `jev-pilot` group's gid. A process has it as its effective gid
    /// only by executing the root-owned setgid install.
    Group(u32),
    /// A development install: the SHA-256 of the executable. It is not
    /// protected against other programs running as the owner.
    DevelopmentSha256(Sha256Digest),
}

/// What vouches for the image on Windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WindowsPin {
    /// The SHA-256 of the installed image, for an unsigned build.
    Sha256(Sha256Digest),
    /// The Authenticode leaf signer's subject, for a signed release.
    Signer(String),
}

impl Expected {
    /// The same pin, with the session a Windows peer must be in. Other OSes
    /// have no session, and are returned unchanged.
    #[must_use]
    pub fn with_session(self, own: Option<u32>) -> Self {
        match self {
            Expected::Windows { sid, path, pin, .. } => Expected::Windows {
                sid,
                path,
                pin,
                session: own,
            },
            other => other,
        }
    }
}

/// A SHA-256 digest, written as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// A digest from its 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The digest's 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256Digest({self})")
    }
}

/// A SHA-256 digest that is not 64 hex digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestError;

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a SHA-256 digest is 64 hex digits")
    }
}

impl std::error::Error for DigestError {}

impl FromStr for Sha256Digest {
    type Err = DigestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.as_bytes();
        if s.len() != 64 {
            return Err(DigestError);
        }
        let mut bytes = [0u8; 32];
        let (pairs, _) = s.as_chunks::<2>();
        for (byte, pair) in bytes.iter_mut().zip(pairs) {
            let pair = std::str::from_utf8(pair).map_err(|_| DigestError)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| DigestError)?;
        }
        Ok(Self(bytes))
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        digest.to_string()
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = DigestError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// One of the checks [`decide`] makes, named in a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Check {
    /// The peer's uid is the owner's.
    Uid,
    /// The peer's SID is the owner's.
    Sid,
    /// The OS named the peer's process.
    Pid,
    /// The peer is in this logon session.
    Session,
    /// The peer's effective gid is the setgid install's group.
    Group,
    /// The peer runs the installed copy.
    Path,
    /// The peer's code satisfies the pinned requirement.
    Requirement,
    /// The peer's image has the pinned SHA-256.
    ImageSha256,
    /// The peer's image is signed by the pinned signer.
    Signer,
    /// The peer is still the process that was checked before reading.
    Token,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Check::Uid => "uid",
            Check::Sid => "user SID",
            Check::Pid => "process id",
            Check::Session => "logon session",
            Check::Group => "group",
            Check::Path => "executable path",
            Check::Requirement => "code requirement",
            Check::ImageSha256 => "image SHA-256",
            Check::Signer => "signer",
            Check::Token => "audit token",
        })
    }
}

/// Why a check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Reason {
    /// The OS did not report the fact the check needs.
    Missing,
    /// The OS reported it, and it is not what was pinned.
    Mismatch,
}

/// A check that failed, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Refusal {
    /// The check.
    pub check: Check,
    /// Why it failed.
    pub reason: Reason,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = match self.reason {
            Reason::Missing => "not reported by the OS",
            Reason::Mismatch => "not the expected value",
        };
        write!(f, "{}: {why}", self.check)
    }
}

/// Whether to trust the program at the other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every check passed.
    Accept,
    /// The first check that failed.
    Refuse(Refusal),
}

impl Verdict {
    /// A refusal by `check`, for `reason`.
    #[must_use]
    pub const fn refuse(check: Check, reason: Reason) -> Self {
        Verdict::Refuse(Refusal { check, reason })
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Accept => f.write_str("accepted"),
            Verdict::Refuse(refusal) => write!(f, "refused: {refusal}"),
        }
    }
}

/// Compares what the OS said about the peer with what the install pinned.
///
/// The checks run in a fixed order, and the first that fails is the one
/// named: the process, then the user, then the session (Windows), then the
/// executable, then the code. A fact a check needs and the OS did not give is
/// a refusal.
#[must_use]
pub fn decide(facts: &Facts, expected: &Expected) -> Verdict {
    match run_checks(facts, expected) {
        Ok(()) => Verdict::Accept,
        Err(refusal) => Verdict::Refuse(refusal),
    }
}

/// Decides on the facts read before the first frame and again on those read
/// after it, and requires that both describe the same process.
///
/// On macOS the kernel names the peer by whichever process last used its
/// socket, at the moment it is asked. A program could connect and write, then
/// exec the genuine binary, or hand the socket to a child; the audit token,
/// whose pid version changes on every fork and exec, is what tells the two
/// apart. Elsewhere the OS records the peer at connect, and the pid is
/// compared.
#[must_use]
pub fn recheck(first: &Facts, again: &Facts, expected: &Expected) -> Verdict {
    let same = || -> Result<(), Refusal> {
        run_checks(first, expected)?;
        run_checks(again, expected)?;
        if let Expected::Macos { .. } = expected {
            same_fact(first.token, again.token, Check::Token)?;
        }
        same_fact(first.pid, again.pid, Check::Pid)
    };
    match same() {
        Ok(()) => Verdict::Accept,
        Err(refusal) => Verdict::Refuse(refusal),
    }
}

fn run_checks(facts: &Facts, expected: &Expected) -> Result<(), Refusal> {
    present(facts.pid.as_ref(), Check::Pid)?;
    match expected {
        Expected::Macos {
            uid,
            path,
            requirement,
        } => {
            equal(facts.uid.as_ref(), uid, Check::Uid)?;
            equal(facts.path.as_ref(), path, Check::Path)?;
            match present(facts.code.as_ref(), Check::Requirement)? {
                Code::Requirement {
                    requirement: checked,
                    satisfied: true,
                } if checked == requirement => Ok(()),
                Code::Requirement { .. } => Err(mismatch(Check::Requirement)),
                _ => Err(missing(Check::Requirement)),
            }
        }
        Expected::Linux { uid, path, pin } => {
            equal(facts.uid.as_ref(), uid, Check::Uid)?;
            match pin {
                LinuxPin::Group(gid) => equal(facts.egid.as_ref(), gid, Check::Group),
                LinuxPin::DevelopmentSha256(sha256) => {
                    equal(facts.path.as_ref(), path, Check::Path)?;
                    image_sha256(facts.code.as_ref(), sha256)
                }
            }
        }
        Expected::Windows {
            sid,
            path,
            pin,
            session,
        } => {
            equal(facts.sid.as_ref(), sid, Check::Sid)?;
            let own = present(session.as_ref(), Check::Session)?;
            equal(facts.session.as_ref(), own, Check::Session)?;
            equal(facts.path.as_ref(), path, Check::Path)?;
            match pin {
                WindowsPin::Sha256(sha256) => image_sha256(facts.code.as_ref(), sha256),
                WindowsPin::Signer(subject) => match present(facts.code.as_ref(), Check::Signer)? {
                    Code::Signer { subject: signer } if signer == subject => Ok(()),
                    Code::Signer { .. } => Err(mismatch(Check::Signer)),
                    _ => Err(missing(Check::Signer)),
                },
            }
        }
    }
}

fn image_sha256(code: Option<&Code>, pinned: &Sha256Digest) -> Result<(), Refusal> {
    match present(code, Check::ImageSha256)? {
        Code::ImageSha256(sha256) if sha256 == pinned => Ok(()),
        Code::ImageSha256(_) => Err(mismatch(Check::ImageSha256)),
        _ => Err(missing(Check::ImageSha256)),
    }
}

fn present<T>(fact: Option<&T>, check: Check) -> Result<&T, Refusal> {
    fact.ok_or(missing(check))
}

fn equal<T: PartialEq>(fact: Option<&T>, pinned: &T, check: Check) -> Result<(), Refusal> {
    if present(fact, check)? == pinned {
        Ok(())
    } else {
        Err(mismatch(check))
    }
}

fn same_fact<T: PartialEq>(
    first: Option<T>,
    again: Option<T>,
    check: Check,
) -> Result<(), Refusal> {
    match (first, again) {
        (Some(first), Some(again)) if first == again => Ok(()),
        (Some(_), Some(_)) => Err(mismatch(check)),
        _ => Err(missing(check)),
    }
}

const fn missing(check: Check) -> Refusal {
    Refusal {
        check,
        reason: Reason::Missing,
    }
}

const fn mismatch(check: Check) -> Refusal {
    Refusal {
        check,
        reason: Reason::Mismatch,
    }
}
