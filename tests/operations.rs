//! The two-dimensional action space: what to do, and what to do it to.

use jev_pilot::act::{Act, Catalog, Direction, Operation, Outcome};
use jev_pilot::judgment::Confidence;
use jev_pilot::platform::{Android, Ios, Platform};
use jev_pilot::step::StepAnswers;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

fn answers(value: serde_json::Value) -> StepAnswers {
    serde_json::from_value(value).expect("answers parse")
}

/// Operation and target are asked as separate heads of one request. A flat list
/// of every operation crossed with every element would grow multiplicatively
/// and could express pairings that make no sense, like scrolling a button.
#[test]
fn the_operation_head_offers_operations_and_the_target_head_offers_elements() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let operations =
        serde_json::to_value(catalog.operation_question("Turn on Wi-Fi")).expect("serializes");
    let targets = serde_json::to_value(
        catalog
            .tap_target_question("Turn on Wi-Fi")
            .expect("rows are tappable"),
    )
    .expect("serializes");

    let ops = operations["criteria"].as_object().expect("a criteria map");
    assert!(ops.contains_key("tap"), "got {ops:?}");
    assert!(ops.contains_key("scroll_down"), "got {ops:?}");
    assert!(ops.contains_key("done"), "got {ops:?}");
    assert!(
        !ops.keys().any(|k| k.starts_with('A')),
        "operations are named, not indexed"
    );

    let rows = targets["criteria"].as_object().expect("a criteria map");
    assert_eq!(
        rows.len(),
        snapshot.len(),
        "every row is a candidate target"
    );
    assert!(rows.contains_key("A3"));
}

/// iOS has no system back, so it must not appear among the operations offered.
#[test]
fn only_operations_the_platform_supports_are_offered() {
    assert!(Android.operations().contains(&Operation::Back));
    assert!(!Ios.operations().contains(&Operation::Back));
    for platform in [&Android as &dyn Platform, &Ios] {
        assert!(platform.operations().contains(&Operation::ScrollDown));
        assert!(platform.operations().contains(&Operation::Done));
    }
}

/// Target heads are answered speculatively, in parallel with the operation. Only
/// the head matching the chosen operation is consumed; the rest are discarded.
#[test]
fn the_target_matching_the_chosen_operation_is_the_one_used() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let act = catalog
        .resolve(
            &answers(serde_json::json!({
                "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
                "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.99 },
                "goal_met": { "type": "noul", "noul": 0.02 },
                "is_error_screen": { "type": "noul", "noul": 0.01 }
            })),
            Confidence::ZERO,
        )
        .expect("a decision");

    let expected = snapshot.refs().nth(2).expect("a third row").0;
    assert_eq!(act, Act::Tap(expected));
}

/// An operation that needs no target ignores the target heads entirely, even
/// when they were answered.
#[test]
fn an_operation_without_a_target_ignores_the_speculative_heads() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    for (choice, expected) in [
        ("scroll_down", Act::Scroll(Direction::Down)),
        ("done", Act::Finish(Outcome::Achieved)),
        ("blocked", Act::Finish(Outcome::Blocked)),
    ] {
        let act = catalog
            .resolve(
                &answers(serde_json::json!({
                    "operation": { "type": "choice", "choice": choice, "confidence": 0.99 },
                    "tap_target": { "type": "choice", "choice": "A7", "confidence": 0.99 },
                    "goal_met": { "type": "noul", "noul": 0.02 },
                    "is_error_screen": { "type": "noul", "noul": 0.01 }
                })),
                Confidence::ZERO,
            )
            .expect("a decision");
        assert_eq!(act, expected, "for {choice}");
    }
}

/// Both heads must clear the floor. A confident operation aimed at a target the
/// model was unsure of is still a guess about where to tap.
#[test]
fn an_uncertain_target_is_refused_even_when_the_operation_is_certain() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);
    let floor = Confidence::new(0.6).expect("valid floor");

    let outcome = catalog.resolve(
        &answers(serde_json::json!({
            "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
            "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.20 },
            "goal_met": { "type": "noul", "noul": 0.02 },
            "is_error_screen": { "type": "noul", "noul": 0.01 }
        })),
        floor,
    );

    assert!(outcome.is_err(), "got {outcome:?}");
}

/// The target head is answered in parallel with the operation, so it cannot see
/// which operation was chosen — but it must still see the goal. Without it the
/// question reduces to "which row looks important?", and the answer spreads
/// across every plausible row instead of concentrating on the one that advances
/// the task.
#[test]
fn the_target_question_states_the_goal_it_is_serving() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_value(
        catalog
            .tap_target_question("Open the Storage screen")
            .expect("rows exist"),
    )
    .expect("serializes");

    let rendered = wire["instructions"].to_string();
    assert!(
        rendered.contains("Open the Storage screen"),
        "the target head must carry the goal: {rendered}"
    );
}
