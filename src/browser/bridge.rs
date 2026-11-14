//! The bridge between a run and Chrome's jev-pilot extension, as values: the
//! frames, the messages, the handshake and the constants both ends agree on.

use std::io::{self, Read};

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
