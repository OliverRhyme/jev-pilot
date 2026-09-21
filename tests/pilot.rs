//! The loop: observe, judge, act, repeat.

use jev_pilot::act::{Direction, Indecision, Outcome};
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
///
/// Answered with `scroll_down` rather than `back`: the fake raises and lowers
/// its keyboard between steps, and with one up the way back is offered as
/// closing it instead, so a script naming `back` would be testing the
/// catalog rather than the step limit.
#[test]
fn the_loop_gives_up_after_its_step_limit() {
    let turns = vec![answer("scroll_down", None, 0.99, 0.02); 3];
    let mut pilot = Pilot::new(Fake::default(), Scripted::new(turns), &Android).limited_to(3);

    let ending = pilot
        .pursue("Something unreachable")
        .expect("the run completes");

    assert_eq!(ending, Ending::OutOfSteps { limit: 3 });
    assert_eq!(pilot.device().performed.len(), 3);
    assert!(matches!(
        pilot.device().performed[0],
        Command::Scroll(Direction::Down)
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

/// Naming the app puts the run in it before the first judgement, rather than
/// hoping a launcher offers a way in. Measured: the icon was in neither the
/// visible page nor the folder that looked right, and the run spent two
/// judgements discovering that a screen cannot be reasoned into containing
/// something it does not contain.
#[test]
fn naming_the_app_brings_it_to_the_front_before_anything_is_judged() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97)]),
    };
    let mut pilot = Pilot::new(Arrived::default(), judge, &Android)
        .about(Some("com.android.settings".into()));

    pilot.pursue("turn wifi on").expect("the run completes");

    assert_eq!(
        pilot.device().performed.first(),
        Some(&Command::Launch("com.android.settings".into())),
        "the named app is brought to the front first",
    );
    // An app takes seconds to start. Observing straight after the launch sees
    // the screen it was launched from, and the run spends its first judgement
    // deciding what to do about a launcher it has already left.
    assert_eq!(
        pilot.device().performed.get(1),
        Some(&Command::Settle),
        "the launch is given time to land before anything is read",
    );
    // Having been launched, the run is in it — not "away from" the launcher it
    // never belonged to.
    let seen = seen.borrow();
    assert!(seen[0].get("started_in").is_none(), "{}", seen[0]);
}

/// A soft keyboard covers the bottom of the screen, and what it covers is
/// usually the button that commits the form being typed into. The rows simply
/// are not there, so nothing in the catalog hints that closing it would reveal
/// one — and a run pressing submit at a form whose Continue is behind the
/// keyboard repeats itself until its own guard stops it. Measured on a
/// transfer form: keyboard up gave five rows and no commit; keyboard down gave
/// the same five and `Continue`.
#[test]
fn the_state_says_when_the_keyboard_is_covering_the_screen() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    // `Fake` raises the keyboard on every other observation.
    let mut pilot = Pilot::new(Fake::default(), judge, &Android);

    pilot.pursue("fill the form in").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(seen[0]["keyboard_open"], false);
    assert_eq!(seen[1]["keyboard_open"], true);
}

/// The words on a screen are most of what "is the goal met?" is a question
/// about, and none of them are rows. Without them the judge sees a set of
/// buttons and no account of the state they act on.
#[test]
fn the_state_carries_what_the_screen_says() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97)]),
    };
    let mut pilot = Pilot::new(PinPad, judge, &Android);

    pilot.pursue("enter the PIN").expect("the run completes");

    let seen = seen.borrow();
    let said = seen[0]["screen_says"].to_string();
    assert!(said.contains("4 of 6 digits entered"), "got {said}");
    assert!(said.contains("Step 3 of 3"), "got {said}");
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

/// A screen that has begun to change has not finished changing. A view being
/// built reports the rows it has so far, and acting on that half-built screen
/// is acting on a screen that will not exist a moment later.
///
/// Measured on a transfer flow: tapping the one live row landed on the next
/// step while only its Back button had rendered, so the only thing left to
/// choose was Back — which returned to the screen just left. The run
/// oscillated between the two until its budget ran out.
///
/// Settling therefore waits for two readings that agree, not for the first
/// reading that differs. That catches a screen still arriving; a screen that
/// holds a half-built state for longer than the settle will wait is caught
/// instead by noticing the run has been on it before — see
/// `a_screen_the_run_has_already_been_on_is_named_as_one`.
#[test]
fn a_screen_is_settled_when_it_stops_changing_not_when_it_starts() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(HalfBuilt::default(), judge, &Android);

    pilot.pursue("get to the next screen").expect("it completes");

    let seen = seen.borrow();
    // The second judgement must be about the finished screen, never the one
    // rendering into it.
    assert_eq!(
        seen[1]["rows"]["A2"], "Continue",
        "judged a half-built screen: {}",
        seen[1],
    );
}

/// Two screens can take turns forever without either one repeating an action
/// against an unchanged screen, so the guard against standing still never
/// fires. Measured: tapping the one live row reached a half-rendered screen
/// whose only row was Back, going back returned to the screen just left, and
/// the pair alternated until the budget ran out.
///
/// A run cannot tell "still loading" from "dead end" by looking. It can tell
/// that it has been here before, which is the fact that makes waiting the
/// obvious move rather than tapping again.
#[test]
fn a_screen_the_run_has_already_been_on_is_named_as_one() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("back", None, 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Oscillating::default(), judge, &Android);

    pilot.pursue("get somewhere").expect("it completes");

    let seen = seen.borrow();
    assert!(seen[0].get("seen_before").is_none(), "{}", seen[0]);
    assert!(seen[1].get("seen_before").is_none(), "{}", seen[1]);
    assert_eq!(
        seen[2]["seen_before"], 2,
        "the third screen is the first one again: {}",
        seen[2],
    );
}

/// A device that alternates between two screens, whatever is done to it.
#[derive(Default)]
struct Oscillating {
    acts: u32,
}

impl Device for Oscillating {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        Ok(if self.acts.is_multiple_of(2) {
            screen_of(&["Go Back", "Select a method"])
        } else {
            screen_of(&["Go Back"])
        })
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.acts += 1;
        Ok(())
    }
}

/// A device whose next screen arrives in two parts, as a real one does.
#[derive(Default)]
struct HalfBuilt {
    reads_after_acting: std::cell::Cell<u32>,
    acted: bool,
}

impl Device for HalfBuilt {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        if !self.acted {
            return Ok(screen_of(&["Go Back", "Select a method"]));
        }
        let read = self.reads_after_acting.get();
        self.reads_after_acting.set(read + 1);
        // The new screen has only its Back button for the first readings; the
        // rest of it arrives after that, as a view being built does.
        Ok(if read < 2 {
            screen_of(&["Go Back"])
        } else {
            screen_of(&["Go Back", "Continue"])
        })
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.acted = true;
        Ok(())
    }
}

fn screen_of(labels: &[&str]) -> Snapshot {
    use jev_pilot::snapshot::{Bounds, Element};

    Snapshot::new(
        labels
            .iter()
            .enumerate()
            .map(|(row, label)| Element {
                label: (*label).into(),
                detail: None,
                editable: false,
                bounds: Bounds::from_origin_size(0, 100 * i32::try_from(row).unwrap_or(0), 500, 80),
            })
            .collect(),
    )
    .expect("a screen")
}
