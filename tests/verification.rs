//! Checking that what was reached is what was asked for.

use jev_pilot::act::Outcome;
use jev_pilot::device::{Command, Device};
use jev_pilot::judgment::Criterion;
use jev_pilot::pilot::{Ending, Judge, Pilot};
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

/// Records the questions it was asked, so the checks can be seen on the wire.
struct Scripted {
    turns: RefCell<Vec<serde_json::Value>>,
    asked: RefCell<Vec<serde_json::Value>>,
}

impl Scripted {
    fn new(turns: Vec<serde_json::Value>) -> Self {
        Self {
            turns: RefCell::new(turns),
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl Judge for Scripted {
    type Error = Infallible;
    fn evaluate(
        &self,
        _state: serde_json::Value,
        questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Infallible> {
        self.asked
            .borrow_mut()
            .push(serde_json::to_value(questions).expect("serializes"));
        let next = self.turns.borrow_mut().remove(0);
        Ok(serde_json::from_value(next).expect("scripted answer parses"))
    }
}

/// `done` plus a satisfied `goal_met`, with one acceptance check failing.
fn verdict(check: f64) -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": "done", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 1.9, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
        "check_0": { "type": "noul", "noul": check },
    })
}

fn keep_going() -> serde_json::Value {
    serde_json::json!({
        "operation": { "type": "choice", "choice": "back", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
        "check_0": { "type": "noul", "noul": 0.02 },
    })
}

/// Each acceptance check becomes its own judgment in the same request, so it
/// costs no extra round trip. They are the difference between "something that
/// looks like the goal" and the goal.
#[test]
fn acceptance_checks_are_asked_alongside_everything_else() {
    let judge = Scripted::new(vec![verdict(0.97)]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android)
        .confirming(["The thing playing is a full video, not a Short"]);

    pilot.pursue("Play a video").expect("completes");

    let asked = &pilot.judge().asked.borrow()[0];
    assert_eq!(asked["check_0"]["type"], "noul", "{asked}");
    let rendered = asked["check_0"]["instructions"].to_string();
    assert!(rendered.contains("not a Short"), "{rendered}");
}

/// The case this exists for. A run reached something that satisfied a general
/// "is the goal met?" and failed the specific thing the goal asked for. Taking
/// the verdict at face value reports success for the wrong outcome.
#[test]
fn a_verdict_is_refused_when_an_acceptance_check_fails() {
    let judge = Scripted::new(vec![verdict(0.03), keep_going()]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android)
        .confirming(["The thing playing is a full video, not a Short"])
        .limited_to(2);

    let ending = pilot.pursue("Play a video about Jev").expect("completes");

    assert_eq!(
        ending,
        Ending::OutOfSteps { limit: 2 },
        "the run must carry on rather than declare success"
    );
}

/// With every check satisfied, the verdict stands.
#[test]
fn a_verdict_stands_when_the_checks_agree() {
    let judge = Scripted::new(vec![verdict(0.97)]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android)
        .confirming(["The thing playing is a full video, not a Short"]);

    assert_eq!(
        pilot.pursue("Play a video").expect("completes"),
        Ending::Finished(Outcome::Achieved)
    );
}

/// The next step is told which check failed, so it can do something about it
/// rather than choosing the same wrong thing again.
#[test]
fn the_failed_check_is_carried_into_the_next_step() {
    let judge = Scripted::new(vec![verdict(0.03), keep_going()]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android)
        .confirming(["The thing playing is a full video, not a Short"])
        .limited_to(2);

    pilot.pursue("Play a video").expect("completes");

    let second = &pilot.judge().asked.borrow()[1];
    let goal_met = second["goal_met"]["instructions"].to_string();
    assert!(
        goal_met.contains("not a Short") || goal_met.contains("rejected"),
        "the second step should know the first was refused: {goal_met}"
    );
}

/// A screen with nothing on it is not evidence that anything was achieved.
///
/// Mid-transition the hierarchy can come back empty. The state then carries no
/// rows, every question is answered from nothing, and a confident yes to "is
/// the goal met?" is a judgement about an empty room. Observed live: a run
/// declared success on a blank frame while the device was in fact on an
/// entirely different screen.
#[test]
fn a_verdict_on_an_empty_screen_is_not_accepted() {
    struct Blank;
    impl Device for Blank {
        type Error = Infallible;
        fn observe(&mut self) -> Result<Snapshot, Infallible> {
            Ok(Snapshot::new(Vec::new()).expect("an empty screen"))
        }
        fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
            Ok(())
        }
    }

    let confident = || {
        serde_json::json!({
            "operation": { "type": "choice", "choice": "done", "confidence": 0.99 },
            "goal_met": { "type": "score", "score": 1.9, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.01 },
            "check_0": { "type": "noul", "noul": 0.99 },
        })
    };

    let mut pilot = Pilot::new(
        Blank,
        Scripted::new(vec![confident(), confident()]),
        &Android,
    )
    .confirming(["The Storage screen is open"])
    .limited_to(2);

    assert_eq!(
        pilot.pursue("Open Storage").expect("completes"),
        Ending::OutOfSteps { limit: 2 },
        "nothing on screen cannot confirm anything"
    );
}

/// A criterion may say what a yes and a no look like for it specifically.
///
/// The generic pair ("the screen plainly shows this to be so" / "it is not so,
/// or cannot be told") is a fallback, not a good answer: the guidance is to put
/// the boundary cases in the criteria, and only the caller knows where the
/// boundary of their own claim lies.
#[test]
fn a_criterion_may_carry_its_own_boundary_cases() {
    let judge = Scripted::new(vec![verdict(0.97)]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android).confirming([Criterion::that(
        "The thing playing is a full-length video",
    )
    .where_true("A player is open on an item lasting minutes, with a scrubber")
    .where_false("A Short, a preview, a thumbnail, or a still results list")]);

    pilot.pursue("Play a video").expect("completes");

    let asked = &pilot.judge().asked.borrow()[0];
    let poles = &asked["check_0"]["criteria"];
    assert!(
        poles["true"].as_str().unwrap_or("").contains("scrubber"),
        "{poles}"
    );
    assert!(
        poles["false"].as_str().unwrap_or("").contains("Short"),
        "{poles}"
    );
}

/// A bare string still works, and gets the generic pair.
#[test]
fn a_plain_string_is_still_a_criterion() {
    let judge = Scripted::new(vec![verdict(0.97)]);
    let mut pilot = Pilot::new(Fake::default(), judge, &Android).confirming(["A video is playing"]);

    pilot.pursue("Play a video").expect("completes");

    let asked = &pilot.judge().asked.borrow()[0];
    assert_eq!(
        asked["check_0"]["instructions"]["criterion"],
        "A video is playing"
    );
    assert!(asked["check_0"]["criteria"]["true"].is_string());
}

/// An acceptance criterion is the caller's own definition of done, and a
/// `goal_met` score is the model's guess at it. The criteria could only ever
/// veto a success, never declare one — so a run standing on a finished screen
/// the model under-rated kept going, off the screen and back through the flow
/// it had just completed.
///
/// Measured driving a bank transfer: step 24 was the receipt, `goal_met` said
/// "under way", and the run pressed Back and began a second transfer of real
/// money. Nothing in the loop looked at the criteria that were being answered
/// on that very step.
#[test]
fn criteria_that_all_hold_end_the_run_whatever_the_progress_score_says() {
    let mut pilot = Pilot::new(
        Fake::default(),
        // Says "keep going, press back", while confirming the criterion.
        Scripted::new(vec![
            serde_json::json!({
                "operation": { "type": "choice", "choice": "back", "confidence": 0.99 },
                "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
                "is_error_screen": { "type": "noul", "noul": 0.01 },
                "check_0": { "type": "noul", "noul": 0.97 },
            });
            20
        ]),
        &Android,
    )
    .confirming(["A transfer receipt is showing"]);

    let ending = pilot.pursue("transfer fifty pesos").expect("it completes");

    assert_eq!(ending, Ending::Finished(Outcome::Achieved));
    assert!(
        pilot.device().performed.is_empty(),
        "nothing should have been done to a screen that already satisfies the goal: {:?}",
        pilot.device().performed,
    );
}

/// One criterion holding is not all of them holding. A run asked for two
/// things and shown one of them is not finished, and must go on until the
/// other holds too.
#[test]
fn a_run_is_not_finished_while_any_criterion_is_unconfirmed() {
    let half = serde_json::json!({
        "operation": { "type": "choice", "choice": "back", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
        "check_0": { "type": "noul", "noul": 0.97 },
        "check_1": { "type": "noul", "noul": 0.10 },
    });
    let both = serde_json::json!({
        "operation": { "type": "choice", "choice": "back", "confidence": 0.99 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
        "check_0": { "type": "noul", "noul": 0.97 },
        "check_1": { "type": "noul", "noul": 0.97 },
    });
    let mut pilot = Pilot::new(Fake::default(), Scripted::new(vec![half, both]), &Android)
        .confirming([
            "A transfer receipt is showing",
            "It carries a reference number",
        ]);

    let ending = pilot.pursue("transfer fifty pesos").expect("it completes");

    assert_eq!(ending, Ending::Finished(Outcome::Achieved));
    assert_eq!(
        pilot.judge().asked.borrow().len(),
        2,
        "the first step had a criterion unconfirmed and had to go on",
    );
}
