//! The typed vocabulary of a System One request.
//!
//! Questions and answers are sum types rather than JSON bags, so the wire
//! contract is checked by the compiler instead of by hope. A question and the
//! answer it produces are paired by name in [`crate::step::StepQuestions`] and
//! [`crate::step::StepAnswers`], whose fields are the wire keys — which is what
//! stops a Choice being read back as a Noul.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// The most options a single Choice may carry, per the API reference.
pub const MAX_OPTIONS: usize = 255;

/// Names one option of a Choice, rendered on the wire as `A1`, `A2`, ...
///
/// A `u8` is not an arbitrary narrowing: it is exactly the range the endpoint
/// accepts, so an id that cannot be built is a request that would be rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OptionId(u8);

impl OptionId {
    /// The id for the nth option, counting from zero.
    ///
    /// Returns `None` for the one index that has no id: ids start at `A1`, so
    /// the last addressable position is `u8::MAX - 1`. Counting from zero and
    /// naming from one leaves exactly one position at the top that cannot be
    /// named, and adding blind there wraps to `A0`, or panics in a debug build.
    #[must_use]
    pub const fn nth(index: u8) -> Option<Self> {
        match index.checked_add(1) {
            Some(id) => Some(Self(id)),
            None => None,
        }
    }
}

impl fmt::Display for OptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "A{}", self.0)
    }
}

impl Serialize for OptionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for OptionId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Cow, not &str: an answer decoded from a non-contiguous or owned source
        // cannot hand out a borrow, and demanding one fails at runtime.
        let raw = std::borrow::Cow::<str>::deserialize(deserializer)?;
        raw.strip_prefix('A')
            .and_then(|digits| digits.parse().ok())
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("not an option id: {raw}")))
    }
}

/// The options of a Choice, in the order they will be presented.
///
/// Insertion order is preserved all the way to the wire; a keyed map would sort
/// them and silently reorder a list the model reads positionally.
///
/// Keys are free-form because the two kinds of Choice read differently. A list
/// of screen elements wants positional ids (`A1`, `A2`), since the name of a row
/// carries no meaning beyond where it sits. A list of operations wants its own
/// names (`tap`, `scroll_down`), because there the name *is* the meaning.
#[derive(Debug, Clone, Default)]
pub struct Options(Vec<(Box<str>, Option<Box<str>>)>);

impl Options {
    /// Offer one more element, addressed positionally.
    ///
    /// # Errors
    /// Returns [`Full`] once [`MAX_OPTIONS`] have been offered.
    pub fn push(&mut self, rubric: impl Into<Box<str>>) -> Result<OptionId, Full> {
        let index = u8::try_from(self.0.len()).map_err(|_| Full)?;
        let id = OptionId::nth(index).ok_or(Full)?;
        self.0
            .push((id.to_string().into_boxed_str(), Some(rubric.into())));
        Ok(id)
    }

    /// Offer one more option that needs no description of its own.
    ///
    /// The Choice carries `null` for it, and whatever the option refers to is
    /// read from the state instead. For a list of screen rows that halves the
    /// text sent, because the rows are already in the state for the other
    /// questions to read.
    ///
    /// # Errors
    /// Returns [`Full`] once [`MAX_OPTIONS`] have been offered.
    pub fn push_bare(&mut self) -> Result<OptionId, Full> {
        let index = u8::try_from(self.0.len()).map_err(|_| Full)?;
        let id = OptionId::nth(index).ok_or(Full)?;
        self.0.push((id.to_string().into_boxed_str(), None));
        Ok(id)
    }

    /// Offer one more option under a name of its own.
    ///
    /// # Errors
    /// Returns [`Full`] once [`MAX_OPTIONS`] have been offered.
    pub fn push_named(
        &mut self,
        key: impl Into<Box<str>>,
        rubric: impl Into<Box<str>>,
    ) -> Result<(), Full> {
        if self.0.len() >= MAX_OPTIONS {
            return Err(Full);
        }
        self.0.push((key.into(), Some(rubric.into())));
        Ok(())
    }

    /// How many options have been offered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no options have been offered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The Choice already holds [`MAX_OPTIONS`] options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

impl fmt::Display for Full {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a Choice may offer at most {MAX_OPTIONS} options")
    }
}

impl core::error::Error for Full {}

impl Serialize for Options {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (id, rubric) in &self.0 {
            map.serialize_entry(id, rubric)?;
        }
        map.end()
    }
}

/// How certain a distribution is, constrained to the unit interval.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    /// Build a confidence, rejecting anything outside `0.0..=1.0`.
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        (0.0..=1.0).contains(&value).then_some(Self(value))
    }

    /// No floor at all: any answer is certain enough to act on.
    pub const ZERO: Self = Self(0.0);

    /// The confidence as a plain number.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Confidence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            serde::de::Error::custom(format!("confidence {value} is outside 0.0..=1.0"))
        })
    }
}

/// What a yes and a no mean for a Noul.
#[derive(Debug, Clone, Serialize)]
pub struct Poles {
    /// What a yes means.
    #[serde(rename = "true")]
    pub yes: Box<str>,
    /// What a no means.
    #[serde(rename = "false")]
    pub no: Box<str>,
}

/// One claim that must hold before success is accepted.
///
/// A bare string is enough and gets a generic pair of poles. Supplying your own
/// is better where the claim has a boundary: the guidance is to state the exact
/// condition in the instructions and put the boundary cases in the criteria,
/// and only the caller knows where the edge of their own claim lies.
#[derive(Debug, Clone)]
pub struct Criterion {
    claim: Box<str>,
    when_true: Option<Box<str>>,
    when_false: Option<Box<str>>,
}

impl Criterion {
    /// A claim, with the generic poles.
    #[must_use]
    pub fn that(claim: impl Into<Box<str>>) -> Self {
        Self {
            claim: claim.into(),
            when_true: None,
            when_false: None,
        }
    }

    /// What a yes looks like for this claim.
    #[must_use]
    pub fn where_true(mut self, meaning: impl Into<Box<str>>) -> Self {
        self.when_true = Some(meaning.into());
        self
    }

    /// What a no looks like for this claim.
    #[must_use]
    pub fn where_false(mut self, meaning: impl Into<Box<str>>) -> Self {
        self.when_false = Some(meaning.into());
        self
    }

    /// The claim itself.
    #[must_use]
    pub fn claim(&self) -> &str {
        &self.claim
    }

    /// The poles to send, falling back to a generic pair.
    #[must_use]
    pub fn poles(&self) -> Poles {
        Poles {
            yes: self
                .when_true
                .clone()
                .unwrap_or_else(|| "The screen plainly shows this to be so".into()),
            no: self
                .when_false
                .clone()
                .unwrap_or_else(|| "It is not so, or cannot be told from this screen".into()),
        }
    }
}

impl From<&str> for Criterion {
    fn from(claim: &str) -> Self {
        Self::that(claim)
    }
}

impl From<String> for Criterion {
    fn from(claim: String) -> Self {
        Self::that(claim)
    }
}

impl From<Box<str>> for Criterion {
    fn from(claim: Box<str>) -> Self {
        Self::that(claim)
    }
}

/// A question the model is asked to answer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question<I: Serialize> {
    /// A yes/no judgment.
    Noul {
        /// The question being asked.
        instructions: I,
        /// What a yes and a no mean.
        criteria: Poles,
    },
    /// A selection among defined options.
    Choice {
        /// The question being asked.
        instructions: I,
        /// The options to choose between.
        criteria: Options,
    },
    /// A position along a described spectrum.
    Score {
        /// The question being asked.
        instructions: I,
        /// The levels, lowest first. Their order is what the answer means.
        criteria: Vec<Box<str>>,
    },
}

/// The answer to a Choice.
#[derive(Debug, Clone, Deserialize)]
pub struct Chosen {
    /// The highest-probability option.
    pub choice: OptionId,
    /// How concentrated the distribution was.
    pub confidence: Confidence,
    /// Every option and its probability.
    ///
    /// Kept rather than discarded once the winner is known: when the winner is
    /// not clear enough to act on, what it was nearly beaten by is the most
    /// useful thing anyone deciding next can be told.
    #[serde(default)]
    pub probabilities: BTreeMap<Box<str>, f64>,
}

/// How far along something is, as a Score answers it.
///
/// A yes/no forces three situations into two answers: the screen has nothing
/// to do with the goal, the screen is a step along the way, and the goal is
/// done. The first two both come back near zero, and a loop reading that
/// cannot tell a wrong turn from progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Progress {
    /// Nothing here relates to the goal.
    NotStarted,
    /// A step towards the goal, but not the goal.
    UnderWay,
    /// The goal is done.
    Achieved,
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotStarted => "not started",
            Self::UnderWay => "under way",
            Self::Achieved => "achieved",
        })
    }
}

impl Progress {
    /// The levels, lowest first, as the question offers them.
    ///
    /// Written as concrete situations rather than labels: a level description
    /// has to stand on its own for the position between them to mean anything.
    #[must_use]
    pub fn levels() -> Vec<Box<str>> {
        vec![
            "Nothing on this screen relates to the goal".into(),
            "This screen is a step towards the goal, but the goal is not done".into(),
            "This screen shows the finished result the goal describes".into(),
        ]
    }

    /// Read a level from the probability-weighted position the API returns.
    ///
    /// The value lands between levels, so each is claimed by the half-interval
    /// around it rather than by an exact match.
    #[must_use]
    pub fn from_score(score: f64) -> Self {
        if score >= 1.5 {
            Self::Achieved
        } else if score >= 0.75 {
            Self::UnderWay
        } else {
            Self::NotStarted
        }
    }
}

/// The answer to a Score.
#[derive(Debug, Clone, Deserialize)]
pub struct Graded {
    /// The probability-weighted position across the levels.
    pub score: f64,
    /// How concentrated the distribution was.
    pub confidence: Confidence,
}

impl Graded {
    /// Which level this lands on.
    #[must_use]
    pub fn progress(&self) -> Progress {
        Progress::from_score(self.score)
    }
}

/// The answer to a Noul: the probability that the answer is yes.
///
/// A value near 0.5 means yes and no are equally likely, not that the condition
/// half-holds, so callers should treat the middle as unknown rather than as a
/// weak yes.
#[derive(Debug, Clone, Copy)]
pub struct Likelihood {
    /// The probability the answer is yes, from 0.0 to 1.0.
    pub noul: f64,
}

/// Validated like [`Confidence`], and for a sharper reason: the guards compare
/// with `>`, and every comparison against NaN is false. An unvalidated NaN
/// would not trip a guard, it would switch the guard off — silently, and
/// exactly when something has already gone wrong upstream.
impl<'de> Deserialize<'de> for Likelihood {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            noul: f64,
        }
        let wire = Wire::deserialize(deserializer)?;
        if !(0.0..=1.0).contains(&wire.noul) {
            return Err(serde::de::Error::custom(format!(
                "probability {} is outside 0.0..=1.0",
                wire.noul
            )));
        }
        Ok(Self { noul: wire.noul })
    }
}
