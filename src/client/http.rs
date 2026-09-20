//! A blocking transport for the System One endpoint.
//!
//! Optional, behind the `http` feature. Without it the crate builds requests
//! and you send them with whatever client you already have — which is the point
//! of keeping [`super::Request`] a plain serialisable value.

use super::{ENDPOINT, Evaluation, Request};
use crate::credential::ApiKey;
use core::fmt;
use core::time::Duration;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A client for one System One endpoint.
pub struct SystemOne {
    key: ApiKey,
    endpoint: Box<str>,
    attempts: u32,
}

impl fmt::Debug for SystemOne {
    /// Written by hand so the key cannot reach a log through this struct.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemOne")
            .field("endpoint", &self.endpoint)
            .field("attempts", &self.attempts)
            // Redacted by ApiKey's own Debug, which is the point of this impl.
            .field("key", &self.key)
            .finish()
    }
}

/// The request could not be sent, or the response could not be read.
///
/// Opaque on purpose. Naming the HTTP client's error type here would make it
/// part of this crate's public API, so a major release of a dependency behind
/// an optional feature would force a major release of this crate. The cause is
/// still reachable through [`core::error::Error::source`].
#[derive(Debug)]
pub struct TransportError(Box<dyn core::error::Error + Send + Sync>);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl core::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&*self.0)
    }
}

/// A request did not produce an answer.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApiError {
    /// The request could not be sent, or the response could not be read.
    Transport(TransportError),
    /// The service answered with a status that is not success.
    Status {
        /// The status code.
        code: u16,
        /// What the body said, truncated.
        detail: Box<str>,
    },
    /// The service stayed busy for every attempt.
    Busy {
        /// How many times it was tried.
        attempts: u32,
    },
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(inner) => write!(f, "transport error: {inner}"),
            Self::Status { code, detail } if detail.is_empty() => match code {
                401 => write!(f, "rejected (401): the API key is missing or not valid"),
                _ => write!(f, "rejected ({code})"),
            },
            Self::Status { code, detail } => write!(f, "rejected ({code}): {detail}"),
            Self::Busy { attempts } => {
                write!(f, "the service was busy on all {attempts} attempts")
            }
        }
    }
}

impl core::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Transport(inner) => Some(inner),
            Self::Status { .. } | Self::Busy { .. } => None,
        }
    }
}

impl SystemOne {
    /// How many times a busy response is retried.
    pub const ATTEMPTS: u32 = 4;

    /// How much of an error body is kept for the message.
    const DETAIL_LIMIT: usize = 400;

    /// Talk to the public endpoint with this key.
    #[must_use]
    pub fn new(key: ApiKey) -> Self {
        Self {
            key,
            endpoint: ENDPOINT.into(),
            attempts: Self::ATTEMPTS,
        }
    }

    /// Talk to a different endpoint, for a proxy or a test double.
    #[must_use]
    pub fn at(mut self, endpoint: impl Into<Box<str>>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Evaluate one request and decode its answers.
    ///
    /// Retries while the service reports itself overloaded. 429 is the
    /// documented rate limit; 529 is a non-standard code this service also
    /// uses for overload, and treating it as fatal would fail runs that would
    /// have succeeded a second later.
    ///
    /// # Errors
    /// Returns [`ApiError`] when the request cannot be sent, the service
    /// rejects it, or it stays busy for every attempt.
    pub fn evaluate<S, Q, A>(&self, request: &Request<S, Q>) -> Result<Evaluation<A>, ApiError>
    where
        S: Serialize,
        Q: Serialize,
        A: DeserializeOwned,
    {
        let mut backoff = Duration::from_millis(500);
        for attempt in 1..=self.attempts {
            match self.send(request) {
                Err(ApiError::Status { code, .. }) if is_busy(code) && attempt < self.attempts => {
                    std::thread::sleep(backoff);
                    backoff *= 2;
                }
                Err(ApiError::Status { code, .. }) if is_busy(code) => {
                    return Err(ApiError::Busy {
                        attempts: self.attempts,
                    });
                }
                other => return other,
            }
        }
        Err(ApiError::Busy {
            attempts: self.attempts,
        })
    }

    fn send<S, Q, A>(&self, request: &Request<S, Q>) -> Result<Evaluation<A>, ApiError>
    where
        S: Serialize,
        Q: Serialize,
        A: DeserializeOwned,
    {
        // Status is checked here rather than raised by the client, because the
        // body is the diagnostic. A 422 says *which* question was malformed,
        // and turning the status into an error before reading it throws that
        // away — leaving the caller a bare code for the one failure where the
        // explanation is the whole point.
        let sent = ureq::post(&*self.endpoint)
            .config()
            .http_status_as_error(false)
            .build()
            .header("Authorization", &format!("Bearer {}", self.key.expose()))
            .header("Content-Type", "application/json")
            .send_json(request);

        let mut response =
            sent.map_err(|error| ApiError::Transport(TransportError(Box::new(error))))?;

        let code = response.status().as_u16();
        if !(200..300).contains(&code) {
            let detail = response
                .body_mut()
                .read_to_string()
                .unwrap_or_default()
                .trim()
                .chars()
                .take(Self::DETAIL_LIMIT)
                .collect::<String>();
            return Err(ApiError::Status {
                code,
                detail: detail.into_boxed_str(),
            });
        }

        response
            .body_mut()
            .read_json()
            .map_err(|error| ApiError::Transport(TransportError(Box::new(error))))
    }
}

/// Whether a status means "try again shortly" rather than "this was wrong".
const fn is_busy(code: u16) -> bool {
    // 529 is not in the HTTP standard; this service uses it for overload.
    matches!(code, 429 | 529 | 503)
}
