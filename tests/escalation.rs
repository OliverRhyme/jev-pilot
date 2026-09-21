//! What happens when Jev was not sure enough to act.

use jev_pilot::act::{Act, Operation};
use jev_pilot::device::{Command, Device};
use jev_pilot::judgment::Confidence;
use jev_pilot::pilot::{Ending, Halt, Impasse, Judge, Pilot, Resolution};
use jev_pilot::platform::{Android, Platform};
use jev_pilot::snapshot::Snapshot;
use jev_pilot::step::{StepAnswers, StepQuestions};
use std::cell::RefCell;
use std::convert::Infallible;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

#[derive(Default)]
struct Fake {
    performed: Vec<Command>,
}

impl Device for Fake {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        Ok(Android.parse_hierarchy(SETTINGS).expect("fixture parses"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
}

struct Scripted(RefCell<Vec<serde_json::Value>>);

impl Judge for Scripted {
    type Error = Infallible;
    fn evaluate(
        &self,
        _state: serde_json::Value,
        _questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Infallible> {
        let next = self.0.borrow_mut().remove(0);
        Ok(serde_json::from_value(next).expect("scripted answer parses"))
    }
}

fn unsure() -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": "tap", "confidence": 0.30 },
        "tap_target": { "type": "choice", "choice": "A8", "confidence": 0.30 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
    })
}

fn floor() -> Confidence {
    Confidence::new(0.6).expect("a valid floor")
}

/// The default is to stop. Acting on a flat distribution is how a loop wanders
/// into unrelated application state, and silently escalating would be a policy
/// decision the caller never asked for.
#[test]
fn without_an_escalation_policy_an_impasse_ends_the_run() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
    .requiring(floor());

    let ending = pilot
        .pursue("Silence notifications")
        .expect("the run completes");

    assert!(matches!(ending, Ending::Uncertain { .. }), "got {ending:?}");
    assert!(pilot.device().performed.is_empty());
}

/// A second opinion picks from the same enumerated set Jev was offered. It is
/// handed the rows and the operations, and answers with an index — so a
/// reasoning model or a person is bound by exactly the same constraint, and
/// cannot invent a coordinate either.
#[test]
fn an_escalation_may_choose_from_the_same_options_jev_had() {
    let seen = std::rc::Rc::new(RefCell::new(None));
    let recorded = std::rc::Rc::clone(&seen);

    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![
            unsure(),
            serde_json::json!({
                "operation": { "type": "choice", "choice": "done", "confidence": 0.99 },
                "goal_met": { "type": "score", "score": 1.9, "confidence": 0.9 },
                "is_error_screen": { "type": "noul", "noul": 0.01 },
            }),
        ])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(
        move |impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
            *recorded.borrow_mut() = Some((impasse.rows.len(), impasse.goal.to_owned()));
            Ok(Resolution::Choose {
                operation: Operation::Tap,
                target: Some(2),
            })
        },
    );

    let ending = pilot
        .pursue("Silence notifications")
        .expect("the run completes");

    let (rows, goal) = seen.borrow().clone().expect("the escalation was consulted");
    assert_eq!(rows, 11, "it is shown every row Jev was shown");
    assert_eq!(goal, "Silence notifications");
    assert!(matches!(ending, Ending::Finished(_)), "got {ending:?}");
    assert!(
        matches!(pilot.device().performed.as_slice(), [Command::Tap(_)]),
        "the escalated choice was carried out"
    );
}

/// An escalation can also decline, which stops the run as if it had not been
/// consulted. A person asked "what now?" is allowed to say "give up".
#[test]
fn an_escalation_may_decline_and_stop_the_run() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> { Ok(Resolution::Stop) });

    let ending = pilot
        .pursue("Silence notifications")
        .expect("the run completes");

    assert!(matches!(ending, Ending::Uncertain { .. }), "got {ending:?}");
    assert!(pilot.device().performed.is_empty());
}

/// The explicit do-nothing policy, for a caller who wants the default named.
#[test]
fn halt_is_the_default_policy_written_out() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(Halt);

    assert!(matches!(
        pilot.pursue("Silence notifications").expect("completes"),
        Ending::Uncertain { .. }
    ));
}

/// The impasse says which head was unsure, so an escalation can tell "I don't
/// know what to do" from "I don't know which row".
#[test]
fn the_impasse_reports_what_jev_was_unsure_about() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
        assert!(impasse.operations.contains(&Operation::ScrollDown));
        assert!(
            format!("{}", impasse.because).contains("0.3"),
            "{}",
            impasse.because
        );
        Ok(Resolution::Stop)
    });

    pilot.pursue("Silence notifications").expect("completes");
}

/// Escalating never widens what may happen: the second opinion is resolved
/// through the same catalog, so an out-of-range row is refused rather than
/// becoming a tap somewhere arbitrary.
#[test]
fn an_escalation_cannot_name_a_row_that_is_not_on_screen() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> {
        Ok(Resolution::Choose {
            operation: Operation::Tap,
            target: Some(999),
        })
    });

    let ending = pilot.pursue("Silence notifications").expect("completes");

    assert!(matches!(ending, Ending::Uncertain { .. }), "got {ending:?}");
    assert!(pilot.device().performed.is_empty(), "nothing may be tapped");
    let _ = Act::Wait;
}

/// A second opinion needs to know what the first one was struggling with. "The
/// confidence was 0.44" says nothing; "it was torn between tapping Internet and
/// tapping SIMs, 0.44 to 0.41" says what to actually decide.
#[test]
fn the_impasse_carries_what_jev_was_torn_between() {
    let seen = std::rc::Rc::new(RefCell::new(None));
    let recorded = std::rc::Rc::clone(&seen);

    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![serde_json::json!({
            "operation": {
                "type": "choice", "choice": "tap", "confidence": 0.30,
                "probabilities": { "tap": 0.44, "scroll_down": 0.41, "back": 0.15 }
            },
            "tap_target": {
                "type": "choice", "choice": "A3", "confidence": 0.28,
                "probabilities": { "A3": 0.40, "A4": 0.38 }
            },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 },
        })])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(
        move |impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
            *recorded.borrow_mut() = Some((
                impasse.leaning,
                impasse.operation_confidence.get(),
                impasse.alternatives.to_vec(),
                impasse.target_confidence.map(Confidence::get),
            ));
            Ok(Resolution::Stop)
        },
    );

    pilot.pursue("Open Wi-Fi").expect("completes");

    let (leaning, confidence, alternatives, target) =
        seen.borrow().clone().expect("the escalation was consulted");
    assert_eq!(leaning, Operation::Tap, "what it was leaning toward");
    assert!((confidence - 0.30).abs() < 1e-9);
    assert_eq!(target, Some(0.28), "the row head was unsure too");
    assert!(
        alternatives
            .iter()
            .any(|(name, p)| &**name == "scroll_down" && (*p - 0.41).abs() < 1e-9),
        "the near miss must be visible: {alternatives:?}"
    );
}

/// A row that turns out to be covered is a reason to choose something else,
/// not a reason to abandon the run. Observed live: a tap resolved to an element
/// hidden behind a bar, and the whole run ended with an error mid-task.
#[test]
fn a_covered_row_is_an_impasse_rather_than_a_failure() {
    use jev_pilot::snapshot::{Bounds, Element};

    struct Covered;
    impl Device for Covered {
        type Error = Infallible;
        fn observe(&mut self) -> Result<Snapshot, Infallible> {
            let at = |t, b| Element {
                label: "row".into(),
                detail: None,
                editable: false,
                bounds: Bounds {
                    left: 0,
                    top: t,
                    right: 100,
                    bottom: b,
                },
            };
            // The first row is entirely beneath the second.
            Ok(Snapshot::new(vec![at(50, 60), at(0, 200)]).expect("a screen"))
        }
        fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
            Ok(())
        }
    }

    let turn = serde_json::json!({
        "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
        "tap_target": { "type": "choice", "choice": "A1", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
    });

    let mut pilot = Pilot::new(Covered, Scripted(RefCell::new(vec![turn])), &Android);

    let ending = pilot
        .pursue("Tap the hidden row")
        .expect("the run must not fail");

    assert!(matches!(ending, Ending::Uncertain { .. }), "got {ending:?}");
}

/// An impasse is where a person is asked to decide, and a person deciding gets
/// less than the model that could not: the rows, and nothing the screen says.
/// On a form whose Continue is switched off until a picker is used, the words
/// are the only account of why nothing looks tappable.
#[test]
fn an_impasse_carries_what_the_screen_says() {
    let said = RefCell::new(Vec::new());
    let mut pilot = Pilot::new(PinPad, Scripted(RefCell::new(vec![unsure(); 30])), &Android)
        .requiring(floor())
        .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
            said.borrow_mut()
                .extend(impasse.says.iter().map(|s| (*s).to_owned()));
            Ok(Resolution::Stop)
        });

    let _ = pilot.pursue("enter the PIN");

    let said = said.borrow();
    assert!(
        said.iter().any(|s| s.contains("4 of 6 digits entered")),
        "{said:?}",
    );
}

/// A screen of keys, and words that say how far through it is.
struct PinPad;

impl Device for PinPad {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        Ok(Android
            .parse_hierarchy(include_str!("fixtures/pin-pad.xml"))
            .expect("fixture parses"))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        Ok(())
    }
}

/// An impasse offers `type <n>`, and `n` counts the fields, not the rows. The
/// rows were listed and the fields were not, so there was nothing to count:
/// on a form of three editable rows among five, the number to give was
/// knowable only by working out which rows the catalog considered typeable.
#[test]
fn an_impasse_lists_the_fields_it_invites_a_number_for() {
    let said = RefCell::new(Vec::new());
    let mut pilot = Pilot::new(Typing, Scripted(RefCell::new(vec![unsure(); 30])), &Android)
        .writing_with(|_: &jev_pilot::pilot::Writing<'_>| -> Result<Box<str>, Infallible> {
            Ok("unused".into())
        })
        .requiring(floor())
        .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
            said.borrow_mut()
                .extend(impasse.fields.iter().cloned());
            Ok(Resolution::Stop)
        });

    let _ = pilot.pursue("fill the form in");

    let said = said.borrow();
    assert_eq!(said.len(), 1, "one editable row means one field: {said:?}");
    assert!(said[0].contains("Number to be loaded"), "{said:?}");
}

/// A form with one field among several rows.
struct Typing;

impl Device for Typing {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        Ok(Android
            .parse_hierarchy(include_str!("fixtures/flutter-form.xml"))
            .expect("fixture parses"))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        Ok(())
    }
}

/// An answer that names something this screen does not offer is a
/// misunderstanding, not a decision to stop. Ending the run on it throws away
/// the work done so far over a wrong guess about what was on screen — and the
/// person who guessed is right there, able to guess again.
///
/// Measured: a login screen answered with "type into the password field" at
/// the moment the app had swapped that field for "Logging in…", which ended
/// a run one step in.
#[test]
fn an_answer_that_cannot_be_carried_out_is_asked_again() {
    let asked = std::cell::Cell::new(0_u32);
    let mut pilot = Pilot::new(Fake::default(), Scripted(RefCell::new(vec![unsure(); 30])), &Android)
        .requiring(floor())
        .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> {
            asked.set(asked.get() + 1);
            Ok(if asked.get() == 1 {
                // A row this screen does not have.
                Resolution::Choose {
                    operation: Operation::Tap,
                    target: Some(9_999),
                }
            } else {
                Resolution::Stop
            })
        });

    let _ = pilot.pursue("Silence notifications");

    assert_eq!(
        asked.get(),
        2,
        "the first answer could not be carried out, so it asked once more",
    );
}
