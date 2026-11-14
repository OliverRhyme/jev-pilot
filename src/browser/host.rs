#![cfg(feature = "extension")]
//! The jev-pilot host: the program Chrome starts for the extension, which
//! relays frames between one verified run and the extension.
//!
//! [`Relay`] is the host's whole behaviour, as a pure state machine: it is
//! told what happened ([`Input`]) and says what to do ([`Output`]). Threads
//! reading stdin and the local channel feed it, and carry out what it says.
//! It understands nothing of DevTools: it passes the run's requests to the
//! extension and everything the extension says back to the run, answers
//! other runs that the bridge is taken, and notes for the audit log what a
//! run did.

pub mod channel;
mod serve;

pub use serve::{Serve, append_audit, serve};

use std::time::SystemTime;

use serde_json::{Value, json};

use crate::browser::bridge::{
    BridgeError, ErrorCode, FrameError, Hello, Message, Op, Outcome, TO_CHROME_LIMIT, TabId,
};

/// The host's own name for one connection on the local channel.
pub type RunId = u64;

/// A run that has passed the host's checks, as it introduced itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// Its process id, as the OS reported it.
    pub pid: u32,
    /// Its goal.
    pub goal: String,
    /// The sites it may go to.
    pub sites: Vec<String>,
    /// The process that started it, as it said.
    pub parent_pid: Option<u32>,
    /// That process's executable, as it said.
    pub parent_exe: Option<String>,
}

/// What happened, for the relay to answer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Input {
    /// The extension sent a message.
    Extension(Message),
    /// A connection passed the identity checks and echoed the nonce.
    RunVerified {
        /// The connection.
        run: RunId,
        /// What the run said of itself.
        started: Started,
    },
    /// A connection failed an identity check, or the nonce.
    RunRefused {
        /// The connection.
        run: RunId,
        /// Its process id, when the OS gave one.
        pid: Option<u32>,
        /// The check that failed, in words.
        check: String,
    },
    /// A run sent a message.
    Run {
        /// The connection.
        run: RunId,
        /// The message.
        message: Message,
    },
    /// A connection closed.
    RunGone {
        /// The connection.
        run: RunId,
    },
    /// Chrome sent a message too large to read, so stdin can no longer be
    /// read in step.
    ExtensionTooLarge,
    /// Chrome closed stdin.
    StdinClosed,
    /// The installed jev-pilot is this version now.
    Installed {
        /// Its version.
        version: String,
    },
}

/// What the relay asks to be done.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Output {
    /// Send this to the extension.
    ToExtension(Message),
    /// Send this to a run.
    ToRun {
        /// The connection.
        run: RunId,
        /// The message.
        message: Message,
    },
    /// Close a connection.
    Close {
        /// The connection.
        run: RunId,
    },
    /// Append this to the audit log.
    Audit(Audit),
    /// Remove the socket and exit.
    Exit,
}

/// One event in the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Audit {
    /// A run took the bridge.
    RunStarted(Started),
    /// A run took hold of a tab.
    TabAttached {
        /// The run.
        pid: u32,
        /// The tab.
        tab: TabId,
    },
    /// A run opened a tab on a site, or sent one to it.
    SiteVisited {
        /// The run.
        pid: u32,
        /// Where.
        url: String,
    },
    /// A run let go of the bridge.
    RunEnded {
        /// The run.
        pid: u32,
        /// How: `closed` (the run's channel closed), `chrome_closed` (Chrome
        /// closed the host's stdin), or `too_large`.
        how: String,
    },
    /// A connection was turned away by an identity check.
    Refused {
        /// Its process id, when the OS gave one.
        pid: Option<u32>,
        /// The check that failed.
        check: String,
    },
}

impl Audit {
    /// The event as one line of JSON, stamped with `time`.
    #[must_use]
    pub fn line(&self, time: &str) -> String {
        let mut value = match self {
            Self::RunStarted(started) => json!({
                "event": "run_started",
                "pid": started.pid,
                "parent_pid": started.parent_pid,
                "parent_exe": started.parent_exe,
                "goal": started.goal,
                "sites": started.sites,
                "identity": "accepted",
            }),
            Self::TabAttached { pid, tab } => {
                json!({"event": "tab_attached", "pid": pid, "tab": tab})
            }
            Self::SiteVisited { pid, url } => {
                json!({"event": "site_visited", "pid": pid, "url": url})
            }
            Self::RunEnded { pid, how } => json!({"event": "run_ended", "pid": pid, "how": how}),
            Self::Refused { pid, check } => {
                json!({"event": "refused", "pid": pid, "check": check})
            }
        };
        value["time"] = Value::from(time);
        let mut line = value.to_string();
        line.push('\n');
        line
    }
}

/// `at` as a UTC time to the second, `2026-11-15T09:30:00Z`.
#[must_use]
pub fn utc(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// How much of a goal another run is shown.
const GOAL_SHOWN: usize = 60;

/// How much of a run the extension is told: its goal, how many of its
/// sites, and how much of each. At most about 136 KiB in all, well within
/// the 1 MiB a message to Chrome may be.
const GOAL_TOLD: usize = 4096;
const SITES_TOLD: usize = 64;
const SITE_TOLD: usize = 2048;

/// The host's behaviour: one verified run at a time, relayed to the
/// extension.
#[derive(Debug)]
pub struct Relay {
    version: String,
    on_disk: Option<String>,
    hello: Option<Hello>,
    active: Option<(RunId, Started)>,
    leaving: bool,
}

impl Relay {
    /// A relay for a host of `version`, whose extension's files on disk are
    /// of version `on_disk`.
    #[must_use]
    pub fn new(version: &str, on_disk: Option<String>) -> Self {
        Self {
            version: version.to_owned(),
            on_disk,
            hello: None,
            active: None,
            leaving: false,
        }
    }

    /// What to do about `input`.
    pub fn handle(&mut self, input: Input) -> Vec<Output> {
        match input {
            Input::Extension(message) => self.extension_said(message),
            Input::RunVerified { run, started } => self.verified(run, started),
            Input::RunRefused { run, pid, check } => vec![
                Output::ToRun {
                    run,
                    message: Message::Refused {
                        check: check.clone(),
                    },
                },
                Output::Close { run },
                Output::Audit(Audit::Refused { pid, check }),
            ],
            Input::Run { run, message } => self.run_said(run, message),
            Input::RunGone { run } => self.gone(run),
            Input::ExtensionTooLarge => {
                let mut out = Vec::new();
                if let Some((run, started)) = self.active.take() {
                    out.push(Output::ToRun {
                        run,
                        message: Message::Ended {
                            reason: "too_large".into(),
                        },
                    });
                    out.push(Output::Close { run });
                    out.push(ended(started.pid, "too_large"));
                }
                out.extend(self.leave());
                out
            }
            Input::StdinClosed => {
                let mut out = Vec::new();
                if let Some((run, started)) = self.active.take() {
                    out.push(Output::Close { run });
                    out.push(ended(started.pid, "chrome_closed"));
                }
                out.extend(self.leave());
                out
            }
            Input::Installed { version } => {
                if version == self.version || self.leaving {
                    return Vec::new();
                }
                self.leaving = true;
                if self.active.is_some() {
                    Vec::new()
                } else {
                    vec![Output::Exit]
                }
            }
        }
    }

    fn extension_said(&mut self, message: Message) -> Vec<Output> {
        match message {
            Message::Hello(hello) => {
                let behind = self
                    .on_disk
                    .as_deref()
                    .is_some_and(|on_disk| newer(on_disk, &hello.extension_version));
                self.hello = Some(hello);
                if behind {
                    vec![Output::ToExtension(Message::Reload)]
                } else {
                    Vec::new()
                }
            }
            Message::Status(hello) => {
                self.hello = Some(hello);
                Vec::new()
            }
            message @ (Message::Reply { .. }
            | Message::Cdp { .. }
            | Message::TabCreated(_)
            | Message::TabRemoved { .. }
            | Message::Detached { .. }) => match &self.active {
                Some((run, _)) => vec![Output::ToRun { run: *run, message }],
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn verified(&mut self, run: RunId, started: Started) -> Vec<Output> {
        let turned_away = |message| vec![Output::ToRun { run, message }, Output::Close { run }];
        if let Some((_, holder)) = &self.active {
            return turned_away(Message::Busy {
                pid: holder.pid,
                goal: shortened(&holder.goal),
            });
        }
        let Some(hello) = self.hello.clone().filter(|_| !self.leaving) else {
            return turned_away(Message::Absent);
        };
        let out = vec![
            Output::ToRun {
                run,
                message: Message::Ready {
                    hello,
                    host_version: self.version.clone(),
                },
            },
            // Bounded, so the message always fits within what Chrome takes.
            Output::ToExtension(Message::RunStarted {
                pid: started.pid,
                goal: cut(&started.goal, GOAL_TOLD),
                sites: started
                    .sites
                    .iter()
                    .take(SITES_TOLD)
                    .map(|site| cut(site, SITE_TOLD))
                    .collect(),
            }),
            Output::Audit(Audit::RunStarted(started.clone())),
        ];
        self.active = Some((run, started));
        out
    }

    fn run_said(&mut self, run: RunId, message: Message) -> Vec<Output> {
        let Some((_, started)) = self.active.as_ref().filter(|(active, _)| *active == run) else {
            return Vec::new();
        };
        let Message::Request { id, op } = message else {
            return Vec::new();
        };
        let request = Message::Request { id, op };
        let length = serde_json::to_vec(&request).map_or(usize::MAX, |text| text.len());
        if length > TO_CHROME_LIMIT {
            return vec![Output::ToRun {
                run,
                message: Message::Reply {
                    id,
                    outcome: Outcome::Error(BridgeError {
                        code: ErrorCode::Chrome,
                        message: FrameError::TooLargeForChrome { length }.to_string(),
                    }),
                },
            }];
        }
        let pid = started.pid;
        let mut out = Vec::new();
        if let Message::Request { op, .. } = &request {
            match op {
                Op::Create { url } => out.push(Output::Audit(Audit::SiteVisited {
                    pid,
                    url: url.clone(),
                })),
                Op::Attach { tab } => {
                    out.push(Output::Audit(Audit::TabAttached { pid, tab: *tab }));
                }
                Op::Send { method, params, .. } if method == "Page.navigate" => {
                    if let Some(url) = params.get("url").and_then(Value::as_str) {
                        out.push(Output::Audit(Audit::SiteVisited {
                            pid,
                            url: url.to_owned(),
                        }));
                    }
                }
                _ => {}
            }
        }
        out.insert(0, Output::ToExtension(request));
        out
    }

    fn gone(&mut self, run: RunId) -> Vec<Output> {
        if self
            .active
            .as_ref()
            .is_none_or(|(active, _)| *active != run)
        {
            return Vec::new();
        }
        let Some((_, started)) = self.active.take() else {
            return Vec::new();
        };
        let mut out = vec![
            Output::ToExtension(Message::Release),
            ended(started.pid, "closed"),
        ];
        if self.leaving {
            out.push(Output::Exit);
        }
        out
    }

    fn leave(&mut self) -> Vec<Output> {
        self.leaving = true;
        vec![Output::Exit]
    }
}

fn ended(pid: u32, how: &str) -> Output {
    Output::Audit(Audit::RunEnded {
        pid,
        how: how.into(),
    })
}

/// The start of a goal, for another run to be shown.
fn shortened(goal: &str) -> String {
    cut(goal, GOAL_SHOWN)
}

/// The first `chars` characters of `text`, marked as cut when it was.
fn cut(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Whether version `a` is later than version `b`, comparing their dotted
/// numbers. A version that is not dotted numbers is never later.
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|n| n.parse().ok()).collect() };
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}
