//! Enumerating actions and turning a model's choice back into one.

use jev_pilot::act::Catalog;
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
