//! What the worker may do to the screen it is looking at.
//!
//! [`Act`] is a sum type, so a tap without a target, or typed text with no
//! field to receive it, is not a runtime check that might be forgotten — it
//! does not compile. Every variant that touches an element carries an
//! [`ElementRef`], binding it to the snapshot it was chosen from.

use crate::judgment::{Confidence, OptionId, Options, Question};
use crate::platform::Platform;
use crate::snapshot::{ElementRef, Snapshot};
use core::fmt;
use serde::Serialize;

/// A gesture aimed at the system rather than at anything on screen.
///
/// Which of these exist is a property of the platform, not of the screen: see
/// [`Platform::operations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemAct {
    /// Return to the previous screen. Android only.
    Back,
    /// Leave the app for the home screen.
    Home,
    /// Show the list of running apps.
    AppSwitcher,
    /// Commit what has been typed, as pressing enter or the keyboard's search
    /// key. Without it, text can be composed into a field and never acted on.
    Submit,
}

impl SystemAct {
    /// How this gesture is described to a model choosing among actions.
    #[must_use]
    pub const fn rubric(self) -> &'static str {
        match self {
            Self::Back => "Go back to the previous screen",
            Self::Home => "Leave the app and return to the home screen",
            Self::AppSwitcher => "Open the list of running apps",
            Self::Submit => "Submit what has been typed",
        }
    }
}

/// Which way a row is dragged.
///
/// Separate from [`Direction`] on purpose: scrolling moves the view vertically
/// and swiping moves a row horizontally, and a single four-way enum would make
/// "scroll a row left" expressible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Swipe {
    /// Towards the leading edge, revealing trailing actions.
    Left,
    /// Towards the trailing edge, revealing leading actions.
    Right,
}

/// Which way a scroll goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Back towards the start of the list.
    Up,
    /// Onward, revealing what lies past the fold.
    Down,
}

/// What to do, as opposed to what to do it to.
///
/// Asking this separately from the target is what keeps the two dimensions
/// from multiplying. A flat list of every operation crossed with every element
/// grows as their product, and can express pairings that make no sense — a
/// scroll aimed at a button, a tap aimed at nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    /// Tap an element. Needs a tap target.
    Tap,
    /// Type into a field.
    ///
    /// Offered only when the screen has a field and the caller can supply the
    /// words. A System One model selects; it does not write, so the text comes
    /// from elsewhere — see [`crate::pilot::Compose`].
    TypeText,
    /// Tap an element twice in quick succession. Needs a tap target.
    DoubleTap,
    /// Press and hold an element, which usually opens a context menu.
    /// Needs a tap target.
    LongPress,
    /// Drag an element to the left, as in swipe-to-delete. Needs a tap target.
    SwipeLeft,
    /// Drag an element to the right, as in swipe-to-archive. Needs a tap target.
    SwipeRight,
    /// Press firmly to preview an element without opening it.
    ///
    /// iOS only: there is no Android equivalent, so [`crate::platform::Android`]
    /// does not offer it and it can never be chosen there.
    Peek,
    /// Reveal what lies above the current view.
    ScrollUp,
    /// Reveal what lies below the current view.
    ///
    /// Not a convenience: a virtualised list only materialises the rows it is
    /// showing, so anything past the fold is genuinely absent from the screen
    /// as observed, and unreachable without this.
    ScrollDown,
    /// Return to the previous screen.
    Back,
    /// Leave for the home screen.
    Home,
    /// Put the soft keyboard away.
    ///
    /// Offered only on a screen that has one up. A keyboard covers the bottom
    /// of the screen, and the rows under it are absent from the catalog rather
    /// than offered and refused — so nothing else on the screen suggests that
    /// putting it away would bring a row back.
    CloseKeyboard,
    /// Go back into the app this goal is about.
    ///
    /// Offered only on a screen belonging to some other app, so it is never a
    /// way of standing still. See [`Catalog::returning_to`].
    Return,
    /// Show the running apps.
    AppSwitcher,
    /// Commit what has been typed into the focused field.
    Submit,
    /// Let the screen settle and look again.
    Wait,
    /// Stop: the goal is satisfied.
    Done,
    /// Stop: the goal cannot be reached from here.
    Blocked,
}

impl Operation {
    /// Every operation there is, in the order they are listed.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        ALL
    }

    /// The operation offered under this name, if there is one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        ALL.iter().copied().find(|candidate| candidate.key() == key)
    }

    /// The name this operation is offered and answered under.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Tap => "tap",
            Self::TypeText => "type_text",
            Self::DoubleTap => "double_tap",
            Self::LongPress => "long_press",
            Self::SwipeLeft => "swipe_left",
            Self::SwipeRight => "swipe_right",
            Self::Peek => "peek",
            Self::ScrollUp => "scroll_up",
            Self::ScrollDown => "scroll_down",
            Self::Back => "back",
            Self::Home => "home",
            Self::CloseKeyboard => "close_keyboard",
            Self::Return => "return_to_app",
            Self::AppSwitcher => "app_switcher",
            Self::Submit => "submit",
            Self::Wait => "wait",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
    }

    /// How this operation is described to a model choosing among them.
    #[must_use]
    pub const fn rubric(self) -> &'static str {
        match self {
            Self::Tap => "Tap one of the rows on screen",
            Self::TypeText => "Type into one of the fields on screen",
            Self::DoubleTap => "Tap a row twice quickly, as for zooming or selecting a word",
            Self::LongPress => "Press and hold a row to open its context menu or selection options",
            Self::SwipeLeft => {
                "Drag a row to the left to reveal its trailing actions, such as delete"
            }
            Self::SwipeRight => {
                "Drag a row to the right to reveal its leading actions, such as archive"
            }
            Self::Peek => "Press firmly on a row to preview it without leaving this screen",
            Self::ScrollUp => "Scroll up to reveal rows above the current view",
            Self::ScrollDown => "Scroll down to reveal rows below the current view",
            Self::Back => "Go back to the previous screen",
            Self::Home => "Leave the app and return to the home screen",
            Self::CloseKeyboard => {
                "Put the keyboard away, revealing the rows it covers — often the button that \
                 commits the form being typed into"
            }
            Self::Return => {
                "Go back into the app this goal is about — this screen belongs to another app"
            }
            Self::AppSwitcher => "Open the list of running apps",
            Self::Submit => "Submit what has been typed, as pressing enter or search",
            Self::Wait => "Wait for the screen to finish loading, then look again",
            Self::Done => "Stop: the goal is satisfied on this screen",
            Self::Blocked => "Stop: the goal cannot be reached from this screen",
        }
    }

    /// Whether this operation needs a row to act on.
    /// What it costs to perform this operation in error.
    #[must_use]
    pub const fn consequence(self) -> Consequence {
        match self {
            Self::Done | Self::Blocked => Consequence::Terminal,
            // Swipe-to-delete and swipe-to-archive are the whole point of these
            // gestures; going back does not bring the row back.
            //
            // Home belongs with them, for the same reason rather than an
            // obvious one: it does not merely move the view, it discards the
            // app's navigation stack, and an app holding a session tears that
            // down too. Coming back lands on a different screen than the one
            // left, and everything typed on the way there is gone.
            Self::SwipeLeft | Self::SwipeRight | Self::Home => Consequence::Destructive,
            _ => Consequence::Ordinary,
        }
    }

    /// Whether this operation needs a field to type into.
    #[must_use]
    pub const fn needs_field(self) -> bool {
        matches!(self, Self::TypeText)
    }

    /// Whether this operation needs a row to act on.
    #[must_use]
    pub const fn needs_tap_target(self) -> bool {
        matches!(
            self,
            Self::Tap
                | Self::DoubleTap
                | Self::LongPress
                | Self::SwipeLeft
                | Self::SwipeRight
                | Self::Peek
        )
    }
}

impl core::fmt::Display for Operation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.key())
    }
}

impl serde::Serialize for Operation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

/// Every operation there is.
///
/// One list, because a second one kept by hand goes stale the moment an
/// operation is added — and an operation that cannot be named back is one a
/// caller can be offered and then not allowed to choose.
const ALL: &[Operation] = &[
    Operation::Tap,
    Operation::TypeText,
    Operation::DoubleTap,
    Operation::LongPress,
    Operation::SwipeLeft,
    Operation::SwipeRight,
    Operation::Peek,
    Operation::ScrollUp,
    Operation::ScrollDown,
    Operation::Back,
    Operation::Home,
    Operation::CloseKeyboard,
    Operation::Return,
    Operation::AppSwitcher,
    Operation::Submit,
    Operation::Wait,
    Operation::Done,
    Operation::Blocked,
];

impl<'de> serde::Deserialize<'de> for Operation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Self::from_key(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("not an operation: {raw}")))
    }
}

/// How a task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The goal is visibly satisfied on this screen.
    Achieved,
    /// The goal cannot be reached from here.
    Blocked,
}

impl Outcome {
    /// How this ending is described to a model choosing among actions.
    #[must_use]
    pub const fn rubric(self) -> &'static str {
        match self {
            Self::Achieved => "Stop: the goal is already satisfied on this screen",
            Self::Blocked => "Stop: the goal cannot be reached from this screen",
        }
    }
}

/// One technically valid thing the worker can do.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Act {
    /// Tap an element.
    Tap(ElementRef),
    /// Tap an element twice in quick succession.
    DoubleTap(ElementRef),
    /// Press and hold an element.
    LongPress(ElementRef),
    /// Drag an element sideways.
    SwipeElement {
        /// The row being dragged.
        target: ElementRef,
        /// Which way it goes.
        direction: Swipe,
    },
    /// Preview an element without opening it.
    Peek(ElementRef),
    /// Type into an element that accepts text.
    ///
    /// Offered only when [`Catalog::accepting_text`] was called and the screen
    /// has a field. A System One model selects rather than generates, so the
    /// words come from [`crate::pilot::Compose`] — a reasoning model, or a
    /// person — after Jev has chosen whether and where to type.
    TypeText {
        /// The field receiving the text.
        into: ElementRef,
        /// What to type.
        text: Box<str>,
    },
    /// Perform a system gesture.
    System(SystemAct),
    /// Put the soft keyboard away.
    ///
    /// The same gesture as [`SystemAct::Back`] on Android and not the same
    /// act. Told it "pressed Back", a run's memory says it navigated when it
    /// did not, and the next step goes back again — off the form it was
    /// filling. Measured on a transfer form, three laps of one run.
    CloseKeyboard,
    /// Bring the named application back to the foreground.
    ///
    /// Carries the identifier rather than reading it from the screen: the
    /// screen the run is looking at is precisely the one it does not want.
    Return(Box<str>),
    /// Scroll the screen.
    Scroll(Direction),
    /// Let the screen settle and observe again.
    Wait,
    /// Stop, with a verdict.
    Finish(Outcome),
}

/// The instructions attached to an action choice.
#[derive(Debug, Clone, Serialize)]
pub struct Deciding<'a> {
    /// What the worker is trying to achieve.
    pub goal: Aim<'a>,
    /// The judgment being asked.
    pub question: &'static str,
    /// Where in the state the answer is to be found, when code already knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// What a step is working towards: the goal as written, or the steps it was
/// broken into.
///
/// Sent as the sentence or as the list, under the same name. A sentence of
/// several steps asks the model to work out on every screen which of them the
/// screen is for; a list asked about "the next unfinished step" leaves it only
/// to find its place. Measured on a transfer form, the second read the account
/// lookup's Confirm at 0.91 where the first read it at 0.16.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(untagged)]
pub enum Aim<'a> {
    /// One sentence saying what to achieve.
    Goal(&'a str),
    /// The steps to take, in the order the app asks for them.
    Plan(&'a [Box<str>]),
}

impl<'a> From<&'a str> for Aim<'a> {
    fn from(goal: &'a str) -> Self {
        Self::Goal(goal)
    }
}

impl Aim<'_> {
    /// Pick the wording of a question for this kind of aim.
    const fn asking(self, for_goal: &'static str, for_plan: &'static str) -> &'static str {
        match self {
            Self::Goal(_) => for_goal,
            Self::Plan(_) => for_plan,
        }
    }
}

/// What it costs to get an action wrong.
///
/// The documented guidance is that a threshold is not one number: different
/// actions in the same system are gated differently according to what a wrong
/// one costs. A misplaced tap on a list row is undone by going back; a wrong
/// verdict ends the run with the wrong answer, and nobody finds out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Consequence {
    /// Recoverable by going back or looking again.
    Ordinary,
    /// Ends the run, right or wrong.
    Terminal,
    /// May remove or send something, and going back does not undo it.
    Destructive,
}

/// How certain the model must be, per kind of consequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floors {
    ordinary: Confidence,
    terminal: Confidence,
    destructive: Confidence,
}

impl Floors {
    /// One floor for every action, whatever it costs.
    #[must_use]
    pub const fn new(everywhere: Confidence) -> Self {
        Self {
            ordinary: everywhere,
            terminal: everywhere,
            destructive: everywhere,
        }
    }

    /// Require more certainty for actions of this kind.
    #[must_use]
    pub const fn requiring_for(mut self, consequence: Consequence, floor: Confidence) -> Self {
        match consequence {
            Consequence::Ordinary => self.ordinary = floor,
            Consequence::Terminal => self.terminal = floor,
            Consequence::Destructive => self.destructive = floor,
        }
        self
    }

    /// The floor an operation must clear.
    #[must_use]
    pub const fn for_operation(&self, operation: Operation) -> Confidence {
        match operation.consequence() {
            Consequence::Ordinary => self.ordinary,
            Consequence::Terminal => self.terminal,
            Consequence::Destructive => self.destructive,
        }
    }

    /// The floor a target choice must clear, given the operation it serves.
    ///
    /// The same as the operation's: naming the wrong row for a destructive
    /// gesture is as costly as choosing the gesture itself.
    #[must_use]
    pub const fn for_target(&self, operation: Operation) -> Confidence {
        self.for_operation(operation)
    }
}

/// What a step's answers resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// Ready to carry out as it stands.
    Ready(Act),
    /// A field was chosen, but the words have not been written yet.
    ///
    /// Kept separate rather than handing back a half-filled `Act::TypeText`,
    /// so text that was never composed is not a state the type can hold.
    NeedsText {
        /// The field to type into.
        into: ElementRef,
    },
}

/// Why a choice did not become an action.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Indecision {
    /// Nothing stood out clearly enough to act on.
    ///
    /// The floor is supplied per call rather than fixed here: the right value
    /// depends on the app and on what a wrong tap costs, so it has to be
    /// calibrated against real runs rather than inherited from an example.
    TooUncertain {
        /// The confidence the model reported.
        got: Confidence,
        /// The floor the caller required.
        floor: Confidence,
    },
    /// The same action kept leaving the screen exactly as it was.
    ///
    /// Confidence says nothing about effect: a run can choose the right-looking
    /// row at 0.95 and achieve nothing, then choose it again because the screen
    /// it is judging is the one it already acted on. Seen on a real form —
    /// twelve identical taps at 0.90 to 0.97, each a live request to a banking
    /// API, none of them progress.
    NoProgress {
        /// How many actions in a row changed nothing.
        repeated: u32,
    },
    /// The model named something this screen never offered.
    NotOffered {
        /// What was named.
        what: Box<str>,
    },
    /// The operation needs a row to act on and none was chosen.
    NoTarget,
    /// The row chosen is covered by whatever is drawn over it.
    Covered,
}

impl fmt::Display for Indecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooUncertain { got, floor } => write!(
                f,
                "confidence {} is below the floor of {}",
                got.get(),
                floor.get()
            ),
            Self::NotOffered { what } => write!(f, "{what} was not offered"),
            Self::NoProgress { repeated } => write!(
                f,
                "{repeated} actions in a row left the screen exactly as it was"
            ),
            Self::NoTarget => write!(f, "the operation needs a target and none was chosen"),
            Self::Covered => write!(f, "the row chosen is covered by what is drawn over it"),
        }
    }
}

impl core::error::Error for Indecision {}

/// The actions available on one screen, as two independent heads.
///
/// `operation` says what to do; `tap_target` says which row to do it to. They
/// are answered in the same request, speculatively: the target head is filled
/// in whether or not the chosen operation turns out to need it, and discarded
/// when it does not. That costs nothing extra — the endpoint evaluates a
/// question map in parallel — and saves a second round trip when it does.
#[derive(Debug)]
pub struct Catalog {
    app: Option<Box<str>>,
    return_to: Option<Box<str>>,
    supported: Vec<Operation>,
    operations: Options,
    targets: Vec<(OptionId, ElementRef, Box<str>)>,
    target_options: Options,
    /// Every row on screen, in screen order, which is how an escalation
    /// numbers them. Not the same list as `targets`, which leaves out rows
    /// the screen names too often to tell apart.
    on_screen: Vec<ElementRef>,
    fields: Vec<(OptionId, ElementRef, Box<str>)>,
    field_options: Options,
}

impl Catalog {
    /// Enumerate what the worker may do to `snapshot` on `platform`.
    #[must_use]
    pub fn for_screen(snapshot: &Snapshot, platform: &dyn Platform) -> Self {
        let mut operations = Options::default();
        let mut supported = Vec::new();
        for operation in platform.operations() {
            // Nothing to put away, and offering it would be a way to stand
            // still on every screen that has no keyboard.
            if *operation == Operation::CloseKeyboard && !snapshot.keyboard_open() {
                continue;
            }
            // The two resolve to the same gesture, so offering both asks the
            // model to choose between two spellings of one action and splits
            // its confidence across them. While there is a keyboard, the one
            // on offer is the one that says what it does to the keyboard.
            if *operation == Operation::Back && snapshot.keyboard_open() {
                continue;
            }
            // A tap with nothing to tap is not an option worth offering.
            if operation.needs_tap_target() && snapshot.is_empty() {
                continue;
            }
            if operations
                .push_named(operation.key(), operation.rubric())
                .is_ok()
            {
                supported.push(*operation);
            }
        }

        let mut target_options = Options::default();
        let mut targets = Vec::new();
        let mut field_options = Options::default();
        let mut fields = Vec::new();
        let mut named = std::collections::HashMap::<String, usize>::new();
        for (_, element) in snapshot.refs() {
            *named
                .entry(element.describe().trim().to_lowercase())
                .or_default() += 1;
        }
        let on_screen = snapshot.refs().map(|(handle, _)| handle).collect();
        for (handle, element) in snapshot.refs() {
            let text = element.describe();
            // Rows named alike this often cannot be told apart by what they
            // say, and each takes a share of the choice from the rows that
            // can. Measured on a Google results page: twenty "About this
            // result" rows among 125, and the wanted result chosen at 0.2 to
            // 0.39 every time.
            let indistinct = named
                .get(&text.trim().to_lowercase())
                .is_some_and(|&count| count > Self::REPEATS_TOLERATED);
            if !indistinct && let Ok(id) = target_options.push_described(text.as_str()) {
                targets.push((id, handle, text.clone().into_boxed_str()));
            }
            if element.editable
                && let Ok(id) = field_options.push_described(text.as_str())
            {
                fields.push((id, handle, text.clone().into_boxed_str()));
            }
        }

        Self {
            app: snapshot.app().map(Box::from),
            return_to: None,
            supported,
            operations,
            targets,
            target_options,
            on_screen,
            fields,
            field_options,
        }
    }

    /// How many rows may share a name and still be offered to choose from.
    ///
    /// A pair, or a handful, is a dialog saying `Close` twice or a short list
    /// of alike buttons, and [`Self::resolve`] already counts their shares
    /// together. Past this it is page furniture repeated under every item.
    pub const REPEATS_TOLERATED: usize = 3;

    /// Also offer typing, when this screen has somewhere to type.
    ///
    /// Left off by default: without something able to write the words, typing
    /// is an operation the model can choose and nothing can carry out.
    #[must_use]
    pub fn accepting_text(mut self) -> Self {
        if !self.fields.is_empty()
            && self
                .operations
                .push_named(Operation::TypeText.key(), Operation::TypeText.rubric())
                .is_ok()
        {
            self.supported.push(Operation::TypeText);
        }
        self
    }

    /// Also offer going back into `app`, when this screen belongs to another.
    ///
    /// A goal is given about one application, and nothing on a screen says
    /// which — a launcher, a browser opened by a link and the app itself all
    /// present rows that read as equally legitimate. Naming the app the run
    /// started in turns "I am somewhere else" into a fact the judge is told
    /// and a single action that fixes it.
    ///
    /// Passing `None`, or the app this screen already belongs to, changes
    /// nothing: returning to where you are is not an action, and offering it
    /// would give the model a way to stand still.
    #[must_use]
    pub fn returning_to(mut self, app: Option<&str>) -> Self {
        let Some(app) = app else { return self };
        if self.app.as_deref() == Some(app) {
            return self;
        }
        if self
            .operations
            .push_named(Operation::Return.key(), Operation::Return.rubric())
            .is_ok()
        {
            self.supported.push(Operation::Return);
            self.return_to = Some(app.into());
        }
        self
    }

    /// The Choice asking which field to type into.
    #[must_use]
    pub fn type_field_question<'a>(
        &self,
        goal: impl Into<Aim<'a>>,
    ) -> Option<Question<Deciding<'a>>> {
        if self.fields.is_empty() {
            return None;
        }
        let goal = goal.into();
        Some(Question::Choice {
            instructions: Deciding {
                goal,
                question: goal.asking(
                    "Which field on the current screen should be typed into to advance \
                     `goal`, assuming the chosen operation is typing?",
                    "Which field on the current screen should be typed into to advance the \
                     next unfinished step of `goal`, assuming the chosen operation is typing?",
                ),
                note: None,
            },
            criteria: self.field_options.clone(),
        })
    }

    /// The fields on this screen that accept text.
    #[must_use]
    pub fn fields(&self) -> usize {
        self.fields.len()
    }

    /// The action an operation that needs no row stands for.
    fn untargeted(operation: Operation) -> Result<Act, Indecision> {
        Ok(match operation {
            Operation::ScrollUp => Act::Scroll(Direction::Up),
            Operation::ScrollDown => Act::Scroll(Direction::Down),
            Operation::Back => Act::System(SystemAct::Back),
            Operation::CloseKeyboard => Act::CloseKeyboard,
            Operation::Home => Act::System(SystemAct::Home),
            Operation::AppSwitcher => Act::System(SystemAct::AppSwitcher),
            Operation::Submit => Act::System(SystemAct::Submit),
            Operation::Wait => Act::Wait,
            Operation::Done => Act::Finish(Outcome::Achieved),
            Operation::Blocked => Act::Finish(Outcome::Blocked),
            // Reached only if a targeted operation is routed here, which the
            // callers prevent; refusing beats silently doing something else.
            _ => return Err(Indecision::NoTarget),
        })
    }

    /// The action an operation aimed at a row stands for.
    ///
    /// Spelled out rather than closed with a wildcard. `Operation` is
    /// `#[non_exhaustive]`, and a catch-all here would quietly turn a gesture
    /// added later into a tap — the same silent-misfire this crate refuses for
    /// [`crate::device::Command`], and worse, because it would act on the
    /// user's screen rather than merely do nothing.
    fn targeted(operation: Operation, target: ElementRef) -> Result<Act, Indecision> {
        Ok(match operation {
            Operation::Tap => Act::Tap(target),
            Operation::DoubleTap => Act::DoubleTap(target),
            Operation::LongPress => Act::LongPress(target),
            Operation::SwipeLeft => Act::SwipeElement {
                target,
                direction: Swipe::Left,
            },
            Operation::SwipeRight => Act::SwipeElement {
                target,
                direction: Swipe::Right,
            },
            Operation::Peek => Act::Peek(target),
            // Typing is routed through `NeedsText`, and every other operation
            // needs no row. Refusing beats acting on a guess.
            _ => return Err(Indecision::NoTarget),
        })
    }

    /// The Choice asking which operation advances `goal`.
    #[must_use]
    pub fn operation_question<'a>(&self, goal: impl Into<Aim<'a>>) -> Question<Deciding<'a>> {
        let goal = goal.into();
        Question::Choice {
            instructions: Deciding {
                goal,
                question: goal.asking(
                    "Which single operation best advances `goal` from the current screen?",
                    "Which single operation best advances the next unfinished step of `goal` \
                     from the current screen?",
                ),
                note: None,
            },
            criteria: self.operations.clone(),
        }
    }

    /// The Choice asking which row to act on, when there is one.
    ///
    /// Carries the goal. This head is answered in parallel with the operation
    /// and so cannot see which operation won, but without the goal the question
    /// degrades into "which row looks important?" and the answer spreads across
    /// every plausible row rather than concentrating on the one that advances
    /// the task.
    #[must_use]
    pub fn tap_target_question<'a>(
        &self,
        goal: impl Into<Aim<'a>>,
    ) -> Option<Question<Deciding<'a>>> {
        if self.targets.is_empty() {
            return None;
        }
        let goal = goal.into();
        Some(Question::Choice {
            instructions: Deciding {
                goal,
                question: goal.asking(
                    "Which single row on the current screen should be acted on to advance \
                     `goal`, assuming the chosen operation needs a row?",
                    "Which single row on the current screen should be acted on to advance the \
                     next unfinished step of `goal`, assuming the chosen operation needs a row?",
                ),
                note: None,
            },
            criteria: self.target_options.clone(),
        })
    }

    /// Turn a step's answers into the one action to carry out.
    ///
    /// Both heads must clear the floor for the chosen operation. A confident
    /// operation aimed at a target the model was unsure of is still a guess
    /// about where to tap, and the floor scales with what a wrong one costs —
    /// see [`Floors`].
    ///
    /// # Errors
    /// Returns [`Indecision`] when either head was too uncertain, when the
    /// operation is unsupported here, or when a target is needed and missing.
    pub fn resolve(
        &self,
        answers: &crate::step::StepAnswers,
        floors: &Floors,
    ) -> Result<Decision, Indecision> {
        let operation = answers.operation.choice;
        let floor = floors.for_operation(operation);
        if answers.operation.confidence < floor {
            return Err(Indecision::TooUncertain {
                got: answers.operation.confidence,
                floor,
            });
        }
        if !self.supported.contains(&operation) {
            return Err(Indecision::NotOffered {
                what: operation.key().into(),
            });
        }

        if operation.needs_field() {
            let chosen = answers.type_field.as_ref().ok_or(Indecision::NoTarget)?;
            let floor = floors.for_target(operation);
            if chosen.confidence < floor {
                return Err(Indecision::TooUncertain {
                    got: chosen.confidence,
                    floor,
                });
            }
            let into = self
                .fields
                .iter()
                .find(|(id, ..)| *id == chosen.choice)
                .map(|(_, handle, _)| *handle)
                .ok_or_else(|| Indecision::NotOffered {
                    what: chosen.choice.to_string().into_boxed_str(),
                })?;
            return Ok(Decision::NeedsText { into });
        }

        if operation == Operation::Return {
            let app = self.return_to.clone().ok_or(Indecision::NoTarget)?;
            return Ok(Decision::Ready(Act::Return(app)));
        }
        if !operation.needs_tap_target() {
            return Self::untargeted(operation).map(Decision::Ready);
        }

        let chosen = answers.tap_target.as_ref().ok_or(Indecision::NoTarget)?;
        let floor = floors.for_target(operation);
        if chosen.confidence < floor && self.named_once(chosen) < floor.get() {
            return Err(Indecision::TooUncertain {
                got: chosen.confidence,
                floor,
            });
        }
        let target = self
            .targets
            .iter()
            .find(|(id, ..)| *id == chosen.choice)
            .map(|(_, handle, _)| *handle)
            .ok_or_else(|| Indecision::NotOffered {
                what: chosen.choice.to_string().into_boxed_str(),
            })?;

        Self::targeted(operation, target).map(Decision::Ready)
    }

    /// Build an action from an operation and a row position.
    ///
    /// The path an escalation takes. A second opinion — a reasoning model, or a
    /// person — chooses from the same enumerated set Jev was offered and is
    /// resolved through the same catalog, so it inherits every constraint:
    /// it cannot name an operation this platform lacks, nor a row that is not
    /// on screen, nor a coordinate at all.
    ///
    /// Rows are numbered as the screen lists them, every one of them, which
    /// includes the repeated rows left out of Jev's own choice: the second
    /// opinion is shown them all and may have a reason to name one.
    ///
    /// # Errors
    /// Returns [`Indecision`] when the operation is not offered here, or when
    /// it needs a row and the one named does not exist.
    pub fn act_from(
        &self,
        operation: Operation,
        target: Option<usize>,
    ) -> Result<Decision, Indecision> {
        if !self.supported.contains(&operation) {
            return Err(Indecision::NotOffered {
                what: operation.key().into(),
            });
        }
        if operation.needs_field() {
            let position = target.ok_or(Indecision::NoTarget)?;
            let into = self
                .fields
                .get(position)
                .map(|(_, handle, _)| *handle)
                .ok_or_else(|| Indecision::NotOffered {
                    what: format!("field {position}").into_boxed_str(),
                })?;
            return Ok(Decision::NeedsText { into });
        }
        if operation == Operation::Return {
            let app = self.return_to.clone().ok_or(Indecision::NoTarget)?;
            return Ok(Decision::Ready(Act::Return(app)));
        }
        if !operation.needs_tap_target() {
            return Self::untargeted(operation).map(Decision::Ready);
        }
        let position = target.ok_or(Indecision::NoTarget)?;
        let handle =
            self.on_screen
                .get(position)
                .copied()
                .ok_or_else(|| Indecision::NotOffered {
                    what: format!("row {position}").into_boxed_str(),
                })?;
        Self::targeted(operation, handle).map(Decision::Ready)
    }

    /// What the chosen row is worth once rows of the same name are counted
    /// together.
    ///
    /// A screen that offers the same thing twice splits the probability mass
    /// across both, and the split reads as uncertainty about what to act on.
    /// It is not: it is certainty about what to do, divided by an accident of
    /// how the screen names its controls — measured on a dialog offering
    /// `Close` and `CLOSE`, where the operation was 0.94 and the row 0.34.
    ///
    /// Same name, not same effect: this crate cannot know whether two
    /// controls do the same thing, only that the screen calls them the same
    /// thing. That is the whole claim, and it is why the comparison is on the
    /// text a person would read rather than on anything about the widgets.
    fn named_once(&self, chosen: &crate::judgment::Chosen) -> f64 {
        let Some(name) = self
            .targets
            .iter()
            .find(|(id, ..)| *id == chosen.choice)
            .map(|(_, _, text)| text.trim().to_lowercase())
        else {
            return 0.0;
        };
        self.targets
            .iter()
            .filter(|(_, _, text)| text.trim().to_lowercase() == name)
            .filter_map(|(id, ..)| chosen.probabilities.get(id.to_string().as_str()))
            .sum()
    }

    /// The rows this screen offers, keyed as the Choice offers them.
    ///
    /// The text lives here rather than in the Choice's own options, so it is
    /// sent once in the state instead of twice.
    pub fn rows(&self) -> impl Iterator<Item = (OptionId, &str)> {
        self.targets.iter().map(|(id, _, text)| (*id, &**text))
    }

    /// The fields this screen offers, keyed as the Choice offers them.
    pub fn fields_offered(&self) -> impl Iterator<Item = (OptionId, &str)> {
        self.fields.iter().map(|(id, _, text)| (*id, &**text))
    }

    /// The operations offered on this screen.
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.supported
    }

    /// How many rows may be acted on.
    #[must_use]
    pub fn targets(&self) -> usize {
        self.targets.len()
    }
}
