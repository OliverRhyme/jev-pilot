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
        .writing_with(
            |_: &jev_pilot::pilot::Writing<'_>| -> Result<Box<str>, Infallible> {
                Ok("unused".into())
            },
        )
        .requiring(floor())
        .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
            said.borrow_mut().extend(impasse.fields.iter().cloned());
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
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![unsure(); 30])),
        &Android,
    )
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

/// A results page read whole: both rows are in the hierarchy from the start,
/// but the second lies under the toolbar until the page is scrolled. Scrolling
/// moves the rows and changes none of their words.
#[derive(Default)]
struct BelowTheFold {
    scrolled: bool,
    performed: Vec<Command>,
}

impl Device for BelowTheFold {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        use jev_pilot::snapshot::{Bounds, Element};

        let row = |label: &str, top: i32| Element {
            label: label.into(),
            detail: None,
            editable: false,
            bounds: Bounds {
                left: 0,
                top,
                right: 100,
                bottom: top + 50,
            },
        };
        let shift = if self.scrolled { 200 } else { 0 };
        Ok(Snapshot::new(vec![
            row("Result A", 100 - shift),
            row("Result B", 250 - shift),
            // Drawn last, so it covers whatever lies beneath it.
            Element {
                label: "Toolbar".into(),
                detail: None,
                editable: false,
                bounds: Bounds {
                    left: 0,
                    top: 200,
                    right: 100,
                    bottom: 400,
                },
            },
        ])
        .expect("a screen"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        if matches!(command, Command::Scroll(_)) {
            self.scrolled = true;
        }
        self.performed.push(command.clone());
        Ok(())
    }
}

/// A judge that records the state it was shown.
struct Recording {
    seen: std::rc::Rc<RefCell<Vec<serde_json::Value>>>,
    turns: RefCell<Vec<serde_json::Value>>,
}

impl Judge for Recording {
    type Error = Infallible;
    fn evaluate(
        &self,
        state: serde_json::Value,
        _questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Infallible> {
        self.seen.borrow_mut().push(state);
        Ok(serde_json::from_value(self.turns.borrow_mut().remove(0))
            .expect("scripted answer parses"))
    }
}

fn sure_tap(row: &str) -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": "tap", "confidence": 0.99 },
        "tap_target": { "type": "choice", "choice": row, "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
    })
}

/// When the row Jev chose is covered and the second opinion answers with
/// something else, the something else is what happened. Measured on a Google
/// results page: the history said "Tapped mrjev.com" after a scroll, and the
/// run then treated its first real tap on that row as a repeat, waited instead
/// of tapping twice, and gave up.
#[test]
fn an_answer_to_a_covered_row_is_what_the_run_remembers_doing() {
    let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
    let reports = std::rc::Rc::new(RefCell::new(Vec::new()));
    let reported = std::rc::Rc::clone(&reports);
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: RefCell::new(vec![sure_tap("A2"), sure_tap("A2"), sure_tap("A2")]),
    };
    let mut pilot = Pilot::new(BelowTheFold::default(), judge, &Android)
        .limited_to(2)
        .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> {
            Ok(Resolution::Choose {
                operation: Operation::ScrollDown,
                target: None,
            })
        })
        .watching(move |step| reported.borrow_mut().push(step.chosen.cloned()));

    let _ = pilot.pursue("Open result B");

    let first = reports.borrow()[0].clone();
    assert!(
        matches!(first, Some(Act::Scroll(_))),
        "the step is reported with the scroll it performed, got {first:?}"
    );
    let told = seen.borrow()[1]["previous_action"].to_string();
    assert!(
        told.contains("Scroll"),
        "the next step is told of the scroll, got {told}"
    );
    assert!(
        !told.contains("Result B"),
        "nothing tapped Result B yet, got {told}"
    );
    assert!(
        matches!(
            pilot.device().performed.as_slice(),
            [Command::Scroll(_), Command::Tap(_)]
        ),
        "the first real tap on B is carried out, not waited out as a repeat: {:?}",
        pilot.device().performed
    );
}

/// A row below the fold is a scroll away, not a question. Measured on a
/// Google results page: Jev chose the right result on its own, the tap was
/// refused because the result lay under the toolbar, and the run stopped to ask
/// a person for what could only be "scroll down".
#[test]
fn a_covered_row_is_scrolled_towards_rather_than_asked_about() {
    let mut pilot = Pilot::new(
        BelowTheFold::default(),
        Scripted(RefCell::new(vec![
            sure_tap("A2"),
            sure_tap("A2"),
            sure_tap("A2"),
        ])),
        &Android,
    )
    .limited_to(2)
    .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> {
        panic!("nothing needed asking")
    });

    let _ = pilot.pursue("Open result B");

    assert!(
        matches!(
            pilot.device().performed.as_slice(),
            [
                Command::Scroll(jev_pilot::act::Direction::Down),
                Command::Tap(_)
            ]
        ),
        "{:?}",
        pilot.device().performed
    );
}

/// A row under a dialog does not come out from under it by scrolling. One
/// scroll that changes nothing is enough to know, and then it is a question.
#[test]
fn a_covered_row_a_scroll_does_not_move_is_asked_about() {
    struct Pinned;
    impl Device for Pinned {
        type Error = Infallible;
        fn observe(&mut self) -> Result<Snapshot, Infallible> {
            BelowTheFold::default().observe()
        }
        fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
            Ok(())
        }
    }
    let asked = std::cell::Cell::new(false);
    let mut pilot = Pilot::new(
        Pinned,
        Scripted(RefCell::new(vec![sure_tap("A2"); 3])),
        &Android,
    )
    .limited_to(3)
    .escalating_to(|_: &Impasse<'_>| -> Result<Resolution, Infallible> {
        asked.set(true);
        Ok(Resolution::Stop)
    });

    let _ = pilot.pursue("Open result B");

    assert!(
        asked.get(),
        "after a scroll that moved nothing, the run asks"
    );
}

/// Whoever answers an impasse picks a row by number, and a covered row is
/// refused however right it is. Measured: asked which result to open on a
/// Google results page, the answer named the right one, which lay under the
/// toolbar, and was refused — the question had not said which rows it could
/// reach.
#[test]
fn an_impasse_says_which_rows_are_covered() {
    let covered = RefCell::new(Vec::new());
    let mut pilot = Pilot::new(
        BelowTheFold::default(),
        Scripted(RefCell::new(vec![unsure()])),
        &Android,
    )
    .requiring(floor())
    .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
        covered.borrow_mut().extend_from_slice(impasse.covered);
        Ok(Resolution::Stop)
    });

    let _ = pilot.pursue("Open result B");

    assert_eq!(
        covered.borrow().as_slice(),
        &[1],
        "Result B, and only it, is under the toolbar"
    );
}

fn screening(commits: f64, warns: f64, operation: &str, confidence: f64) -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": operation, "confidence": confidence },
        "tap_target": { "type": "choice", "choice": "A2", "confidence": confidence },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
        "commits": { "type": "noul", "noul": commits },
        "warns": { "type": "noul", "noul": warns },
    })
}

/// Going on past a warning is a person's decision, however sure the model is.
/// Measured on a transfer: "It seems you've made a similar transaction — do you
/// want to proceed?", and the run tapped Proceed at 0.94 without asking.
#[test]
fn a_screen_warning_the_user_off_is_asked_about_rather_than_acted_on() {
    let asked = std::cell::Cell::new(false);
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![screening(0.73, 0.89, "tap", 0.99)])),
        &Android,
    )
    .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
        asked.set(true);
        assert!(
            impasse.because.to_string().contains("warning"),
            "{}",
            impasse.because
        );
        Ok(Resolution::Stop)
    });

    let _ = pilot.pursue("Send fifty pesos");

    assert!(asked.get(), "the warning went to a person");
    assert!(
        pilot.device().performed.is_empty(),
        "{:?}",
        pilot.device().performed
    );
}

/// Backing away from a warning needs nobody's leave.
#[test]
fn backing_away_from_a_warning_is_not_asked_about() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![
            screening(0.73, 0.89, "back", 0.99),
            screening(0.0, 0.0, "done", 0.99),
        ])),
        &Android,
    )
    .limited_to(2);

    let _ = pilot.pursue("Cancel the transfer");

    assert_eq!(
        pilot.device().performed.first(),
        Some(&Command::System(jev_pilot::act::SystemAct::Back))
    );
}

/// A tap on the screen that commits the money is held to the floor for
/// actions that cannot be undone, not the one for ordinary gestures.
#[test]
fn a_tap_on_a_screen_that_commits_is_held_to_the_higher_floor() {
    use jev_pilot::act::{Consequence, Floors};
    let low = Confidence::new(0.4).expect("valid");
    let high = Confidence::new(0.6).expect("valid");
    let floors = Floors::new(low).requiring_for(Consequence::Destructive, high);

    let mut committing = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![screening(0.91, 0.05, "tap", 0.5)])),
        &Android,
    )
    .with_floors(floors);
    let ending = committing
        .pursue("Send fifty pesos")
        .expect("the run completes");
    assert!(matches!(ending, Ending::Uncertain { .. }), "{ending:?}");
    assert!(committing.device().performed.is_empty());

    let mut ordinary = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![
            screening(0.05, 0.05, "tap", 0.5),
            screening(0.0, 0.0, "done", 0.99),
        ])),
        &Android,
    )
    .with_floors(floors)
    .limited_to(2);
    let _ = ordinary.pursue("Open the account");
    assert!(matches!(
        ordinary.device().performed.first(),
        Some(Command::Tap(_))
    ));
}

/// A run that is getting nowhere asks before it gives up. Measured: a run
/// tapped a line of Google's AI Overview that is not a link, chose it again at
/// 0.93 on the unchanged screen, and ended for going in circles, when a second
/// opinion could have picked a real result.
#[test]
fn a_run_getting_nowhere_asks_before_it_ends() {
    let asked = RefCell::new(Vec::new());
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![sure_tap("A3"); 12])),
        &Android,
    )
    .limited_to(10)
    .escalating_to(|impasse: &Impasse<'_>| -> Result<Resolution, Infallible> {
        asked.borrow_mut().push(impasse.because.to_string());
        Ok(Resolution::Stop)
    });

    let ending = pilot.pursue("Open the result").expect("the run completes");

    assert!(
        asked
            .borrow()
            .iter()
            .any(|why| why.contains("screen exactly as it was")),
        "asked: {:?}",
        asked.borrow()
    );
    assert!(matches!(ending, Ending::Uncertain { .. }), "{ending:?}");
}
