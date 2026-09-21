//! Telling "wrong screen" from "on the way there".

use jev_pilot::act::{Catalog, Outcome};
use jev_pilot::device::{Command, Device};
use jev_pilot::judgment::Progress;
use jev_pilot::pilot::{Ending, Judge, Pilot, StepReport};
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

fn turn(operation: &str, score: f64) -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": operation, "confidence": 0.99 },
        "goal_met": {
            "type": "score", "score": score,
            "legend": { "0": "Not started", "1": "Under way", "2": "Achieved" },
            "probabilities": { "0": 0.1, "1": 0.2, "2": 0.7 },
            "confidence": 0.9
        },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
    })
}

/// "Is the goal met?" as a yes/no forces three situations into two answers: the
/// screen has nothing to do with the goal, the screen is a step along the way,
/// and the goal is done. The first two both come back near zero, and the loop
/// cannot tell a wrong turn from progress.
#[test]
fn progress_distinguishes_a_wrong_screen_from_one_on_the_way() {
    assert_eq!(Progress::from_score(0.1), Progress::NotStarted);
    assert_eq!(Progress::from_score(1.0), Progress::UnderWay);
    assert_eq!(Progress::from_score(1.9), Progress::Achieved);
    // The value is probability-weighted, so it lands between levels.
    assert_eq!(Progress::from_score(0.6), Progress::NotStarted);
    assert_eq!(Progress::from_score(1.6), Progress::Achieved);
}

/// The top level ends the run, as the old yes/no did.
#[test]
fn reaching_the_top_level_finishes_the_run() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted(RefCell::new(vec![turn("tap", 1.9)])),
        &Android,
    );

    assert_eq!(
        pilot.pursue("Open Storage").expect("completes"),
        Ending::Finished(Outcome::Achieved)
    );
}

/// Being on the way is not being there, and must not end the run.
#[test]
fn being_under_way_is_not_being_finished() {
    let turns = vec![turn("back", 1.0), turn("back", 1.0)];
    let mut pilot =
        Pilot::new(Fake::default(), Scripted(RefCell::new(turns)), &Android).limited_to(2);

    assert_eq!(
        pilot.pursue("Open Storage").expect("completes"),
        Ending::OutOfSteps { limit: 2 }
    );
}

/// The question reaches the wire as a Score with its levels in order, lowest
/// first, which is what makes the returned number mean anything.
#[test]
fn the_progress_question_is_a_score_with_ordered_levels() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire =
        serde_json::to_value(StepQuestions::new("Open Storage", &catalog)).expect("serializes");

    assert_eq!(wire["goal_met"]["type"], "score");
    let levels = wire["goal_met"]["criteria"]
        .as_array()
        .expect("ordered levels");
    assert_eq!(levels.len(), 3, "{levels:?}");
    assert!(
        levels[0].to_string().to_lowercase().contains("nothing"),
        "{levels:?}"
    );
}

/// A run that ends at step one leaves no account of what it was looking at,
/// because only an impasse prints the screen. Then the ending is all there is
/// — "Blocked" with no way to ask blocked by what — and the next move is to
/// guess. A report carries the screen so a finished run can be explained.
#[test]
fn a_step_is_reported_with_what_the_screen_said_and_which_app_it_was() {
    let seen = std::cell::RefCell::new(Vec::new());
    {
        let mut pilot = Pilot::new(PinPad, Scripted(RefCell::new(vec![turn("done", 1.9)])), &Android)
            .watching(|report: &StepReport<'_>| {
                seen.borrow_mut()
                    .push((report.app.map(str::to_owned), report.says.clone()));
            });
        pilot.pursue("enter the PIN").expect("the run completes");
    }

    let seen = seen.borrow();
    let (app, says) = &seen[0];
    assert_eq!(app.as_deref(), Some("com.example.wallet"));
    assert!(
        says.iter().any(|s| s.contains("4 of 6 digits entered")),
        "{says:?}",
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

/// A run that is slow gives no account of where the time went. The steps are
/// there and the actions are there, and nothing says whether a step spent its
/// seconds reading the screen, waiting on a judgement, or settling after the
/// act — so "it is slow" can only be answered by guessing or by measuring the
/// whole thing again from outside.
#[test]
fn a_step_is_reported_with_how_long_its_parts_took() {
    let seen = std::cell::RefCell::new(Vec::new());
    {
        let mut pilot = Pilot::new(
            PinPad,
            Scripted(RefCell::new(vec![turn("done", 1.9)])),
            &Android,
        )
        .watching(|report: &StepReport<'_>| {
            seen.borrow_mut().push((report.read_ms, report.step_ms));
        });
        pilot.pursue("enter the PIN").expect("the run completes");
    }

    let seen = seen.borrow();
    let (read, step) = seen[0];
    assert!(
        read <= step,
        "reading the screen is part of the step, not longer than it: {read} > {step}",
    );
}

/// A step that stops to ask spends most of its wall time waiting for a person
/// to type, and counting that as the step's cost makes the timings useless for
/// the one purpose they have. Measured: a step of 26 seconds, 90 milliseconds
/// of which was the machine.
#[test]
fn time_spent_waiting_for_an_answer_is_reported_apart_from_the_work() {
    let seen = std::cell::RefCell::new(Vec::new());
    {
        let mut pilot = Pilot::new(
            PinPad,
            Scripted(RefCell::new(vec![turn("tap", 0.2), turn("done", 1.9)])),
            &Android,
        )
        .writing_with(|_: &jev_pilot::pilot::Writing<'_>| -> Result<Box<str>, Infallible> {
            Ok("unused".into())
        })
        .escalating_to(
            |_: &jev_pilot::pilot::Impasse<'_>| -> Result<jev_pilot::pilot::Resolution, Infallible> {
                std::thread::sleep(std::time::Duration::from_millis(400));
                Ok(jev_pilot::pilot::Resolution::Stop)
            },
        )
        .requiring(jev_pilot::judgment::Confidence::new(0.99).expect("a floor"))
        .watching(|report: &StepReport<'_>| {
            seen.borrow_mut().push((report.waited_ms, report.step_ms));
        });
        let _ = pilot.pursue("enter the PIN");
    }

    let seen = seen.borrow();
    let (waited, step) = seen[0];
    assert!(waited >= 400, "the wait is counted: {waited}ms");
    assert!(
        step.saturating_sub(waited) < 400,
        "and the work is what is left: {step}ms total, {waited}ms of it waiting",
    );
}
