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
        "goal_met": { "type": "noul", "noul": 0.02 },
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
        Scripted(RefCell::new(vec![unsure()])),
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
                "goal_met": { "type": "noul", "noul": 0.99 },
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
        Scripted(RefCell::new(vec![unsure()])),
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
        Scripted(RefCell::new(vec![unsure()])),
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
        Scripted(RefCell::new(vec![unsure()])),
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
        Scripted(RefCell::new(vec![unsure()])),
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
