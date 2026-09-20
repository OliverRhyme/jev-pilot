//! The System One request, and where to send it.
//!
//! Transport is deliberately absent. Building the request is pure and testable;
//! choosing between a blocking client, an async one, or a caller's existing
//! HTTP stack is not this crate's decision to make.

#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub mod http;

use serde::{Deserialize, Serialize};

/// Where evaluations are sent.
pub const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The flagship System One model.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// One evaluation request.
#[derive(Debug, Clone, Serialize)]
pub struct Request<S: Serialize, Q: Serialize> {
    /// The application state being judged.
    pub state: S,
    /// Which model handles the request.
    pub model: &'static str,
    /// The questions asked of it, evaluated in parallel.
    pub questions: Q,
}

impl<S: Serialize, Q: Serialize> Request<S, Q> {
    /// Assemble a request against [`DEFAULT_MODEL`].
    pub fn new(state: S, questions: Q) -> Self {
        Self {
            state,
            model: DEFAULT_MODEL,
            questions,
        }
    }

    /// Send this request to a different model.
    #[must_use]
    pub fn with_model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }
}

/// What the endpoint returns: the answers, plus what produced them.
#[derive(Debug, Clone, Deserialize)]
pub struct Evaluation<A> {
    /// The concrete model version that answered.
    ///
    /// Worth keeping even when `jev-latest` was requested: that is an alias,
    /// and the version it resolves to is what a result is reproducible against.
    pub model: Box<str>,
    /// The answers, keyed as the questions were.
    pub answers: A,
    /// What the request cost.
    pub usage: Usage,
}

/// Token usage for one evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Usage {
    /// Tokens consumed by the state and questions.
    pub input_tokens: u64,
    /// Tokens produced by the answers.
    pub output_tokens: u64,
}
