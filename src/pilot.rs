//! The loop: observe, judge, act, repeat.
//!
//! The judging is behind [`Judge`] rather than wired straight to the HTTP
//! client, so the loop's control flow — the guards, the confidence floor, the
//! step limit — is tested against scripted answers instead of against a model.

use crate::act::{Act, Catalog, Consequence, Decision, Floors, Indecision, Operation, Outcome};
use crate::device::{Command, Device, command_for};
use crate::judgment::{Confidence, Criterion, Progress};
use crate::platform::Platform;
use crate::snapshot::{Element, Snapshot, TapError};
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
    /// The operation Jev leaned toward, even though it was not sure enough.
    pub leaning: Operation,
    /// How sure it was about the operation.
    pub operation_confidence: Confidence,
    /// How sure it was about the row, when it named one.
    pub target_confidence: Option<Confidence>,
    /// What the operation was nearly beaten by, highest first.
    pub alternatives: &'i [(Box<str>, f64)],
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

/// What a field is waiting to be given.
#[derive(Debug)]
#[non_exhaustive]
pub struct Writing<'w> {
    /// What the run is trying to achieve.
    pub goal: &'w str,
    /// Which step this is.
    pub step: u32,
    /// The field the words will be typed into.
    pub field: &'w Element,
    /// The rows on screen, as the model was shown them.
    pub rows: &'w [String],
    /// What the previous step did, if there was one.
    pub previous: Option<&'w str>,
}

/// Writes the text a System One model cannot.
///
/// Jev decides *whether* to type and *where*; it selects among options and does
/// not generate, so the words themselves must come from somewhere else. This is
/// the same handover as [`Escalate`] and for the same reason — a slower, more
/// general judge, which may be a reasoning model or a person.
pub trait Compose {
    /// What can go wrong writing.
    type Error;

    /// Write what should be typed into this field.
    ///
    /// # Errors
    /// Returns [`Self::Error`] when the text cannot be obtained.
    fn compose(&mut self, request: &Writing<'_>) -> Result<Box<str>, Self::Error>;
}

/// The default: nothing can write, so typing is never offered.
#[derive(Debug, Clone, Copy, Default)]
pub struct Mute;

impl Compose for Mute {
    type Error = core::convert::Infallible;

    fn compose(&mut self, _request: &Writing<'_>) -> Result<Box<str>, Self::Error> {
        // Unreachable: `Mute` means typing is not offered, so nothing resolves
        // to a request for words.
        Ok(Box::from(""))
    }
}

impl<F, E> Compose for F
where
    F: FnMut(&Writing<'_>) -> Result<Box<str>, E>,
{
    type Error = E;

    fn compose(&mut self, request: &Writing<'_>) -> Result<Box<str>, Self::Error> {
        self(request)
    }
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
pub enum RunError<D, J, X, C> {
    /// The device could not be observed or driven.
    Device(D),
    /// The judgment could not be obtained.
    Judge(J),
    /// The second opinion could not be obtained.
    Escalation(X),
    /// The text to type could not be written.
    Composition(C),
    /// An action could not be turned into a point on the screen.
    Unreachable(TapError),
}

impl<D: fmt::Display, J: fmt::Display, X: fmt::Display, C: fmt::Display> fmt::Display
    for RunError<D, J, X, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(inner) => write!(f, "device: {inner}"),
            Self::Judge(inner) => write!(f, "judge: {inner}"),
            Self::Escalation(inner) => write!(f, "escalation: {inner}"),
            Self::Composition(inner) => write!(f, "composing text: {inner}"),
            Self::Unreachable(inner) => write!(f, "{inner}"),
        }
    }
}

impl<D, J, X, C> core::error::Error for RunError<D, J, X, C>
where
    D: core::error::Error + 'static,
    J: core::error::Error + 'static,
    X: core::error::Error + 'static,
    C: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Device(inner) => Some(inner),
            Self::Judge(inner) => Some(inner),
            Self::Escalation(inner) => Some(inner),
            Self::Composition(inner) => Some(inner),
            Self::Unreachable(inner) => Some(inner),
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
    /// How far along the goal was judged to be.
    pub goal_met: Progress,
    /// How likely the screen was an error state.
    pub is_error_screen: f64,
}

/// A failure from one of a run's four parts.
pub type Failure<D, J, X, C> = RunError<
    <D as Device>::Error,
    <J as Judge>::Error,
    <X as Escalate>::Error,
    <C as Compose>::Error,
>;

/// What a run produces: an ending, or a failure from one of its four parts.
pub type RunResult<D, J, X, C> = Result<Ending, Failure<D, J, X, C>>;

/// Something told about each step as it happens.
type Observer<'p> = Box<dyn FnMut(&StepReport<'_>) + 'p>;

/// What came of turning a decision into something the device can do.
enum Reached {
    /// Carry this out.
    Command(Command),
    /// There is nothing to carry out; the act does not touch the device.
    Nothing,
    /// The row is covered and no alternative was offered.
    Unreachable,
}

/// Gather one step's context for whatever has to decide about it.
const fn taken_at<'s>(
    goal: &'s str,
    step: u32,
    snapshot: &'s Snapshot,
    catalog: &'s Catalog,
    answers: &'s StepAnswers,
    previous: Option<&'s str>,
) -> Taken<'s> {
    Taken {
        goal,
        step,
        snapshot,
        catalog,
        answers,
        previous,
    }
}

/// One step, as everything that decides about it needs to see it.
struct Taken<'s> {
    goal: &'s str,
    step: u32,
    snapshot: &'s Snapshot,
    catalog: &'s Catalog,
    answers: &'s StepAnswers,
    previous: Option<&'s str>,
}

/// Drives one device towards a goal.
///
/// `Debug` is written out rather than derived: the step observer is a boxed
/// closure, which has no `Debug` of its own, so its presence is reported
/// instead of its contents.
pub struct Pilot<'p, D, J, X = Halt, C = Mute> {
    device: D,
    judge: J,
    escalation: X,
    composer: C,
    platform: &'p dyn Platform,
    floors: Floors,
    limit: u32,
    certainty: f64,
    criteria: Vec<Criterion>,
    types: bool,
    /// The application the goal is about, when the caller named one.
    app: Option<Box<str>>,
    observer: Option<Observer<'p>>,
}

impl<'p, D: Device, J: Judge, X: Escalate, C: Compose> Pilot<'p, D, J, X, C> {
    /// How many steps a run takes before giving up.
    pub const STEP_LIMIT: u32 = 25;

    /// Above this probability, a guard question is treated as decided.
    pub const CERTAINTY: f64 = 0.8;

    /// Send impasses to a second opinion instead of stopping at them.
    #[must_use]
    pub fn escalating_to<Y: Escalate>(self, escalation: Y) -> Pilot<'p, D, J, Y, C> {
        Pilot {
            device: self.device,
            judge: self.judge,
            escalation,
            composer: self.composer,
            platform: self.platform,
            floors: self.floors,
            limit: self.limit,
            certainty: self.certainty,
            criteria: self.criteria,
            types: self.types,
            app: self.app,
            observer: self.observer,
        }
    }

    /// Offer typing, with this to write the words.
    ///
    /// Typing is not offered until something can supply the text, because an
    /// operation the model can choose and nothing can carry out is worse than
    /// one it was never offered.
    #[must_use]
    pub fn writing_with<W: Compose>(self, composer: W) -> Pilot<'p, D, J, X, W> {
        Pilot {
            device: self.device,
            judge: self.judge,
            escalation: self.escalation,
            composer,
            platform: self.platform,
            floors: self.floors,
            limit: self.limit,
            certainty: self.certainty,
            criteria: self.criteria,
            types: true,
            app: self.app,
            observer: self.observer,
        }
    }
}

impl<'p, D: Device, J: Judge> Pilot<'p, D, J, Halt, Mute> {
    /// Pair a device with something that can judge what to do next.
    pub fn new(device: D, judge: J, platform: &'p dyn Platform) -> Self {
        Self {
            device,
            judge,
            escalation: Halt,
            composer: Mute,
            platform,
            floors: Floors::new(Confidence::ZERO),
            limit: Self::STEP_LIMIT,
            certainty: Self::CERTAINTY,
            criteria: Vec::new(),
            types: false,
            app: None,
            observer: None,
        }
    }
}

impl<D: fmt::Debug, J: fmt::Debug, X: fmt::Debug, C: fmt::Debug> fmt::Debug
    for Pilot<'_, D, J, X, C>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pilot")
            .field("device", &self.device)
            .field("judge", &self.judge)
            .field("escalation", &self.escalation)
            .field("composer", &self.composer)
            .field("platform", &self.platform.name())
            .field("floors", &self.floors)
            .field("limit", &self.limit)
            .field("certainty", &self.certainty)
            .field("observed", &self.observer.is_some())
            .finish()
    }
}

impl<'p, D: Device, J: Judge, X: Escalate, C: Compose> Pilot<'p, D, J, X, C> {
    /// Require these to be true before a verdict of success is accepted.
    ///
    /// A general "is the goal met?" is judged against whatever the goal seems
    /// to mean, and something that merely resembles the goal can satisfy it —
    /// a short clip about the right subject passes "play a video about X" on a
    /// loose reading. Each criterion is asked as its own judgment in the same
    /// parallel request, so confirming costs nothing, and a verdict that fails
    /// any of them is refused rather than reported as success.
    ///
    /// # Write one claim per criterion
    ///
    /// This matters more than it looks, and getting it wrong looks like the
    /// model being wrong. Measured against a real player screen:
    ///
    /// | criterion | answer |
    /// |---|---|
    /// | `"A full-length video is playing, not a Short and not a search results page"` | 0.31 |
    /// | `"A video is currently playing"` | 0.86 |
    /// | `"The thing playing is a full-length video rather than a Short"` | 0.86 |
    /// | `"This screen is a list of search results"` | 0.18 |
    ///
    /// The three narrow questions each answer correctly; the one that bundles
    /// them answers 0.31 and rejects a verdict that was right. A judgment asked
    /// about three things at once has no coherent yes. Splitting costs nothing,
    /// because they are evaluated in the same parallel pass.
    #[must_use]
    pub fn confirming<K: Into<Criterion>>(mut self, criteria: impl IntoIterator<Item = K>) -> Self {
        self.criteria = criteria.into_iter().map(Into::into).collect();
        self
    }

    /// The thing answering the questions.
    pub const fn judge(&self) -> &J {
        &self.judge
    }

    /// Report every step as it happens.
    #[must_use]
    pub fn watching(mut self, observer: impl FnMut(&StepReport<'_>) + 'p) -> Self {
        self.observer = Some(Box::new(observer));
        self
    }

    /// Refuse to act on an answer less certain than this.
    ///
    /// Applies to every action. To gate a verdict or a destructive gesture more
    /// strictly than an ordinary tap, follow it with [`Self::requiring_for`].
    #[must_use]
    pub const fn requiring(mut self, floor: Confidence) -> Self {
        self.floors = Floors::new(floor);
        self
    }

    /// Name the application this goal is about.
    ///
    /// Brought to the front before the first judgement, and treated as the app
    /// the run belongs to from then on. A launcher is not a reliable way to
    /// reach an app — the icon may be in a folder, in the drawer, or on a page
    /// that is not showing — and no amount of judgement fixes a screen that
    /// does not contain the thing being looked for.
    #[must_use]
    pub fn about(mut self, app: Option<Box<str>>) -> Self {
        self.app = app;
        self
    }

    /// Use these floors, already scaled per consequence by the caller.
    #[must_use]
    pub const fn with_floors(mut self, floors: Floors) -> Self {
        self.floors = floors;
        self
    }

    /// Require more certainty for actions of a given consequence.
    ///
    /// A wrong tap on a list row is undone by going back; a wrong verdict ends
    /// the run with the wrong answer. Gating both at one number means either
    /// acting on guesses or refusing sound decisions.
    #[must_use]
    pub const fn requiring_for(mut self, consequence: Consequence, floor: Confidence) -> Self {
        self.floors = self.floors.requiring_for(consequence, floor);
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

    /// How long to keep looking for the screen to change after acting.
    ///
    /// Long enough for a dialog to swap its contents or a keyboard to come and
    /// go, which are slower than an ordinary screen transition, and short
    /// enough that an action which genuinely changed nothing is noticed rather
    /// than waited on.
    ///
    /// The whole budget is only ever spent when nothing changes, and that is
    /// what sets it: measured, three scrolls at the end of a list cost this
    /// each before the run gave up on them. A screen that is going to move
    /// does so in a fraction of it.
    pub const CHANGE_BUDGET_MS: u64 = 1_200;

    /// How often to look while waiting.
    ///
    /// A read through the helper is about 60ms, so looking this often costs
    /// little and returns as soon as the screen moves rather than at the end
    /// of a fixed delay.
    pub const CHANGE_POLL_MS: u64 = 80;

    /// How many actions in a row may leave the screen untouched before a run
    /// stops repeating itself and asks.
    pub const INEFFECTIVE_LIMIT: u32 = 3;

    /// Wait for the screen to become something other than `before`.
    ///
    /// Returns whether it did. The reads are cheap through the helper — about
    /// 60ms — so this costs a fraction of the fixed delay it replaces, and
    /// unlike a fixed delay it is right for both a quick transition and a slow
    /// one.
    fn settled_on_a_new_screen(&mut self, before: u64) -> Result<bool, Failure<D, J, X, C>> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(Self::CHANGE_BUDGET_MS);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(Self::CHANGE_POLL_MS));
            let now = self.device.observe().map_err(RunError::Device)?;
            if now.fingerprint() != before {
                return Ok(true);
            }
        }
        Ok(false)
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
            goal_met: answers.goal_met.progress(),
            is_error_screen: answers.is_error_screen.noul,
        });
    }

    /// Turn an act into something the device can do, recovering from a covered row.
    ///
    /// A row that turns out to be hidden behind what is drawn over it is a
    /// reason to choose something else, not to abandon the run: the screen is
    /// fine, this one row is simply not reachable. `None` means even the second
    /// opinion had nothing to offer.
    fn reach(&mut self, act: &Act, at: &Taken<'_>) -> Result<Reached, Failure<D, J, X, C>> {
        fn settle(command: Option<Command>) -> Reached {
            command.map_or(Reached::Nothing, Reached::Command)
        }
        match command_for(act, at.snapshot) {
            Ok(command) => Ok(settle(command)),
            Err(TapError::Obscured(_)) => match self.consult(at, &Indecision::Covered)? {
                Some(Decision::Ready(instead)) => command_for(&instead, at.snapshot)
                    .map(settle)
                    .map_err(RunError::Unreachable),
                _ => Ok(Reached::Unreachable),
            },
            Err(stale) => Err(RunError::Unreachable(stale)),
        }
    }

    /// Why a verdict of success should not be taken at face value, if it should not.
    ///
    /// Two ways it can be wrong. The screen may have been empty, in which case
    /// every judgment about it was made from nothing. Or an acceptance
    /// criterion may not hold, in which case something that resembles the goal
    /// was reached rather than the goal. Either way the reason is handed to the
    /// next step, so the same wrong thing is not chosen again.
    fn refuse_verdict(
        &self,
        outcome: Outcome,
        blind: bool,
        answers: &StepAnswers,
    ) -> Option<String> {
        if outcome != Outcome::Achieved {
            return None;
        }
        if blind {
            return Some(
                "Declared the goal done, but the screen was empty, so there was nothing \
                 to have judged it against"
                    .to_owned(),
            );
        }
        answers.unmet(&self.criteria, self.certainty).map(|unmet| {
            format!("Declared the goal done, but that was rejected: {unmet:?} was not true")
        })
    }

    /// Ask a second opinion what to do about an impasse.
    ///
    /// `None` means the run should end: either the opinion declined, or what it
    /// named could not be resolved through this screen's catalog. A second
    /// opinion is bound by exactly the constraints Jev was, so it can widen who
    /// decides without widening what may happen.
    fn consult(
        &mut self,
        at: &Taken<'_>,
        because: &Indecision,
    ) -> Result<Option<Decision>, Failure<D, J, X, C>> {
        let (goal, step, snapshot, catalog, answers, previous) = (
            at.goal,
            at.step,
            at.snapshot,
            at.catalog,
            at.answers,
            at.previous,
        );
        let rows: Vec<String> = snapshot
            .refs()
            .map(|(_, element)| element.describe())
            .collect();

        // Sorted so the thing it was nearly beaten by comes first: that is the
        // decision actually being asked about.
        let mut alternatives: Vec<(Box<str>, f64)> = answers
            .operation
            .probabilities
            .iter()
            .map(|(name, probability)| (name.clone(), *probability))
            .collect();
        alternatives.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

        let resolution = self
            .escalation
            .consult(&Impasse {
                goal,
                step,
                because,
                leaning: answers.operation.choice,
                operation_confidence: answers.operation.confidence,
                target_confidence: answers.tap_target.as_ref().map(|chosen| chosen.confidence),
                alternatives: &alternatives,
                rows: &rows,
                operations: catalog.operations(),
                previous,
            })
            .map_err(RunError::Escalation)?;

        Ok(match resolution {
            Resolution::Stop => None,
            Resolution::Choose { operation, target } => catalog.act_from(operation, target).ok(),
        })
    }

    /// Work towards `goal` until the run ends.
    ///
    /// # Errors
    /// Returns [`RunError`] when the device or the judge fails. A run that
    /// merely does not succeed — uncertain, blocked, out of steps — is an
    /// [`Ending`], not an error.
    // The loop reads as one sequence — observe, judge, guard, decide, act —
    // and splitting it further scatters an order that has to be followed.
    #[allow(clippy::too_many_lines)]
    pub fn pursue(&mut self, goal: &str) -> RunResult<D, J, X, C> {
        let mut previous: Option<String> = None;
        // How many actions in a row have left the screen exactly as it was.
        let mut ineffective: u32 = 0;
        // The app the goal is about: whichever one was in front when the run
        // was given it. A run can only tell it has wandered off by comparing
        // against somewhere, and nothing else in a run names an app.
        let mut origin: Option<Box<str>> = self.app.clone();
        // Named rather than discovered: put the run where it was told to be,
        // before anything is judged about where it is.
        if let Some(app) = self.app.clone() {
            self.device
                .perform(&Command::Launch(app))
                .map_err(RunError::Device)?;
            // An app takes seconds to start. Reading straight after the launch
            // returns the screen it was launched from, and the run spends its
            // first judgement deciding what to do about a launcher it has
            // already left.
            self.device
                .perform(&Command::Settle)
                .map_err(RunError::Device)?;
        }
        let launcher = self.device.home_screen_app();
        for index in 1..=self.limit {
            let snapshot = self.device.observe().map_err(RunError::Device)?;
            if origin.is_none()
                && let Some(app) = snapshot.app()
                && launcher.as_deref() != Some(app)
            {
                origin = Some(app.into());
            }
            let mut catalog = Catalog::for_screen(&snapshot, self.platform)
                .returning_to(origin.as_deref());
            if self.types {
                catalog = catalog.accepting_text();
            }
            let questions = StepQuestions::checked(goal, &catalog, &self.criteria);

            let answers = self
                .judge
                .evaluate(
                    describe(
                        &catalog,
                        self.platform,
                        previous.as_deref(),
                        snapshot.app(),
                        origin.as_deref(),
                        snapshot.keyboard_open(),
                    ),
                    &questions,
                )
                .map_err(RunError::Judge)?;

            let decided = catalog.resolve(&answers, &self.floors);

            if answers.is_error_screen.noul > self.certainty {
                self.report(index, &snapshot, &answers, None);
                return Ok(Ending::ErrorScreen);
            }
            // A screen with nothing on it is not evidence. Mid-transition the
            // hierarchy comes back empty, every question is then answered from
            // nothing, and a confident yes is a judgement about an empty room.
            let blind = snapshot.is_empty();

            let reached = answers.goal_met.progress();

            if !blind
                && reached == Progress::Achieved
                && let Some(unmet) = answers.unmet(&self.criteria, self.certainty)
            {
                // Reached something that passes for the goal without being it.
                // Saying so in the next state stops the same wrong thing being
                // chosen again.
                previous = Some(format!(
                    "Judged the goal met, but that was rejected: {unmet:?} was not true"
                ));
                continue;
            }
            if !blind && reached == Progress::Achieved {
                self.report(index, &snapshot, &answers, None);
                return Ok(Ending::Finished(Outcome::Achieved));
            }

            let decision = match decided {
                Ok(decision) => decision,
                Err(because) => {
                    let taken = Taken {
                        goal,
                        step: index,
                        snapshot: &snapshot,
                        catalog: &catalog,
                        answers: &answers,
                        previous: previous.as_deref(),
                    };
                    if let Some(decision) = self.consult(&taken, &because)? {
                        decision
                    } else {
                        self.report(index, &snapshot, &answers, None);
                        return Ok(Ending::Uncertain { because });
                    }
                }
            };

            // Jev chose whether and where to type; it cannot choose what, so
            // the words are asked for here, once a field is settled on.
            let act = match decision {
                Decision::Ready(act) => act,
                Decision::NeedsText { into } => {
                    let rows: Vec<String> = snapshot
                        .refs()
                        .map(|(_, element)| element.describe())
                        .collect();
                    let field = snapshot
                        .resolve(into)
                        .map_err(|stale| RunError::Unreachable(TapError::Stale(stale)))?;
                    let text = self
                        .composer
                        .compose(&Writing {
                            goal,
                            step: index,
                            field,
                            rows: &rows,
                            previous: previous.as_deref(),
                        })
                        .map_err(RunError::Composition)?;
                    Act::TypeText { into, text }
                }
            };

            // Reported once the act is settled, so a step resolved by an
            // escalation is logged with what was actually sent to the device
            // rather than with the refusal that preceded it.
            self.report(index, &snapshot, &answers, Some(&act));

            // A verdict ends the run. It resolves to no command, so without
            // this the loop carries on driving a screen it has just declared
            // finished — and the goal-met guard hides that whenever the two
            // happen to agree.
            if let Act::Finish(outcome) = act {
                match self.refuse_verdict(outcome, blind, &answers) {
                    Some(reason) => {
                        previous = Some(reason);
                        continue;
                    }
                    None => return Ok(Ending::Finished(outcome)),
                }
            }

            previous = Some(recount(&act, &snapshot));
            let resolved = self.reach(
                &act,
                &taken_at(
                    goal,
                    index,
                    &snapshot,
                    &catalog,
                    &answers,
                    previous.as_deref(),
                ),
            )?;
            match resolved {
                Reached::Command(command) => {
                    self.device.perform(&command).map_err(RunError::Device)?;
                    // An action and its effect are not the same instant. Read
                    // straight after acting and the screen is still the one
                    // acted on, so the next judgement is made about the past —
                    // and the same action gets chosen again against what looks
                    // like an unchanged screen. Wait for it to move instead of
                    // for a fixed time, so a quick transition costs a moment
                    // and a slow one is still waited out.
                    let moved = self.settled_on_a_new_screen(snapshot.fingerprint())?;
                    if moved {
                        ineffective = 0;
                    } else {
                        // It never moved. That is worth saying: a run that
                        // does not know its action achieved nothing will
                        // cheerfully do it again.
                        ineffective += 1;
                        previous = Some(format!(
                            "{} — the screen did not change",
                            previous.as_deref().unwrap_or("Acted")
                        ));
                        if ineffective >= Self::INEFFECTIVE_LIMIT {
                            // Already reported for this step, above.
                            return Ok(Ending::Uncertain {
                                because: Indecision::NoProgress {
                                    repeated: ineffective,
                                },
                            });
                        }
                    }
                }
                Reached::Nothing => {}
                Reached::Unreachable => {
                    self.report(index, &snapshot, &answers, None);
                    return Ok(Ending::Uncertain {
                        because: Indecision::Covered,
                    });
                }
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
    catalog: &Catalog,
    platform: &dyn Platform,
    previous: Option<&str>,
    app: Option<&str>,
    origin: Option<&str>,
    keyboard_open: bool,
) -> serde_json::Value {
    // Rows are keyed the way the Choice offers them, so its options can be
    // bare keys and the text travels once rather than twice.
    let keyed = |pairs: &mut dyn Iterator<Item = (crate::judgment::OptionId, &str)>| {
        pairs
            .map(|(id, text)| (id.to_string(), serde_json::Value::from(text)))
            .collect::<serde_json::Map<_, _>>()
    };
    let mut state = serde_json::json!({
        "platform": platform.name(),
        "previous_action": previous,
        // The keyboard covers the bottom of the screen, and what it covers is
        // usually the button that commits the form being typed into. Those
        // rows are absent from the catalog rather than marked unavailable, so
        // without this the screen looks like one that simply has no way on.
        "keyboard_open": keyboard_open,
        "rows": keyed(&mut catalog.rows()),
    });
    if let Some(app) = app {
        state["app"] = app.into();
        // Said only when it differs. On the app the goal is about, repeating
        // the name twice invites the model to reason about a discrepancy that
        // is not there; the absence is the signal that nothing is wrong.
        if origin.is_some_and(|origin| origin != app) {
            state["started_in"] = origin.into();
        }
    }
    let fields = keyed(&mut catalog.fields_offered());
    if !fields.is_empty() {
        state["fields"] = serde_json::Value::Object(fields);
    }
    state
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
        Act::Return(app) => format!("Returned to {app}"),
        Act::Wait => "Waited for the screen to settle".to_owned(),
        Act::Finish(outcome) => format!("Declared {outcome:?}"),
    }
}

/// A command is cloned into the device call, so it must be cloneable.
const _: fn() = || {
    fn assert_clone<T: Clone>() {}
    assert_clone::<Command>();
};
