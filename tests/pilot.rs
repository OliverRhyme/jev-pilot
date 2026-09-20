//! The loop: observe, judge, act, repeat.

use jev_pilot::act::{Indecision, Outcome, SystemAct};
use jev_pilot::device::{Command, Device};
use jev_pilot::judgment::Confidence;
use jev_pilot::pilot::{Ending, Judge, Pilot};
use jev_pilot::platform::{Android, Platform};
use jev_pilot::snapshot::Snapshot;
use jev_pilot::step::{StepAnswers, StepQuestions};
use std::convert::Infallible;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

/// A device that always shows the same screen and records what it was told.
#[derive(Default)]
struct Fake {
    performed: Vec<Command>,
    /// Whether acting on this device changes what is on it.
    ///
    /// A device that never moves is the interesting case for one test and the
    /// wrong case for every other: the loop now notices when its actions
    /// achieve nothing, so a fake that never changes ends runs early.
    inert: bool,
}

impl Device for Fake {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let mut screen = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
        if !self.inert {
            // Something about it differs after each action, as a real screen
            // does; the keyboard is the cheapest thing to vary.
            screen = screen.with_keyboard_open(self.performed.len() % 2 == 1);
        }
        Ok(screen)
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
}

/// A judge that replies from a script, so the loop is tested without a model.
struct Scripted(std::cell::RefCell<Vec<serde_json::Value>>);

impl Scripted {
    fn new(turns: Vec<serde_json::Value>) -> Self {
        Self(std::cell::RefCell::new(turns))
    }
}

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

/// One scripted turn: an operation, optionally aimed at a row.
fn answer(
    operation: &str,
    target: Option<&str>,
    confidence: f64,
    goal_met: f64,
) -> serde_json::Value {
    let mut turn = serde_json::json!({
        "operation": { "type": "choice", "choice": operation, "confidence": confidence },
        "goal_met": { "type": "score", "score": goal_met, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 },
    });
    if let Some(row) = target {
        turn["tap_target"] =
            serde_json::json!({ "type": "choice", "choice": row, "confidence": confidence });
    }
    turn
}

/// A tap is carried out, then the next turn declares the goal reached.
#[test]
fn the_loop_acts_then_stops_when_the_goal_is_reached() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    );

    let ending = pilot.pursue("Turn on Wi-Fi").expect("the run completes");

    assert_eq!(ending, Ending::Finished(Outcome::Achieved));
    assert!(
        matches!(pilot.device().performed.as_slice(), [Command::Tap(_)]),
        "exactly one tap should have been carried out"
    );
}

/// A flat distribution must stop the run rather than pick the nominal winner.
#[test]
fn the_loop_stops_rather_than_acting_on_an_uncertain_answer() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![answer("tap", Some("A8"), 0.51, 0.02)]),
        &Android,
    )
    .requiring(Confidence::new(0.6).expect("valid floor"));

    let ending = pilot
        .pursue("Silence notifications")
        .expect("the run completes");

    assert!(matches!(ending, Ending::Uncertain { .. }), "got {ending:?}");
    assert!(
        pilot.device().performed.is_empty(),
        "nothing may be touched when the answer was not clear enough"
    );
}

/// A goal that is never reached must end, not spin.
#[test]
fn the_loop_gives_up_after_its_step_limit() {
    let turns = vec![answer("back", None, 0.99, 0.02); 3];
    let mut pilot = Pilot::new(Fake::default(), Scripted::new(turns), &Android).limited_to(3);

    let ending = pilot
        .pursue("Something unreachable")
        .expect("the run completes");

    assert_eq!(ending, Ending::OutOfSteps { limit: 3 });
    assert_eq!(pilot.device().performed.len(), 3);
    assert!(matches!(
        pilot.device().performed[0],
        Command::System(SystemAct::Back)
    ));
}

/// Confidence says nothing about effect. A run can choose the right-looking
/// row at 0.99 and achieve nothing, then choose it again, because the screen
/// it is judging is the one it already acted on — which on a real form meant
/// twelve identical taps, each a live request to a banking API.
///
/// A device that never moves is exactly that case, and the run stops asking
/// rather than spending its whole budget on it.
#[test]
fn a_run_stops_repeating_an_action_that_changes_nothing() {
    let turns = vec![answer("back", None, 0.99, 0.02); 12];
    let mut pilot = Pilot::new(
        Fake {
            inert: true,
            ..Fake::default()
        },
        Scripted::new(turns),
        &Android,
    )
    .limited_to(12);

    let ending = pilot
        .pursue("Something that cannot be reached by going back")
        .expect("the run completes");

    assert!(
        matches!(
            ending,
            Ending::Uncertain {
                because: Indecision::NoProgress { .. }
            }
        ),
        "got {ending:?}"
    );
    assert!(
        pilot.device().performed.len() < 12,
        "it must stop short of the budget, did {} of 12",
        pilot.device().performed.len()
    );
}

/// A step is shown what the previous one did. Without it the model judging
/// "is the goal met?" cannot tell a screen it just arrived at from one it has
/// been stuck on, and cannot tell whether its last action had any effect.
#[test]
fn each_step_is_told_what_the_previous_step_did() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };

    let mut pilot = Pilot::new(Fake::default(), judge, &Android);
    pilot.pursue("Turn on Wi-Fi").expect("the run completes");

    let states = seen.borrow();
    assert_eq!(states[0]["previous_action"], serde_json::Value::Null);
    let second = states[1]["previous_action"].to_string();
    assert!(second.contains("Tap"), "got {second}");
    assert!(second.contains("Network and Internet"), "got {second}");
}

/// A judge that records the state it was shown.
struct Recording {
    seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>>,
    turns: std::cell::RefCell<Vec<serde_json::Value>>,
}

impl Judge for Recording {
    type Error = Infallible;
    fn evaluate(
        &self,
        state: serde_json::Value,
        _questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Infallible> {
        self.seen.borrow_mut().push(state);
        let next = self.turns.borrow_mut().remove(0);
        Ok(serde_json::from_value(next).expect("scripted answer parses"))
    }
}

/// `wait` is offered with the rubric "wait for the screen to finish loading",
/// so it must actually cost time. If it resolves to nothing, a loop facing a
/// spinner re-observes the same unchanged screen immediately, picks wait again,
/// and burns its whole step budget — and that many paid judgments — in under a
/// second.
#[test]
fn waiting_reaches_the_device_instead_of_spinning_the_loop() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![
            answer("wait", None, 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    );

    pilot
        .pursue("Let the screen load")
        .expect("the run completes");

    assert_eq!(
        pilot.device().performed.as_slice(),
        &[Command::Settle],
        "waiting must be carried out, not skipped"
    );
}

/// The step log exists so an uncertain run is diagnosable. Reporting "nothing
/// was chosen" for a step whose escalation then drove the device makes the log
/// contradict what actually happened.
#[test]
fn a_step_resolved_by_escalation_is_reported_with_the_act_it_performed() {
    let reports = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = std::rc::Rc::clone(&reports);

    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![
            answer("tap", Some("A3"), 0.10, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    )
    .requiring(jev_pilot::judgment::Confidence::new(0.6).expect("valid floor"))
    .escalating_to(|_: &jev_pilot::pilot::Impasse<'_>| {
        Ok::<_, Infallible>(jev_pilot::pilot::Resolution::Choose {
            operation: jev_pilot::act::Operation::Tap,
            target: Some(0),
        })
    })
    .watching(move |step| seen.borrow_mut().push(step.chosen.cloned()));

    pilot
        .pursue("Something ambiguous")
        .expect("the run completes");

    let first = reports.borrow()[0].clone();
    assert!(
        matches!(first, Some(jev_pilot::act::Act::Tap(_))),
        "the escalated act should appear in the log, got {first:?}"
    );
}

/// Choosing to stop must stop, on its own. The goal-met guard can also end a
/// run, and when both fire together it hides a missing check: a verdict that
/// resolves to a command the device cannot carry out becomes a no-op, and the
/// loop keeps driving a screen it has already declared finished.
#[test]
fn a_verdict_ends_the_run_even_when_the_guard_does_not_agree() {
    let mut pilot = Pilot::new(
        Fake::default(),
        // goal_met stays low: only the chosen operation says to stop.
        Scripted::new(vec![answer("done", None, 0.99, 0.05)]),
        &Android,
    );

    let ending = pilot
        .pursue("Something already satisfied")
        .expect("completes");

    assert_eq!(ending, Ending::Finished(Outcome::Achieved));
    assert!(
        pilot.device().performed.is_empty(),
        "a verdict touches nothing"
    );
}

/// The same for giving up.
#[test]
fn declaring_the_goal_unreachable_ends_the_run() {
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![answer("blocked", None, 0.99, 0.05)]),
        &Android,
    );

    assert_eq!(
        pilot.pursue("Something impossible").expect("completes"),
        Ending::Finished(Outcome::Blocked)
    );
}

const LAUNCHER: &str = include_str!("fixtures/helper-home.xml");

/// A device that starts in Settings and is in the launcher from then on, as a
/// run that pressed Home is.
#[derive(Default)]
struct Wandered {
    performed: Vec<Command>,
}

impl Device for Wandered {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let raw = if self.performed.is_empty() {
            SETTINGS
        } else {
            LAUNCHER
        };
        Ok(Android.parse_hierarchy(raw).expect("fixture parses"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
}

/// Pressing Home, following a notification or being bounced into a browser all
/// leave the run somewhere its goal does not apply. The state has to say so:
/// without it the launcher is just another list of rows, and every icon on it
/// reads as plausible as the right one.
#[test]
fn a_run_that_has_left_its_app_is_told_so_and_offered_the_way_back() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("home", None, 0.99, 0.02),
            answer("return_to_app", None, 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Wandered::default(), judge, &Android);

    pilot.pursue("open wifi settings").expect("the run completes");

    let seen = seen.borrow();
    let first = &seen[0];
    assert_eq!(first["app"], "com.android.settings");
    assert!(
        first.get("started_in").is_none(),
        "a run that has not left has nothing to say about where it started: {first}",
    );

    let second = &seen[1];
    assert_eq!(second["started_in"], "com.android.settings");
    assert_eq!(second["app"], "com.google.android.apps.nexuslauncher");

    assert_eq!(
        pilot.device().performed.last(),
        Some(&Command::Launch("com.android.settings".into())),
    );
}

/// A run launched from the home screen is not "in" the launcher — the launcher
/// is how you reach an app, never the app a goal is about. Pinning it would
/// make entering the right app read as leaving, and offer going back out to
/// the icons as a way to make progress.
#[test]
fn the_home_launcher_is_never_the_app_a_goal_is_about() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    // Starts on the launcher, and is in Settings from the first tap onward.
    let mut pilot = Pilot::new(Arrived::default(), judge, &Android);

    pilot.pursue("open wifi settings").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(seen[1]["app"], "com.android.settings");
    assert!(
        seen[1].get("started_in").is_none(),
        "reaching the app is not leaving it: {}",
        seen[1],
    );
}

/// A device on the launcher until something is tapped, and in Settings after.
#[derive(Default)]
struct Arrived {
    performed: Vec<Command>,
}

impl Device for Arrived {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let raw = if self.performed.is_empty() {
            LAUNCHER
        } else {
            SETTINGS
        };
        Ok(Android.parse_hierarchy(raw).expect("fixture parses"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
    fn home_screen_app(&mut self) -> Option<Box<str>> {
        Some("com.google.android.apps.nexuslauncher".into())
    }
}
