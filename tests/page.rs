//! The engine-neutral page read, as a fixture holds it.

#![cfg(feature = "browser")]

use jev_pilot::platform::page::{Checked, PageBox, PageNode, PageRead, PageTab, Viewport};
use jev_pilot::snapshot::{Dialog, DialogKind, NodeKey, TabKey};

fn sample() -> PageRead {
    PageRead {
        url: "https://example.test/search".into(),
        title: "Search".into(),
        viewport: Viewport {
            width: 1280.0,
            height: 720.0,
        },
        dialog: Some(Dialog {
            kind: DialogKind::Prompt,
            message: "Name?".into(),
            default_prompt: Some("Ada".into()),
        }),
        accepted_alert: Some("Saved".into()),
        tabs: vec![PageTab {
            key: TabKey::new("tab-2"),
            title: "Results".into(),
            url: "https://example.test/results".into(),
            opened_by_run: true,
        }],
        nodes: vec![
            PageNode {
                frame: "main".into(),
                role: "combobox".into(),
                name: "Country".into(),
                value: Some("Philippines".into()),
                protected: false,
                description: Some("Where you live".into()),
                states: jev_pilot::platform::page::NodeStates {
                    focusable: true,
                    editable: false,
                    disabled: false,
                    checked: Some(Checked::Mixed),
                    expanded: Some(false),
                    selected: true,
                    focused: true,
                    modal: false,
                },
                clickable: true,
                options: vec!["Japan".into(), "Philippines".into()],
                context: Some("Address".into()),
                bounds: Some(PageBox {
                    x: 10.5,
                    y: 20.0,
                    width: 200.0,
                    height: 32.0,
                }),
                key: NodeKey::new("57:3"),
            },
            PageNode {
                frame: "main".into(),
                role: "generic".into(),
                name: String::new().into(),
                value: None,
                protected: false,
                description: None,
                states: jev_pilot::platform::page::NodeStates::default(),
                clickable: false,
                options: Vec::new(),
                context: None,
                bounds: None,
                key: NodeKey::new("58:3"),
            },
        ],
    }
}

/// Fixtures hold the JSON, so what is written must read back as the same page.
#[test]
fn a_page_read_round_trips_through_json() {
    let read = sample();

    let json = serde_json::to_string(&read).expect("serialises");
    let back: PageRead = serde_json::from_str(&json).expect("deserialises");

    assert_eq!(back, read);
}

/// A recorded fixture names only what a node has; the rest takes its quiet
/// default, so a fixture stays short enough to read.
#[test]
fn a_fixture_may_leave_out_what_a_node_does_not_have() {
    let json = r#"{
        "url": "https://example.test/",
        "title": "Home",
        "viewport": {"width": 800, "height": 600},
        "nodes": [
            {"frame": "f1", "role": "button", "name": "Go", "key": "9:1",
             "clickable": true, "bounds": {"x": 1, "y": 2, "width": 3, "height": 4}}
        ]
    }"#;

    let read: PageRead = serde_json::from_str(json).expect("deserialises");

    assert_eq!(read.dialog, None);
    assert_eq!(read.accepted_alert, None);
    assert_eq!(read.tabs, Vec::new());
    let node = &read.nodes[0];
    assert_eq!(node.key, NodeKey::new("9:1"));
    assert_eq!(
        node.states,
        jev_pilot::platform::page::NodeStates::default()
    );
    assert_eq!(node.options, Vec::<Box<str>>::new());
    assert_eq!(node.value, None);
    assert!(
        !node.protected,
        "a node is not a password field unless it says so"
    );
}
