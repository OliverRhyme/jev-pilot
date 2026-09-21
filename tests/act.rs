//! Enumerating actions and turning a model's choice back into one.

use jev_pilot::act::{Catalog, Consequence, Operation};
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

/// Row text belongs in the state, not repeated in the options.
///
/// A Choice may carry `null` for an option that needs no description, and the
/// rows are already in the state for the other questions to read. Sending each
/// label twice measured 2187 input tokens against 1590 for the same request
/// with the labels referenced instead — 27% more, for the same answer at the
/// same confidence. Billing is on input tokens only, so the duplicate is pure
/// waste.
#[test]
fn target_options_are_keys_only_with_the_text_left_in_the_state() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_value(
        catalog
            .tap_target_question("Open Wi-Fi settings")
            .expect("rows exist"),
    )
    .expect("serializes");

    let criteria = wire["criteria"].as_object().expect("a criteria map");
    assert!(criteria.contains_key("A3"));
    assert!(
        criteria.values().all(serde_json::Value::is_null),
        "options carry no text of their own: {criteria:?}"
    );
    // And the question says where to look.
    assert!(wire["instructions"].to_string().contains("rows"));
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
