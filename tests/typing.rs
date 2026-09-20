//! Typing: the one thing a System One model cannot do for itself.

use jev_pilot::act::{Act, Catalog, Decision, Floors, Operation};
use jev_pilot::judgment::Confidence;
use jev_pilot::platform::Android;
use jev_pilot::snapshot::{Bounds, Element, Snapshot};
use jev_pilot::step::StepAnswers;

fn field(label: &str, editable: bool) -> Element {
    Element {
        label: label.into(),
        detail: None,
        editable,
        bounds: Bounds {
            left: 0,
            top: 0,
            right: 100,
            bottom: 40,
        },
    }
}

fn screen() -> Snapshot {
    Snapshot::new(vec![
        field("Search YouTube", true),
        field("Home", false),
        field("Subscriptions", false),
    ])
    .expect("a small screen")
}

fn answers(value: serde_json::Value) -> StepAnswers {
    serde_json::from_value(value).expect("answers parse")
}

/// Typing is offered only when something can receive text. Offering it on a
/// screen with no field invites a decision that cannot be carried out.
#[test]
fn typing_is_offered_only_when_the_screen_has_a_field() {
    let with_field = Catalog::for_screen(&screen(), &Android).accepting_text();
    assert!(with_field.operations().contains(&Operation::TypeText));

    let no_fields = Snapshot::new(vec![field("Home", false)]).expect("a screen");
    let without = Catalog::for_screen(&no_fields, &Android).accepting_text();
    assert!(!without.operations().contains(&Operation::TypeText));

    // And never when the caller has nothing to write the text with.
    let unsupported = Catalog::for_screen(&screen(), &Android);
    assert!(!unsupported.operations().contains(&Operation::TypeText));
}

/// The field head offers only rows that accept text, so a tap target and a
/// typing target cannot be confused for one another.
#[test]
fn the_field_head_offers_only_rows_that_accept_text() {
    let catalog = Catalog::for_screen(&screen(), &Android).accepting_text();

    let wire = serde_json::to_value(catalog.type_field_question("search").expect("a field"))
        .expect("serializes");
    let fields = wire["criteria"].as_object().expect("a criteria map");

    assert_eq!(
        fields.len(),
        1,
        "only the search box accepts text: {fields:?}"
    );
    assert!(
        fields.values().all(serde_json::Value::is_null),
        "the text lives in the state, not in the options: {fields:?}"
    );

    // The state-side text is what the key refers to, and the keys agree.
    let described: Vec<(String, &str)> = catalog
        .fields_offered()
        .map(|(id, text)| (id.to_string(), text))
        .collect();
    assert_eq!(described.len(), 1);
    assert!(described[0].1.contains("Search YouTube"), "{described:?}");
    assert!(fields.contains_key(&described[0].0), "keys must agree");
}

/// Jev decides whether and where to type. It cannot decide *what*, so the
/// decision comes back needing words rather than pretending to have them.
#[test]
fn a_typing_decision_comes_back_needing_words() {
    let snapshot = screen();
    let catalog = Catalog::for_screen(&snapshot, &Android).accepting_text();

    let decision = catalog
        .resolve(
            &answers(serde_json::json!({
                "operation": { "type": "choice", "choice": "type_text", "confidence": 0.95 },
                "tap_target": { "type": "choice", "choice": "A1", "confidence": 0.9 },
                "type_field": { "type": "choice", "choice": "A1", "confidence": 0.95 },
                "goal_met": { "type": "noul", "noul": 0.02 },
                "is_error_screen": { "type": "noul", "noul": 0.01 }
            })),
            &Floors::new(Confidence::ZERO),
        )
        .expect("a decision");

    let into = match decision {
        Decision::NeedsText { into } => into,
        other => panic!("expected a request for words, got {other:?}"),
    };
    assert_eq!(
        &*snapshot.resolve(into).expect("live").label,
        "Search YouTube"
    );
}

/// Every other operation resolves straight to something the device can do.
#[test]
fn a_non_typing_decision_is_ready_as_it_stands() {
    let snapshot = screen();
    let catalog = Catalog::for_screen(&snapshot, &Android).accepting_text();

    let decision = catalog
        .resolve(
            &answers(serde_json::json!({
                "operation": { "type": "choice", "choice": "tap", "confidence": 0.95 },
                "tap_target": { "type": "choice", "choice": "A2", "confidence": 0.95 },
                "goal_met": { "type": "noul", "noul": 0.02 },
                "is_error_screen": { "type": "noul", "noul": 0.01 }
            })),
            &Floors::new(Confidence::ZERO),
        )
        .expect("a decision");

    assert!(
        matches!(decision, Decision::Ready(Act::Tap(_))),
        "{decision:?}"
    );
}
