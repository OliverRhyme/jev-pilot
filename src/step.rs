//! One iteration of the loop, as a request and the response that answers it.
//!
//! The two structs mirror each other field for field, and those field names are
//! the wire keys. A question added without its answer, or an answer read under
//! a name nothing asked, is a compile error rather than a missing key at
//! runtime.
//!
//! Every question here is independent of the others, which is the point: the
//! endpoint evaluates a whole question map in parallel against one state, so a
//! step asking three narrow things costs the round trip of one.

use crate::act::{Catalog, Deciding, Operation};
use crate::judgment::{Chosen, Confidence, Likelihood, Poles, Question};
use serde::{Deserialize, Serialize};

/// The instructions for a judgment made against a stated goal.
#[derive(Debug, Clone, Serialize)]
pub struct Checking<'a> {
    /// What the worker is trying to achieve.
    pub goal: &'a str,
    /// The judgment being asked.
    pub question: &'static str,
}

/// The operation a step chose, and how sure it was.
#[derive(Debug, Clone, Deserialize)]
pub struct ChosenOperation {
    /// The operation.
    pub choice: Operation,
    /// How concentrated the distribution was.
    pub confidence: Confidence,
}

/// Everything one step asks of the model.
///
/// The target heads are speculative: they are answered alongside the operation
/// whether or not it turns out to need one. Questions in a map are evaluated in
/// parallel, so asking costs nothing extra, and having the answer already saves
/// a second round trip whenever the operation does need a target.
#[derive(Debug, Serialize)]
pub struct StepQuestions<'a> {
    /// What to do.
    pub operation: Question<Deciding<'a>>,
    /// Which row to do it to, when the screen has any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tap_target: Option<Question<Deciding<'a>>>,
    /// Whether the goal is already satisfied.
    pub goal_met: Question<Checking<'a>>,
    /// Whether the screen is an error state.
    pub is_error_screen: Question<&'static str>,
}

impl<'a> StepQuestions<'a> {
    /// Build the questions for one iteration against `catalog`.
    #[must_use]
    pub fn new(goal: &'a str, catalog: &Catalog) -> Self {
        Self {
            operation: catalog.operation_question(goal),
            tap_target: catalog.tap_target_question(goal),
            goal_met: Question::Noul {
                instructions: Checking {
                    goal,
                    question: "Is `goal` already fully accomplished on the current screen?",
                },
                criteria: Poles {
                    yes: "The screen shows the finished result the goal describes".into(),
                    no: "The goal is unstarted, partially done, or not visible here".into(),
                },
            },
            is_error_screen: Question::Noul {
                instructions: "Is the current screen an error, crash, or permission-denied state?",
                criteria: Poles {
                    yes: "An error message, crash dialog, or blocked-access screen is showing"
                        .into(),
                    no: "An ordinary working screen".into(),
                },
            },
        }
    }
}

/// Everything one step learns back, keyed to match [`StepQuestions`].
#[derive(Debug, Deserialize)]
pub struct StepAnswers {
    /// The operation the model chose.
    pub operation: ChosenOperation,
    /// The row it would act on, if the operation needs one.
    #[serde(default)]
    pub tap_target: Option<Chosen>,
    /// How likely the goal is already met.
    pub goal_met: Likelihood,
    /// How likely the screen is an error state.
    pub is_error_screen: Likelihood,
}
