//! The request and response that make up one iteration of the loop.
//!
//! Exact float comparison is deliberate below: these assert that a literal
//! survives a JSON round trip unchanged, which is a fidelity check on the
//! decoder rather than a comparison of computed values.
#![allow(clippy::float_cmp)]

use jev_pilot::act::Catalog;
use jev_pilot::client::Evaluation;
use jev_pilot::platform::{Android, Platform};
use jev_pilot::step::{StepAnswers, StepQuestions};

const SETTINGS: &str = include_str!("fixtures/settings.xml");

/// Independent questions over one state are evaluated in parallel, so a step
/// that asks three narrow things costs the round trip of one. Naming them as
/// struct fields makes the request and the response agree at compile time.
#[test]
fn a_step_asks_every_judgment_in_a_single_request() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Android);

    let wire = serde_json::to_value(StepQuestions::new("Open Wi-Fi settings", &catalog))
        .expect("serializes");

    assert_eq!(wire["operation"]["type"], "choice");
    assert_eq!(wire["tap_target"]["type"], "choice");
    assert_eq!(wire["goal_met"]["type"], "score");
    assert_eq!(wire["is_error_screen"]["type"], "noul");
}

/// Shape taken from the API reference: every answer carries its `type`, and a
/// Noul reports the probability of yes alongside it.
#[test]
fn step_answers_parse_from_the_documented_response_shape() {
    let answers: StepAnswers = serde_json::from_value(serde_json::json!({
        "operation": { "type": "choice", "choice": "tap", "confidence": 0.88 },
        "tap_target": { "type": "choice", "choice": "A3", "confidence": 0.91 },
        "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
        "is_error_screen": { "type": "noul", "noul": 0.01 }
    }))
    .expect("answers parse");

    assert_eq!(answers.operation.confidence.get(), 0.88);
    assert_eq!(
        answers.goal_met.progress(),
        jev_pilot::judgment::Progress::NotStarted
    );
}

/// The endpoint wraps answers in an envelope carrying the concrete model
/// version and token usage. Decoding straight into the answers loses the
/// version actually used, which matters because `jev-latest` is an alias.
#[test]
fn the_response_envelope_is_decoded_around_the_answers() {
    let body = serde_json::json!({
        "model": "jev-1.13.0",
        "answers": {
            "operation": { "type": "choice", "choice": "tap", "confidence": 1.0 },
            "tap_target": { "type": "choice", "choice": "A3", "confidence": 1.0 },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": 0.02 }
        },
        "usage": { "input_tokens": 900, "output_tokens": 189 }
    });

    let evaluation: Evaluation<StepAnswers> =
        serde_json::from_value(body).expect("envelope parses");

    assert_eq!(&*evaluation.model, "jev-1.13.0");
    assert_eq!(evaluation.usage.input_tokens, 900);
    assert_eq!(evaluation.answers.operation.confidence.get(), 1.0);
}

/// A probability outside the unit interval, or NaN, must not decode. Both
/// guard comparisons are `>`, so a NaN silently evaluates false and the loop
/// keeps driving the device across an error screen with the guard never firing.
#[test]
fn a_probability_that_is_not_one_is_refused_at_the_boundary() {
    let malformed = |value: serde_json::Value| {
        serde_json::from_value::<StepAnswers>(serde_json::json!({
            "operation": { "type": "choice", "choice": "done", "confidence": 0.9 },
            "goal_met": { "type": "score", "score": 0.2, "confidence": 0.9 },
            "is_error_screen": { "type": "noul", "noul": value }
        }))
    };

    assert!(malformed(serde_json::json!(1.5)).is_err(), "above one");
    assert!(malformed(serde_json::json!(-0.2)).is_err(), "below zero");
    assert!(
        malformed(serde_json::json!(0.5)).is_ok(),
        "a real probability"
    );
}
