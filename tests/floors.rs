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

/// Accepting a dialog is held to the stricter floor than dismissing it: a
/// confirm is usually "really delete?", and a "leave this site?" discards what
/// was typed. The same 0.7 dismisses and does not accept.
#[test]
fn accepting_a_dialog_needs_more_certainty_than_dismissing_it() {
    use jev_pilot::platform::HierarchyError;
    use jev_pilot::snapshot::{Bounds, Dialog, DialogKind, Element, Snapshot};

    struct Web;
    impl Platform for Web {
        fn name(&self) -> &'static str {
            "Web"
        }
        fn operations(&self) -> &'static [Operation] {
            &[
                Operation::AcceptDialog,
                Operation::DismissDialog,
                Operation::Wait,
                Operation::Done,
                Operation::Blocked,
            ]
        }
        fn parse_hierarchy(&self, _raw: &str) -> Result<Snapshot, HierarchyError> {
            unreachable!("these tests build snapshots by hand")
        }
    }

    let under = Snapshot::new(vec![Element {
        label: "Delete".into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, 0, 10, 10),
    }])
    .expect("fits")
    .with_dialog(Some(Dialog {
        kind: DialogKind::Confirm,
        message: "Delete this draft?".into(),
        default_prompt: None,
    }));
    let catalog = Catalog::for_screen(&under, &Web);
    let floors = Floors::new(at(0.6)).requiring_for(Consequence::Destructive, at(0.85));

    assert!(
        catalog
            .resolve(&answers("dismiss_dialog", 0.70), &floors)
            .is_ok()
    );
    assert!(
        catalog
            .resolve(&answers("accept_dialog", 0.70), &floors)
            .is_err()
    );
    assert!(
        catalog
            .resolve(&answers("accept_dialog", 0.90), &floors)
            .is_ok()
    );
}
