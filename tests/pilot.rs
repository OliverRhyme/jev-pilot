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

    pilot
        .pursue("open wifi settings")
        .expect("the run completes");

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

    pilot
        .pursue("open wifi settings")
        .expect("the run completes");

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
    let mut pilot =
        Pilot::new(Arrived::default(), judge, &Android).about(Some("com.android.settings".into()));

    pilot.pursue("turn wifi on").expect("the run completes");

    assert_eq!(
        pilot.device().performed.first(),
        Some(&Command::Launch("com.android.settings".into())),
        "the named app is brought to the front first",
    );
    // Nothing else is done to the device on the way in. An app takes seconds
    // to start, and it is waited for by looking rather than by a fixed pause
    // — see `a_launched_app_is_waited_for_until_it_has_drawn_something`.
    assert_eq!(
        pilot.device().performed.len(),
        1,
        "launching is the only thing done before the first judgement: {:?}",
        pilot.device().performed,
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

    pilot
        .pursue("get to the next screen")
        .expect("it completes");

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
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        // Waiting is not one of the two screens taking turns, and the loop
        // waits of its own accord.
        if !matches!(command, Command::Settle) {
            self.acts += 1;
        }
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

/// A form that is still validating swallows the tap on its commit button, and
/// the screen changes anyway — the amount is reformatted, the payee name comes
/// back from the server — so the guard against standing still never fires and
/// the run taps the button again, and again.
///
/// Measured on a transfer form: `Continue` tapped on three consecutive steps,
/// all of them saying "Step 1 of 3", the first two swallowed while the payee
/// resolved. On a rail that honoured them instead, that is three submissions
/// of the same money.
///
/// A run is told when it is about to repeat itself, so it can choose to wait.
#[test]
fn a_run_is_told_when_it_keeps_doing_the_same_thing() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Validating::default(), judge, &Android);

    pilot
        .pursue("continue past the form")
        .expect("it completes");

    let seen = seen.borrow();
    // Doing a thing once, then twice, is not yet a pattern.
    assert!(seen[0].get("repeating").is_none(), "{}", seen[0]);
    assert!(seen[1].get("repeating").is_none(), "{}", seen[1]);
    assert_eq!(
        seen[2]["repeating"], 2,
        "the same tap has already been made twice: {}",
        seen[2],
    );
}

/// A device whose screen changes after every action without the action having
/// taken effect, as a form reformatting its own fields does.
#[derive(Default)]
struct Validating {
    acts: usize,
}

impl Device for Validating {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        // The amount is rewritten each time, so no two readings are alike and
        // nothing looks like standing still.
        let amount = format!("{}.0{}", 50, self.acts);
        Ok(screen_of(&["Go Back", &amount, "Continue"]))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.acts += 1;
        Ok(())
    }
}

/// A form whose commit button is greyed out shows a screen with no way on.
/// The button is absent from the rows rather than offered and refused, so
/// without being told, the judge sees a dead end and starts looking for a way
/// out of it — which is how a run walks off a form it had nearly finished.
#[test]
fn the_state_names_the_controls_the_screen_will_not_let_it_use() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97)]),
    };
    let mut pilot = Pilot::new(HalfFilled, judge, &Android);

    pilot.pursue("finish the form").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(
        seen[0]["unavailable"],
        serde_json::json!(["Continue"]),
        "{}",
        seen[0],
    );
}

/// A transfer form with its Continue greyed out until a source account is
/// chosen.
struct HalfFilled;

impl Device for HalfFilled {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        Ok(Android
            .parse_hierarchy(include_str!("fixtures/disabled-continue.xml"))
            .expect("fixture parses"))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        Ok(())
    }
}

/// A PIN is entered one key at a time from a pad of identical-looking rows,
/// and knowing which key comes next means knowing how many have gone in. With
/// one step of memory that position has to be re-derived from scratch every
/// step, and the derivation gets longer as the sequence goes on.
///
/// Measured entering a six-digit PIN: the operation stayed at 0.92-0.98 — it
/// always knew to tap — while the row fell 0.95, 0.75, 0.70, 0.55, 0.31,
/// 0.14, until the run stopped to ask which key. It was never unsure whether
/// to tap; it was unsure which digit it was on.
#[test]
fn a_step_is_told_the_run_of_recent_actions_not_only_the_last() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A1"), 0.99, 0.02),
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Keypad::default(), judge, &Android);

    pilot.pursue("enter the code").expect("the run completes");

    let seen = seen.borrow();
    assert!(seen[0].get("recent_actions").is_none(), "{}", seen[0]);
    assert_eq!(
        seen[3]["recent_actions"],
        serde_json::json!(["Tapped one", "Tapped two", "Tapped three"]),
        "oldest first, so the sequence reads as one: {}",
        seen[3],
    );
}

/// Three keys whose labels never change, and a counter that does — as a PIN
/// pad reporting how many digits are in.
#[derive(Default)]
struct Keypad {
    pressed: usize,
}

impl Device for Keypad {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let counted = format!("{} of 3 entered", self.pressed);
        Ok(screen_of(&["one", "two", "three", &counted]))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.pressed += 1;
        Ok(())
    }
}

/// A screen that changes and changes back has not changed. Settling reported
/// movement as soon as any reading differed from the screen acted on, so a
/// transition that bounced — the next screen appearing and the app returning
/// to the form behind it — reset the guard against standing still, and the
/// button was pressed again on a screen identical to the one it was pressed
/// on.
///
/// Measured on a transfer form: two consecutive steps with the same rows and
/// the same words, both tapping Continue, with the summary appearing and
/// withdrawing in between.
#[test]
fn a_screen_that_changes_and_changes_back_has_not_moved() {
    let judge = Scripted::new(vec![answer("tap", Some("A2"), 0.99, 0.02); 30]);
    let mut pilot = Pilot::new(Bouncing::default(), judge, &Android);

    let ending = pilot
        .pursue("get past the form")
        .expect("the run completes");

    // Caught as a round trip rather than as a screen that will not move:
    // each screen does change, into the other one, so only the count of
    // times the run has been on each of them gives it away.
    assert_eq!(
        ending,
        Ending::Uncertain {
            because: Indecision::NoProgress {
                repeated: Pilot::<Bouncing, Scripted>::VISITS_ALLOWED
            }
        },
        "two screens taking turns is going in circles",
    );
}

/// A device that flashes a second screen after each action and returns to the
/// first, as a form does when it rejects what it was given.
///
/// Counted from the last action rather than from the start, so that how often
/// the settle happens to poll cannot change what it sees.
#[derive(Default)]
struct Bouncing {
    since_acting: u32,
}

impl Device for Bouncing {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        self.since_acting += 1;
        // The first reading after acting catches the screen it moved to.
        // Every reading after that is the one it came back to.
        Ok(if self.since_acting == 1 {
            screen_of(&["Go Back", "Summary"])
        } else {
            screen_of(&["Go Back", "Continue"])
        })
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.since_acting = 0;
        Ok(())
    }
}

/// An app that has been launched is not on screen yet, and the fixed pause
/// after launching either ends too early or wastes what it does not need.
/// Measured: the first step of a run read an empty screen for 983ms, then
/// spent a whole judgement — a paid one — deciding to wait.
///
/// Waiting for the app to draw something is not a judgement. It is waiting.
#[test]
fn a_launched_app_is_waited_for_until_it_has_drawn_something() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97); 4]),
    };
    let mut pilot =
        Pilot::new(SlowToDraw::default(), judge, &Android).about(Some("com.example.wallet".into()));

    pilot.pursue("do the thing").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(seen.len(), 1, "no judgement is spent on an empty screen");
    assert_eq!(
        seen[0]["app"], "com.example.wallet",
        "the first judgement is about the app, not the launcher it came from: {}",
        seen[0],
    );
    assert!(
        !seen[0]["rows"].as_object().expect("rows").is_empty(),
        "and it has something on it: {}",
        seen[0],
    );
}

/// The launcher, then nothing, then the app — as a cold start looks from
/// outside: the screen it was launched from stays up, goes blank, and is
/// replaced.
#[derive(Default)]
struct SlowToDraw {
    reads: u32,
}

impl Device for SlowToDraw {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        self.reads += 1;
        Ok(match self.reads {
            // Still on the launcher, which is full of rows and none of them
            // are the app's.
            1..=2 => screen_of(&["Phone", "Camera", "Wallet"])
                .in_app(Some("com.example.launcher".into())),
            3..=4 => Snapshot::new(Vec::new())
                .expect("an empty screen")
                .in_app(Some("com.example.wallet".into())),
            _ => screen_of(&["Go Back", "Continue"]).in_app(Some("com.example.wallet".into())),
        })
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        Ok(())
    }
    fn home_screen_app(&mut self) -> Option<Box<str>> {
        Some("com.example.launcher".into())
    }
}

/// When the device can say how long the screen has been quiet, that is the
/// answer — not a guess assembled from watching the tree. A screen still
/// being drawn emits accessibility events; one that has finished does not.
///
/// Settling waits for the quiet to reach the threshold and then stops, so a
/// screen that went still immediately costs one reading rather than two
/// agreeing ones.
#[test]
fn a_device_that_can_say_it_is_quiet_is_believed() {
    let judge = Scripted::new(vec![
        answer("tap", Some("A2"), 0.99, 0.02),
        answer("done", None, 0.99, 0.97),
    ]);
    let mut pilot = Pilot::new(Quietening::default(), judge, &Android);

    pilot.pursue("tap the thing").expect("the run completes");

    // One reading to act on, one to see the screen is quiet, one for the
    // next step. A tree-watching settle needs two agreeing readings instead
    // of the one, and so takes four.
    assert!(
        pilot.device().reads <= 3,
        "a screen that says it is quiet is not polled at: {} reads",
        pilot.device().reads,
    );
}

/// A device that reports the screen has been still for a good while.
#[derive(Default)]
struct Quietening {
    reads: u32,
    acted: bool,
}

impl Device for Quietening {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        self.reads += 1;
        let rows: &[&str] = if self.acted {
            &["Go Back", "Done"]
        } else {
            &["Go Back", "Continue"]
        };
        Ok(screen_of(rows).quiet_for(Some(400)))
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.acted = true;
        Ok(())
    }
}

/// The repetition count is kept by comparing what was just done with what was
/// done before. What was done before is also the sentence shown to the judge,
/// and that sentence gets a note appended to it when the screen did not move
/// — so the comparison stopped matching and the count silently reset.
///
/// Measured on a transfer form: five consecutive taps on the same Continue,
/// counted as `None, None, None, 2, None`.
#[test]
fn repeating_is_counted_even_when_the_screen_never_moves() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("tap", Some("A2"), 0.99, 0.02); 6]),
    };
    // Inert: every action leaves the screen exactly as it was, so every step
    // after the first has the note appended to what it did.
    let mut pilot = Pilot::new(
        Fake {
            inert: true,
            ..Fake::default()
        },
        judge,
        &Android,
    );

    let _ = pilot.pursue("tap the same thing");

    let seen = seen.borrow();
    assert_eq!(
        seen[2]["repeating"], 2,
        "the third step has done the same thing twice: {}",
        seen[2],
    );
}

/// An action that has achieved nothing yet is not the same as one that has
/// achieved nothing. A button backed by a network call shows no change and
/// emits no events while the call is in flight, so both the watching settle
/// and the device's own account of the silence agree — correctly — that the
/// screen has not moved.
///
/// Measured on a transfer form: Continue tapped five times, the run stopped
/// for want of progress, and the summary appeared afterwards. The app was
/// not ignoring the taps; it was slower than the run's patience.
///
/// So patience grows with each action that changes nothing, and costs
/// nothing at all when actions work.
#[test]
fn an_action_that_changes_nothing_is_given_longer_the_next_time() {
    // Different rows each time, so this is about the budget growing rather
    // than about repeating one action — that is
    // `an_action_that_changed_nothing_is_waited_out_rather_than_repeated`.
    let mut pilot = Pilot::new(
        Fake {
            inert: true,
            ..Fake::default()
        },
        Scripted::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("tap", Some("A4"), 0.99, 0.02),
            answer("tap", Some("A5"), 0.99, 0.02),
        ]),
        &Android,
    );

    let began = std::time::Instant::now();
    let ending = pilot
        .pursue("tap the same thing")
        .expect("the run completes");
    let spent = began.elapsed();

    assert!(
        matches!(ending, Ending::Uncertain { .. }),
        "it still gives up: {ending:?}",
    );
    // Three attempts at a flat budget come to three times it. Waiting
    // longer each time comes to six, so five is a threshold only a backoff
    // can clear.
    let flat = 5 * u128::from(Pilot::<Fake, Scripted>::CHANGE_BUDGET_MS);
    assert!(
        spent.as_millis() > flat,
        "later attempts must wait longer: {}ms is no more than {flat}ms",
        spent.as_millis(),
    );
}

/// Closing a keyboard that has already closed is going back. The catalog
/// offers it from the screen as it was when the step began, and a keyboard
/// dismissed by the step before may be gone by the time this one acts — so
/// the gesture lands as navigation and the run leaves the form it was
/// filling.
///
/// Measured on a transfer form: two `close_keyboard` steps in a row, the
/// second of which returned the run to the dashboard.
#[test]
fn closing_a_keyboard_that_has_already_closed_does_nothing() {
    let judge = Scripted::new(vec![
        answer("close_keyboard", None, 0.99, 0.02),
        answer("done", None, 0.99, 0.97),
    ]);
    let mut pilot = Pilot::new(Closing::default(), judge, &Android);

    pilot
        .pursue("put the keyboard away")
        .expect("the run completes");

    assert!(
        pilot.device().performed.is_empty(),
        "the keyboard was already down, so there was nothing to do: {:?}",
        pilot.device().performed,
    );
}

/// A device whose keyboard is up when the step is judged and down by the time
/// it acts, as one is when the step before dismissed it.
#[derive(Default)]
struct Closing {
    reads: u32,
    performed: Vec<Command>,
}

impl Device for Closing {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        self.reads += 1;
        Ok(screen_of(&["Go Back", "Continue"]).with_keyboard_open(self.reads == 1))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
}

/// An action that changed nothing, chosen again, is the one move that cannot
/// help. The screen is the same, so the judgement is the same, so the action
/// is the same — and on a commit button backed by a network call, each repeat
/// may restart the work it is waiting for.
///
/// Measured on a transfer form: Continue tapped five times while the app
/// validated, the run stopped for want of progress, and the summary arrived
/// moments later. Waiting is what was wanted, and waiting is never harmful.
#[test]
fn an_action_that_changed_nothing_is_waited_out_rather_than_repeated() {
    let mut pilot = Pilot::new(
        Fake {
            inert: true,
            ..Fake::default()
        },
        Scripted::new(vec![answer("tap", Some("A2"), 0.99, 0.02); 6]),
        &Android,
    );

    let _ = pilot.pursue("tap the same thing");

    let taps = pilot
        .device()
        .performed
        .iter()
        .filter(|command| matches!(command, Command::Tap(_)))
        .count();
    assert_eq!(
        taps,
        1,
        "the tap that changed nothing is not sent again: {:?}",
        pilot.device().performed,
    );
    assert!(
        pilot
            .device()
            .performed
            .iter()
            .any(|command| matches!(command, Command::Settle)),
        "it waits instead: {:?}",
        pilot.device().performed,
    );
}

/// A sheet whose scrim renders before its contents offers exactly one row,
/// and that row is the way out of it. Acting then taps the only thing there
/// — dismissing the sheet that was just opened, which must then be opened
/// again.
///
/// Measured on a transfer form: tap the source-account picker, read a screen
/// of one row called `Dismiss`, tap it, and reopen the picker. The same shape
/// as a step whose only rendered control was its Back button.
///
/// One row is looked at twice before it is believed. A screen that genuinely
/// offers one thing costs a moment; a screen still arriving stops costing a
/// wasted action.
#[test]
fn a_screen_offering_a_single_row_is_looked_at_again_before_acting() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Unfurling::default(), judge, &Android);

    pilot
        .pursue("choose an account")
        .expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(
        seen[0]["rows"].as_object().expect("rows").len(),
        3,
        "the sheet is judged once it has unfurled, not while it is a scrim: {}",
        seen[0],
    );
}

/// A sheet that shows its scrim first and its contents a moment later.
#[derive(Default)]
struct Unfurling {
    reads: u32,
    performed: Vec<Command>,
}

impl Device for Unfurling {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        self.reads += 1;
        Ok(if self.reads == 1 {
            screen_of(&["Dismiss"])
        } else {
            screen_of(&["Dismiss", "Current account", "Savings account"])
        })
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        self.performed.push(command.clone());
        Ok(())
    }
}

/// Waiting is not an action that achieved nothing. A keypad whose counter
/// lags a tap looks unchanged for a reading or two, so the run waits — and
/// counting each wait towards "this is getting nowhere" ends a run that is
/// getting somewhere slowly.
///
/// Measured entering a PIN: tap, wait, tap, and the run stopped for want of
/// progress while the digits were going in.
#[test]
fn waiting_does_not_count_towards_getting_nowhere() {
    let judge = Scripted::new(vec![answer("tap", Some("A2"), 0.99, 0.02); 30]);
    let mut pilot = Pilot::new(
        Fake {
            inert: true,
            ..Fake::default()
        },
        judge,
        &Android,
    );

    let ending = pilot.pursue("press the key").expect("the run completes");

    // A screen that never moves still ends the run — as going in circles,
    // which is what it is, rather than as three failed actions when only one
    // action was ever taken.
    assert_eq!(
        ending,
        Ending::Uncertain {
            because: Indecision::NoProgress {
                repeated: Pilot::<Fake, Scripted>::VISITS_ALLOWED
            }
        },
        "got {ending:?}",
    );
}

/// A run that finds its goal already met on the first screen has done nothing,
/// and whoever started it should be able to tell. Measured: a search-and-open
/// run started on the article the run before it had left open, declared the
/// goal achieved, and had searched for nothing.
#[test]
fn a_run_can_say_how_many_actions_it_took() {
    let mut idle = Pilot::new(
        Fake::default(),
        Scripted::new(vec![answer("done", None, 0.99, 0.97)]),
        &Android,
    );
    let ending = idle.pursue("Open the article").expect("the run completes");
    assert!(
        matches!(ending, Ending::Finished(Outcome::Achieved)),
        "{ending:?}"
    );
    assert_eq!(idle.actions_taken(), 0);

    let mut busy = Pilot::new(
        Fake::default(),
        Scripted::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    );
    busy.pursue("Open the article").expect("the run completes");
    assert_eq!(busy.actions_taken(), 1);
}

/// A screen that has been still since before the action was sent says nothing
/// about the action. Measured on a Flutter transfer form: `Continue` started a
/// quote that took about a second, the helper still reported the screen quiet
/// from before the tap, the run called the tap ineffective within 300ms, and
/// tapped Continue three times. Putting the keyboard away was misjudged the
/// same way.
#[test]
fn quiet_from_before_the_action_is_not_taken_as_its_result() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Slow::default(), judge, &Android);

    pilot.pursue("continue").expect("the run completes");

    let told = seen.borrow()[1]["previous_action"].to_string();
    assert!(
        !told.contains("did not change"),
        "the tap did change the screen: {told}"
    );
}

/// A device whose screen changes a few readings after an action, and which
/// reports it quiet all the while, since nothing has been drawn yet.
#[derive(Default)]
struct Slow {
    since_action: Option<u32>,
}

impl Device for Slow {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let arrived = self.since_action.is_some_and(|reads| reads >= 4);
        if let Some(reads) = self.since_action.as_mut() {
            *reads += 1;
        }
        Ok(if arrived {
            screen_of(&["Go Back", "Confirm Transfer"]).quiet_for(Some(200))
        } else {
            screen_of(&["Go Back", "Continue"]).quiet_for(Some(5_000))
        })
    }
    fn perform(&mut self, _command: &Command) -> Result<(), Infallible> {
        self.since_action = Some(0);
        Ok(())
    }
}

/// A plan given to the pilot reaches the questions every step is asked.
#[test]
fn a_run_given_steps_asks_each_step_about_them() {
    struct Asked(std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>>);
    impl Judge for Asked {
        type Error = Infallible;
        fn evaluate(
            &self,
            _state: serde_json::Value,
            questions: &StepQuestions<'_>,
        ) -> Result<StepAnswers, Infallible> {
            self.0
                .borrow_mut()
                .push(serde_json::to_value(questions).expect("serializes"));
            Ok(serde_json::from_value(answer("done", None, 0.99, 0.97)).expect("parses"))
        }
    }
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut pilot = Pilot::new(Fake::default(), Asked(std::rc::Rc::clone(&asked)), &Android)
        .following(["Open Network and Internet", "Turn on Wi-Fi"]);

    pilot.pursue("Turn on Wi-Fi").expect("the run completes");

    assert_eq!(
        asked.borrow()[0]["operation"]["instructions"]["goal"],
        serde_json::json!(["Open Network and Internet", "Turn on Wi-Fi"])
    );
}

/// A keypad that shows only how many digits are in, as a PIN pad does.
#[derive(Default)]
struct DigitPad {
    entered: usize,
}

impl Device for DigitPad {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        let count = format!("{} digits entered", self.entered);
        Ok(screen_of(&[
            "Go Back", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "DEL", &count,
        ]))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        if matches!(command, Command::Tap(_)) {
            self.entered += 1;
        }
        Ok(())
    }
}

/// Which digit comes next is counting, and counting belongs in code. Told
/// only "tap 2, 4, 6, 8, 1, 0 in that order" and a pad that says how many are
/// in, Jev tapped 1, 2, 2, 2. Told the next key by name, it put 0.98 to 1.00
/// on the right one.
#[test]
fn a_run_entering_keys_is_told_which_key_comes_next() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("tap", Some("A5"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(DigitPad::default(), judge, &Android).entering_keys("246");

    pilot.pursue("enter the PIN").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(seen[0]["sequence"]["next_key"], "2", "{}", seen[0]);
    assert_eq!(seen[0]["sequence"]["entered_so_far"], 0);
    assert_eq!(seen[1]["sequence"]["next_key"], "4", "{}", seen[1]);
    assert_eq!(seen[2]["sequence"]["next_key"], "6", "{}", seen[2]);
}

/// The keys are named only where they can be pressed.
#[test]
fn a_screen_without_the_keys_is_told_nothing_about_them() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97)]),
    };
    let mut pilot = Pilot::new(Fake::default(), judge, &Android).entering_keys("246");

    pilot.pursue("open settings").expect("the run completes");

    assert!(
        seen.borrow()[0].get("sequence").is_none(),
        "{}",
        seen.borrow()[0]
    );
}

/// The questions say to look at it, as well as the state carrying it: with a
/// list of steps as the goal, the state alone left the operation at 0.62.
#[test]
fn a_run_entering_keys_points_its_questions_at_the_next_key() {
    struct Asked(std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>>);
    impl Judge for Asked {
        type Error = Infallible;
        fn evaluate(
            &self,
            _state: serde_json::Value,
            questions: &StepQuestions<'_>,
        ) -> Result<StepAnswers, Infallible> {
            self.0
                .borrow_mut()
                .push(serde_json::to_value(questions).expect("serializes"));
            Ok(serde_json::from_value(answer("done", None, 0.99, 0.97)).expect("parses"))
        }
    }
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut pilot = Pilot::new(
        DigitPad::default(),
        Asked(std::rc::Rc::clone(&asked)),
        &Android,
    )
    .entering_keys("246");

    pilot.pursue("enter the PIN").expect("the run completes");

    let asked = asked.borrow();
    for head in ["operation", "tap_target"] {
        assert!(
            asked[0][head]["instructions"]["note"]
                .to_string()
                .contains("sequence.next_key"),
            "{head}: {}",
            asked[0][head]
        );
    }
}

/// A form that reformats what was typed into it, and does nothing else, while
/// the quote its Continue asked for is on its way.
#[derive(Default)]
struct Reformatting {
    taps: usize,
    reads: usize,
}

impl Device for Reformatting {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        use jev_pilot::snapshot::{Bounds, Element};
        self.reads += 1;
        let amount = if self.taps == 0 { "50" } else { "50.00" };
        Ok(Snapshot::new(vec![
            Element {
                label: amount.into(),
                detail: None,
                editable: true,
                bounds: Bounds::from_origin_size(0, 0, 400, 80),
            },
            Element {
                label: "Continue".into(),
                detail: None,
                editable: false,
                bounds: Bounds::from_origin_size(0, 200, 400, 80),
            },
        ])
        .expect("a screen"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        if matches!(command, Command::Tap(_)) {
            self.taps += 1;
        }
        Ok(())
    }
}

/// A form is the same place whatever its fields hold. Measured on a transfer
/// form: Continue's first tap turned "50" into "50.00" while its quote was on
/// the way, the reformatted form looked like somewhere new, and Continue was
/// tapped three times — here a quote each time, on a commit button a payment.
#[test]
fn a_button_is_not_pressed_again_because_a_field_reformatted_itself() {
    let mut pilot = Pilot::new(
        Reformatting::default(),
        Scripted::new(vec![
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("tap", Some("A2"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    );

    pilot
        .pursue("continue past the form")
        .expect("the run completes");

    assert_eq!(pilot.device().taps, 1, "the second tap was waited out");
}

/// Each request can be kept as it was sent, so a step that went wrong can be
/// replayed against the model with its wording changed, rather than guessed at
/// from a reconstruction that asks something slightly different.
#[test]
fn a_recorded_judge_keeps_each_request_as_it_was_sent() {
    let dir = std::env::temp_dir().join("jev-pilot-test-recorded-requests");
    let _ = std::fs::remove_dir_all(&dir);
    let judge = jev_pilot::pilot::Recorded::new(
        Scripted::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        dir.clone(),
    );
    let mut pilot = Pilot::new(Fake::default(), judge, &Android);

    pilot.pursue("Turn on Wi-Fi").expect("the run completes");

    let first: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("step-001.json")).expect("the first request was kept"),
    )
    .expect("it is JSON");
    assert!(first["state"]["rows"].is_object(), "{first}");
    assert_eq!(first["questions"]["operation"]["type"], "choice");
    assert_eq!(first["model"], jev_pilot::client::DEFAULT_MODEL);
    assert!(dir.join("step-002.json").exists());
}

/// Told only that a keyboard is up, Jev put 0.42 on putting it away with the
/// form's Continue hidden behind it; told what a keyboard covers, 0.92.
#[test]
fn the_state_says_what_an_open_keyboard_covers() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
    };
    let mut pilot = Pilot::new(Fake::default(), judge, &Android);

    pilot.pursue("fill the form in").expect("the run completes");

    let seen = seen.borrow();
    assert!(seen[0].get("keyboard_covers").is_none(), "{}", seen[0]);
    assert!(seen[1]["keyboard_covers"].is_string(), "{}", seen[1]);
}

/// A screen moving from one thing to the next is read as it passes: Confirm
/// Transfer gone from the summary and the PIN pad not yet drawn. Asked what to
/// do there, Jev was unsure and nothing it could be told raised waiting above
/// 0.14. Having just seen the screen move, the run looks again once before
/// asking anyone.
#[test]
fn an_uncertain_step_right_after_the_screen_moved_looks_again_before_asking() {
    let asked = std::cell::Cell::new(0_u32);
    let mut pilot = Pilot::new(
        Fake::default(),
        Scripted::new(vec![
            answer("tap", Some("A3"), 0.99, 0.02),
            answer("tap", Some("A3"), 0.10, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    )
    .requiring(Confidence::new(0.6).expect("valid"))
    .escalating_to(|_: &jev_pilot::pilot::Impasse<'_>| {
        asked.set(asked.get() + 1);
        Ok::<_, Infallible>(jev_pilot::pilot::Resolution::Stop)
    });

    pilot.pursue("go on").expect("the run completes");

    assert_eq!(asked.get(), 0, "it looked again rather than asking");
    assert!(
        pilot.device().performed.contains(&Command::Settle),
        "{:?}",
        pilot.device().performed
    );
}

/// A login form that has reset itself: one field, empty, and its button off.
#[derive(Default)]
struct ResetLogin {
    typed: Vec<String>,
}

impl Device for ResetLogin {
    type Error = Infallible;
    fn observe(&mut self) -> Result<Snapshot, Infallible> {
        use jev_pilot::snapshot::{Bounds, Element};
        let filled = !self.typed.is_empty();
        Ok(Snapshot::new(vec![
            Element {
                label: "Password".into(),
                detail: (!filled).then(|| "empty".into()),
                editable: true,
                bounds: Bounds::from_origin_size(0, 0, 400, 80),
            },
            Element {
                label: "Forgot your password?".into(),
                detail: None,
                editable: false,
                bounds: Bounds::from_origin_size(0, 200, 400, 80),
            },
        ])
        .expect("a screen"))
    }
    fn perform(&mut self, command: &Command) -> Result<(), Infallible> {
        if let Command::TypeText { text, .. } = command {
            self.typed.push(text.to_string());
        }
        Ok(())
    }
}

/// The state names the fields that are empty and those the run has words for.
/// On a reset login form, with the history saying the password had been typed
/// and submitted, retyping it read 0.00; told both, 0.47 to 0.54.
#[test]
fn the_state_names_empty_fields_and_those_there_are_words_for() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let judge = Recording {
        seen: std::rc::Rc::clone(&seen),
        turns: std::cell::RefCell::new(vec![answer("done", None, 0.99, 0.97)]),
    };
    let mut pilot = Pilot::new(ResetLogin::default(), judge, &Android).supplying(["password"]);

    pilot.pursue("log in").expect("the run completes");

    let seen = seen.borrow();
    assert_eq!(
        seen[0]["empty_fields"],
        serde_json::json!(["Password"]),
        "{}",
        seen[0]
    );
    assert_eq!(
        seen[0]["words_ready_for"],
        serde_json::json!(["Password"]),
        "{}",
        seen[0]
    );
}

/// Unsure, on a screen with exactly one empty field it has the words for, the
/// run fills it in rather than asking: that is harmless, and it is what the
/// words were given for.
#[test]
fn an_unsure_step_fills_the_one_empty_field_it_has_words_for() {
    use jev_pilot::pilot::{Resolution, Writing};
    let asked = std::cell::Cell::new(false);
    let mut pilot = Pilot::new(
        ResetLogin::default(),
        Scripted::new(vec![
            answer("tap", Some("A2"), 0.20, 0.02),
            answer("done", None, 0.99, 0.97),
        ]),
        &Android,
    )
    .requiring(Confidence::new(0.6).expect("valid"))
    .supplying(["password"])
    .escalating_to(|_: &jev_pilot::pilot::Impasse<'_>| {
        asked.set(true);
        Ok::<_, Infallible>(Resolution::Stop)
    })
    .writing_with(|_: &Writing<'_>| Ok::<_, Infallible>(Box::<str>::from("Password123!")));

    pilot.pursue("log in").expect("the run completes");

    assert!(!asked.get(), "nobody was asked");
    assert_eq!(pilot.device().typed, ["Password123!"]);
}
