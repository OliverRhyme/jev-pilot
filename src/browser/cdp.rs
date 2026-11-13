//! The Chrome DevTools Protocol as plain values.
//!
//! As [`Adb`](crate::device::adb::Adb) builds argument vectors and leaves
//! running them to someone else, [`Cdp`] builds each request as a JSON value
//! and parses each incoming message, and never touches a socket. Messages go
//! out as `{id, method, params, sessionId?}` and come back as
//! `{id, result | error}` or `{method, params, sessionId?}`, which `serde_json`
//! handles directly; no generated bindings for the whole protocol are needed
//! for the few dozen methods used here.

use serde_json::{Map, Value};

/// Numbers requests and reads replies.
#[derive(Debug, Clone, Default)]
pub struct Cdp {
    next_id: u64,
}

/// One request, ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The id its reply will carry.
    pub id: u64,
    /// The message itself.
    pub message: Value,
}

/// What the browser said went wrong with a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpError {
    /// The JSON-RPC error code.
    pub code: i64,
    /// The browser's own words.
    pub message: Box<str>,
}

impl core::fmt::Display for CdpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl core::error::Error for CdpError {}

/// Something the browser reported without being asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Which event, such as `Page.javascriptDialogOpening`.
    pub method: Box<str>,
    /// What it carries.
    pub params: Value,
    /// The session it came from; `None` for the browser itself.
    pub session: Option<Box<str>>,
}

/// One message from the browser.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// The answer to a request.
    Reply {
        /// The id of the request it answers.
        id: u64,
        /// The result, or what went wrong.
        outcome: Result<Value, CdpError>,
    },
    /// Something that happened.
    Event(Event),
}

/// A message that could not be read as CDP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(Box<str>);

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "not a DevTools message: {}", self.0)
    }
}

impl core::error::Error for ParseError {}

impl Cdp {
    /// Start numbering from one.
    #[must_use]
    pub fn new() -> Self {
        Self { next_id: 1 }
    }

    /// Build a request, giving it the next id.
    ///
    /// `session` names the attached tab a page command goes to; a command to
    /// the browser itself has none.
    pub fn request(&mut self, method: &str, params: Value, session: Option<&str>) -> Request {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        let mut message = Map::new();
        message.insert("id".into(), id.into());
        message.insert("method".into(), method.into());
        message.insert("params".into(), params);
        if let Some(session) = session {
            message.insert("sessionId".into(), session.into());
        }
        Request {
            id,
            message: Value::Object(message),
        }
    }

    /// Read one message from the browser.
    ///
    /// # Errors
    /// Returns [`ParseError`] when the text is not JSON, or is neither a reply
    /// nor an event.
    pub fn parse(text: &str) -> Result<Incoming, ParseError> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| ParseError(e.to_string().into()))?;
        Self::parse_value(value)
    }

    /// Read one message that is already JSON.
    ///
    /// # Errors
    /// Returns [`ParseError`] when it is neither a reply nor an event.
    pub fn parse_value(mut value: Value) -> Result<Incoming, ParseError> {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(error) = value.get("error") {
                return Ok(Incoming::Reply {
                    id,
                    outcome: Err(CdpError {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    }),
                });
            }
            let result = value.get_mut("result").map_or(Value::Null, Value::take);
            return Ok(Incoming::Reply {
                id,
                outcome: Ok(result),
            });
        }
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            let method = method.into();
            let session = value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(Into::into);
            let params = value.get_mut("params").map_or(Value::Null, Value::take);
            return Ok(Incoming::Event(Event {
                method,
                params,
                session,
            }));
        }
        Err(ParseError("neither a reply nor an event".into()))
    }
}
