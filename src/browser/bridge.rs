//! The bridge between a run and Chrome's jev-pilot extension, as values: the
//! frames, the messages, the handshake and the constants both ends agree on.

use std::io::{self, Read};

/// The name Chrome looks the host up by, in its native messaging manifest.
pub const HOST_NAME: &str = "dev.jevpilot.bridge";

/// The version of this protocol. Both ends say which they speak.
pub const PROTOCOL: u32 = 1;

/// The extension's ID, which the host manifest allows and nothing else.
///
/// Chrome derives an unpacked extension's ID from the public key in its
/// manifest, so the key below pins it wherever the folder is loaded from.
pub const EXTENSION_ID: &str = "eckgjcpdiehapkphgcflmcikippoegcp";

/// The public key in the extension's manifest (`key`), as base64 DER. Only
/// the public half exists: Chrome needs nothing more to derive the ID from
/// an unpacked folder, and the private half was never kept.
pub const EXTENSION_KEY: &str = concat!(
    "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwvv4+qZfjTcyMjkiAwty",
    "71pf796lx5WteLL8kNlgOYq7lK5tJB59p5Eoy/p106DUVCD9/jTjiCUi6cHCFQnl",
    "9tPQyK2yrvDz21+H9lNbvs/kWTK5SrePfra3MrktmFOEQ8/QpqaG7oz1KtPIwwHc",
    "Q6slt3TUJcm9zPydWP5yPrQNM0o8GeeZUlLIPAFbEr1LACn9pYrYag2WgVhGIOxh",
    "iYnWqGNBc75I4eb0VZM3Kj2XK6PX37GahjONhj8TGVMgPOJUj+EedeOan/acTfZS",
    "3qsnGcVrQwSuggd+cLfbTyid8JwoWYeyln4pmj+J/cXRfehEI0fRyPVHzH+Dw59J",
    "EQIDAQAB",
);

/// The largest message that may go to Chrome. Chrome closes the channel on a
/// larger one.
pub const TO_CHROME_LIMIT: usize = 1 << 20;

/// The largest message read from the bridge. A page read past this is not
/// read at all.
pub const READ_LIMIT: usize = 64 << 20;

/// The byte order of a frame's length prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// Little-endian, on the local channel between a run and the host.
    Little,
    /// The machine's own order, which Chrome's native messaging uses between
    /// the host and the extension.
    Native,
}

impl ByteOrder {
    fn bytes(self, length: u32) -> [u8; 4] {
        match self {
            Self::Little => length.to_le_bytes(),
            Self::Native => length.to_ne_bytes(),
        }
    }

    fn length(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Native => u32::from_ne_bytes(bytes),
        }
    }
}

/// One message as a frame: its length in four bytes, then the message.
#[must_use]
pub fn encode(message: &[u8], order: ByteOrder) -> Vec<u8> {
    let length = u32::try_from(message.len()).unwrap_or(u32::MAX);
    let mut frame = Vec::with_capacity(message.len() + 4);
    frame.extend_from_slice(&order.bytes(length));
    frame.extend_from_slice(message);
    frame
}

/// One message headed for Chrome as a frame.
///
/// # Errors
/// Returns [`FrameError::TooLargeForChrome`] for a message over
/// [`TO_CHROME_LIMIT`].
pub fn encode_for_chrome(message: &[u8], order: ByteOrder) -> Result<Vec<u8>, FrameError> {
    if message.len() > TO_CHROME_LIMIT {
        return Err(FrameError::TooLargeForChrome {
            length: message.len(),
        });
    }
    Ok(encode(message, order))
}

/// Read the next frame's message. `None` when the stream ends between
/// frames.
///
/// # Errors
/// Returns [`FrameError`] when reading fails, the stream ends inside a
/// frame, or the frame claims more than [`READ_LIMIT`]; such a frame is
/// refused from its length alone, before anything is allocated for it.
pub fn decode(reader: &mut impl Read, order: ByteOrder) -> Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        match reader.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let length = order.length(prefix) as usize;
    if length > READ_LIMIT {
        return Err(FrameError::TooLargeToRead { length });
    }
    let mut message = vec![0u8; length];
    reader
        .read_exact(&mut message)
        .map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::Truncated,
            _ => FrameError::Io(e),
        })?;
    Ok(Some(message))
}

/// Why a frame could not be read or built.
#[derive(Debug)]
#[non_exhaustive]
pub enum FrameError {
    /// The stream ended inside a frame.
    Truncated,
    /// Reading or writing the stream failed.
    Io(io::Error),
    /// A message for Chrome over [`TO_CHROME_LIMIT`].
    TooLargeForChrome {
        /// Its length in bytes.
        length: usize,
    },
    /// A frame claiming more than [`READ_LIMIT`].
    TooLargeToRead {
        /// The length it claimed.
        length: usize,
    },
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => f.write_str("the bridge closed in the middle of a message"),
            Self::Io(e) => write!(f, "the bridge failed: {e}"),
            Self::TooLargeForChrome { .. } => {
                f.write_str("that command is too large to pass to Chrome (over 1 MiB)")
            }
            Self::TooLargeToRead { .. } => {
                f.write_str("the page is too large to read through the extension")
            }
        }
    }
}

impl core::error::Error for FrameError {}

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A Chrome tab, by the extension's own id for it.
pub type TabId = i64;

/// One message on the bridge, told apart by its `type`.
///
/// The run and the extension speak through the host, which reads only the
/// messages meant for it and passes the rest on unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Message {
    /// Host to run, before anything else: a fresh nonce to echo.
    Nonce {
        /// 32 random bytes, as 64 hex digits.
        nonce: String,
    },
    /// Run to host, its first frame: the nonce it was given, and what the
    /// run is for, which the host passes on to the extension to show.
    Echo {
        /// The nonce, as received.
        nonce: String,
        /// The run's goal.
        #[serde(default)]
        goal: String,
        /// The sites the run may go to.
        #[serde(default)]
        sites: Vec<String>,
        /// The process that started the run, for the audit log. The run
        /// says it of itself, and the host believes it only because it has
        /// already found the run to be the installed jev-pilot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_pid: Option<u32>,
        /// That process's executable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_exe: Option<String>,
    },
    /// Extension to host, on connecting.
    Hello(Hello),
    /// Extension to host, when what it said in its hello changes.
    Status(Hello),
    /// Host to run: the bridge is the run's.
    Ready {
        /// What the extension said, as of now.
        #[serde(flatten)]
        hello: Hello,
        /// The host's own version.
        host_version: String,
    },
    /// Host to run: another run holds the bridge.
    Busy {
        /// That run's process id.
        pid: u32,
        /// The start of its goal.
        goal: String,
    },
    /// Host to run: no extension is connected.
    Absent,
    /// Host to run: the run failed the host's check of who it is.
    Refused {
        /// Which check failed.
        check: String,
    },
    /// Host to run: the bridge ended, for a reason the run cannot see for
    /// itself. The host exits after sending it.
    Ended {
        /// Why: `too_large`, a message from Chrome over [`READ_LIMIT`].
        reason: String,
    },
    /// Run to extension: one operation.
    Request {
        /// Echoed in the reply.
        id: u64,
        /// What to do.
        #[serde(flatten)]
        op: Op,
    },
    /// Extension to run: how a request went.
    Reply {
        /// The request's id.
        id: u64,
        /// Its result, or why there is none.
        #[serde(flatten)]
        outcome: Outcome,
    },
    /// Extension to run: a DevTools event from a held tab.
    Cdp {
        /// The tab it came from.
        tab: TabId,
        /// The child session, for an event from one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The event's method.
        method: String,
        /// The event's params.
        #[serde(default)]
        params: Value,
    },
    /// Extension to run: a tab was opened while the run was connected.
    TabCreated(TabInfo),
    /// Extension to run: a tab was closed.
    TabRemoved {
        /// The tab.
        tab: TabId,
    },
    /// Extension to run: Chrome let go of a held tab.
    Detached {
        /// The tab.
        tab: TabId,
        /// Chrome's reason, or the extension's own (`left_allowed_sites`).
        reason: String,
        /// Where the tab went, when it left the allowed sites.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Host to extension: a run took the bridge.
    RunStarted {
        /// Its process id.
        pid: u32,
        /// Its goal.
        goal: String,
        /// The sites it may go to.
        sites: Vec<String>,
    },
    /// Host to extension: the run has gone; let go of every tab it held.
    Release,
    /// Host to extension: the files on disk are newer than what is running.
    Reload,
    /// Host to extension: another browser, or another profile, already
    /// serves jev-pilot. The host exits after sending it.
    Elsewhere,
}

/// The process that started a run, as the run knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parent {
    /// Its process id.
    pub pid: Option<u32>,
    /// Its executable.
    pub exe: Option<String>,
}

/// What the extension says about itself and its browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// The bridge protocol it speaks.
    pub protocol: u32,
    /// Its own version.
    pub extension_version: String,
    /// The browser's product, as `Browser.getVersion` gives it.
    pub product: String,
    /// Whether the owner has paused it.
    pub paused: bool,
    /// The policy it loaded, as it loaded it.
    pub policy: Value,
}

/// An operation a run asks of the extension, told apart by its `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Op {
    /// List every tab.
    Tabs,
    /// Open a tab on `url`, in the "jev-pilot" group.
    Create {
        /// Where.
        url: String,
    },
    /// Take hold of a tab with the debugger.
    Attach {
        /// Which.
        tab: TabId,
    },
    /// Let go of a tab.
    Detach {
        /// Which.
        tab: TabId,
    },
    /// Bring a tab to the front.
    Activate {
        /// Which.
        tab: TabId,
    },
    /// Send a DevTools command to a held tab.
    Send {
        /// Which.
        tab: TabId,
        /// A child session of the tab, for a command meant for one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The command.
        method: String,
        /// Its params.
        #[serde(default)]
        params: Value,
    },
}

/// A reply's result, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// What the operation gave.
    Result(Value),
    /// Why it was not done.
    Error(BridgeError),
}

/// Why the extension did not carry out a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeError {
    /// Which kind of refusal or failure.
    pub code: ErrorCode,
    /// In words.
    pub message: String,
}

/// The kinds of refusal and failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// A DevTools method the engine does not use.
    RefusedMethod,
    /// A page script that is not one of the engine's own.
    RefusedScript,
    /// A site not on the allowed list.
    RefusedSite,
    /// A page extensions may not drive.
    RefusedPage,
    /// A tab the run may not take.
    RefusedTab,
    /// The owner paused the extension.
    Paused,
    /// No such tab.
    NoTab,
    /// Chrome would not attach.
    AttachFailed,
    /// Chrome's own error, in its words.
    Chrome,
}

/// A tab as the extension lists it, and as it reports one opening.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    /// The extension's id for it.
    pub tab: TabId,
    /// Its DevTools target id, which is the run's key for it.
    pub target: String,
    /// Where it is.
    pub url: String,
    /// Its title.
    pub title: String,
    /// The tab that opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener: Option<TabId>,
}

/// The host's challenge on the local channel: 32 random bytes the run must
/// echo in its first frame, before the host reads anything else from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nonce([u8; 32]);

impl Nonce {
    /// A nonce from the operating system's entropy source.
    ///
    /// # Panics
    /// Panics when the system has no entropy to give, as no safe nonce can
    /// then be made.
    #[must_use]
    pub fn fresh() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the system's entropy source answers");
        Self(bytes)
    }

    /// The frame the host opens the channel with.
    #[must_use]
    pub fn message(&self) -> Message {
        use core::fmt::Write as _;
        let mut nonce = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(nonce, "{byte:02x}");
        }
        Message::Nonce { nonce }
    }

    /// A run's answer to the host's first frame, when it is a nonce: the
    /// nonce, with the run's goal, the sites it may go to, and the process
    /// that started it.
    #[must_use]
    pub fn echoing(
        first: &Message,
        goal: &str,
        sites: &[String],
        parent: &Parent,
    ) -> Option<Message> {
        match first {
            Message::Nonce { nonce } => Some(Message::Echo {
                nonce: nonce.clone(),
                goal: goal.to_owned(),
                sites: sites.to_vec(),
                parent_pid: parent.pid,
                parent_exe: parent.exe.clone(),
            }),
            _ => None,
        }
    }

    /// Whether `answer` is this nonce, echoed.
    #[must_use]
    pub fn is_echoed_by(&self, answer: &Message) -> bool {
        let Message::Nonce { nonce: ours } = self.message() else {
            return false;
        };
        matches!(answer, Message::Echo { nonce, .. } if *nonce == ours)
    }
}
