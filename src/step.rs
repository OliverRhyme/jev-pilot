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

use crate::act::{Aim, Catalog, Deciding, Operation};
use crate::judgment::{
    Chosen, Confidence, Criterion, Graded, Likelihood, Poles, Progress, Question,
};
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
    #[serde(
        serialize_with = "claims",
        skip_serializing_if = "<[Criterion]>::is_empty"
    )]
    pub must_also: &'a [Criterion],
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
    /// How far along the goal is.
    pub goal_met: Question<Checking<'a>>,
    /// Whether the screen is an error state.
    pub is_error_screen: Question<&'static str>,
    /// Whether this screen is the final confirmation of something that cannot
    /// be undone, such as sending money or deleting.
    pub commits: Question<Screening>,
    /// Whether this screen is warning the user to reconsider before going on.
    pub warns: Question<Screening>,
    /// One judgment per acceptance criterion, keyed `check_0`, `check_1`, ...
    ///
    /// Flattened into the same map as the rest, so they are evaluated in the
    /// same parallel pass and cost no additional round trip.
    #[serde(flatten)]
    pub checks: BTreeMap<Box<str>, Question<Confirming<'a>>>,
}

/// A yes-or-no question about the screen as a whole, with examples of each.
///
/// With examples, a transfer summary read 0.91 as a final confirmation where
/// the bare question read it at 0.67.
#[derive(Debug, Clone, Serialize)]
pub struct Screening {
    /// The judgment being asked.
    pub question: &'static str,
    /// Screens that are a yes.
    pub examples_yes: &'static [&'static str],
    /// Screens that are a no.
    pub examples_no: &'static [&'static str],
}

impl Screening {
    /// Is acting here the final confirmation of something that cannot be undone?
    pub const COMMITS: Self = Self {
        question: "Does the current screen ask for the final confirmation of an irreversible action?",
        examples_yes: &[
            "Confirm Transfer ₱50.00",
            "Pay now",
            "Delete account",
            "Submit application",
        ],
        examples_no: &["Continue to review", "Select an account", "Enter amount"],
    };

    /// Is the screen warning the user to reconsider?
    pub const WARNS: Self = Self {
        question: "Is the current screen a warning that asks the user to reconsider before going on?",
        examples_yes: &[
            "It seems you've made a similar transaction. Proceed?",
            "This cannot be undone. Delete?",
            "This site may be unsafe. Continue?",
        ],
        examples_no: &[
            "An announcement with a Close button",
            "A login form",
            "A confirmation summary of a transfer",
        ],
    };
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
    pub fn checked(goal: &'a str, catalog: &Catalog, criteria: &'a [Criterion]) -> Self {
        Self::planned(goal, &[], catalog, criteria)
    }

    /// The questions for one step of a run that was given its steps in order.
    ///
    /// The choices are asked about the next unfinished step rather than the
    /// goal as a whole. Whether the goal has been met is still asked about the
    /// goal: finishing the steps is how it is reached, not what it is. An
    /// empty plan asks about the goal as written.
    #[must_use]
    pub fn planned(
        goal: &'a str,
        plan: &'a [Box<str>],
        catalog: &Catalog,
        criteria: &'a [Criterion],
    ) -> Self {
        let aim = if plan.is_empty() {
            Aim::Goal(goal)
        } else {
            Aim::Plan(plan)
        };
        let checks = criteria
            .iter()
            .enumerate()
            .map(|(index, criterion)| {
                (
                    format!("check_{index}").into_boxed_str(),
                    Question::Noul {
                        instructions: Confirming {
                            goal,
                            criterion: criterion.claim(),
                            question: "Is `criterion` true of the current screen?",
                        },
                        criteria: criterion.poles(),
                    },
                )
            })
            .collect();
        Self::assembled(goal, aim, catalog, criteria, checks)
    }

    fn assembled(
        goal: &'a str,
        aim: Aim<'a>,
        catalog: &Catalog,
        criteria: &'a [Criterion],
        checks: BTreeMap<Box<str>, Question<Confirming<'a>>>,
    ) -> Self {
        Self {
            operation: catalog.operation_question(aim),
            tap_target: catalog.tap_target_question(aim),
            type_field: catalog
                .operations()
                .contains(&Operation::TypeText)
                .then(|| catalog.type_field_question(aim))
                .flatten(),
            goal_met: Question::Score {
                instructions: Checking {
                    goal,
                    must_also: criteria,
                    question: "How far towards `goal` does the current screen get, \
                               counting everything in `must_also` as part of it?",
                },
                criteria: Progress::levels(),
            },
            checks,
            commits: Question::Noul {
                instructions: Screening::COMMITS,
                criteria: Poles {
                    yes: "Acting on this screen commits something that cannot be undone".into(),
                    no: "Nothing on this screen is final".into(),
                },
            },
            warns: Question::Noul {
                instructions: Screening::WARNS,
                criteria: Poles {
                    yes: "The screen warns of a risk and asks whether to go on".into(),
                    no: "The screen is not a warning".into(),
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

impl StepQuestions<'_> {
    /// What the choices are told when the state names the next key to press.
    pub const NEXT_KEY: &'static str = "Tap `sequence.next_key` on the keypad.";

    /// Point the operation and the row at the key the state names.
    ///
    /// Carried in the state alone, the next key raised the right row but left
    /// the operation at 0.62 once the goal was a list of steps; said in the
    /// questions as well, both were at 0.99.
    #[must_use]
    pub fn pointing_at_next_key(mut self) -> Self {
        for head in [Some(&mut self.operation), self.tap_target.as_mut()]
            .into_iter()
            .flatten()
        {
            if let Question::Choice { instructions, .. } = head {
                instructions.note = Some(Self::NEXT_KEY);
            }
        }
        self
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
    /// How far along the goal is.
    pub goal_met: Graded,
    /// How likely the screen is an error state.
    pub is_error_screen: Likelihood,
    /// Whether acting on this screen commits something that cannot be undone.
    #[serde(default)]
    pub commits: Option<Likelihood>,
    /// Whether this screen is warning the user to reconsider.
    #[serde(default)]
    pub warns: Option<Likelihood>,
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
    pub fn unmet<'c>(&self, criteria: &'c [Criterion], certainty: f64) -> Option<&'c str> {
        criteria.iter().enumerate().find_map(|(index, criterion)| {
            let key = format!("check_{index}");
            match self.checks.get(key.as_str()) {
                Some(answer) if answer.noul > certainty => None,
                // A missing answer counts as unmet: a criterion nobody
                // confirmed is not a criterion that was met.
                _ => Some(criterion.claim()),
            }
        })
    }
}

/// Serialise criteria as the claims they make, not as their whole shape.
fn claims<S: serde::Serializer>(criteria: &[Criterion], serializer: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(criteria.len()))?;
    for criterion in criteria {
        seq.serialize_element(criterion.claim())?;
    }
    seq.end()
}
