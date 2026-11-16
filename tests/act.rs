//! Enumerating actions and turning a model's choice back into one.

use jev_pilot::act::{Catalog, Consequence, Operation};
use jev_pilot::device::{Command, command_for};
use jev_pilot::judgment::Confidence;
use jev_pilot::platform::{Android, Platform};

const SETTINGS: &str = include_str!("fixtures/settings.xml");

/// Options must reach the model in the order they appear on screen. A map keyed
/// by name sorts "A10" before "A2", quietly reshuffling the list a position-
/// sensitive model is reading.
#[test]
fn targets_reach_the_wire_in_screen_order_not_alphabetical_order() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_string(
        &catalog
            .tap_target_question("Open Wi-Fi settings")
            .expect("rows exist"),
    )
    .expect("question serializes");

    let ninth = wire.find("\"A9\"").expect("A9 is offered");
    let tenth = wire.find("\"A10\"").expect("A10 is offered");
    assert!(
        ninth < tenth,
        "A9 must precede A10 on the wire, got:\n{wire}"
    );
}

/// Confidence is a probability. A threshold outside the unit interval is a bug
/// in the caller, and the type refuses to represent one.
#[test]
fn confidence_outside_the_unit_interval_does_not_exist() {
    assert!(Confidence::new(-3.0).is_none());
    assert!(Confidence::new(1.5).is_none());
    assert!(Confidence::new(0.5).is_some());
}

/// Each row option carries the row's own words, not a key to look up in the
/// state. A bare key is a hop the model has to make, and on a PIN pad the hop
/// is where it falls: `A2` is the key labelled "1", and asked for the digit 2
/// with bare keys Jev chose `A2` — the key "1" — with 0.10 on the right one.
/// With each option described by its label, the same request put 0.92 on the
/// right key, and 0.70 against 0.10 four digits later. The labels travel twice,
/// once in the state and once here, and that is the price of the answer.
#[test]
fn target_options_carry_the_words_of_the_row_they_name() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_value(
        catalog
            .tap_target_question("Open Wi-Fi settings")
            .expect("rows exist"),
    )
    .expect("serializes");

    let criteria = wire["criteria"].as_object().expect("a criteria map");
    let rows: std::collections::HashMap<String, &str> = catalog
        .rows()
        .map(|(id, text)| (id.to_string(), text))
        .collect();
    assert!(!criteria.is_empty());
    for (key, described) in criteria {
        assert_eq!(described.as_str(), Some(rows[key]), "option {key}");
    }
}

/// `Home` is not recoverable by going back. It discards the app's navigation
/// stack, and an app that guards a session tears it down — so the screen a run
/// returns to is not the screen it left. Measured: a run chose `home` at 0.48
/// on its first step, was handed a launcher, and spent the rest of its budget
/// picking plausible-looking icons in a different app entirely.
#[test]
fn leaving_the_app_costs_more_than_an_ordinary_gesture() {
    assert_eq!(
        Operation::Home.consequence(),
        Consequence::Destructive,
        "pressing home cannot be undone by going back",
    );
    assert_eq!(Operation::Back.consequence(), Consequence::Ordinary);
}

/// Every operation must be reachable by the name it is offered under. A
/// caller answering an impasse names one, and a name that resolves to nothing
/// is an answer that cannot be carried out.
///
/// A list of names kept by hand somewhere else goes stale the moment an
/// operation is added: `return_to_app` and `close_keyboard` were both offered
/// by the catalog and unknown to the command line, where an unrecognised name
/// silently ended the run.
#[test]
fn every_operation_can_be_found_by_the_name_it_is_offered_under() {
    for operation in Operation::all() {
        assert_eq!(
            Operation::from_key(operation.key()),
            Some(*operation),
            "{} is offered but cannot be named back",
            operation.key(),
        );
    }
    assert_eq!(Operation::from_key("bogus"), None);
}

/// The platform's operations are drawn from the same set, so nothing can be
/// offered on a screen that could not then be named.
#[test]
fn a_platform_offers_nothing_that_cannot_be_named() {
    for operation in Android.operations() {
        assert!(
            Operation::all().contains(operation),
            "{} is offered by Android and is not among the operations",
            operation.key(),
        );
    }
}

/// Putting the keyboard away and going back are the same gesture and not the
/// same act. Reported as "Pressed Back", the run's own memory says it
/// navigated when it did not — and the next step, told it has just gone back,
/// goes back again and leaves the form it was filling.
///
/// Measured on a transfer form: type, close keyboard, back, and round to the
/// start of the flow. Three times in one run.
#[test]
fn putting_the_keyboard_away_is_not_recounted_as_going_back() {
    use jev_pilot::act::{Act, Decision};
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let snapshot = Snapshot::new(vec![Element {
        label: "Continue".into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, 0, 400, 80),
    }])
    .expect("a screen")
    .with_keyboard_open(true);
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let Ok(Decision::Ready(act)) = catalog.act_from(Operation::CloseKeyboard, None) else {
        panic!("closing the keyboard is offered while one is up");
    };
    assert!(
        matches!(act, Act::CloseKeyboard),
        "it is its own act, so it can be described as itself: {act:?}",
    );
}

/// A Google results page offers "About this result" under every result, some
/// twenty times. Rows a screen names identically that often cannot be told
/// apart by what they say, and each one takes a share of the choice from the
/// rows that can. Measured: the right result chosen at 0.20 to 0.39 among 125
/// rows, below the floor every time.
#[test]
fn a_label_repeated_across_the_screen_is_not_offered_to_choose_from() {
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let row = |label: &str, top: i32| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, top, 400, 40),
    };
    let mut rows = vec![row("Result A", 0), row("Result B", 50)];
    rows.extend((0..5).map(|n| row("About this result", 100 + 50 * n)));
    let snapshot = Snapshot::new(rows).expect("a screen");

    let catalog = Catalog::for_screen(&snapshot, &Android);

    let offered: Vec<&str> = catalog.rows().map(|(_, text)| text).collect();
    assert_eq!(offered, ["Result A", "Result B"]);
}

/// Left out of Jev's choice, not out of reach: an escalation numbers rows as
/// the screen lists them, and may still name one of these.
#[test]
fn a_row_left_out_of_the_choice_can_still_be_named_by_an_escalation() {
    use jev_pilot::act::{Act, Decision};
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let row = |label: &str, top: i32| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, top, 400, 40),
    };
    let mut rows: Vec<Element> = (0..5).map(|n| row("About this result", 50 * n)).collect();
    rows.push(row("Result A", 300));
    let snapshot = Snapshot::new(rows).expect("a screen");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let Ok(Decision::Ready(Act::Tap(first))) = catalog.act_from(Operation::Tap, Some(0)) else {
        panic!("row 0 is on screen and can be tapped");
    };
    let Ok(Decision::Ready(Act::Tap(last))) = catalog.act_from(Operation::Tap, Some(5)) else {
        panic!("row 5 is on screen and can be tapped");
    };
    assert_eq!(
        snapshot.resolve(first).expect("current").label.as_ref(),
        "About this result"
    );
    assert_eq!(
        snapshot.resolve(last).expect("current").label.as_ref(),
        "Result A"
    );
}

/// A browser can be sent to a supplied address, switched to another of the
/// run's tabs, and asked to answer a dialog. Each has its own name, and
/// accepting is held to the stricter floor: a confirm is usually "really
/// delete?" and a "leave site?" is "discard what you typed?".
#[test]
fn the_browser_operations_are_named_and_priced() {
    for (operation, key) in [
        (Operation::Navigate, "navigate"),
        (Operation::SwitchTab, "switch_tab"),
        (Operation::AcceptDialog, "accept_dialog"),
        (Operation::DismissDialog, "dismiss_dialog"),
    ] {
        assert_eq!(operation.key(), key);
        assert_eq!(Operation::from_key(key), Some(operation));
        assert!(Operation::all().contains(&operation));
        assert_ne!(operation.rubric(), "");
        assert!(!operation.needs_tap_target());
        assert!(!operation.needs_field());
    }
    assert_eq!(
        Operation::AcceptDialog.consequence(),
        Consequence::Destructive
    );
    assert_eq!(
        Operation::DismissDialog.consequence(),
        Consequence::Ordinary
    );
    assert_eq!(Operation::Navigate.consequence(), Consequence::Ordinary);
    assert_eq!(Operation::SwitchTab.consequence(), Consequence::Ordinary);
}

fn row(label: &str, editable: bool, top: i32) -> jev_pilot::snapshot::Element {
    jev_pilot::snapshot::Element {
        label: label.into(),
        detail: None,
        editable,
        bounds: jev_pilot::snapshot::Bounds::from_origin_size(0, top, 100, 20),
    }
}

/// Geometry leaves a snapshot only through a tap point. A row the reader gave
/// a node key for is pressed or filled by that key, so an engine can check it
/// is still the thing under the pointer; a row with none is tapped and typed
/// into as on a phone, whose adapters know nothing of keys.
#[test]
fn a_row_with_a_node_key_is_pressed_and_filled_and_one_without_is_not() {
    use jev_pilot::act::Act;
    use jev_pilot::snapshot::{NodeKey, Point, Snapshot};

    let snapshot = Snapshot::new(vec![row("Search", true, 0), row("Go", false, 40)])
        .expect("fits")
        .with_node_keys(vec![Some(NodeKey::new("7:1")), None]);
    let handles: Vec<_> = snapshot.refs().map(|(handle, _)| handle).collect();

    assert_eq!(
        command_for(
            &Act::TypeText {
                into: handles[0],
                text: "opus".into()
            },
            &snapshot
        )
        .expect("resolves"),
        Some(Command::Fill {
            at: Point { x: 50, y: 10 },
            node: NodeKey::new("7:1"),
            text: "opus".into(),
        }),
    );
    assert_eq!(
        command_for(&Act::Tap(handles[0]), &snapshot).expect("resolves"),
        Some(Command::Press {
            at: Point { x: 50, y: 10 },
            node: NodeKey::new("7:1"),
        }),
    );
    assert_eq!(
        command_for(&Act::Tap(handles[1]), &snapshot).expect("resolves"),
        Some(Command::Tap(Point { x: 50, y: 50 })),
    );
    assert_eq!(
        command_for(
            &Act::TypeText {
                into: handles[1],
                text: "x".into()
            },
            &snapshot
        )
        .expect("resolves"),
        Some(Command::TypeText {
            at: Point { x: 50, y: 50 },
            text: "x".into()
        }),
    );
}

/// Navigating, switching tab and answering a dialog touch no row. A tab is
/// resolved to the engine's own key here, against the snapshot that issued the
/// reference, so a choice from an earlier read of the tab list is refused.
#[test]
fn navigation_tabs_and_dialogs_become_commands() {
    use jev_pilot::act::{Act, DialogReply};
    use jev_pilot::snapshot::{Snapshot, Tab, TabKey, TapError};

    let tabs = || {
        vec![Tab {
            key: TabKey::new("tab-2"),
            title: "Results".into(),
            url: "https://example.test/r".into(),
        }]
    };
    let first = Snapshot::new(vec![row("Go", false, 0)])
        .expect("fits")
        .with_tabs(tabs());
    let second = Snapshot::new(vec![row("Go", false, 0)])
        .expect("fits")
        .with_tabs(tabs());
    let chosen = first.tabs().next().expect("a tab").0;

    assert_eq!(
        command_for(&Act::Navigate("https://example.test/".into()), &first).expect("resolves"),
        Some(Command::Navigate("https://example.test/".into())),
    );
    assert_eq!(
        command_for(&Act::SwitchTab(chosen), &first).expect("resolves"),
        Some(Command::SwitchTab(TabKey::new("tab-2"))),
    );
    assert!(matches!(
        command_for(&Act::SwitchTab(chosen), &second),
        Err(TapError::Stale(_))
    ));
    let accept = DialogReply::Accept {
        words: Some("Ada".into()),
    };
    assert_eq!(
        command_for(&Act::Dialog(accept.clone()), &first).expect("resolves"),
        Some(Command::Dialog(accept)),
    );
    assert_eq!(
        command_for(&Act::Dialog(DialogReply::Dismiss), &first).expect("resolves"),
        Some(Command::Dialog(DialogReply::Dismiss)),
    );
}
