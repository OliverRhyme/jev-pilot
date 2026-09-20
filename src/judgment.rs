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
pub struct Options(Vec<(Box<str>, Box<str>)>);

impl Options {
    /// Offer one more element, addressed positionally.
    ///
    /// # Errors
    /// Returns [`Full`] once [`MAX_OPTIONS`] have been offered.
    pub fn push(&mut self, rubric: impl Into<Box<str>>) -> Result<OptionId, Full> {
        let index = u8::try_from(self.0.len()).map_err(|_| Full)?;
        let id = OptionId::nth(index).ok_or(Full)?;
        self.0
            .push((id.to_string().into_boxed_str(), rubric.into()));
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
        self.0.push((key.into(), rubric.into()));
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
