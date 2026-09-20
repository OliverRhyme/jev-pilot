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
use std::collections::BTreeMap;

/// The instructions for a judgment made against a stated goal.
#[derive(Debug, Clone, Serialize)]
pub struct Checking<'a> {
    /// What the worker is trying to achieve.
    pub goal: &'a str,
    /// What must also be true for the goal to count as met.
    ///
    /// Stated here as well as asked separately, because "is the goal met?" is
    /// otherwise judged against whatever the reader takes the goal to mean. A
    /// Short about the right subject satisfies "play a video about Jev" on a
    /// loose reading and fails it on the one that was intended.
    #[serde(skip_serializing_if = "<[Box<str>]>::is_empty")]
    pub must_also: &'a [Box<str>],
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
    /// Every operation and its probability. See [`Chosen::probabilities`].
    #[serde(default)]
    pub probabilities: std::collections::BTreeMap<Box<str>, f64>,
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
    /// Which field to type into, when the screen has one and typing is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_field: Option<Question<Deciding<'a>>>,
    /// Whether the goal is already satisfied.
    pub goal_met: Question<Checking<'a>>,
    /// Whether the screen is an error state.
    pub is_error_screen: Question<&'static str>,
    /// One judgment per acceptance criterion, keyed `check_0`, `check_1`, ...
    ///
    /// Flattened into the same map as the rest, so they are evaluated in the
    /// same parallel pass and cost no additional round trip.
    #[serde(flatten)]
    pub checks: BTreeMap<Box<str>, Question<Confirming<'a>>>,
}

/// The instructions for one acceptance criterion.
#[derive(Debug, Clone, Serialize)]
pub struct Confirming<'a> {
    /// What the worker is trying to achieve.
    pub goal: &'a str,
    /// The specific thing being checked.
    pub criterion: &'a str,
    /// The judgment being asked.
    pub question: &'static str,
}

impl<'a> StepQuestions<'a> {
    /// Build the questions for one iteration against `catalog`.
    #[must_use]
    pub fn new(goal: &'a str, catalog: &Catalog) -> Self {
        Self::checked(goal, catalog, &[])
    }

    /// Build the questions, with acceptance criteria to confirm alongside.
    #[must_use]
    pub fn checked(goal: &'a str, catalog: &Catalog, criteria: &'a [Box<str>]) -> Self {
        let checks = criteria
            .iter()
            .enumerate()
            .map(|(index, criterion)| {
                (
                    format!("check_{index}").into_boxed_str(),
                    Question::Noul {
                        instructions: Confirming {
                            goal,
                            criterion,
                            question: "Is `criterion` true of the current screen?",
                        },
                        criteria: Poles {
                            yes: "The screen plainly shows this to be so".into(),
                            no: "It is not so, or cannot be told from this screen".into(),
                        },
                    },
                )
            })
            .collect();
        Self::assembled(goal, catalog, criteria, checks)
    }

    fn assembled(
        goal: &'a str,
        catalog: &Catalog,
        criteria: &'a [Box<str>],
        checks: BTreeMap<Box<str>, Question<Confirming<'a>>>,
    ) -> Self {
        Self {
            operation: catalog.operation_question(goal),
            tap_target: catalog.tap_target_question(goal),
            type_field: catalog
                .operations()
                .contains(&Operation::TypeText)
                .then(|| catalog.type_field_question(goal))
                .flatten(),
            goal_met: Question::Noul {
                instructions: Checking {
                    goal,
                    must_also: criteria,
                    question: "Is `goal` already fully accomplished on the current screen, \
                               including everything in `must_also`?",
                },
                criteria: Poles {
                    yes: "The screen shows the finished result the goal describes".into(),
                    no: "The goal is unstarted, partially done, or not visible here".into(),
                },
            },
            checks,
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
    /// The field it would type into, if the operation is typing.
    #[serde(default)]
    pub type_field: Option<Chosen>,
    /// How likely the goal is already met.
    pub goal_met: Likelihood,
    /// How likely the screen is an error state.
    pub is_error_screen: Likelihood,
    /// One answer per acceptance criterion, keyed to match the questions.
    #[serde(flatten, default)]
    pub checks: BTreeMap<Box<str>, Likelihood>,
}

impl StepAnswers {
    /// The first criterion the screen did not satisfy, by position.
    ///
    /// A verdict that passes `goal_met` can still fail here: that is the whole
    /// point of asking separately.
    #[must_use]
    pub fn unmet<'c>(&self, criteria: &'c [Box<str>], certainty: f64) -> Option<&'c str> {
        criteria.iter().enumerate().find_map(|(index, criterion)| {
            let key = format!("check_{index}");
            match self.checks.get(key.as_str()) {
                Some(answer) if answer.noul > certainty => None,
                // A missing answer counts as unmet: a criterion nobody
                // confirmed is not a criterion that was met.
                _ => Some(&**criterion),
            }
        })
    }
}
