//! The engine seam: the one trait a browser engine implements, and its first
//! implementation, over the Chrome DevTools Protocol.
//!
//! An engine reads the driven tab into a [`PageRead`] and carries out
//! [`BrowserCommand`]s, which name rows by the opaque [`NodeKey`]s it issued
//! and tabs by its [`TabKey`]s. A second engine (WebDriver BiDi, say) is a new
//! implementation producing the same page read.
//!
//! [`CdpEngine`] makes every decision from replies it has parsed, so it is
//! driven in tests over a scripted [`Wire`]; only [`Socket`](super::transport::Socket)
//! touches a browser.

use crate::act::Direction;
use crate::browser::cdp::Event;
use crate::browser::input;
use crate::browser::read::{self, KeyParts, Loaders, OwnedTabs, RawRead, ReadError};
use crate::browser::transport::{Connection, TransportError, Waited, Wire};
use crate::platform::page::{PageRead, PageTab, Viewport};
use crate::snapshot::{Dialog, DialogKind, NodeKey, Point, TabKey};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How a JavaScript dialog is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAnswer {
    /// Accept it; a prompt takes these words.
    Accept(Option<Box<str>>),
    /// Dismiss it.
    Dismiss,
}

/// One thing an engine can do to the driven tab.
///
/// The engine's own vocabulary: translating the crate's `Command` into it is
/// the browser device's job, so engines know nothing of the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrowserCommand {
    /// Click the row's node, at this point if it is still the node there.
    Press {
        /// Where the snapshot chose to click, in viewport CSS pixels.
        at: Point,
        /// The row's node.
        node: NodeKey,
    },
    /// Focus the field and replace its contents with these words, or choose
    /// the option they name in a native drop-down.
    Fill {
        /// Where the snapshot chose to click, in viewport CSS pixels.
        at: Point,
        /// The field's node.
        node: NodeKey,
        /// What it should hold.
        text: Box<str>,
    },
    /// Press Enter in the focused field.
    Submit,
    /// Scroll most of a view in this direction.
    Scroll(Direction),
    /// Go back one page in the tab's history.
    Back,
    /// Load this address in the tab.
    Navigate(Box<str>),
    /// Drive another tab the run owns.
    SwitchTab(TabKey),
    /// Answer the dialog that is open.
    Dialog(DialogAnswer),
    /// Let the page finish what it is doing.
    Settle,
}

/// A browser engine: what a browser device drives.
///
/// Implementations release the browser (detach, close the connection) when
/// dropped.
pub trait BrowserEngine {
    /// What can go wrong talking to the browser.
    type Error: core::error::Error;

    /// Read the driven tab.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the tab cannot be read.
    fn read(&mut self) -> Result<PageRead, Self::Error>;

    /// Carry out one command, and let the page settle after it.
    ///
    /// A click the engine decided not to make is not an error: it is reported
    /// by [`outcome`](Self::outcome), and the run goes on.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the browser cannot be reached or fails
    /// the command.
    fn perform(&mut self, command: &BrowserCommand) -> Result<(), Self::Error>;

    /// What the last command did, as the engine saw it.
    fn outcome(&self) -> Option<&str>;

    /// How long the page had gone without a DOM change at the last read.
    fn quiet_for_ms(&self) -> Option<u64> {
        None
    }

    /// The browser's tabs, whether the run owns them or not.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the browser cannot be reached.
    fn tabs(&mut self) -> Result<Vec<PageTab>, Self::Error>;

    /// Drive this tab from now on.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the tab cannot be attached to.
    fn select_tab(&mut self, tab: &TabKey) -> Result<(), Self::Error>;
}

/// How long each kind of wait lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// One ordinary call.
    pub call: Duration,
    /// How long a command waits for a navigation to start.
    pub navigation_start: Duration,
    /// How long a started navigation is given to load.
    pub load: Duration,
    /// The DOM-quiet window.
    pub quiet_window: Duration,
    /// The longest the DOM-quiet wait lasts.
    pub quiet_cap: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            call: Duration::from_secs(10),
            navigation_start: Duration::from_millis(100),
            load: Duration::from_secs(3),
            quiet_window: Duration::from_millis(100),
            quiet_cap: Duration::from_secs(3),
        }
    }
}

/// What can go wrong driving a browser over the DevTools Protocol.
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineError {
    /// A call failed.
    Transport(TransportError),
    /// A read got no answer. The usual cause is a JavaScript dialog that was
    /// open before the run attached: Chrome does not report it again, and the
    /// page answers nothing until it is closed.
    ReadTimedOut,
    /// The replies of a read could not be understood.
    Read(ReadError),
    /// The tab is not one the run owns.
    NotOwned(TabKey),
    /// The tab is not in the browser.
    NoSuchTab(TabKey),
}

impl core::fmt::Display for EngineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(e) => e.fmt(f),
            Self::ReadTimedOut => f.write_str(
                "the page did not answer: a JavaScript dialog may be open in this tab; answer it in Chrome",
            ),
            Self::Read(e) => e.fmt(f),
            Self::NotOwned(tab) => write!(f, "tab {} is not one this run owns", tab.as_str()),
            Self::NoSuchTab(tab) => write!(f, "there is no tab {}", tab.as_str()),
        }
    }
}

impl core::error::Error for EngineError {}

impl From<TransportError> for EngineError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

/// Why a call through the driven session did not return a reply.
enum Stop {
    /// A dialog opened, and the page will answer nothing until it is closed.
    Dialog,
    Failed(EngineError),
}

impl From<TransportError> for Stop {
    fn from(e: TransportError) -> Self {
        Self::Failed(e.into())
    }
}

impl From<EngineError> for Stop {
    fn from(e: EngineError) -> Self {
        Self::Failed(e)
    }
}

/// What happened while a command ran, for its outcome line.
#[derive(Debug, Default)]
struct Seen {
    navigated: Option<Box<str>>,
    navigation_started: bool,
    loaded: bool,
    new_tab: Option<Box<str>>,
    dialog: Option<Box<str>>,
}

/// The CDP engine: one flattened session on the driven tab, over one wire.
#[derive(Debug)]
pub struct CdpEngine<W: Wire> {
    connection: Connection<W>,
    timeouts: Timeouts,
    target: Box<str>,
    session: Box<str>,
    owned: OwnedTabs,
    loaders: Loaders,
    viewport: Option<Viewport>,
    dialog: Option<Dialog>,
    accepted_alert: Option<Box<str>>,
    outcome: Option<Box<str>>,
    quiet_for_ms: Option<u64>,
    mutations: Option<u64>,
    world: Option<i64>,
    seen: Seen,
}

impl<W: Wire> CdpEngine<W> {
    /// Attach to an existing tab and drive it.
    ///
    /// `opened_by_run` says whether the run created the tab, as against
    /// starting on one the owner had open.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the browser refuses the session.
    pub fn attach(wire: W, tab: &TabKey, opened_by_run: bool) -> Result<Self, EngineError> {
        Self::attach_with(wire, tab, opened_by_run, Timeouts::default())
    }

    /// [`attach`](Self::attach), with other waits.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the browser refuses the session.
    pub fn attach_with(
        wire: W,
        tab: &TabKey,
        opened_by_run: bool,
        timeouts: Timeouts,
    ) -> Result<Self, EngineError> {
        let mut engine = Self {
            connection: Connection::new(wire),
            timeouts,
            target: tab.as_str().into(),
            session: "".into(),
            owned: OwnedTabs::default(),
            loaders: Loaders::default(),
            viewport: None,
            dialog: None,
            accepted_alert: None,
            outcome: None,
            quiet_for_ms: None,
            mutations: None,
            world: None,
            seen: Seen::default(),
        };
        // Listed before discovery starts, because discovery reports every tab
        // already open as if it had just been created, opener and all.
        let open_already = engine.browser_call("Target.getTargets", json!({}))?;
        for info in open_already
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = info.get("targetId").and_then(Value::as_str)
                && id != tab.as_str()
            {
                engine.owned.existed(id);
            }
        }
        engine.browser_call("Target.setDiscoverTargets", json!({"discover": true}))?;
        engine.owned.start_on(tab.as_str(), opened_by_run);
        engine.attach_session(tab.as_str())?;
        Ok(engine)
    }

    /// Open a new tab on `url` and drive it.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the browser refuses the tab or the
    /// session.
    pub fn open(wire: W, url: &str) -> Result<Self, EngineError> {
        let mut connection = Connection::new(wire);
        // Made blank and then sent to its address, so its load is waited for
        // as any navigation's is. Created straight at the address, the tab
        // was read before it had committed its navigation: no address, and
        // no rows.
        let created = connection.call(
            "Target.createTarget",
            json!({"url": "about:blank"}),
            None,
            Timeouts::default().call,
        )?;
        let target = created
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let wire = connection.into_wire();
        let mut engine = Self::attach(wire, &TabKey::new(target), true)?;
        engine.perform(&BrowserCommand::Navigate(url.into()))?;
        Ok(engine)
    }

    /// The wire underneath.
    pub fn wire(&self) -> &W {
        self.connection.wire()
    }

    /// The tab being driven.
    #[must_use]
    pub fn current_tab(&self) -> TabKey {
        TabKey::new(&*self.target)
    }

    /// Close the whole browser, as a run that launched it does at its end.
    ///
    /// # Errors
    /// Returns [`EngineError`] when the browser cannot be reached.
    pub fn close_browser(&mut self) -> Result<(), EngineError> {
        self.connection.send("Browser.close", json!({}), None)?;
        Ok(())
    }

    fn attach_session(&mut self, target: &str) -> Result<(), EngineError> {
        let attached = self.browser_call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
        )?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.session = session.into();
        self.target = target.into();
        self.world = None;
        self.dialog = None;
        self.viewport = None;
        // A tab under a dialog that opened before the run attached answers
        // nothing, Page.enable included, and Chrome never reports the dialog.
        self.call("Page.enable", json!({}))
            .map_err(|stop| match stop {
                Stop::Dialog
                | Stop::Failed(EngineError::Transport(TransportError::Timeout { .. })) => {
                    EngineError::ReadTimedOut
                }
                Stop::Failed(e) => e,
            })?;
        Ok(())
    }

    /// A call to the browser itself, outside any tab's session.
    fn browser_call(&mut self, method: &str, params: Value) -> Result<Value, EngineError> {
        let value = self
            .connection
            .call(method, params, None, self.timeouts.call)?;
        self.pump();
        Ok(value)
    }

    /// A call to the driven tab, given up as soon as a dialog opens that
    /// needs an answer. An alert is answered on the spot and the wait goes on.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Stop> {
        self.call_within(method, params, self.timeouts.call)
    }

    fn call_within(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Stop> {
        let id = self.connection.send(method, params, Some(&self.session))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let waited = self.connection.wait(id, method, left, &|event: &Event| {
                &*event.method == "Page.javascriptDialogOpening"
            });
            match waited {
                Ok(Waited::Reply(value)) => {
                    self.pump();
                    return Ok(value);
                }
                Ok(Waited::Interrupted) => {
                    self.pump();
                    if self.dialog.is_some() {
                        return Err(Stop::Dialog);
                    }
                }
                Err(e) => {
                    self.pump();
                    return Err(e.into());
                }
            }
        }
    }

    /// Handle every queued event.
    fn pump(&mut self) {
        for event in self.connection.take_events() {
            self.handle(&event);
        }
    }

    fn ours(&self, event: &Event) -> bool {
        event.session.as_deref() == Some(&*self.session)
    }

    fn handle(&mut self, event: &Event) {
        let p = &event.params;
        let str_of = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default();
        match &*event.method {
            "Page.javascriptDialogOpening" if self.ours(event) => {
                let message: Box<str> = str_of("message").into();
                self.seen.dialog = Some(message.clone());
                let kind = match str_of("type") {
                    "alert" => None,
                    "prompt" => Some(DialogKind::Prompt),
                    "beforeunload" => Some(DialogKind::BeforeUnload),
                    _ => Some(DialogKind::Confirm),
                };
                match kind {
                    // An alert has one button; answering it is all a person
                    // could do. Sent without waiting: this may run while
                    // another call's reply is awaited.
                    None => {
                        let _ = self.connection.send(
                            "Page.handleJavaScriptDialog",
                            json!({"accept": true}),
                            Some(&self.session),
                        );
                        self.accepted_alert = Some(message);
                    }
                    Some(kind) => {
                        let default_prompt = (kind == DialogKind::Prompt)
                            .then(|| str_of("defaultPrompt"))
                            .filter(|d| !d.is_empty())
                            .map(Into::into);
                        self.dialog = Some(Dialog {
                            kind,
                            message,
                            default_prompt,
                        });
                    }
                }
            }
            "Page.javascriptDialogClosed" if self.ours(event) => self.dialog = None,
            "Page.frameStartedLoading"
                if self.ours(event) && str_of("frameId") == &*self.target =>
            {
                self.seen.navigation_started = true;
            }
            "Page.frameNavigated" if self.ours(event) => {
                let frame = p.get("frame").cloned().unwrap_or_default();
                if frame.get("parentId").is_none() {
                    self.seen.navigation_started = true;
                    let url = frame.get("url").and_then(Value::as_str).unwrap_or_default();
                    self.seen.navigated = Some(origin(url).into());
                    self.world = None;
                }
            }
            "Page.loadEventFired" if self.ours(event) => self.seen.loaded = true,
            // Only a tab created while the run watches can be the run's; a
            // change to a tab says nothing about who it belongs to.
            "Target.targetCreated" => {
                let info = p.get("targetInfo").cloned().unwrap_or_default();
                let id = info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if info.get("type").and_then(Value::as_str) != Some("page") || id.is_empty() {
                    return;
                }
                let known = self.owned.owns(id);
                let opener = info.get("openerId").and_then(Value::as_str);
                if self.owned.adopt(id, opener) && !known {
                    let title = info
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
                    let shown = if title.is_empty() { url } else { title };
                    self.seen.new_tab = Some(shown.into());
                }
            }
            "Target.targetDestroyed" => self.owned.forget(str_of("targetId")),
            _ => {}
        }
    }

    /// A read made of the dialog alone, when the page is blocked under one
    /// and would answer nothing.
    fn dialog_read(&mut self) -> Result<PageRead, EngineError> {
        let targets = self.browser_call("Target.getTargets", json!({}))?;
        let info = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|t| t.get("targetId").and_then(Value::as_str) == Some(&*self.target));
        let field = |k: &str| {
            info.and_then(|t| t.get(k))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into()
        };
        let tabs = read::owned_tabs(&targets, &self.owned);
        Ok(PageRead {
            url: field("url"),
            title: field("title"),
            viewport: self.viewport.unwrap_or(Viewport {
                width: 0.0,
                height: 0.0,
            }),
            dialog: self.dialog.clone(),
            accepted_alert: self.accepted_alert.take(),
            tabs,
            current: Some(TabKey::new(&*self.target)),
            unreadable_frames: 0,
            nodes: Vec::new(),
        })
    }

    fn full_read(&mut self) -> Result<PageRead, Stop> {
        let frame_tree = self.read_call("Page.getFrameTree", json!({}))?;
        let mut ax_trees = vec![(
            self.target.clone(),
            self.read_call(
                "Accessibility.getFullAXTree",
                json!({"frameId": &*self.target}),
            )?,
        )];
        let mut unreadable_frames = 0;
        for frame in read::child_frames(&frame_tree) {
            // A frame in another process has its own target; its tree is not
            // reachable from this session, and it is left out and counted.
            match self.call("Accessibility.getFullAXTree", json!({"frameId": frame})) {
                Ok(tree) => ax_trees.push((frame.into(), tree)),
                Err(Stop::Dialog) => return Err(Stop::Dialog),
                Err(Stop::Failed(_)) => unreadable_frames += 1,
            }
        }
        let snapshot = self.read_call(
            "DOMSnapshot.captureSnapshot",
            json!({"computedStyles": [], "includePaintOrder": true, "includeDOMRects": true}),
        )?;
        let layout_metrics = self.read_call("Page.getLayoutMetrics", json!({}))?;
        let targets = self.browser_call("Target.getTargets", json!({}))?;
        let raw = RawRead {
            frame_tree,
            ax_trees,
            snapshot,
            layout_metrics,
            targets,
        };
        let mut read = read::translate(&raw, &self.target, &self.owned, &mut self.loaders)
            .map_err(EngineError::Read)?;
        if let Some((quiet, mutations)) = self.watch(&input::quiet_now(), false) {
            self.quiet_for_ms = Some(quiet);
            self.mutations = Some(mutations);
        }
        read.unreadable_frames = unreadable_frames;
        read.accepted_alert = self.accepted_alert.take();
        read.dialog.clone_from(&self.dialog);
        self.viewport = Some(read.viewport);
        Ok(read)
    }

    /// A call made while reading, where no answer means a blocked page.
    fn read_call(&mut self, method: &str, params: Value) -> Result<Value, Stop> {
        self.call(method, params).map_err(|stop| match stop {
            Stop::Failed(EngineError::Transport(TransportError::Timeout { .. })) => {
                Stop::Failed(EngineError::ReadTimedOut)
            }
            other => other,
        })
    }

    /// Evaluate one of the watch's expressions in the isolated world, making
    /// the world first if this document has none. `None` when the page would
    /// not answer; the watch is a help, never a requirement.
    fn watch(&mut self, expression: &str, wait: bool) -> Option<(u64, u64)> {
        for _ in 0..2 {
            let context = if let Some(context) = self.world {
                context
            } else {
                let made = self
                    .call(
                        "Page.createIsolatedWorld",
                        json!({"frameId": &*self.target, "worldName": "jev-pilot", "grantUniveralAccess": false}),
                    )
                    .ok()?;
                let context = made.get("executionContextId").and_then(Value::as_i64)?;
                self.world = Some(context);
                context
            };
            let mut params =
                json!({"expression": expression, "contextId": context, "returnByValue": true});
            let timeout = if wait {
                params["awaitPromise"] = true.into();
                self.timeouts.quiet_cap + self.timeouts.call
            } else {
                self.timeouts.call
            };
            match self.call_within("Runtime.evaluate", params, timeout) {
                Ok(answer) => {
                    let value = answer.pointer("/result/value")?;
                    return Some((
                        value.get("quiet").and_then(Value::as_u64)?,
                        value.get("mutations").and_then(Value::as_u64)?,
                    ));
                }
                // The document went away with its world; make a new one.
                Err(Stop::Failed(EngineError::Transport(TransportError::Refused { .. }))) => {
                    self.world = None;
                }
                Err(_) => return None,
            }
        }
        None
    }

    /// Wait for a navigation to start, then for it to load, then for the DOM
    /// to go quiet.
    fn settle(&mut self) {
        let start = Instant::now() + self.timeouts.navigation_start;
        while !self.seen.navigation_started && self.dialog.is_none() {
            let left = start.saturating_duration_since(Instant::now());
            match self.connection.next_event(left) {
                Ok(Some(event)) => self.handle(&event),
                _ => break,
            }
        }
        if self.seen.navigation_started {
            let load = Instant::now() + self.timeouts.load;
            while !self.seen.loaded && self.dialog.is_none() {
                let left = load.saturating_duration_since(Instant::now());
                match self.connection.next_event(left) {
                    Ok(Some(event)) => self.handle(&event),
                    _ => break,
                }
            }
        }
        if self.dialog.is_some() {
            return;
        }
        let window = u64::try_from(self.timeouts.quiet_window.as_millis()).unwrap_or(u64::MAX);
        let cap = u64::try_from(self.timeouts.quiet_cap.as_millis()).unwrap_or(u64::MAX);
        let before = self.mutations;
        if let Some((quiet, mutations)) = self.watch(&input::quiet_within(window, cap), true) {
            self.quiet_for_ms = Some(quiet);
            self.mutations = Some(mutations);
            if before == Some(mutations) && self.seen.navigated.is_none() {
                self.outcome.get_or_insert_with(|| "nothing changed".into());
            }
        }
    }

    /// Check a row is still the node at the point to click, scrolling it into
    /// view first if it was off screen, and click it if so. `Err` carries why
    /// the click was not made.
    fn press(&mut self, at: Point, node: &NodeKey) -> Result<Result<(), Box<str>>, Stop> {
        let (x, y) = match self.aim(at, node)? {
            Ok(point) => point,
            Err(reason) => return Ok(Err(reason)),
        };
        for event in input::click(x, y) {
            self.call("Input.dispatchMouseEvent", event)?;
        }
        Ok(Ok(()))
    }

    /// The point at which the row is the node drawn, after checking the page
    /// is the document that was read and scrolling an off-screen row into
    /// view. `Err` carries why nothing should be done to it.
    fn aim(&mut self, at: Point, node: &NodeKey) -> Result<Result<(f64, f64), Box<str>>, Stop> {
        let Some(parts) = KeyParts::parse(node) else {
            return Ok(Err("the row is not one this browser read".into()));
        };
        let Some((frame, loader)) = self
            .loaders
            .get(parts.document)
            .map(|(f, l)| (f.to_owned(), l.to_owned()))
        else {
            return Ok(Err("the row is not one this browser read".into()));
        };
        let tree = self.call("Page.getFrameTree", json!({}))?;
        let current = read::frame_loaders(&tree)
            .into_iter()
            .find(|(f, _)| *f == frame)
            .map(|(_, l)| l);
        if current.as_deref() != Some(loader.as_str()) {
            return Ok(Err("the page has changed since it was read".into()));
        }

        let off = self.viewport.is_none_or(|v| input::off_screen(at, v));
        let (x, y) = if off {
            let target = json!({"backendNodeId": parts.backend});
            if gone(self.call("DOM.scrollIntoViewIfNeeded", target.clone()))?.is_none() {
                return Ok(Err("the row is no longer on the page".into()));
            }
            let measured = gone(self.call("DOM.getBoxModel", target))?;
            match measured.as_ref().and_then(input::centre) {
                Some(point) => point,
                None => return Ok(Err("the row is no longer on the page".into())),
            }
        } else {
            (f64::from(at.x), f64::from(at.y))
        };

        let hit = self.call(
            "DOM.getNodeForLocation",
            json!({"x": x, "y": y, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true}),
        );
        let hit = match hit {
            Ok(hit) => hit.get("backendNodeId").and_then(Value::as_i64),
            Err(Stop::Dialog) => return Err(Stop::Dialog),
            Err(Stop::Failed(_)) => None,
        };
        let Some(hit) = hit else {
            return Ok(Err("nothing is drawn where it is".into()));
        };
        if hit != parts.backend
            && let Some(what) = self.covering(parts.backend, hit)?
        {
            return Ok(Err(format!("{what} covers it").into()));
        }
        Ok(Ok((x, y)))
    }

    /// `None` when the hit node is inside the row's, else what it is.
    fn covering(&mut self, row: i64, hit: i64) -> Result<Option<Box<str>>, Stop> {
        let row_object = self.object(row)?;
        let hit_object = self.object(hit)?;
        let (Some(row_object), Some(hit_object)) = (row_object, hit_object) else {
            return Ok(Some("something else".into()));
        };
        let answer = self.call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": row_object,
                "functionDeclaration": input::CONTAINS,
                "arguments": [{"objectId": hit_object}],
                "returnByValue": true,
            }),
        );
        let answer = match answer {
            Ok(answer) => answer,
            Err(Stop::Dialog) => return Err(Stop::Dialog),
            // The two nodes are in different frames, so one cannot be inside
            // the other.
            Err(Stop::Failed(_)) => return Ok(Some("something in another frame".into())),
        };
        let value = answer.pointer("/result/value").cloned().unwrap_or_default();
        if value.get("inside").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        let what = value
            .get("what")
            .and_then(Value::as_str)
            .unwrap_or("something else");
        Ok(Some(what.into()))
    }

    /// A handle on a node for page script, or `None` if it is gone.
    fn object(&mut self, backend: i64) -> Result<Option<String>, Stop> {
        match self.call("DOM.resolveNode", json!({"backendNodeId": backend})) {
            Ok(resolved) => Ok(resolved
                .pointer("/object/objectId")
                .and_then(Value::as_str)
                .map(str::to_owned)),
            Err(Stop::Dialog) => Err(Stop::Dialog),
            Err(Stop::Failed(EngineError::Transport(TransportError::Refused { .. }))) => Ok(None),
            Err(Stop::Failed(e)) => Err(Stop::Failed(e)),
        }
    }

    fn fill(&mut self, at: Point, node: &NodeKey, text: &str) -> Result<(), Stop> {
        let Some(parts) = KeyParts::parse(node) else {
            self.outcome =
                Some("the tap was not made: the row is not one this browser read".into());
            return Ok(());
        };
        let described = self.call("DOM.describeNode", json!({"backendNodeId": parts.backend}));
        let is_select = match &described {
            Ok(d) => d
                .pointer("/node/nodeName")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case("SELECT")),
            Err(Stop::Dialog) => return Err(Stop::Dialog),
            Err(Stop::Failed(_)) => false,
        };
        if is_select {
            // Checked as a tap is, though nothing is clicked: on a page that
            // has been replaced the node id may name another control, and one
            // that is covered is not one a person could change.
            if let Err(reason) = self.aim(at, node)? {
                self.outcome = Some(format!("nothing was chosen: {reason}").into());
                return Ok(());
            }
            let Some(object) = self.object(parts.backend)? else {
                self.outcome =
                    Some("the tap was not made: the row is no longer on the page".into());
                return Ok(());
            };
            let answer = self.call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": input::SELECT_OPTION,
                    "arguments": [{"value": text}],
                    "returnByValue": true,
                }),
            )?;
            let value = answer.pointer("/result/value").cloned().unwrap_or_default();
            self.outcome = Some(input::select_outcome(&value, text));
            return Ok(());
        }
        if let Err(reason) = self.press(at, node)? {
            self.outcome = Some(format!("the tap was not made: {reason}").into());
            return Ok(());
        }
        for event in input::select_all() {
            self.call("Input.dispatchKeyEvent", event)?;
        }
        self.call("Input.insertText", json!({"text": text}))?;
        if let Some(object) = self.object(parts.backend)? {
            let answer = self.call(
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": input::READ_VALUE, "returnByValue": true}),
            )?;
            let value = answer.pointer("/result/value").cloned().unwrap_or_default();
            let line = if value.get("password").and_then(Value::as_bool) == Some(true) {
                "the field now holds what was typed".to_owned()
            } else {
                let held = value
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                format!("the field now holds \u{201c}{held}\u{201d}")
            };
            self.outcome = Some(line.into());
        }
        Ok(())
    }

    fn run(&mut self, command: &BrowserCommand) -> Result<(), Stop> {
        match command {
            BrowserCommand::Press { at, node } => {
                if let Err(reason) = self.press(*at, node)? {
                    self.outcome = Some(format!("the tap was not made: {reason}").into());
                }
            }
            BrowserCommand::Fill { at, node, text } => self.fill(*at, node, text)?,
            BrowserCommand::Submit => {
                for event in input::enter() {
                    self.call("Input.dispatchKeyEvent", event)?;
                }
            }
            BrowserCommand::Scroll(direction) => {
                let viewport = if let Some(v) = self.viewport {
                    v
                } else {
                    let metrics = self.call("Page.getLayoutMetrics", json!({}))?;
                    let v = metrics
                        .get("cssVisualViewport")
                        .cloned()
                        .unwrap_or_default();
                    Viewport {
                        width: v.get("clientWidth").and_then(Value::as_f64).unwrap_or(0.0),
                        height: v.get("clientHeight").and_then(Value::as_f64).unwrap_or(0.0),
                    }
                };
                self.call(
                    "Input.dispatchMouseEvent",
                    input::wheel(viewport, *direction),
                )?;
            }
            BrowserCommand::Back => {
                let history = self.call("Page.getNavigationHistory", json!({}))?;
                let current = history
                    .get("currentIndex")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .unwrap_or(0);
                let previous = current.checked_sub(1).and_then(|i| {
                    history
                        .get("entries")
                        .and_then(Value::as_array)
                        .and_then(|e| e.get(i))
                        .and_then(|e| e.get("id"))
                        .cloned()
                });
                match previous {
                    Some(entry) => {
                        self.call("Page.navigateToHistoryEntry", json!({"entryId": entry}))?;
                    }
                    None => {
                        self.outcome =
                            Some("nothing changed: there is no earlier page in this tab".into());
                    }
                }
            }
            BrowserCommand::Navigate(url) => {
                let answer = self.call("Page.navigate", json!({"url": &**url}))?;
                if let Some(error) = answer
                    .get("errorText")
                    .and_then(Value::as_str)
                    .filter(|e| !e.is_empty())
                {
                    self.outcome = Some(format!("the page did not load: {error}").into());
                }
            }
            BrowserCommand::SwitchTab(tab) => {
                if !self.owned.owns(tab.as_str()) {
                    return Err(Stop::Failed(EngineError::NotOwned(tab.clone())));
                }
                self.switch_to(tab)?;
            }
            BrowserCommand::Dialog(answer) => {
                if self.dialog.is_none() {
                    self.outcome = Some("nothing changed: no dialog is open".into());
                    return Ok(());
                }
                let params = match answer {
                    DialogAnswer::Accept(Some(words)) => {
                        json!({"accept": true, "promptText": &**words})
                    }
                    DialogAnswer::Accept(None) => json!({"accept": true}),
                    DialogAnswer::Dismiss => json!({"accept": false}),
                };
                self.dialog = None;
                self.call("Page.handleJavaScriptDialog", params)?;
            }
            BrowserCommand::Settle => {}
        }
        Ok(())
    }

    fn switch_to(&mut self, tab: &TabKey) -> Result<(), EngineError> {
        self.browser_call("Target.activateTarget", json!({"targetId": tab.as_str()}))?;
        let old = self.session.clone();
        self.attach_session(tab.as_str())?;
        let _ = self
            .connection
            .send("Target.detachFromTarget", json!({"sessionId": &*old}), None);
        Ok(())
    }

    /// The final outcome line, from what was seen while the command ran.
    fn conclude(&mut self) {
        let seen = std::mem::take(&mut self.seen);
        let line = if let Some(message) = seen.dialog {
            Some(format!("a dialog opened: {message}").into())
        } else if let Some(title) = seen.new_tab {
            Some(format!("opened a new tab: {title}").into())
        } else if let Some(origin) = seen.navigated {
            Some(format!("navigated to {origin}").into())
        } else {
            self.outcome.take()
        };
        self.outcome = line;
    }
}

/// A call's reply, or `None` when it failed because its node is gone.
fn gone(result: Result<Value, Stop>) -> Result<Option<Value>, Stop> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Stop::Failed(EngineError::Transport(TransportError::Refused { .. }))) => Ok(None),
        Err(stop) => Err(stop),
    }
}

/// The scheme, host and port of an address.
fn origin(url: &str) -> &str {
    let Some(scheme_end) = url.find("://") else {
        return url;
    };
    let rest = &url[scheme_end + 3..];
    let end = rest
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| scheme_end + 3 + i);
    &url[..end]
}

impl<W: Wire> BrowserEngine for CdpEngine<W> {
    type Error = EngineError;

    fn read(&mut self) -> Result<PageRead, EngineError> {
        self.pump();
        if self.dialog.is_some() {
            return self.dialog_read();
        }
        match self.full_read() {
            Ok(read) => Ok(read),
            Err(Stop::Dialog) => self.dialog_read(),
            Err(Stop::Failed(e)) => Err(e),
        }
    }

    fn perform(&mut self, command: &BrowserCommand) -> Result<(), EngineError> {
        self.pump();
        self.outcome = None;
        self.seen = Seen::default();
        match self.run(command) {
            Ok(()) => {
                if self.dialog.is_none() {
                    self.settle();
                }
            }
            Err(Stop::Dialog) => {}
            Err(Stop::Failed(e)) => {
                self.seen = Seen::default();
                return Err(e);
            }
        }
        self.conclude();
        Ok(())
    }

    fn outcome(&self) -> Option<&str> {
        self.outcome.as_deref()
    }

    fn quiet_for_ms(&self) -> Option<u64> {
        self.quiet_for_ms
    }

    fn tabs(&mut self) -> Result<Vec<PageTab>, EngineError> {
        let targets = self.browser_call("Target.getTargets", json!({}))?;
        Ok(targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
            .map(|t| {
                let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or_default();
                PageTab {
                    key: TabKey::new(s("targetId")),
                    title: s("title").into(),
                    url: s("url").into(),
                    opened_by_run: self.owned.opened_by_run(s("targetId")),
                }
            })
            .collect())
    }

    fn select_tab(&mut self, tab: &TabKey) -> Result<(), EngineError> {
        if !self.tabs()?.iter().any(|t| t.key == *tab) {
            return Err(EngineError::NoSuchTab(tab.clone()));
        }
        self.owned.start_on(tab.as_str(), false);
        self.switch_to(tab)
    }
}

impl<W: Wire> Drop for CdpEngine<W> {
    fn drop(&mut self) {
        let _ = self.connection.send(
            "Target.detachFromTarget",
            json!({"sessionId": &*self.session}),
            None,
        );
        self.connection.close();
    }
}
