//! Driving tabs through Chrome's jev-pilot extension instead of Chrome's own
//! websocket.
//!
//! [`ExtensionWire`] is a [`Wire`](crate::browser::transport::Wire) over the
//! local channel to the jev-pilot host, so the engine keeps believing it holds
//! one browser-level DevTools connection with flattened sessions. An
//! extension's debugger reaches one tab per attachment and may not use the
//! browser-level `Target` domain, so a translator maps what the engine sends
//! onto the bridge's operations, and the extension's replies and events back
//! onto DevTools messages. All of it is pure but the stream, which comes from
//! whoever opened the channel.

use crate::browser::bridge::{
    self, BridgeError, ByteOrder, FrameError, Hello, Message, Nonce, Op, Outcome, TabId, TabInfo,
};
use crate::browser::transport::{Wire, WireError};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// A run's connection to Chrome through the jev-pilot extension.
#[derive(Debug)]
pub struct ExtensionWire<S: Read + Write> {
    stream: S,
    hello: Hello,
    host_version: String,
    translator: Translator,
    /// DevTools messages translated and waiting to be received.
    waiting: VecDeque<String>,
}

impl<S: Read + Write> ExtensionWire<S> {
    /// Answer the host's nonce on `stream`, and take the bridge if the host
    /// offers it.
    ///
    /// # Errors
    /// Returns [`ConnectError`] when the other end is not the host, the host
    /// turns the run away, or the extension cannot serve it.
    pub fn connect(mut stream: S) -> Result<Self, ConnectError> {
        let first = read_message(&mut stream)?;
        let Some(echo) = Nonce::echoing(&first) else {
            return Err(ConnectError::NotTheHost {
                why: "it did not open with a nonce".into(),
            });
        };
        write_message(&mut stream, &echo)?;
        match read_message(&mut stream)? {
            Message::Ready {
                hello,
                host_version,
            } => {
                if hello.protocol != bridge::PROTOCOL {
                    return Err(ConnectError::Outdated {
                        extension: hello.extension_version,
                    });
                }
                if hello.paused {
                    return Err(ConnectError::Paused);
                }
                Ok(Self {
                    stream,
                    translator: Translator::new(&hello),
                    hello,
                    host_version,
                    waiting: VecDeque::new(),
                })
            }
            Message::Busy { pid, goal } => Err(ConnectError::Busy { pid, goal }),
            Message::Absent => Err(ConnectError::Absent),
            Message::Refused { check } => Err(ConnectError::Refused { check }),
            _ => Err(ConnectError::NotTheHost {
                why: "it answered the echo with something other than the bridge's state".into(),
            }),
        }
    }

    /// What the extension said about itself and its browser.
    #[must_use]
    pub const fn hello(&self) -> &Hello {
        &self.hello
    }

    /// The host's version.
    #[must_use]
    pub fn host_version(&self) -> &str {
        &self.host_version
    }
}

impl<S: Read + Write> Wire for ExtensionWire<S> {
    fn send(&mut self, text: &str) -> Result<(), WireError> {
        for action in self.translator.outgoing(text) {
            match action {
                Action::Ask(message) => {
                    write_message(&mut self.stream, &message).map_err(wire_error)?;
                }
                Action::Answer(text) => self.waiting.push_back(text),
            }
        }
        Ok(())
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<String>, WireError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(text) = self.waiting.pop_front() {
                return Ok(Some(text));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let message = match read_message(&mut self.stream) {
                Ok(message) => message,
                Err(ConnectError::Frame(FrameError::Io(e)))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(wire_error(e)),
            };
            if let Message::Status(hello) = &message {
                self.hello = hello.clone();
            }
            self.waiting.extend(self.translator.incoming(message));
        }
    }
}

/// What a failure on the channel means to the engine.
fn wire_error(e: ConnectError) -> WireError {
    match e {
        ConnectError::Closed => WireError::Closed,
        ConnectError::Frame(FrameError::Io(e)) => WireError::Io(e),
        other => WireError::Protocol(other.to_string().into()),
    }
}

/// What to do with one message the engine sent.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    /// Ask the extension.
    Ask(Message),
    /// Answer the engine at once, with this DevTools text.
    Answer(String),
}

/// What a reply from the extension answers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// `Target.getTargets`.
    Tabs,
    /// `Target.createTarget`.
    Create,
    /// `Target.attachToTarget`, for this tab.
    Attach(TabId),
    /// `Target.detachFromTarget`, of this session.
    Detach(String),
    /// `Target.activateTarget`.
    Activate,
    /// A command under this session.
    Send(String),
}

/// The mapping between the engine's DevTools messages and the bridge's.
#[derive(Debug)]
struct Translator {
    product: String,
    /// Each tab's target id, which is the engine's key for it.
    targets: HashMap<TabId, String>,
    /// What each request still waiting for its reply was.
    pending: HashMap<u64, Pending>,
    /// The session made up for each attached tab.
    sessions: HashMap<String, TabId>,
    /// How many sessions have been made up.
    made: u64,
}

impl Translator {
    fn new(hello: &Hello) -> Self {
        Self {
            product: hello.product.clone(),
            targets: HashMap::new(),
            pending: HashMap::new(),
            sessions: HashMap::new(),
            made: 0,
        }
    }

    /// The tab whose target id this is.
    fn tab_of(&self, target: Option<&str>) -> Option<TabId> {
        let target = target?;
        self.targets
            .iter()
            .find(|(_, t)| t.as_str() == target)
            .map(|(tab, _)| *tab)
    }

    /// The session made up for a tab.
    fn session_of(&self, tab: TabId) -> Option<&str> {
        self.sessions
            .iter()
            .find(|(_, t)| **t == tab)
            .map(|(s, _)| s.as_str())
    }

    /// Ask the extension, remembering what the reply will answer.
    fn ask(&mut self, id: &Value, op: Op, pending: Pending) -> Vec<Action> {
        let Some(id) = id.as_u64() else {
            return Vec::new();
        };
        self.pending.insert(id, pending);
        vec![Action::Ask(Message::Request { id, op })]
    }

    /// A tab as a DevTools target, remembering its target id.
    fn target_info(&mut self, tab: &TabInfo) -> Value {
        self.targets.insert(tab.tab, tab.target.clone());
        let mut info = json!({
            "targetId": tab.target,
            "type": "page",
            "title": tab.title,
            "url": tab.url,
            "attached": false,
            "canAccessOpener": false,
        });
        if let Some(opener) = tab.opener.and_then(|o| self.targets.get(&o)) {
            info["openerId"] = opener.clone().into();
        }
        info
    }

    /// What one DevTools message from the engine becomes.
    fn outgoing(&mut self, text: &str) -> Vec<Action> {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let param = |key: &str| params.get(key).and_then(Value::as_str);
        // A command under a session is a command to that session's tab.
        if let Some(session) = message.get("sessionId").and_then(Value::as_str) {
            let Some(&tab) = self.sessions.get(session) else {
                return vec![Action::Answer(refusal(
                    &id,
                    "No session with given id found",
                ))];
            };
            let op = Op::Send {
                tab,
                session: None,
                method: method.to_owned(),
                params: params.clone(),
            };
            return self.ask(&id, op, Pending::Send(session.to_owned()));
        }
        let unknown = || {
            vec![Action::Answer(refusal(
                &id,
                "No target with given id found",
            ))]
        };
        match method {
            "Browser.getVersion" => vec![Action::Answer(reply(
                &id,
                &json!({"protocolVersion": "1.3", "product": self.product}),
            ))],
            "Target.getTargets" => self.ask(&id, Op::Tabs, Pending::Tabs),
            "Target.setDiscoverTargets" => vec![Action::Answer(reply(&id, &json!({})))],
            "Target.createTarget" => {
                let url = param("url").unwrap_or("about:blank").to_owned();
                self.ask(&id, Op::Create { url }, Pending::Create)
            }
            "Target.attachToTarget" => match self.tab_of(param("targetId")) {
                Some(tab) => self.ask(&id, Op::Attach { tab }, Pending::Attach(tab)),
                None => unknown(),
            },
            "Target.activateTarget" => match self.tab_of(param("targetId")) {
                Some(tab) => self.ask(&id, Op::Activate { tab }, Pending::Activate),
                None => unknown(),
            },
            "Target.detachFromTarget" => {
                let session = param("sessionId").unwrap_or_default().to_owned();
                match self.sessions.get(&session) {
                    Some(&tab) => self.ask(&id, Op::Detach { tab }, Pending::Detach(session)),
                    None => vec![Action::Answer(refusal(
                        &id,
                        "No session with given id found",
                    ))],
                }
            }
            _ => vec![Action::Answer(refusal(
                &id,
                &format!("{method} is not available through the jev-pilot extension"),
            ))],
        }
    }

    /// What one message from the extension becomes, for the engine.
    fn incoming(&mut self, message: Message) -> Vec<String> {
        match message {
            Message::Reply { id, outcome } => {
                let Some(pending) = self.pending.remove(&id) else {
                    return Vec::new();
                };
                let key = Value::from(id);
                match outcome {
                    Outcome::Result(result) => vec![self.answer(&key, &pending, result)],
                    Outcome::Error(error) => {
                        let session = match &pending {
                            Pending::Send(session) => Some(session.as_str()),
                            _ => None,
                        };
                        vec![coded_refusal(&key, &error, session)]
                    }
                }
            }
            Message::Cdp {
                tab,
                session,
                method,
                params,
            } => {
                let Some(session) = session.or_else(|| self.session_of(tab).map(str::to_owned))
                else {
                    return Vec::new();
                };
                vec![json!({"method": method, "params": params, "sessionId": session}).to_string()]
            }
            Message::TabCreated(tab) => {
                let info = self.target_info(&tab);
                vec![
                    json!({"method": "Target.targetCreated", "params": {"targetInfo": info}})
                        .to_string(),
                ]
            }
            Message::TabRemoved { tab } => match self.targets.remove(&tab) {
                Some(target) => vec![
                    json!({"method": "Target.targetDestroyed", "params": {"targetId": target}})
                        .to_string(),
                ],
                None => Vec::new(),
            },
            Message::Detached { tab, reason, url } => {
                let Some(session) = self.session_of(tab).map(str::to_owned) else {
                    return Vec::new();
                };
                self.sessions.remove(&session);
                let target = self.targets.get(&tab).cloned().unwrap_or_default();
                let mut params =
                    json!({"sessionId": session, "targetId": target, "reason": reason});
                if let Some(url) = url {
                    params["url"] = url.into();
                }
                vec![json!({"method": "Target.detachedFromTarget", "params": params}).to_string()]
            }
            _ => Vec::new(),
        }
    }

    /// The DevTools reply to a request the extension carried out.
    fn answer(&mut self, id: &Value, pending: &Pending, result: Value) -> String {
        match pending {
            Pending::Tabs => {
                let tabs: Vec<TabInfo> = result
                    .get("tabs")
                    .cloned()
                    .and_then(|t| serde_json::from_value(t).ok())
                    .unwrap_or_default();
                // Openers are named by target id, so every tab is known first.
                for tab in &tabs {
                    self.targets.insert(tab.tab, tab.target.clone());
                }
                let infos: Vec<Value> = tabs.iter().map(|t| self.target_info(t)).collect();
                reply(id, &json!({"targetInfos": infos}))
            }
            Pending::Create => {
                let tab = result.get("tab").and_then(Value::as_i64);
                let target = result
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(tab) = tab {
                    self.targets.insert(tab, target.clone());
                }
                reply(id, &json!({"targetId": target}))
            }
            Pending::Attach(tab) => {
                self.made += 1;
                let session = format!("jev-pilot-{}", self.made);
                self.sessions.insert(session.clone(), *tab);
                reply(id, &json!({"sessionId": session}))
            }
            Pending::Detach(session) => {
                self.sessions.remove(session);
                reply(id, &json!({}))
            }
            Pending::Activate => reply(id, &json!({})),
            Pending::Send(session) => {
                json!({"id": id, "result": result, "sessionId": session}).to_string()
            }
        }
    }
}

/// A DevTools reply.
fn reply(id: &Value, result: &Value) -> String {
    json!({"id": id, "result": result}).to_string()
}

/// A DevTools error reply for the extension's refusal, carrying its code as
/// the error's `data`.
fn coded_refusal(id: &Value, error: &BridgeError, session: Option<&str>) -> String {
    let code = serde_json::to_value(error.code).unwrap_or(Value::Null);
    let mut refused =
        json!({"id": id, "error": {"code": -32000, "message": error.message, "data": code}});
    if let Some(session) = session {
        refused["sessionId"] = session.into();
    }
    refused.to_string()
}

/// A DevTools error reply.
fn refusal(id: &Value, message: &str) -> String {
    json!({"id": id, "error": {"code": -32000, "message": message}}).to_string()
}

/// Read one message from the local channel.
fn read_message(stream: &mut impl Read) -> Result<Message, ConnectError> {
    let frame = bridge::decode(stream, ByteOrder::Little)?.ok_or(ConnectError::Closed)?;
    serde_json::from_slice(&frame).map_err(|e| ConnectError::NotTheHost {
        why: format!("it sent something that is not a bridge message: {e}").into(),
    })
}

/// Write one message to the local channel.
fn write_message(stream: &mut impl Write, message: &Message) -> Result<(), ConnectError> {
    let text = serde_json::to_vec(message).unwrap_or_default();
    let frame = bridge::encode_for_chrome(&text, ByteOrder::Little)?;
    stream
        .write_all(&frame)
        .and_then(|()| stream.flush())
        .map_err(|e| ConnectError::Frame(FrameError::Io(e)))
}

/// Why a run could not take the bridge.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectError {
    /// What answered on the channel is not the jev-pilot host.
    NotTheHost {
        /// What gave it away.
        why: Box<str>,
    },
    /// Another run holds the bridge.
    Busy {
        /// Its process id.
        pid: u32,
        /// The start of its goal.
        goal: String,
    },
    /// No extension is connected to the host.
    Absent,
    /// The host would not serve this run.
    Refused {
        /// Which of its checks failed.
        check: String,
    },
    /// The owner has paused the extension.
    Paused,
    /// The extension speaks another version of the protocol.
    Outdated {
        /// Its version.
        extension: String,
    },
    /// The channel closed during the handshake.
    Closed,
    /// A frame could not be read or written.
    Frame(FrameError),
}

impl From<FrameError> for ConnectError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

impl core::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotTheHost { why } => write!(
                f,
                "something other than the installed jev-pilot answered on the bridge ({why}); \
                 nothing was sent"
            ),
            Self::Busy { pid, goal } => write!(
                f,
                "another run (pid {pid}, '{goal}') is using Chrome: stop it first"
            ),
            Self::Absent => f.write_str(
                "Chrome is not running with the jev-pilot extension: open Chrome; if it is open, \
                 check the extension is on in chrome://extensions with Developer mode on \
                 (`jev-pilot extension status` says more)",
            ),
            Self::Refused { check } => write!(
                f,
                "the bridge only serves the installed jev-pilot ({check} failed)"
            ),
            Self::Paused => f.write_str(
                "jev-pilot is paused in Chrome: resume it from the extension's toolbar button",
            ),
            Self::Outdated { extension } => write!(
                f,
                "the extension in Chrome is {extension}, this jev-pilot needs {}: run the \
                 install again",
                env!("CARGO_PKG_VERSION")
            ),
            Self::Closed => f.write_str(
                "the connection to Chrome's jev-pilot extension dropped: Chrome may have quit",
            ),
            Self::Frame(e) => e.fmt(f),
        }
    }
}

impl core::error::Error for ConnectError {}
