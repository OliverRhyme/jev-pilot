//! Moving DevTools messages over one websocket.
//!
//! The only impure part of the engine is [`Socket`], which moves text frames.
//! Everything above it goes through the [`Wire`] trait, so the engine is
//! driven in tests by a scripted wire and never by a browser.
//!
//! There is one blocking connection to the browser endpoint, and every page
//! command reaches its tab through a flattened session on that same socket:
//! Chrome's approval mode serves nothing else, and each new connection would
//! ask the owner again.

use crate::browser::cdp::{Cdp, CdpError, Event, Incoming, ParseError};
use serde_json::Value;
use std::collections::VecDeque;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Something that carries text frames to and from a browser.
pub trait Wire {
    /// Send one text frame.
    ///
    /// # Errors
    /// Returns [`WireError`] when the connection is gone.
    fn send(&mut self, text: &str) -> Result<(), WireError>;

    /// Wait up to `timeout` for the next text frame.
    ///
    /// `Ok(None)` means nothing arrived in time, and the caller takes it as
    /// the whole of its wait having run out.
    ///
    /// # Errors
    /// Returns [`WireError`] when the connection is gone.
    fn receive(&mut self, timeout: Duration) -> Result<Option<String>, WireError>;

    /// Close the connection. A wire with nothing to close does nothing.
    fn close(&mut self) {}
}

/// The connection to the browser failed.
#[derive(Debug)]
pub enum WireError {
    /// The browser closed the connection, or exited.
    Closed,
    /// Reading or writing the socket failed.
    Io(io::Error),
    /// The websocket itself misbehaved.
    Protocol(Box<str>),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Closed => f.write_str("the browser closed the connection"),
            Self::Io(e) => write!(f, "the connection to the browser failed: {e}"),
            Self::Protocol(e) => write!(f, "the browser's websocket misbehaved: {e}"),
        }
    }
}

impl core::error::Error for WireError {}

/// A call that did not get its answer.
#[derive(Debug)]
pub enum TransportError {
    /// The connection failed.
    Wire(WireError),
    /// No reply arrived in time.
    Timeout {
        /// The method that was called.
        method: Box<str>,
    },
    /// The browser answered with an error.
    Refused {
        /// The method that was called.
        method: Box<str>,
        /// What the browser said.
        error: CdpError,
    },
    /// The browser sent something that is not a DevTools message.
    Malformed(ParseError),
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Wire(e) => e.fmt(f),
            Self::Timeout { method } => write!(f, "{method} got no reply in time"),
            Self::Refused { method, error } => write!(f, "{method} failed: {error}"),
            Self::Malformed(e) => e.fmt(f),
        }
    }
}

impl core::error::Error for TransportError {}

impl From<WireError> for TransportError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

/// How a wait for a reply ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Waited {
    /// The reply arrived.
    Reply(Value),
    /// An event the caller asked to be woken for arrived first. It is queued
    /// with the others, and the reply may still come.
    Interrupted,
}

/// Requests and replies over one wire, with the events in between kept.
#[derive(Debug)]
pub struct Connection<W> {
    wire: W,
    cdp: Cdp,
    events: VecDeque<Event>,
}

impl<W: Wire> Connection<W> {
    /// Talk over this wire.
    pub fn new(wire: W) -> Self {
        Self {
            wire,
            cdp: Cdp::new(),
            events: VecDeque::new(),
        }
    }

    /// The wire underneath.
    pub fn wire(&self) -> &W {
        &self.wire
    }

    /// Give the wire back, with whatever events were still queued dropped.
    pub fn into_wire(self) -> W {
        self.wire
    }

    /// Close the wire.
    pub fn close(&mut self) {
        self.wire.close();
    }

    /// Send a request without waiting for its reply, and return its id.
    ///
    /// # Errors
    /// Returns [`TransportError`] when the connection is gone.
    pub fn send(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<u64, TransportError> {
        let request = self.cdp.request(method, params, session);
        self.wire.send(&request.message.to_string())?;
        Ok(request.id)
    }

    /// Wait for the reply to request `id`, queueing every event that arrives
    /// first, and stopping early on one that `interrupt` picks out.
    ///
    /// Replies to other requests, such as one that timed out earlier, are
    /// dropped: only one call is ever in flight.
    ///
    /// # Errors
    /// Returns [`TransportError`] when the reply is an error, does not arrive
    /// within `timeout`, or the connection fails.
    pub fn wait(
        &mut self,
        id: u64,
        method: &str,
        timeout: Duration,
        interrupt: &dyn Fn(&Event) -> bool,
    ) -> Result<Waited, TransportError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(text) = self.wire.receive(left)? else {
                return Err(TransportError::Timeout {
                    method: method.into(),
                });
            };
            match Cdp::parse(&text).map_err(TransportError::Malformed)? {
                Incoming::Reply { id: got, outcome } if got == id => {
                    return outcome
                        .map(Waited::Reply)
                        .map_err(|error| TransportError::Refused {
                            method: method.into(),
                            error,
                        });
                }
                Incoming::Reply { .. } => {}
                Incoming::Event(event) => {
                    let wake = interrupt(&event);
                    self.events.push_back(event);
                    if wake {
                        return Ok(Waited::Interrupted);
                    }
                }
            }
        }
    }

    /// Send a request and wait for its reply.
    ///
    /// # Errors
    /// Returns [`TransportError`] when the reply is an error, does not arrive
    /// within `timeout`, or the connection fails.
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, TransportError> {
        let id = self.send(method, params, session)?;
        match self.wait(id, method, timeout, &|_| false)? {
            Waited::Reply(value) => Ok(value),
            Waited::Interrupted => unreachable!("nothing interrupts a plain call"),
        }
    }

    /// The next event: a queued one, or else one that arrives within
    /// `timeout`.
    ///
    /// # Errors
    /// Returns [`TransportError`] when the connection fails.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>, TransportError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(text) = self.wire.receive(left)? else {
                return Ok(None);
            };
            if let Incoming::Event(event) = Cdp::parse(&text).map_err(TransportError::Malformed)? {
                return Ok(Some(event));
            }
        }
    }

    /// Every event queued so far, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }
}

/// Why a websocket to the browser could not be opened.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectError {
    /// Nothing is listening on that port.
    Refused,
    /// The browser answered the upgrade with an HTTP error, such as `403`
    /// when the owner chose Cancel.
    Rejected {
        /// The HTTP status.
        status: u16,
    },
    /// The browser did not finish the handshake in time.
    TimedOut,
    /// The address is not a `ws://` endpoint this crate can reach.
    BadEndpoint(Box<str>),
    /// Anything else.
    Other(Box<str>),
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Refused => f.write_str("the browser refused the connection"),
            Self::Rejected { status } => {
                write!(f, "the browser rejected the connection ({status})")
            }
            Self::TimedOut => f.write_str("the browser did not answer in time"),
            Self::BadEndpoint(e) => write!(f, "not a usable DevTools endpoint: {e}"),
            Self::Other(e) => write!(f, "could not connect to the browser: {e}"),
        }
    }
}

impl core::error::Error for ConnectError {}

/// A blocking websocket to a browser's DevTools endpoint.
#[derive(Debug)]
pub struct Socket {
    socket: tungstenite::WebSocket<TcpStream>,
}

impl Socket {
    /// Open the websocket at `endpoint` (`ws://127.0.0.1:<port>/devtools/browser/<id>`),
    /// waiting up to `timeout` for the handshake, which in approval mode lasts
    /// until the owner answers Chrome's prompt.
    ///
    /// # Errors
    /// Returns [`ConnectError`] telling refused, rejected and timed-out
    /// connections apart, because each has its own advice.
    pub fn connect(endpoint: &str, timeout: Duration) -> Result<Self, ConnectError> {
        let rest = endpoint
            .strip_prefix("ws://")
            .ok_or_else(|| ConnectError::BadEndpoint(endpoint.into()))?;
        let authority = rest.split('/').next().unwrap_or_default();
        let address = authority
            .to_socket_addrs()
            .map_err(|e| ConnectError::BadEndpoint(e.to_string().into()))?
            .next()
            .ok_or_else(|| ConnectError::BadEndpoint(endpoint.into()))?;
        let stream = TcpStream::connect_timeout(&address, timeout).map_err(|e| match e.kind() {
            io::ErrorKind::ConnectionRefused => ConnectError::Refused,
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => ConnectError::TimedOut,
            _ => ConnectError::Other(e.to_string().into()),
        })?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))
            .map_err(|e| ConnectError::Other(e.to_string().into()))?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(256 << 20))
            .max_frame_size(Some(256 << 20));
        let (socket, _) = tungstenite::client::client_with_config(endpoint, stream, Some(config))
            .map_err(|e| match e {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
                ConnectError::Rejected {
                    status: response.status().as_u16(),
                }
            }
            tungstenite::HandshakeError::Failure(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                ConnectError::TimedOut
            }
            tungstenite::HandshakeError::Interrupted(_) => ConnectError::TimedOut,
            tungstenite::HandshakeError::Failure(e) => ConnectError::Other(e.to_string().into()),
        })?;
        Ok(Self { socket })
    }
}

impl Wire for Socket {
    fn close(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }

    fn send(&mut self, text: &str) -> Result<(), WireError> {
        self.socket
            .send(tungstenite::Message::text(text))
            .map_err(wire_error)
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<String>, WireError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.socket
                .get_ref()
                .set_read_timeout(Some(left))
                .map_err(WireError::Io)?;
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => return Ok(Some(text.as_str().to_owned())),
                Ok(tungstenite::Message::Close(_)) => return Err(WireError::Closed),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(wire_error(e)),
            }
        }
    }
}

fn wire_error(e: tungstenite::Error) -> WireError {
    match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            WireError::Closed
        }
        tungstenite::Error::Io(e) => WireError::Io(e),
        e => WireError::Protocol(e.to_string().into()),
    }
}
