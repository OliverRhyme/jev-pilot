//! The Chrome DevTools Protocol engine, checked as JSON messages.
//!
//! No browser runs here. Requests are compared as `serde_json` values, and the
//! engine is driven over a scripted wire that answers each method from a list
//! of recorded or hand-written replies.

#![cfg(feature = "browser")]

use jev_pilot::browser::cdp::{Cdp, Incoming};
use serde_json::json;

/// Every request carries a fresh id, so a reply can be matched to its call;
/// a page command names its session, and a browser command does not.
#[test]
fn requests_carry_increasing_ids_and_their_session() {
    let mut cdp = Cdp::new();

    let first = cdp.request("Target.getTargets", json!({}), None);
    let second = cdp.request(
        "Page.navigate",
        json!({"url": "https://example.test/"}),
        Some("S1"),
    );

    assert_eq!(first.id + 1, second.id);
    assert_eq!(
        first.message,
        json!({"id": first.id, "method": "Target.getTargets", "params": {}})
    );
    assert_eq!(
        second.message,
        json!({
            "id": second.id,
            "method": "Page.navigate",
            "params": {"url": "https://example.test/"},
            "sessionId": "S1"
        })
    );
}

#[test]
fn a_reply_parses_with_its_result() {
    let incoming = Cdp::parse(r#"{"id":7,"result":{"frameId":"F"}}"#).expect("parses");

    assert_eq!(
        incoming,
        Incoming::Reply {
            id: 7,
            outcome: Ok(json!({"frameId": "F"}))
        }
    );
}

#[test]
fn an_error_reply_parses_with_its_code_and_message() {
    let incoming =
        Cdp::parse(r#"{"id":8,"error":{"code":-32000,"message":"No node with given id found"}}"#)
            .expect("parses");

    let Incoming::Reply {
        id: 8,
        outcome: Err(error),
    } = incoming
    else {
        panic!("expected an error reply, got {incoming:?}");
    };
    assert_eq!(error.code, -32000);
    assert_eq!(&*error.message, "No node with given id found");
}

#[test]
fn an_event_parses_with_its_session() {
    let incoming = Cdp::parse(
        r#"{"method":"Page.javascriptDialogOpening","params":{"type":"alert","message":"Saved"},"sessionId":"S1"}"#,
    )
    .expect("parses");

    let Incoming::Event(event) = incoming else {
        panic!("expected an event, got {incoming:?}");
    };
    assert_eq!(&*event.method, "Page.javascriptDialogOpening");
    assert_eq!(event.session.as_deref(), Some("S1"));
    assert_eq!(event.params["message"], "Saved");
}

#[test]
fn a_message_that_is_neither_reply_nor_event_is_refused() {
    assert!(Cdp::parse(r#"{"hello":1}"#).is_err());
    assert!(Cdp::parse("not json").is_err());
}

/// A wire that answers each method from a script, and remembers what was sent.
///
/// Each method has a queue of answers, used in order. A method with nothing
/// left in its queue gets an empty result; one scripted with
/// [`Answer::Silence`] gets no answer at all, which a caller sees as a timeout.
/// Events scripted before an answer arrive ahead of it, as the browser sends
/// them while a call is still in flight.
mod scripted {
    use jev_pilot::browser::transport::{Wire, WireError};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;
    use std::time::Duration;

    pub enum Answer {
        Result(Value),
        Error(&'static str),
        EventsThen(Vec<Value>, Box<Answer>),
        ThenEvents(Box<Answer>, Vec<Value>),
        Silence,
    }

    #[derive(Clone, Default)]
    pub struct Script {
        answers: Rc<RefCell<HashMap<String, VecDeque<Answer>>>>,
        sent: Rc<RefCell<Vec<Value>>>,
        inbox: Rc<RefCell<VecDeque<String>>>,
    }

    impl Script {
        pub fn new() -> Self {
            Self::default()
        }

        /// Answer the next call of `method` with this result.
        pub fn on(&self, method: &str, result: Value) -> &Self {
            self.answer(method, Answer::Result(result))
        }

        pub fn answer(&self, method: &str, answer: Answer) -> &Self {
            self.answers
                .borrow_mut()
                .entry(method.to_owned())
                .or_default()
                .push_back(answer);
            self
        }

        /// Deliver an event now, before whatever is sent next is answered.
        pub fn event(&self, event: &Value) -> &Self {
            self.inbox.borrow_mut().push_back(event.to_string());
            self
        }

        /// Forget what was sent so far.
        pub fn clear_sent(&self) {
            self.sent.borrow_mut().clear();
        }

        /// Everything sent so far.
        pub fn sent(&self) -> Vec<Value> {
            self.sent.borrow().clone()
        }

        /// The methods sent so far, in order.
        pub fn methods(&self) -> Vec<String> {
            self.sent()
                .iter()
                .map(|m| m["method"].as_str().unwrap_or_default().to_owned())
                .collect()
        }

        /// The params of every call of `method`, in order.
        pub fn params_of(&self, method: &str) -> Vec<Value> {
            self.sent()
                .into_iter()
                .filter(|m| m["method"] == method)
                .map(|m| m["params"].clone())
                .collect()
        }

        fn deliver(&self, id: &Value, answer: Answer) {
            match answer {
                Answer::Result(result) => self
                    .inbox
                    .borrow_mut()
                    .push_back(json!({"id": id, "result": result}).to_string()),
                Answer::Error(message) => self.inbox.borrow_mut().push_back(
                    json!({"id": id, "error": {"code": -32000, "message": message}}).to_string(),
                ),
                Answer::EventsThen(events, then) => {
                    for event in events {
                        self.inbox.borrow_mut().push_back(event.to_string());
                    }
                    self.deliver(id, *then);
                }
                Answer::ThenEvents(first, events) => {
                    self.deliver(id, *first);
                    for event in events {
                        self.inbox.borrow_mut().push_back(event.to_string());
                    }
                }
                Answer::Silence => {}
            }
        }
    }

    impl Wire for Script {
        fn send(&mut self, text: &str) -> Result<(), WireError> {
            let message: Value = serde_json::from_str(text).expect("the engine sends JSON");
            let method = message["method"].as_str().unwrap_or_default().to_owned();
            let id = message["id"].clone();
            self.sent.borrow_mut().push(message);
            let answer = self
                .answers
                .borrow_mut()
                .get_mut(&method)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Answer::Result(json!({})));
            self.deliver(&id, answer);
            Ok(())
        }

        fn receive(&mut self, _timeout: Duration) -> Result<Option<String>, WireError> {
            Ok(self.inbox.borrow_mut().pop_front())
        }
    }
}

use jev_pilot::browser::transport::{Connection, TransportError};
use scripted::{Answer, Script};
use std::time::Duration;

const SECOND: Duration = Duration::from_secs(1);

/// The browser keeps talking while a call is in flight. An event that arrives
/// before the reply must be kept for later, not mistaken for the reply or lost.
#[test]
fn events_that_arrive_before_a_reply_are_queued() {
    let script = Script::new();
    script.answer(
        "Page.navigate",
        Answer::EventsThen(
            vec![json!({"method": "Page.frameStartedLoading", "params": {"frameId": "F"}, "sessionId": "S1"})],
            Box::new(Answer::Result(json!({"frameId": "F", "loaderId": "L2"}))),
        ),
    );
    let mut connection = Connection::new(script.clone());

    let reply = connection
        .call(
            "Page.navigate",
            json!({"url": "https://example.test/"}),
            Some("S1"),
            SECOND,
        )
        .expect("replied");

    assert_eq!(reply["loaderId"], "L2");
    let events = connection.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(&*events[0].method, "Page.frameStartedLoading");
}

#[test]
fn an_error_reply_is_an_error_naming_the_method() {
    let script = Script::new();
    script.answer(
        "DOM.getBoxModel",
        Answer::Error("No node with given id found"),
    );
    let mut connection = Connection::new(script);

    let error = connection
        .call(
            "DOM.getBoxModel",
            json!({"backendNodeId": 5}),
            Some("S1"),
            SECOND,
        )
        .expect_err("refused");

    assert!(
        matches!(&error, TransportError::Refused { method, .. } if &**method == "DOM.getBoxModel")
    );
    assert!(error.to_string().contains("DOM.getBoxModel"), "{error}");
}

#[test]
fn a_call_with_no_reply_times_out_naming_the_method() {
    let script = Script::new();
    script.answer("Accessibility.getFullAXTree", Answer::Silence);
    let mut connection = Connection::new(script);

    let error = connection
        .call("Accessibility.getFullAXTree", json!({}), Some("S1"), SECOND)
        .expect_err("no reply");

    assert!(
        matches!(&error, TransportError::Timeout { method } if &**method == "Accessibility.getFullAXTree")
    );
}

/// A reply to an earlier call that timed out must not be taken for the reply
/// to the call that came after it.
#[test]
fn a_late_reply_to_another_call_is_skipped() {
    let script = Script::new();
    script.answer("Page.getFrameTree", Answer::Silence);
    let mut connection = Connection::new(script.clone());
    let _ = connection.call("Page.getFrameTree", json!({}), Some("S1"), SECOND);
    script.event(&json!({"id": 1, "result": {"stale": true}}));
    script.on("Page.getLayoutMetrics", json!({"fresh": true}));

    let reply = connection
        .call("Page.getLayoutMetrics", json!({}), Some("S1"), SECOND)
        .expect("replied");

    assert_eq!(reply, json!({"fresh": true}));
}

// Recording the fixtures under tests/fixtures/cdp/
//
// The replies there are a real Chrome's, recorded once by hand. To record them
// again (the version that produced them is in each folder's version.json):
//
// 1. Get a Chrome for Testing or Chromium build; Google-branded Chrome ignores
//    remote debugging on its default profile, and the owner's profile is never
//    used. `npx @puppeteer/browsers install chrome@stable` prints its path.
// 2. `node tests/fixtures/cdp/record.mjs <chrome binary> <out dir>` with
//    `SCRATCH` set to a throwaway folder. The script serves the sign-up page it
//    holds on 127.0.0.1, starts the browser headless with
//    `--remote-debugging-port=0 --user-data-dir=<a new folder under SCRATCH>`,
//    scrolls the page down 120 pixels, saves the replies of the calls a read
//    makes, opens a confirm to see which calls it blocks, then closes the
//    browser and deletes the profile.
// 3. Copy the JSON files into a folder here and update the expectations below.
//
// What the recording of Chrome 153 showed:
//
// - `isClickable` is precise about the element itself: links, inputs, labels
//   that wrap an input, and a div with a click listener are marked; a native
//   select, a disabled button, and the children of a clickable element are not.
// - An open confirm blocks every call to the tab's session, `Page.getFrameTree`
//   included, not only `Accessibility.getFullAXTree` (dialog-notes.json). Calls
//   to the browser itself, such as `Target.getTargets`, still answer.

mod signup {
    use serde_json::Value;

    pub const MAIN_FRAME: &str = "933B271DD0EC81D3402A478AE60B40E5";
    pub const CHILD_FRAME: &str = "4634BC906F07018102C158CE6AEB76E4";
    pub const TARGET: &str = MAIN_FRAME;

    pub fn load(name: &str) -> Value {
        let path = format!(
            "{}/tests/fixtures/cdp/signup/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&text).expect("fixture is JSON")
    }
}

use jev_pilot::browser::read::{Loaders, OwnedTabs, RawRead, translate};
use jev_pilot::platform::page::{PageBox, PageNode, PageRead, PageTab, Viewport};
use jev_pilot::snapshot::TabKey;

fn signup_read() -> PageRead {
    let raw = RawRead {
        frame_tree: signup::load("frame-tree"),
        ax_trees: vec![
            (signup::MAIN_FRAME.into(), signup::load("ax-0")),
            (signup::CHILD_FRAME.into(), signup::load("ax-1")),
        ],
        snapshot: signup::load("snapshot"),
        layout_metrics: signup::load("layout-metrics"),
        targets: signup::load("targets"),
    };
    let mut owned = OwnedTabs::default();
    owned.start_on(signup::TARGET, true);
    translate(&raw, signup::TARGET, &owned, &mut Loaders::default()).expect("translates")
}

fn node<'a>(read: &'a PageRead, role: &str, name: &str) -> &'a PageNode {
    read.nodes
        .iter()
        .find(|n| &*n.role == role && n.name.trim() == name)
        .unwrap_or_else(|| panic!("no {role} named {name:?} in {:#?}", read.nodes))
}

fn position(read: &PageRead, role: &str, name: &str) -> usize {
    let wanted = node(read, role, name);
    read.nodes
        .iter()
        .position(|n| n == wanted)
        .expect("present")
}

#[test]
fn a_recorded_read_gives_the_page_address_title_and_viewport() {
    let read = signup_read();

    assert_eq!(&*read.url, "http://127.0.0.1:52578/signup");
    assert_eq!(&*read.title, "Sign up");
    assert_eq!(
        read.viewport,
        Viewport {
            width: 1280.0,
            height: 657.0
        }
    );
}

/// The layout reports boxes in document coordinates; the page was scrolled
/// down 120 pixels, so everything sits 120 pixels higher in the viewport.
#[test]
fn boxes_are_converted_to_viewport_pixels() {
    let read = signup_read();

    assert_eq!(
        node(&read, "textbox", "Email").bounds,
        Some(PageBox {
            x: 44.453_125,
            y: 8.875,
            width: 153.0,
            height: 21.0
        })
    );
    assert_eq!(
        node(&read, "link", "Terms of service").bounds,
        Some(PageBox {
            x: 0.0,
            y: 1121.6875,
            width: 116.468_75,
            height: 18.0
        }),
        "a row below the fold keeps a box past the bottom of the viewport"
    );
}

/// A node in a same-process iframe is placed by the iframe's content box: its
/// border (2 pixels here) and the iframe's own place in the viewport.
#[test]
fn frame_nodes_carry_their_frame_and_a_box_in_the_main_viewport() {
    let read = signup_read();

    let inside = node(&read, "button", "Inside frame");
    assert_eq!(&*inside.frame, signup::CHILD_FRAME);
    assert_eq!(
        inside.bounds,
        Some(PageBox {
            x: 10.0,
            y: 75.875,
            width: 89.359_375,
            height: 21.0
        })
    );
    assert_eq!(&*node(&read, "textbox", "Email").frame, signup::MAIN_FRAME);
}

/// The reader has no link from a frame to its iframe, so a frame's nodes
/// follow the iframe directly, before anything painted over it.
#[test]
fn frame_nodes_follow_their_iframe() {
    let read = signup_read();

    let iframe = position(&read, "Iframe", "");
    assert_eq!(position(&read, "button", "Inside frame"), iframe + 1);
    assert_eq!(position(&read, "StaticText", "Frame text"), iframe + 2);
}

/// The sticky header is drawn after the form even though it comes first in
/// the document, so its links come after the form's fields.
#[test]
fn nodes_are_in_paint_order() {
    let read = signup_read();

    assert!(position(&read, "textbox", "Email") < position(&read, "link", "Home"));
    assert!(position(&read, "button", "Create account") < position(&read, "link", "Help"));
}

#[test]
fn clickable_comes_from_the_layout() {
    let read = signup_read();

    let card = node(&read, "generic", "Pick the free plan");
    assert!(card.clickable, "a div with a click listener is clickable");
    assert!(node(&read, "textbox", "Email").clickable);
    assert!(!node(&read, "button", "Create account").clickable);
}

#[test]
fn states_values_and_options_come_from_the_accessibility_tree() {
    let read = signup_read();

    let submit = node(&read, "button", "Create account");
    assert!(submit.states.disabled);

    let email = node(&read, "textbox", "Email");
    assert!(email.states.editable && email.states.focusable);
    assert_eq!(email.value, None, "the field is empty");

    let country = node(&read, "combobox", "Country");
    assert_eq!(country.value.as_deref(), Some("Philippines"));
    assert_eq!(
        country.options,
        ["Japan", "Philippines", "Spain"].map(Box::<str>::from)
    );

    let news = node(&read, "checkbox", "Send me news");
    assert_eq!(
        news.states.checked,
        Some(jev_pilot::platform::page::Checked::Checked)
    );
}

#[test]
fn a_password_value_is_never_read() {
    let read = signup_read();

    let password = node(&read, "textbox", "Password");
    assert_eq!(password.value, None);
    assert!(password.protected);
    assert!(!node(&read, "textbox", "Email").protected);
    let json = serde_json::to_string(&read).expect("serialises");
    assert!(!json.contains("hunter2") && !json.contains('•'), "{json}");
}

/// Text already in a link's or a heading's name is not repeated as a node of
/// its own; a paragraph's text is kept, for the page's words.
#[test]
fn text_inside_a_named_node_is_not_repeated() {
    let read = signup_read();

    assert!(
        !read
            .nodes
            .iter()
            .any(|n| &*n.role == "StaticText" && &*n.name == "Home")
    );
    assert_eq!(
        &*node(&read, "StaticText", "Frame text").frame,
        signup::CHILD_FRAME
    );
}

#[test]
fn a_node_takes_the_nearest_heading_or_landmark_as_context() {
    let read = signup_read();

    assert_eq!(
        node(&read, "link", "Terms of service").context.as_deref(),
        Some("Further down")
    );
    assert_eq!(node(&read, "link", "Home").context.as_deref(), Some("Main"));
}

#[test]
fn only_run_owned_tabs_are_listed() {
    let read = signup_read();

    assert_eq!(
        read.tabs,
        [PageTab {
            key: TabKey::new(signup::TARGET),
            title: "Sign up".into(),
            url: "http://127.0.0.1:52578/signup".into(),
            opened_by_run: true,
        }]
    );
}

/// The key names the node and the document it was read from, so the engine
/// can tell at click time that the page has been replaced since.
#[test]
fn node_keys_name_the_node_and_its_document() {
    let read = signup_read();

    assert_eq!(node(&read, "textbox", "Email").key.as_str(), "2:1");
    assert_eq!(node(&read, "button", "Inside frame").key.as_str(), "27:2");
}

// The engine, over the scripted wire.

use jev_pilot::act::Direction;
use jev_pilot::browser::engine::{BrowserCommand, BrowserEngine, CdpEngine, DialogAnswer};
use jev_pilot::snapshot::{DialogKind, NodeKey, Point};

const SESSION: &str = "S1";

/// The replies attaching to the sign-up tab gets.
fn script_attach(script: &Script) {
    script
        .on("Target.getTargets", signup::load("targets"))
        .on("Target.setDiscoverTargets", json!({}))
        .on("Target.attachToTarget", json!({"sessionId": SESSION}))
        .on("Page.enable", json!({}));
}

/// The replies one read of the sign-up page gets.
fn script_read(script: &Script) {
    script
        .on("Page.getFrameTree", signup::load("frame-tree"))
        .on("Accessibility.getFullAXTree", signup::load("ax-0"))
        .on("Accessibility.getFullAXTree", signup::load("ax-1"))
        .on("DOMSnapshot.captureSnapshot", signup::load("snapshot"))
        .on("Page.getLayoutMetrics", signup::load("layout-metrics"))
        .on("Target.getTargets", signup::load("targets"))
        .on(
            "Page.createIsolatedWorld",
            json!({"executionContextId": 41}),
        )
        .on(
            "Runtime.evaluate",
            json!({"result": {"type": "object", "value": {"quiet": 250, "mutations": 3}}}),
        );
}

/// The replies the settling after a command gets: no navigation starts, and
/// the page is quiet with the same number of changes as at the read.
fn script_settle(script: &Script, mutations: u64) {
    script.on(
        "Runtime.evaluate",
        json!({"result": {"type": "object", "value": {"quiet": 120, "mutations": mutations}}}),
    );
}

/// An engine attached to the sign-up tab, which it has read once.
fn engine_after_read() -> (Script, CdpEngine<Script>) {
    let script = Script::new();
    script_attach(&script);
    script_read(&script);
    let mut engine =
        CdpEngine::attach(script.clone(), &TabKey::new(signup::TARGET), false).expect("attaches");
    engine.read().expect("reads");
    script.clear_sent();
    (script, engine)
}

/// What a command sent, leaving out the settling that follows every command.
fn command_traffic(script: &Script) -> Vec<Value> {
    script
        .sent()
        .into_iter()
        .filter(|m| m["method"] != "Runtime.evaluate" && m["method"] != "Page.createIsolatedWorld")
        .collect()
}

fn without_ids(messages: Vec<Value>) -> Vec<Value> {
    messages
        .into_iter()
        .map(|mut m| {
            assert!(m["id"].is_u64(), "every request has an id: {m}");
            m.as_object_mut().expect("an object").remove("id");
            m
        })
        .collect()
}

fn click_at(x: f64, y: f64) -> Vec<Value> {
    ["mouseMoved", "mousePressed", "mouseReleased"]
        .into_iter()
        .map(|kind| {
            let mut params =
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1});
            if kind == "mouseMoved" {
                params = json!({"type": kind, "x": x, "y": y});
            }
            json!({"method": "Input.dispatchMouseEvent", "params": params, "sessionId": SESSION})
        })
        .collect()
}

fn email() -> NodeKey {
    NodeKey::new("2:1")
}

use serde_json::Value;

/// Attaching uses one flattened session, with Page enabled from the start so
/// that a dialog opening later is reported to it.
#[test]
fn attaching_discovers_tabs_and_enables_page_on_a_flat_session() {
    let script = Script::new();
    script_attach(&script);

    let _engine =
        CdpEngine::attach(script.clone(), &TabKey::new(signup::TARGET), false).expect("attaches");

    assert_eq!(
        without_ids(script.sent()),
        [
            json!({"method": "Target.getTargets", "params": {}}),
            json!({"method": "Target.setDiscoverTargets", "params": {"discover": true}}),
            json!({"method": "Target.attachToTarget", "params": {"targetId": signup::TARGET, "flatten": true}}),
            json!({"method": "Page.enable", "params": {}, "sessionId": SESSION}),
        ]
    );
}

#[test]
fn the_engine_reads_the_page_through_its_session() {
    let script = Script::new();
    script_attach(&script);
    script_read(&script);
    let mut engine =
        CdpEngine::attach(script.clone(), &TabKey::new(signup::TARGET), false).expect("attaches");

    let read = engine.read().expect("reads");

    assert_eq!(read, signup_read_as_attached());
    assert_eq!(engine.quiet_for_ms(), Some(250));
    assert_eq!(
        script.params_of("Accessibility.getFullAXTree"),
        [
            json!({"frameId": signup::MAIN_FRAME}),
            json!({"frameId": signup::CHILD_FRAME})
        ]
    );
    assert_eq!(
        script.params_of("DOMSnapshot.captureSnapshot"),
        [json!({"computedStyles": [], "includePaintOrder": true, "includeDOMRects": true})]
    );
}

fn signup_read_as_attached() -> PageRead {
    let mut read = signup_read();
    for tab in &mut read.tabs {
        tab.opened_by_run = false;
    }
    read
}

/// A press checks the document is the one that was read, hit-tests the point,
/// and clicks only when the node found there is the row's own.
#[test]
fn a_press_that_hits_its_node_clicks() {
    let (script, mut engine) = engine_after_read();
    script.on("Page.getFrameTree", signup::load("frame-tree"));
    script.on(
        "DOM.getNodeForLocation",
        json!({"backendNodeId": 2, "frameId": signup::MAIN_FRAME}),
    );
    script_settle(&script, 4);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");

    let mut expected = vec![
        json!({"method": "Page.getFrameTree", "params": {}, "sessionId": SESSION}),
        json!({"method": "DOM.getNodeForLocation", "params": {"x": 120.0, "y": 19.0, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true}, "sessionId": SESSION}),
    ];
    expected.extend(click_at(120.0, 19.0));
    assert_eq!(without_ids(command_traffic(&script)), expected);
}

fn clicks(script: &Script) -> usize {
    script.params_of("Input.dispatchMouseEvent").len()
}

/// A tap on a link's text lands on the text node inside it, which is still the
/// link: a descendant counts as a hit.
#[test]
fn a_press_that_hits_a_descendant_clicks() {
    let (script, mut engine) = engine_after_read();
    script.on("Page.getFrameTree", signup::load("frame-tree"));
    script.on("DOM.getNodeForLocation", json!({"backendNodeId": 12}));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "row"}}));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "hit"}}));
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"inside": true, "what": "a \u{201c}Home\u{201d}"}}}),
    );
    script_settle(&script, 4);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 20, y: 9 },
            node: NodeKey::new("11:1"),
        })
        .expect("performs");

    assert_eq!(
        script.params_of("DOM.resolveNode"),
        [json!({"backendNodeId": 11}), json!({"backendNodeId": 12})]
    );
    let check = &script.params_of("Runtime.callFunctionOn")[0];
    assert_eq!(check["objectId"], "row");
    assert_eq!(check["arguments"], json!([{"objectId": "hit"}]));
    assert_eq!(clicks(&script), 3);
}

/// Something drawn over the row (a cookie banner, a sticky header) would take
/// the click. No click is made, and the outcome says what is in the way.
#[test]
fn a_press_on_a_covered_row_refuses_and_names_what_covers_it() {
    let (script, mut engine) = engine_after_read();
    script.on("Page.getFrameTree", signup::load("frame-tree"));
    script.on("DOM.getNodeForLocation", json!({"backendNodeId": 90}));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "row"}}));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "hit"}}));
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"inside": false, "what": "div \u{201c}Accept all\u{201d}"}}}),
    );
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("a refused click is not an error");

    assert_eq!(clicks(&script), 0);
    assert_eq!(
        engine.outcome(),
        Some("the tap was not made: div \u{201c}Accept all\u{201d} covers it")
    );
}

/// The read's node ids belong to the document it read. Once the frame has
/// loaded another, the same id may name something else, so nothing is
/// hit-tested or clicked.
#[test]
fn a_press_after_the_page_changed_refuses_without_hit_testing() {
    let (script, mut engine) = engine_after_read();
    let mut tree = signup::load("frame-tree");
    tree["frameTree"]["frame"]["loaderId"] = json!("ANOTHER-LOADER");
    script.on("Page.getFrameTree", tree);
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("a refused click is not an error");

    assert_eq!(
        script.params_of("DOM.getNodeForLocation"),
        Vec::<Value>::new()
    );
    assert_eq!(clicks(&script), 0);
    assert_eq!(
        engine.outcome(),
        Some("the tap was not made: the page has changed since it was read")
    );
}

/// A row below the fold is scrolled into view and measured again, and the
/// new centre is what is hit-tested and clicked.
#[test]
fn a_press_on_an_off_screen_row_scrolls_measures_then_hit_tests() {
    let (script, mut engine) = engine_after_read();
    script.on("Page.getFrameTree", signup::load("frame-tree"));
    script.on("DOM.scrollIntoViewIfNeeded", json!({}));
    script.on(
        "DOM.getBoxModel",
        json!({"model": {"content": [0, 290, 116, 290, 116, 308, 0, 308], "width": 116, "height": 18}}),
    );
    script.on("DOM.getNodeForLocation", json!({"backendNodeId": 34}));
    script_settle(&script, 4);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 58, y: 1130 },
            node: NodeKey::new("34:1"),
        })
        .expect("performs");

    let mut expected = vec![
        json!({"method": "Page.getFrameTree", "params": {}, "sessionId": SESSION}),
        json!({"method": "DOM.scrollIntoViewIfNeeded", "params": {"backendNodeId": 34}, "sessionId": SESSION}),
        json!({"method": "DOM.getBoxModel", "params": {"backendNodeId": 34}, "sessionId": SESSION}),
        json!({"method": "DOM.getNodeForLocation", "params": {"x": 58.0, "y": 299.0, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true}, "sessionId": SESSION}),
    ];
    expected.extend(click_at(58.0, 299.0));
    assert_eq!(without_ids(command_traffic(&script)), expected);
}

fn dialog_event(kind: &str, message: &str) -> Value {
    json!({
        "method": "Page.javascriptDialogOpening",
        "params": {"url": "http://127.0.0.1:52578/signup", "frameId": signup::MAIN_FRAME, "message": message, "type": kind, "hasBrowserHandler": true, "defaultPrompt": ""},
        "sessionId": SESSION,
    })
}

/// Script the click on the email field, with `last` as the answer to the
/// release, which is where a dialog the click opens arrives.
fn script_press_email(script: &Script, last: Answer) {
    script.on("Page.getFrameTree", signup::load("frame-tree"));
    script.on("DOM.getNodeForLocation", json!({"backendNodeId": 2}));
    script.on("Input.dispatchMouseEvent", json!({}));
    script.on("Input.dispatchMouseEvent", json!({}));
    script.answer("Input.dispatchMouseEvent", last);
}

/// An alert has one button, so it is accepted at once; its words are kept for
/// the next read, so the run still learns what it said.
#[test]
fn an_alert_is_accepted_and_its_text_kept_for_the_next_read() {
    let (script, mut engine) = engine_after_read();
    script_press_email(
        &script,
        Answer::EventsThen(
            vec![dialog_event("alert", "Saved")],
            Box::new(Answer::Result(json!({}))),
        ),
    );
    script_settle(&script, 4);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");

    assert_eq!(
        script.params_of("Page.handleJavaScriptDialog"),
        [json!({"accept": true})]
    );
    assert_eq!(engine.outcome(), Some("a dialog opened: Saved"));

    script_read(&script);
    let read = engine.read().expect("reads");
    assert_eq!(read.accepted_alert.as_deref(), Some("Saved"));
    assert_eq!(read.dialog, None);
}

/// A confirm is a decision, so it is held for the run to answer. The page is
/// blocked under it and answers nothing, so the next read is the dialog alone,
/// made without asking the page.
#[test]
fn a_confirm_is_held_and_the_next_read_is_the_dialog_alone() {
    let (script, mut engine) = engine_after_read();
    script_press_email(
        &script,
        Answer::EventsThen(
            vec![dialog_event("confirm", "Really delete?")],
            Box::new(Answer::Silence),
        ),
    );

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");

    assert_eq!(
        script.params_of("Page.handleJavaScriptDialog"),
        Vec::<Value>::new()
    );
    assert_eq!(engine.outcome(), Some("a dialog opened: Really delete?"));

    script.clear_sent();
    script.on("Target.getTargets", signup::load("targets"));
    let read = engine.read().expect("reads");
    assert_eq!(script.methods(), ["Target.getTargets"]);
    let dialog = read.dialog.expect("the confirm is in the read");
    assert_eq!(dialog.kind, DialogKind::Confirm);
    assert_eq!(&*dialog.message, "Really delete?");
    assert_eq!(read.nodes, []);
    assert_eq!(&*read.url, "http://127.0.0.1:52578/signup");

    script.on("Page.handleJavaScriptDialog", json!({}));
    script_settle(&script, 4);
    engine
        .perform(&BrowserCommand::Dialog(DialogAnswer::Dismiss))
        .expect("answers");
    assert_eq!(
        script.params_of("Page.handleJavaScriptDialog"),
        [json!({"accept": false})]
    );
}

#[test]
fn accepting_a_prompt_carries_its_words() {
    let (script, mut engine) = engine_after_read();
    script.event(&dialog_event("prompt", "Your name?"));
    script.on("Target.getTargets", signup::load("targets"));
    let read = engine.read().expect("reads");
    assert_eq!(read.dialog.map(|d| d.kind), Some(DialogKind::Prompt));

    script.on("Page.handleJavaScriptDialog", json!({}));
    script_settle(&script, 4);
    engine
        .perform(&BrowserCommand::Dialog(DialogAnswer::Accept(Some(
            "Ada".into(),
        ))))
        .expect("answers");

    assert_eq!(
        script.params_of("Page.handleJavaScriptDialog"),
        [json!({"accept": true, "promptText": "Ada"})]
    );
}

/// A dialog open before the run attached is never reported, and the page
/// answers nothing under it; a read that gets no answer says so.
#[test]
fn a_read_that_times_out_says_a_dialog_may_be_open() {
    let script = Script::new();
    script_attach(&script);
    script.answer("Page.getFrameTree", Answer::Silence);
    let mut engine =
        CdpEngine::attach(script.clone(), &TabKey::new(signup::TARGET), false).expect("attaches");

    let error = engine.read().expect_err("no answer");

    assert!(
        error
            .to_string()
            .contains("a JavaScript dialog may be open in this tab"),
        "{error}"
    );
}

/// Typing selects what the field holds through the editing command, then
/// inserts the words, and reads back what the field now holds.
#[test]
fn typing_taps_selects_all_through_commands_then_inserts_the_words() {
    let (script, mut engine) = engine_after_read();
    script.on(
        "DOM.describeNode",
        json!({"node": {"nodeName": "INPUT", "backendNodeId": 2}}),
    );
    script_press_email(&script, Answer::Result(json!({})));
    script
        .on("Input.dispatchKeyEvent", json!({}))
        .on("Input.dispatchKeyEvent", json!({}));
    script.on("Input.insertText", json!({}));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "field"}}));
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"value": "ada@example.test", "password": false}}}),
    );
    script_settle(&script, 5);

    engine
        .perform(&BrowserCommand::Fill {
            at: Point { x: 120, y: 19 },
            node: email(),
            text: "ada@example.test".into(),
        })
        .expect("performs");

    assert_eq!(clicks(&script), 3, "the field is tapped first");
    let keys = script.params_of("Input.dispatchKeyEvent");
    assert_eq!(keys[0]["commands"], json!(["selectAll"]));
    assert!(
        keys.iter().all(|k| k.get("modifiers").is_none()),
        "no ⌘A or Ctrl+A chord: {keys:?}"
    );
    assert_eq!(
        script.params_of("Input.insertText"),
        [json!({"text": "ada@example.test"})]
    );
    assert_eq!(
        engine.outcome(),
        Some("the field now holds \u{201c}ada@example.test\u{201d}")
    );
}

#[test]
fn a_password_field_does_not_have_its_value_repeated() {
    let (script, mut engine) = engine_after_read();
    script.on("DOM.describeNode", json!({"node": {"nodeName": "INPUT"}}));
    script_press_email(&script, Answer::Result(json!({})));
    script.on("DOM.resolveNode", json!({"object": {"objectId": "field"}}));
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"value": "", "password": true}}}),
    );
    script_settle(&script, 5);

    engine
        .perform(&BrowserCommand::Fill {
            at: Point { x: 120, y: 19 },
            node: email(),
            text: "hunter2".into(),
        })
        .expect("performs");

    assert_eq!(engine.outcome(), Some("the field now holds what was typed"));
}

/// A native drop-down opens a pop-up nothing can read, so it is never tapped:
/// its option is chosen by label in page script.
#[test]
fn a_select_is_set_by_its_label_without_a_tap() {
    let (script, mut engine) = engine_after_read();
    script.on("DOM.describeNode", json!({"node": {"nodeName": "SELECT"}}));
    script.on(
        "DOM.resolveNode",
        json!({"object": {"objectId": "country"}}),
    );
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"set": true, "chosen": "Spain", "options": ["Japan", "Philippines", "Spain"]}}}),
    );
    script_settle(&script, 5);

    engine
        .perform(&BrowserCommand::Fill {
            at: Point { x: 538, y: 19 },
            node: NodeKey::new("4:1"),
            text: "spain".into(),
        })
        .expect("performs");

    assert_eq!(clicks(&script), 0);
    let call = &script.params_of("Runtime.callFunctionOn")[0];
    assert_eq!(call["objectId"], "country");
    assert_eq!(call["arguments"], json!([{"value": "spain"}]));
    let script_text = call["functionDeclaration"].as_str().expect("a function");
    assert!(
        script_text.contains("'input'") && script_text.contains("'change'"),
        "{script_text}"
    );
    assert_eq!(
        engine.outcome(),
        Some("the field now holds \u{201c}Spain\u{201d}")
    );
}

#[test]
fn a_select_with_no_single_matching_option_lists_the_options() {
    let (script, mut engine) = engine_after_read();
    script.on("DOM.describeNode", json!({"node": {"nodeName": "SELECT"}}));
    script.on(
        "DOM.resolveNode",
        json!({"object": {"objectId": "country"}}),
    );
    script.on(
        "Runtime.callFunctionOn",
        json!({"result": {"type": "object", "value": {"set": false, "options": ["Japan", "Philippines", "Spain"]}}}),
    );
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Fill {
            at: Point { x: 538, y: 19 },
            node: NodeKey::new("4:1"),
            text: "France".into(),
        })
        .expect("performs");

    assert_eq!(
        engine.outcome(),
        Some(
            "nothing was chosen: no single option matches \u{201c}France\u{201d}; the options are Japan, Philippines, Spain"
        )
    );
}

#[test]
fn submit_presses_enter() {
    let (script, mut engine) = engine_after_read();
    script
        .on("Input.dispatchKeyEvent", json!({}))
        .on("Input.dispatchKeyEvent", json!({}));
    script_settle(&script, 3);

    engine.perform(&BrowserCommand::Submit).expect("performs");

    assert_eq!(
        without_ids(command_traffic(&script)),
        [
            json!({"method": "Input.dispatchKeyEvent", "params": {"type": "keyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": "\r"}, "sessionId": SESSION}),
            json!({"method": "Input.dispatchKeyEvent", "params": {"type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13}, "sessionId": SESSION}),
        ]
    );
}

/// A scroll is a wheel at the viewport's centre, of most of its height
/// (657 CSS pixels tall here).
#[test]
fn a_scroll_is_a_wheel_of_most_of_the_viewport() {
    let (script, mut engine) = engine_after_read();
    script.on("Input.dispatchMouseEvent", json!({}));
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Scroll(Direction::Down))
        .expect("performs");

    assert_eq!(
        without_ids(command_traffic(&script)),
        [
            json!({"method": "Input.dispatchMouseEvent", "params": {"type": "mouseWheel", "x": 640.0, "y": 329.0, "deltaX": 0, "deltaY": 526.0}, "sessionId": SESSION})
        ]
    );
}

#[test]
fn back_goes_to_the_previous_history_entry() {
    let (script, mut engine) = engine_after_read();
    script.on(
        "Page.getNavigationHistory",
        json!({"currentIndex": 1, "entries": [{"id": 5, "url": "https://example.test/"}, {"id": 6, "url": "http://127.0.0.1:52578/signup"}]}),
    );
    script.on("Page.navigateToHistoryEntry", json!({}));
    script_settle(&script, 0);

    engine.perform(&BrowserCommand::Back).expect("performs");

    assert_eq!(
        without_ids(command_traffic(&script)),
        [
            json!({"method": "Page.getNavigationHistory", "params": {}, "sessionId": SESSION}),
            json!({"method": "Page.navigateToHistoryEntry", "params": {"entryId": 5}, "sessionId": SESSION}),
        ]
    );
}

/// The navigation starts while the call is in flight; the settling waits for
/// its load, and the outcome names where the tab went.
#[test]
fn navigate_loads_the_address_and_reports_where_it_went() {
    let (script, mut engine) = engine_after_read();
    script.answer(
        "Page.navigate",
        Answer::EventsThen(
            vec![json!({"method": "Page.frameNavigated", "params": {"frame": {"id": signup::MAIN_FRAME, "loaderId": "L2", "url": "https://example.test/next?q=1"}}, "sessionId": SESSION})],
            Box::new(Answer::ThenEvents(
                Box::new(Answer::Result(json!({"frameId": signup::MAIN_FRAME, "loaderId": "L2"}))),
                vec![json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.0}, "sessionId": SESSION})],
            )),
        ),
    );
    script.on(
        "Page.createIsolatedWorld",
        json!({"executionContextId": 42}),
    );
    script_settle(&script, 0);

    engine
        .perform(&BrowserCommand::Navigate(
            "https://example.test/next?q=1".into(),
        ))
        .expect("performs");

    assert_eq!(
        script.params_of("Page.navigate"),
        [json!({"url": "https://example.test/next?q=1"})]
    );
    assert_eq!(engine.outcome(), Some("navigated to https://example.test"));
    assert_eq!(
        script.params_of("Page.createIsolatedWorld").len(),
        1,
        "the new document needs a new world for its watch"
    );
}

/// A tab opened by an owned tab is owned too, and can then be switched to:
/// activated, attached with Page enabled, and driven from then on.
#[test]
fn a_tab_the_page_opens_is_reported_and_can_be_switched_to() {
    let (script, mut engine) = engine_after_read();
    script_press_email(
        &script,
        Answer::EventsThen(
            vec![
                json!({"method": "Target.targetCreated", "params": {"targetInfo": {"targetId": "T2", "type": "page", "title": "Results", "url": "https://example.test/results", "attached": false, "openerId": signup::TARGET, "canAccessOpener": false}}}),
            ],
            Box::new(Answer::Result(json!({}))),
        ),
    );
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");
    assert_eq!(engine.outcome(), Some("opened a new tab: Results"));

    script.clear_sent();
    script.on("Target.activateTarget", json!({}));
    script.on("Target.attachToTarget", json!({"sessionId": "S2"}));
    script.on("Page.enable", json!({}));
    script.on("Page.createIsolatedWorld", json!({"executionContextId": 7}));
    script.on(
        "Runtime.evaluate",
        json!({"result": {"type": "object", "value": {"quiet": 500, "mutations": 0}}}),
    );

    engine
        .perform(&BrowserCommand::SwitchTab(TabKey::new("T2")))
        .expect("switches");

    assert_eq!(
        without_ids(command_traffic(&script))[..4],
        [
            json!({"method": "Target.activateTarget", "params": {"targetId": "T2"}}),
            json!({"method": "Target.attachToTarget", "params": {"targetId": "T2", "flatten": true}}),
            json!({"method": "Page.enable", "params": {}, "sessionId": "S2"}),
            json!({"method": "Target.detachFromTarget", "params": {"sessionId": SESSION}}),
        ]
    );
    assert_eq!(engine.current_tab(), TabKey::new("T2"));
}

#[test]
fn switching_to_a_tab_the_run_does_not_own_is_refused() {
    let (_script, mut engine) = engine_after_read();

    let error = engine
        .perform(&BrowserCommand::SwitchTab(TabKey::new(
            "E8DD778F620BA08560C3270923315360",
        )))
        .expect_err("not owned");

    assert!(
        error.to_string().contains("not one this run owns"),
        "{error}"
    );
}

/// Settling waits for the DOM to go quiet in an isolated world, so the page's
/// scripts cannot see the watch; with no change at all, the outcome says so.
#[test]
fn settling_waits_for_quiet_in_an_isolated_world_and_reports_no_change() {
    let (script, mut engine) = engine_after_read();
    script_settle(&script, 3);

    engine.perform(&BrowserCommand::Settle).expect("settles");

    let evaluate = &script.params_of("Runtime.evaluate")[0];
    assert_eq!(evaluate["contextId"], 41);
    assert_eq!(evaluate["awaitPromise"], true);
    assert!(
        evaluate["expression"]
            .as_str()
            .is_some_and(|e| e.contains("MutationObserver"))
    );
    assert_eq!(engine.outcome(), Some("nothing changed"));
    assert_eq!(engine.quiet_for_ms(), Some(120));
}

#[test]
fn a_page_that_changed_in_place_reports_no_line() {
    let (script, mut engine) = engine_after_read();
    script_settle(&script, 9);

    engine.perform(&BrowserCommand::Settle).expect("settles");

    assert_eq!(engine.outcome(), None);
}

/// Dropping the engine leaves the tab as it was: it detaches and goes.
#[test]
fn dropping_the_engine_detaches_from_the_tab() {
    let (script, engine) = engine_after_read();

    drop(engine);

    assert_eq!(
        without_ids(script.sent()),
        [json!({"method": "Target.detachFromTarget", "params": {"sessionId": SESSION}})]
    );
}

/// A modal's rows are what the reader offers, and it finds them as the nodes
/// after the dialog. A control inside the dialog drawn on a layer of its own
/// must still come before page nodes painted between the two.
#[test]
fn a_dialogs_nodes_follow_it_even_when_painted_later() {
    // Document order: dialog (backend 3), its button (4), then a page link (5).
    // Paint order: dialog 2, page link 4, the dialog's button 6.
    let snapshot = json!({
        "strings": ["", "#document", "BODY", "DIV", "BUTTON", "A", "Modal page"],
        "documents": [{
            "title": 6, "frameId": 0, "scrollOffsetX": 0, "scrollOffsetY": 0,
            "nodes": {
                "parentIndex": [-1, 0, 1, 2, 1],
                "nodeName": [1, 2, 3, 4, 5],
                "backendNodeId": [1, 2, 3, 4, 5],
                "attributes": [[], [], [], [], []],
                "isClickable": {"index": [3, 4]},
                "contentDocumentIndex": {"index": [], "value": []}
            },
            "layout": {
                "nodeIndex": [0, 1, 2, 3, 4],
                "bounds": [[0, 0, 800, 600], [0, 0, 800, 600], [200, 100, 400, 300], [250, 300, 80, 30], [10, 10, 60, 20]],
                "paintOrders": [0, 1, 2, 6, 4],
                "clientRects": [[], [], [], [], []]
            }
        }]
    });
    let ax = json!({"nodes": [
        {"nodeId": "1", "ignored": false, "role": {"value": "RootWebArea"}, "name": {"value": "Modal page"}, "childIds": ["3", "5"], "backendDOMNodeId": 1},
        {"nodeId": "3", "ignored": false, "role": {"value": "dialog"}, "name": {"value": "Delete file"}, "parentId": "1", "childIds": ["4"], "backendDOMNodeId": 3,
         "properties": [{"name": "modal", "value": {"type": "boolean", "value": true}}]},
        {"nodeId": "4", "ignored": false, "role": {"value": "button"}, "name": {"value": "Delete"}, "parentId": "3", "childIds": [], "backendDOMNodeId": 4},
        {"nodeId": "5", "ignored": false, "role": {"value": "link"}, "name": {"value": "Behind"}, "parentId": "1", "childIds": [], "backendDOMNodeId": 5}
    ]});
    let raw = RawRead {
        frame_tree: json!({"frameTree": {"frame": {"id": "M", "loaderId": "L", "url": "https://example.test/"}}}),
        ax_trees: vec![("M".into(), ax)],
        snapshot,
        layout_metrics: json!({"cssVisualViewport": {"clientWidth": 800, "clientHeight": 600, "pageX": 0, "pageY": 0}}),
        targets: json!({"targetInfos": []}),
    };

    let read =
        translate(&raw, "M", &OwnedTabs::default(), &mut Loaders::default()).expect("translates");

    let names: Vec<&str> = read.nodes.iter().map(|n| &*n.name).collect();
    assert_eq!(names, ["Delete file", "Delete", "Behind"]);
    assert!(read.nodes[0].states.modal);
}

/// The owner's tabs are never the run's. A tab that was already open when the
/// run attached stays the owner's even when the protocol reports the start
/// tab as its opener, whether it is reported again as discovery starts or
/// later as it changes.
#[test]
fn a_tab_open_before_attaching_is_never_adopted() {
    const OWNERS: &str = "E8DD778F620BA08560C3270923315360";
    let owners_tab = |method: &str| {
        json!({
            "method": method,
            "params": {"targetInfo": {"targetId": OWNERS, "type": "page", "title": "Bank", "url": "https://bank.example.test/", "attached": false, "openerId": signup::TARGET, "canAccessOpener": false}},
        })
    };
    let script = Script::new();
    script.on("Target.getTargets", signup::load("targets"));
    script.answer(
        "Target.setDiscoverTargets",
        Answer::EventsThen(
            vec![owners_tab("Target.targetCreated")],
            Box::new(Answer::Result(json!({}))),
        ),
    );
    script.on("Target.attachToTarget", json!({"sessionId": SESSION}));
    script.on("Page.enable", json!({}));
    script_read(&script);
    let mut engine =
        CdpEngine::attach(script.clone(), &TabKey::new(signup::TARGET), false).expect("attaches");
    engine.read().expect("reads");

    script_press_email(
        &script,
        Answer::EventsThen(
            vec![owners_tab("Target.targetInfoChanged")],
            Box::new(Answer::Result(json!({}))),
        ),
    );
    script_settle(&script, 3);
    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");

    assert_ne!(engine.outcome(), Some("opened a new tab: Bank"));
    script_read(&script);
    let read = engine.read().expect("reads");
    assert!(
        read.tabs.iter().all(|tab| tab.key.as_str() != OWNERS),
        "the owner's tab is not the run's: {:?}",
        read.tabs
    );
}

/// A tab the run's own tab opens after attaching is the run's.
#[test]
fn a_tab_created_after_attaching_by_the_start_tab_is_adopted() {
    let (script, mut engine) = engine_after_read();
    script_press_email(
        &script,
        Answer::EventsThen(
            vec![
                json!({"method": "Target.targetCreated", "params": {"targetInfo": {"targetId": "T9", "type": "page", "title": "Receipt", "url": "https://example.test/receipt", "attached": false, "openerId": signup::TARGET}}}),
            ],
            Box::new(Answer::Result(json!({}))),
        ),
    );
    script_settle(&script, 3);

    engine
        .perform(&BrowserCommand::Press {
            at: Point { x: 120, y: 19 },
            node: email(),
        })
        .expect("performs");

    assert_eq!(engine.outcome(), Some("opened a new tab: Receipt"));
}
