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
    /// Whether a soft keyboard is covering part of the screen.
    ///
    /// The rows it covers are absent rather than refused, so a screen with no
    /// apparent way on often has one behind the keyboard.
    pub keyboard_open: bool,
    /// Controls the screen shows and will not let anything act on.
    ///
    /// A greyed-out commit button is absent from the rows rather than offered
    /// and refused, so a screen waiting on one input looks like a dead end.
    pub unavailable: &'i [&'i str],
    /// The fields that can be typed into, in the order they are numbered.
    ///
    /// Separate from the rows because typing is numbered separately: a form
    /// of three editable rows among five offers fields 0, 1 and 2, and
    /// answering with a row's number types into the wrong one, or into
    /// nothing.
    pub fields: &'i [String],
    /// What the screen says, beyond what it offers to act on.
    ///
    /// A person deciding an impasse should not be shown less than the model
    /// that could not decide it. On a form whose commit button is switched off
    /// until a picker is used, these words are the only account of why nothing
    /// on the screen looks like a way forward.
    pub says: &'i [&'i str],
    /// The operations this platform offers here.
    pub operations: &'i [Operation],
    /// What the previous step did, if there was one.
    pub previous: Option<&'i str>,
    /// What the run has done lately, oldest first.
    ///
    /// A person asked which key comes next needs the run of keys already
    /// pressed, not just the last of them.
    pub lately: &'i [&'i str],
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
    /// What the screen said, beyond what it offered to act on.
    pub says: Vec<String>,
    /// Controls the screen showed and would not let anything act on.
    pub unavailable: Vec<String>,
    /// How many times in a row the same action had already been taken.
    ///
    /// Recorded so the transcript says what the judge was told, rather than
    /// leaving it to be inferred from the run of identical actions above it.
    pub repeating: Option<u32>,
    /// Which application the screen belonged to, when the reader could say.
    ///
    /// Carried so a run can be explained afterwards. An ending of "blocked"
    /// means something quite different in the app the goal was about than on a
    /// launcher the run wandered onto.
    pub app: Option<&'s str>,
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
    /// How long reading the screen took, in milliseconds.
    ///
    /// Apart from the step's own total because they are spent very
    /// differently: a screen read through an accessibility helper costs tens
    /// of milliseconds and one read through the shell costs thousands, and a
    /// run that has quietly fallen back to the shell looks exactly like a run
    /// that is waiting on a slow judgement.
    pub read_ms: u64,
    /// How long the whole step took, in milliseconds.
    pub step_ms: u64,
    /// How much of that was spent waiting for somebody to answer.
    ///
    /// A step that stops to ask spends most of its wall time on the person
    /// typing, and counting that as the step's cost makes the timings useless
    /// for the one purpose they have. The work is `step_ms` less this.
    pub waited_ms: u64,
    /// How long the judgement took, in milliseconds.
    ///
    /// Usually a network round trip, and usually most of a step. Worth its
    /// own number because nothing can be done about it locally, so a run that
    /// is slow for any other reason is the only kind worth optimising.
    pub judged_ms: u64,
    /// How long acting and waiting for the screen to settle took.
    ///
    /// Zero for a step that touched nothing — a verdict, a refusal, a screen
    /// that could not be reached.
    pub settled_ms: u64,
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
    lately: &'s [&'s str],
) -> Taken<'s> {
    Taken {
        goal,
        step,
        snapshot,
        catalog,
        answers,
        previous,
        lately,
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
    lately: &'s [&'s str],
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

    /// How long a screen must have gone without an accessibility event
    /// before the device's own account of it is taken as settled.
    ///
    /// Short, because it is a real measurement rather than a guess: the
    /// device is reporting silence, not a caller hoping for it.
    pub const QUIET_ENOUGH_MS: u64 = 120;

    /// How many screens back a run remembers having been on.
    pub const MEMORY: usize = 12;

    /// How long a named app is given to put something on screen.
    ///
    /// Longer than a transition's budget: this is a cold start, which loads a
    /// process before it draws anything.
    pub const LAUNCH_BUDGET_MS: u64 = 8_000;

    /// Wait for the screen to become something other than `before`.
    ///
    /// Returns whether it did. The reads are cheap through the helper — about
    /// 60ms — so this costs a fraction of the fixed delay it replaces, and
    /// unlike a fixed delay it is right for both a quick transition and a slow
    /// one.
    /// Wait until `app` is in front with something drawn, or the budget runs
    /// out.
    ///
    /// Not a judgement: nothing about an app that has not come up yet is
    /// worth asking a model about, and the only answer available is to wait.
    ///
    /// Both conditions, because either alone stops too early. A screen with
    /// rows on it is the launcher for the first moment after a launch, and
    /// the app is in front before it has drawn anything.
    fn waited_for_it_to_draw(&mut self, app: &str) -> Result<(), Failure<D, J, X, C>> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(Self::LAUNCH_BUDGET_MS);
        while std::time::Instant::now() < deadline {
            let screen = self.device.observe().map_err(RunError::Device)?;
            // A reader that cannot name the app can still say the screen has
            // something on it, which is the best that can be done there.
            let arrived = screen.app().is_none_or(|seen| seen == app);
            if arrived && screen.worth_acting_on() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(Self::CHANGE_POLL_MS));
        }
        // Out of time. A screen that is still empty is the run's problem to
        // judge, not a reason to refuse to start.
        Ok(())
    }

    fn settled_on_a_new_screen(
        &mut self,
        before: u64,
        attempt: u32,
    ) -> Result<bool, Failure<D, J, X, C>> {
        let budget = Self::CHANGE_BUDGET_MS.saturating_mul(u64::from(attempt));
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(budget);
        let mut last = before;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(Self::CHANGE_POLL_MS));
            let screen = self.device.observe().map_err(RunError::Device)?;
            // When the device can say how long the screen has been quiet,
            // that is the answer rather than one assembled from watching.
            // A screen still being drawn emits accessibility events and a
            // settled one does not, so one reading settles it where watching
            // needs two that agree — and watching cannot tell a screen that
            // has finished from one that is between two others and happens
            // to be still for a moment.
            if screen
                .quiet_for_ms()
                .is_some_and(|quiet| u64::from(quiet) >= Self::QUIET_ENOUGH_MS)
            {
                return Ok(screen.fingerprint() != before);
            }
            let now = screen.fingerprint();
            // A screen that has begun to change has not finished changing. A
            // view being built reports the rows it has so far, and acting on
            // that is acting on a screen that will not exist a moment later —
            // measured on a transfer flow, where only the new screen's Back
            // button had rendered, so going back was the only thing to choose
            // and the run oscillated between two screens.
            //
            // Two readings that agree, then, rather than the first that
            // differs. The screen still has to have moved: unchanged twice
            // over is exactly the action that did nothing.
            //
            // Measured against `before` each time, not against "did it ever
            // differ". A screen that changes and changes back has not
            // changed: a form rejecting what it was given shows the next
            // screen and withdraws it, and treating that as movement resets
            // the guard against standing still — so the button is pressed
            // again on a screen identical to the one it was pressed on.
            if now != before && now == last {
                return Ok(true);
            }
            last = now;
        }
        // Out of time. Whether it moved is whether it ended up somewhere
        // else, not whether it passed through somewhere else.
        Ok(last != before)
    }

    /// Tell the observer, if there is one, what this step saw and did.
    fn report(
        &mut self,
        index: u32,
        snapshot: &Snapshot,
        answers: &StepAnswers,
        chosen: Option<&Act>,
        repeating: Option<u32>,
        spent: Spent,
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
            says: snapshot.notices().map(ToOwned::to_owned).collect(),
            unavailable: snapshot.unavailable().map(ToOwned::to_owned).collect(),
            repeating,
            app: snapshot.app(),
            chosen,
            operation_confidence: answers.operation.confidence,
            target_confidence: answers.tap_target.as_ref().map(|chosen| chosen.confidence),
            goal_met: answers.goal_met.progress(),
            is_error_screen: answers.is_error_screen.noul,
            read_ms: spent.read_ms,
            step_ms: spent.step_ms,
            waited_ms: spent.waited_ms,
            judged_ms: spent.judged_ms,
            settled_ms: spent.settled_ms,
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
        let lately = at.lately;
        let rows: Vec<String> = snapshot
            .refs()
            .map(|(_, element)| element.describe())
            .collect();
        let says: Vec<&str> = snapshot.notices().collect();
        let unavailable: Vec<&str> = snapshot.unavailable().collect();
        let fields: Vec<String> = catalog.fields_offered().map(|(_, at)| at.to_owned()).collect();

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
                keyboard_open: snapshot.keyboard_open(),
                    fields: &fields,
                says: &says,
                unavailable: &unavailable,
                operations: catalog.operations(),
                previous,
                lately,
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
        // What the run has done lately, oldest first. One step of memory is
        // enough to tell an action that worked from one that did not; it is
        // not enough to place yourself in a sequence. Entering a PIN on a pad
        // of identical keys, the position has to be re-derived every step
        // from the count the screen reports, and the derivation gets longer
        // as the sequence goes on — measured, the row confidence fell from
        // 0.95 to 0.14 across six digits while the operation stayed above
        // 0.92 throughout.
        let mut lately: Vec<String> = Vec::new();
        // How many actions in a row have left the screen exactly as it was.
        let mut ineffective: u32 = 0;
        // The app the goal is about: whichever one was in front when the run
        // was given it. A run can only tell it has wandered off by comparing
        // against somewhere, and nothing else in a run names an app.
        // The screens this run has judged, newest last. Two screens can take
        // turns for ever without either one repeating an action against an
        // unchanged screen, so the guard against standing still never fires:
        // tap, half-rendered screen, back, the screen just left, tap again.
        // Knowing it has been here before is what makes waiting the obvious
        // move rather than tapping again.
        let mut visited: Vec<u64> = Vec::new();
        // How many times in a row the same thing has been done. Counted by
        // what the action was, not by whether the screen moved: a form still
        // validating swallows the tap on its commit button and changes
        // anyway — reformatting the amount, filling in the payee it has just
        // looked up — so the guard against standing still never fires while
        // the button is pressed over and over.
        let mut repeating: u32 = 0;
        // The last action on its own, apart from `previous`. They start the
        // same, but `previous` is prose for the judge and gets a note
        // appended when the screen did not move — comparing against that
        // makes an action look new the moment it stops working, which is
        // exactly when the repetition matters.
        let mut last_did: Option<String> = None;
        let mut origin: Option<Box<str>> = self.app.clone();
        // Named rather than discovered: put the run where it was told to be,
        // before anything is judged about where it is.
        if let Some(app) = self.app.clone() {
            self.device
                .perform(&Command::Launch(app.clone()))
                .map_err(RunError::Device)?;
            // An app takes seconds to start, and for most of them it has
            // nothing on screen. Reading straight after the launch returns
            // the screen it was launched from; reading a moment later returns
            // an empty one. Either way the run spends its first judgement —
            // a paid one — deciding to wait, which is not a judgement.
            //
            // Measured: 983ms of empty readings followed by a whole step
            // whose only outcome was `wait`.
            self.waited_for_it_to_draw(app.as_ref())?;
        }
        let launcher = self.device.home_screen_app();
        for index in 1..=self.limit {
            let began = std::time::Instant::now();
            // Time this step spent on somebody else, accumulated as it is
            // asked for.
            let waited = std::cell::Cell::new(0_u64);
            // Filled in as the step goes: zero until the act, since a step
            // that touches nothing settles nothing.
            let mut settled_ms = 0_u64;
            let snapshot = self.device.observe().map_err(RunError::Device)?;
            let read_ms = elapsed_ms(began);
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
            let here = snapshot.fingerprint();
            let seen_before = visited
                .iter()
                .rev()
                .position(|been| *been == here)
                .map(|ago| u32::try_from(ago + 1).unwrap_or(u32::MAX));
            visited.push(here);
            if visited.len() > Self::MEMORY {
                visited.remove(0);
            }

            // Said once it has actually happened twice: doing a thing once is
            // not repeating oneself, and a warning on every step is noise.
            let repeated = (repeating >= 2).then_some(repeating);
            // Copied rather than borrowed: this step's own action joins the
            // history below, and what was shown to the judge is what the
            // escalation and the report must show too.
            let so_far = lately.clone();
            let recent: Vec<&str> = so_far.iter().map(String::as_str).collect();
            let questions = StepQuestions::checked(goal, &catalog, &self.criteria);

            let judging = std::time::Instant::now();
            let answers = self
                .judge
                .evaluate(
                    describe(
                        &catalog,
                        self.platform,
                        &Standing {
                            previous: previous.as_deref(),
                            app: snapshot.app(),
                            origin: origin.as_deref(),
                            keyboard_open: snapshot.keyboard_open(),
                            says: &snapshot.notices().collect::<Vec<_>>(),
                            unavailable: &snapshot.unavailable().collect::<Vec<_>>(),
                            seen_before,
                            repeating: repeated,
                            lately: &recent,
                        },
                    ),
                    &questions,
                )
                .map_err(RunError::Judge);
            let judged_ms = elapsed_ms(judging);
            let answers = answers?;

            let decided = catalog.resolve(&answers, &self.floors);

            if answers.is_error_screen.noul > self.certainty {
                self.report(index, &snapshot, &answers, None, repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
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
                self.report(index, &snapshot, &answers, None, repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
                return Ok(Ending::Finished(Outcome::Achieved));
            }
            // An acceptance criterion is the caller's own definition of done,
            // and a progress score is the model's guess at it. Until this, the
            // criteria could only veto a success, never declare one — so a run
            // standing on a finished screen the model under-rated carried on,
            // off that screen and back through the flow it had just completed.
            //
            // Measured driving a bank transfer: the receipt was on screen,
            // `goal_met` said "under way", and the run pressed Back and began
            // a second transfer. The criteria were answered on that very step
            // and nothing looked at them.
            //
            // Every criterion, or none of it counts: a run asked for two
            // things and shown one of them is not finished. With no criteria
            // given there is nothing to be satisfied by, and the progress
            // score above remains the only word on it.
            if !blind
                && !self.criteria.is_empty()
                && answers.unmet(&self.criteria, self.certainty).is_none()
            {
                self.report(index, &snapshot, &answers, None, repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
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
                        lately: &recent,
                    };
                    let asking = std::time::Instant::now();
                    let answered = self.consult(&taken, &because);
                    waited.set(waited.get().saturating_add(elapsed_ms(asking)));
                    if let Some(decision) = answered? {
                        decision
                    } else {
                        self.report(index, &snapshot, &answers, None, repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
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
                    let asking = std::time::Instant::now();
                    let text = self
                        .composer
                        .compose(&Writing {
                            goal,
                            step: index,
                            field,
                            rows: &rows,
                            previous: previous.as_deref(),
                        })
                        .map_err(RunError::Composition);
                    waited.set(waited.get().saturating_add(elapsed_ms(asking)));
                    let text = text?;
                    Act::TypeText { into, text }
                }
            };

            // A verdict ends the run. It resolves to no command, so without
            // this the loop carries on driving a screen it has just declared
            // finished — and the goal-met guard hides that whenever the two
            // happen to agree.
            if let Act::Finish(outcome) = act {
                // Reported here rather than with the acting steps below: a
                // verdict touches nothing, so there is no settle to wait for
                // and nothing to be learned by reporting it later.
                self.report(index, &snapshot, &answers, Some(&act), repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
                match self.refuse_verdict(outcome, blind, &answers) {
                    Some(reason) => {
                        previous = Some(reason);
                        continue;
                    }
                    None => return Ok(Ending::Finished(outcome)),
                }
            }

            let did = recount(&act, &snapshot);
            repeating = if last_did.as_deref() == Some(did.as_str()) {
                repeating.saturating_add(1)
            } else {
                1
            };
            last_did = Some(did.clone());
            lately.push(did.clone());
            if lately.len() > Self::MEMORY {
                lately.remove(0);
            }
            previous = Some(did);
            let resolved = self.reach(
                &act,
                &taken_at(
                    goal,
                    index,
                    &snapshot,
                    &catalog,
                    &answers,
                    previous.as_deref(),
                    &recent,
                ),
            )?;
            match resolved {
                Reached::Command(command) => {
                    let acting = std::time::Instant::now();
                    self.device.perform(&command).map_err(RunError::Device)?;
                    // An action and its effect are not the same instant. Read
                    // straight after acting and the screen is still the one
                    // acted on, so the next judgement is made about the past —
                    // and the same action gets chosen again against what looks
                    // like an unchanged screen. Wait for it to move instead of
                    // for a fixed time, so a quick transition costs a moment
                    // and a slow one is still waited out.
                    // Patience grows with each action that changed nothing.
                    // A button backed by a network call shows no change and
                    // emits no events while the call is in flight, so every
                    // signal available agrees — correctly — that the screen
                    // has not moved, and the only thing separating "not yet"
                    // from "never" is how long the run is willing to wait.
                    //
                    // Measured: Continue tapped five times on a transfer
                    // form, the run stopped for want of progress, and the
                    // summary appeared afterwards. Costs nothing when
                    // actions work, because then this is always the first
                    // attempt.
                    let moved = self
                        .settled_on_a_new_screen(snapshot.fingerprint(), ineffective + 1)?;
                    settled_ms = elapsed_ms(acting);
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
                            self.report(index, &snapshot, &answers, Some(&act), repeated, Spent {
                                read_ms,
                                step_ms: elapsed_ms(began),
                                waited_ms: waited.get(),
                                judged_ms,
                                settled_ms,
                            });
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
                    self.report(index, &snapshot, &answers, None, repeated, Spent {
                    read_ms,
                    step_ms: elapsed_ms(began),
                    waited_ms: waited.get(),
                    judged_ms,
                    settled_ms,
                });
                    return Ok(Ending::Uncertain {
                        because: Indecision::Covered,
                    });
                }
            }

            // Reported once the act has been carried out and the screen has
            // settled, so the step's own cost includes both. A step resolved
            // by an escalation is logged with what was actually sent to the
            // device rather than with the refusal that preceded it.
            self.report(index, &snapshot, &answers, Some(&act), repeated, Spent {
                read_ms,
                step_ms: elapsed_ms(began),
                waited_ms: waited.get(),
                judged_ms,
                settled_ms,
            });
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
struct Standing<'s> {
    /// What the previous step did, if there was one.
    previous: Option<&'s str>,
    /// Which application this screen belongs to.
    app: Option<&'s str>,
    /// The application the goal is about, when it is a different one.
    origin: Option<&'s str>,
    /// Whether the soft keyboard is covering part of the screen.
    keyboard_open: bool,
    /// What the screen says, beyond what it offers to act on.
    says: &'s [&'s str],
    /// Controls the screen shows and will not let anything act on.
    unavailable: &'s [&'s str],
    /// How many steps ago this screen was last judged, if it was.
    seen_before: Option<u32>,
    /// How many times in a row the same action has already been taken.
    repeating: Option<u32>,
    /// What the run has done lately, oldest first.
    lately: &'s [&'s str],
}

fn describe(
    catalog: &Catalog,
    platform: &dyn Platform,
    where_it_stands: &Standing<'_>,
) -> serde_json::Value {
    let &Standing {
        previous,
        app,
        origin,
        keyboard_open,
        says,
        unavailable,
        seen_before,
        repeating,
        lately,
    } = where_it_stands;
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
    if !lately.is_empty() {
        // Oldest first, so the run of them reads as a sequence rather than
        // needing to be reversed to be understood.
        state["recent_actions"] = lately.into();
    }
    if let Some(times) = repeating {
        // Said before the action is chosen, so the choice can be a different
        // one. A button pressed while a form is still validating is ignored,
        // and a button pressed once the form is ready is not — so doing it
        // again is sometimes right and is never right without noticing.
        state["repeating"] = times.into();
    }
    if let Some(ago) = seen_before {
        // How many steps back, rather than a bare flag: a screen returned to
        // after two steps is a loop, and one returned to after ten is a form
        // that was worked through and come back to.
        state["seen_before"] = ago.into();
    }
    if !unavailable.is_empty() {
        // Named apart from the rows and from the words. A greyed-out commit
        // button is not something the screen says, it is the way on waiting
        // for something — and a screen that appears to have no way on is one
        // a run starts trying to leave.
        state["unavailable"] = unavailable.into();
    }
    if !says.is_empty() {
        // Apart from the rows, and unkeyed: none of it can be chosen, and a
        // key is an invitation to try. It is here to be read, not picked.
        state["screen_says"] = says.into();
    }
    let fields = keyed(&mut catalog.fields_offered());
    if !fields.is_empty() {
        state["fields"] = serde_json::Value::Object(fields);
    }
    state
}

/// Where a step's time went.
///
/// The `_ms` on every field is the unit and is kept: these are handed
/// straight to [`StepReport`], whose fields are public and where a bare
/// `read` would not say what it counts.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_field_names)]
struct Spent {
    read_ms: u64,
    step_ms: u64,
    waited_ms: u64,
    judged_ms: u64,
    settled_ms: u64,
}

/// Milliseconds since an instant, saturated rather than wrapped.
fn elapsed_ms(since: std::time::Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
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
