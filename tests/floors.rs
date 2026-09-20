//! Confidence gating scaled to what an action costs if it is wrong.

use jev_pilot::act::{Catalog, Consequence, Floors, Operation};
use jev_pilot::judgment::Confidence;
use jev_pilot::platform::{Android, Platform};
use jev_pilot::step::StepAnswers;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

fn answers(operation: &str, confidence: f64) -> StepAnswers {
    serde_json::from_value(serde_json::json!({
        "operation": { "type": "choice", "choice": operation, "confidence": confidence },
        "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 }
    }))
    .expect("answers parse")
}

fn at(value: f64) -> Confidence {
    Confidence::new(value).expect("a probability")
}

/// A wrong tap on a list row costs a `back`. A wrong verdict ends the run with
/// the wrong answer, and a wrong swipe can delete something. Gating all three
/// at one number means either acting on guesses or refusing sound decisions.
#[test]
fn a_verdict_needs_more_certainty_than_an_ordinary_tap() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);
    let floors = Floors::new(at(0.6)).requiring_for(Consequence::Terminal, at(0.85));

    // The same 0.7 is enough to tap and not enough to stop.
    assert!(catalog.resolve(&answers("tap", 0.70), &floors).is_ok());
    assert!(catalog.resolve(&answers("done", 0.70), &floors).is_err());
    assert!(catalog.resolve(&answers("done", 0.90), &floors).is_ok());
}

/// A gesture that deletes or archives is not undone by going back.
#[test]
fn a_destructive_gesture_is_gated_above_an_ordinary_one() {
    assert_eq!(Operation::SwipeLeft.consequence(), Consequence::Destructive);
    assert_eq!(Operation::Done.consequence(), Consequence::Terminal);
    assert_eq!(Operation::Blocked.consequence(), Consequence::Terminal);
    assert_eq!(Operation::Tap.consequence(), Consequence::Ordinary);
    assert_eq!(Operation::ScrollDown.consequence(), Consequence::Ordinary);
    assert_eq!(Operation::Back.consequence(), Consequence::Ordinary);
}

/// One floor for everything remains expressible, and stays the default.
#[test]
fn a_single_floor_still_applies_to_everything() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);
    let floors = Floors::new(at(0.6));

    assert!(catalog.resolve(&answers("done", 0.70), &floors).is_ok());
    assert!(catalog.resolve(&answers("tap", 0.50), &floors).is_err());
}
