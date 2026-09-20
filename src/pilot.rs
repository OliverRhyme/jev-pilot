//! The loop: observe, judge, act, repeat.
//!
//! The judging is behind [`Judge`] rather than wired straight to the HTTP
//! client, so the loop's control flow — the guards, the confidence floor, the
//! step limit — is tested against scripted answers instead of against a model.

use crate::act::{Act, Catalog, Indecision, Operation, Outcome};
use crate::device::{Command, Device, command_for};
use crate::judgment::Confidence;
use crate::platform::Platform;
use crate::snapshot::{Element, Snapshot, StaleRef};
use crate::step::{StepAnswers, StepQuestions};
use core::fmt;

/// Something that can answer a step's questions.
pub trait Judge {
    /// What can go wrong asking.
    type Error;

    /// Evaluate one step's questions against one state.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the judgment cannot be obtained.
    fn evaluate(
        &self,
        state: serde_json::Value,
        questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Self::Error>;
}

#[cfg(feature = "http")]
impl Judge for crate::client::http::SystemOne {
    type Error = crate::client::http::ApiError;

    fn evaluate(
        &self,
        state: serde_json::Value,
        questions: &StepQuestions<'_>,
    ) -> Result<StepAnswers, Self::Error> {
        self.evaluate(&crate::client::Request::new(state, questions))
            .map(|evaluation: crate::client::Evaluation<StepAnswers>| evaluation.answers)
    }
}

/// Everything a second opinion is told about an impasse.
///
/// It is shown what Jev was shown, so it is judging the same situation rather
/// than a summary of it.
#[derive(Debug)]
pub struct Impasse<'i> {
    /// What the run is trying to achieve.
    pub goal: &'i str,
    /// Which step this is.
    pub step: u32,
    /// Why the answer could not be acted on.
    pub because: &'i Indecision,
    /// The rows on screen, in the order they were offered.
    pub rows: &'i [String],
    /// The operations this platform offers here.
    pub operations: &'i [Operation],
    /// What the previous step did, if there was one.
    pub previous: Option<&'i str>,
}

/// What a second opinion decided.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// Carry out this operation, on this row by position.
    ///
    /// Deliberately an index rather than an action: a second opinion is bound
    /// by the same catalog Jev was, so it cannot name an operation the platform
    /// lacks, a row that is not on screen, or a coordinate at all.
    Choose {
        /// What to do.
        operation: Operation,
        /// Which row, for an operation that needs one.
        target: Option<usize>,
    },
    /// Decline, ending the run as if nothing had been consulted.
    Stop,
}

/// Consulted when Jev was not sure enough to act.
///
/// This is where a System Two model, or a person, belongs. Re-asking Jev the
/// same question would be wasted: it returns a calibrated distribution over the
/// state it was given, so an unchanged state yields the same answer and the
/// same refusal. Only a different judge, or a changed screen, makes progress.
pub trait Escalate {
    /// What can go wrong asking.
    type Error;

    /// Decide what to do about an impasse.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the second opinion cannot be obtained.
    fn consult(&mut self, impasse: &Impasse<'_>) -> Result<Resolution, Self::Error>;
}

/// The default policy: stop, and let the caller decide what to do about it.
///
/// Acting on a flat distribution is how a loop wanders into unrelated state,
/// and escalating without being asked would be a policy decision the caller
/// never made.
#[derive(Debug, Clone, Copy, Default)]
pub struct Halt;

impl Escalate for Halt {
    type Error = core::convert::Infallible;

    fn consult(&mut self, _impasse: &Impasse<'_>) -> Result<Resolution, Self::Error> {
        Ok(Resolution::Stop)
    }
}

impl<F, E> Escalate for F
where
    F: FnMut(&Impasse<'_>) -> Result<Resolution, E>,
{
    type Error = E;

    fn consult(&mut self, impasse: &Impasse<'_>) -> Result<Resolution, Self::Error> {
        self(impasse)
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Ending {
    /// The model declared the run over.
    Finished(Outcome),
    /// The screen was judged to be an error state.
    ErrorScreen,
    /// No option stood out clearly enough to act on.
    Uncertain {
        /// Why the choice was refused.
        because: Indecision,
    },
    /// The step limit was reached without a verdict.
    OutOfSteps {
        /// The limit that was hit.
        limit: u32,
    },
}

/// A run could not be carried out.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError<D, J, X> {
    /// The device could not be observed or driven.
    Device(D),
    /// The judgment could not be obtained.
    Judge(J),
    /// The second opinion could not be obtained.
    Escalation(X),
    /// An action named a screen that had already been replaced.
    Stale(StaleRef),
}

impl<D: fmt::Display, J: fmt::Display, X: fmt::Display> fmt::Display for RunError<D, J, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(inner) => write!(f, "device: {inner}"),
            Self::Judge(inner) => write!(f, "judge: {inner}"),
            Self::Escalation(inner) => write!(f, "escalation: {inner}"),
            Self::Stale(inner) => write!(f, "{inner}"),
        }
    }
}

impl<D, J, X> core::error::Error for RunError<D, J, X>
where
    D: core::error::Error + 'static,
    J: core::error::Error + 'static,
    X: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Device(inner) => Some(inner),
            Self::Judge(inner) => Some(inner),
            Self::Escalation(inner) => Some(inner),
            Self::Stale(inner) => Some(inner),
        }
    }
}

/// What one iteration saw and decided.
///
/// A run that ends uncertain or out of steps is only diagnosable if the screens
/// it saw are visible, so the loop reports each step rather than only its
/// verdict.
#[derive(Debug)]
pub struct StepReport<'s> {
    /// Which step this is, counting from one.
    pub index: u32,
    /// The rows the screen offered, as the model was shown them.
    pub rows: Vec<String>,
    /// What the model chose, if it was certain enough to act.
    pub chosen: Option<&'s Act>,
    /// How sure the model was about *what* to do.
    pub operation_confidence: Confidence,
    /// How sure it was about *which row*, when it named one.
    pub target_confidence: Option<Confidence>,
    /// How likely the goal was already met.
    pub goal_met: f64,
    /// How likely the screen was an error state.
    pub is_error_screen: f64,
}

/// What a run produces: an ending, or a failure from one of its three parts.
pub type RunResult<D, J, X> =
    Result<Ending, RunError<<D as Device>::Error, <J as Judge>::Error, <X as Escalate>::Error>>;

/// Something told about each step as it happens.
type Observer<'p> = Box<dyn FnMut(&StepReport<'_>) + 'p>;

/// Drives one device towards a goal.
///
/// `Debug` is written out rather than derived: the step observer is a boxed
/// closure, which has no `Debug` of its own, so its presence is reported
/// instead of its contents.
pub struct Pilot<'p, D, J, X = Halt> {
    device: D,
    judge: J,
    escalation: X,
    platform: &'p dyn Platform,
    floor: Confidence,
    limit: u32,
    certainty: f64,
    observer: Option<Observer<'p>>,
}

impl<'p, D: Device, J: Judge, X: Escalate> Pilot<'p, D, J, X> {
    /// How many steps a run takes before giving up.
    pub const STEP_LIMIT: u32 = 25;

    /// Above this probability, a guard question is treated as decided.
    pub const CERTAINTY: f64 = 0.8;

    /// Send impasses to a second opinion instead of stopping at them.
    #[must_use]
    pub fn escalating_to<Y: Escalate>(self, escalation: Y) -> Pilot<'p, D, J, Y> {
        Pilot {
            device: self.device,
            judge: self.judge,
            escalation,
            platform: self.platform,
            floor: self.floor,
            limit: self.limit,
            certainty: self.certainty,
            observer: self.observer,
        }
    }
}

impl<'p, D: Device, J: Judge> Pilot<'p, D, J, Halt> {
    /// Pair a device with something that can judge what to do next.
    pub fn new(device: D, judge: J, platform: &'p dyn Platform) -> Self {
        Self {
            device,
            judge,
            escalation: Halt,
            platform,
            floor: Confidence::ZERO,
            limit: Self::STEP_LIMIT,
            certainty: Self::CERTAINTY,
            observer: None,
        }
    }
}

impl<D: fmt::Debug, J: fmt::Debug, X: fmt::Debug> fmt::Debug for Pilot<'_, D, J, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pilot")
            .field("device", &self.device)
            .field("judge", &self.judge)
            .field("escalation", &self.escalation)
            .field("platform", &self.platform.name())
            .field("floor", &self.floor)
            .field("limit", &self.limit)
            .field("certainty", &self.certainty)
            .field("observed", &self.observer.is_some())
            .finish()
    }
}

impl<'p, D: Device, J: Judge, X: Escalate> Pilot<'p, D, J, X> {
    /// Report every step as it happens.
    #[must_use]
    pub fn watching(mut self, observer: impl FnMut(&StepReport<'_>) + 'p) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    /// Refuse to act on an answer less certain than this.
    #[must_use]
    pub const fn requiring(mut self, floor: Confidence) -> Self {
        self.floor = floor;
        self
    }

    /// Give up after this many steps.
    #[must_use]
    pub const fn limited_to(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// The device being driven.
    pub const fn device(&self) -> &D {
        &self.device
    }

    /// Tell the observer, if there is one, what this step saw and did.
    fn report(
        &mut self,
        index: u32,
        snapshot: &Snapshot,
        answers: &StepAnswers,
        chosen: Option<&Act>,
    ) {
        let Some(observer) = self.observer.as_mut() else {
            return;
        };
        observer(&StepReport {
            index,
            rows: snapshot
                .refs()
                .map(|(_, element)| element.describe())
                .collect(),
            chosen,
            operation_confidence: answers.operation.confidence,
            target_confidence: answers.tap_target.as_ref().map(|chosen| chosen.confidence),
            goal_met: answers.goal_met.noul,
            is_error_screen: answers.is_error_screen.noul,
        });
    }

    /// Work towards `goal` until the run ends.
    ///
    /// # Errors
    /// Returns [`RunError`] when the device or the judge fails. A run that
    /// merely does not succeed — uncertain, blocked, out of steps — is an
    /// [`Ending`], not an error.
    pub fn pursue(&mut self, goal: &str) -> RunResult<D, J, X> {
        let mut previous: Option<String> = None;
        for index in 1..=self.limit {
            let snapshot = self.device.observe().map_err(RunError::Device)?;
            let catalog = Catalog::for_screen(&snapshot, self.platform);
            let questions = StepQuestions::new(goal, &catalog);

            let answers = self
                .judge
                .evaluate(
                    describe(&snapshot, self.platform, previous.as_deref()),
                    &questions,
                )
                .map_err(RunError::Judge)?;

            let decided = catalog.resolve(&answers, self.floor);

            if answers.is_error_screen.noul > self.certainty {
                self.report(index, &snapshot, &answers, decided.as_ref().ok());
                return Ok(Ending::ErrorScreen);
            }
            if answers.goal_met.noul > self.certainty {
                self.report(index, &snapshot, &answers, decided.as_ref().ok());
                return Ok(Ending::Finished(Outcome::Achieved));
            }

            let act = match decided {
                Ok(act) => act,
                Err(because) => {
                    let rows: Vec<String> = snapshot
                        .refs()
                        .map(|(_, element)| element.describe())
                        .collect();
                    let resolution = self
                        .escalation
                        .consult(&Impasse {
                            goal,
                            step: index,
                            because: &because,
                            rows: &rows,
                            operations: catalog.operations(),
                            previous: previous.as_deref(),
                        })
                        .map_err(RunError::Escalation)?;
                    match resolution {
                        Resolution::Stop => {
                            self.report(index, &snapshot, &answers, None);
                            return Ok(Ending::Uncertain { because });
                        }
                        // Resolved through the same catalog, so a second opinion
                        // is bound by every constraint Jev was.
                        Resolution::Choose { operation, target } => {
                            match catalog.act_from(operation, target) {
                                Ok(act) => act,
                                Err(because) => {
                                    self.report(index, &snapshot, &answers, None);
                                    return Ok(Ending::Uncertain { because });
                                }
                            }
                        }
                    }
                }
            };

            // Reported once the act is settled, so a step resolved by an
            // escalation is logged with what was actually sent to the device
            // rather than with the refusal that preceded it.
            self.report(index, &snapshot, &answers, Some(&act));

            previous = Some(recount(&act, &snapshot));
            let command = command_for(&act, &snapshot).map_err(RunError::Stale)?;
            if let Some(command) = command {
                self.device.perform(&command).map_err(RunError::Device)?;
            }
        }
        Ok(Ending::OutOfSteps { limit: self.limit })
    }
}

/// The state a step is judged against.
///
/// Only semantics: the rows as a person would read them, which platform this
/// is, and what the previous step did. No coordinates, because a model that
/// cannot see one cannot invent one.
///
/// The previous action matters more than it looks. Without it, the judgment
/// "is the goal met?" is made by something with no memory: it cannot tell a
/// screen it has just reached from one it has been stuck on for five turns,
/// nor whether its last action changed anything at all.
fn describe(
    snapshot: &Snapshot,
    platform: &dyn Platform,
    previous: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "platform": platform.name(),
        "previous_action": previous,
        "visible_rows": snapshot
            .refs()
            .map(|(_, element)| element.describe())
            .collect::<Vec<_>>(),
    })
}

/// Describe an action the way the next step should hear about it.
fn recount(act: &Act, snapshot: &Snapshot) -> String {
    let named = |handle| {
        snapshot
            .resolve(handle)
            .map_or_else(|_| "a row that is gone".to_owned(), Element::describe)
    };
    match act {
        Act::Tap(handle) => format!("Tapped {}", named(*handle)),
        Act::DoubleTap(handle) => format!("Double-tapped {}", named(*handle)),
        Act::LongPress(handle) => format!("Long-pressed {}", named(*handle)),
        Act::Peek(handle) => format!("Peeked at {}", named(*handle)),
        Act::SwipeElement { target, direction } => {
            format!("Swiped {} {direction:?}", named(*target))
        }
        Act::TypeText { into, text } => format!("Typed {text:?} into {}", named(*into)),
        Act::Scroll(direction) => format!("Scrolled {direction:?}"),
        Act::System(gesture) => format!("Pressed {gesture:?}"),
        Act::Wait => "Waited for the screen to settle".to_owned(),
        Act::Finish(outcome) => format!("Declared {outcome:?}"),
    }
}

/// A command is cloned into the device call, so it must be cloneable.
const _: fn() = || {
    fn assert_clone<T: Clone>() {}
    assert_clone::<Command>();
};
