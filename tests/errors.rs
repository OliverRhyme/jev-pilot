//! Errors behave like errors: they print, and they chain.

use jev_pilot::credential::{ApiKey, CredentialError};
use jev_pilot::judgment::{MAX_OPTIONS, Options};
use jev_pilot::snapshot::Snapshot;
use std::error::Error as _;

/// A cause that is wrapped but not reachable is a cause that is lost: tooling
/// that walks `source()` to print "caused by" prints nothing.
#[test]
fn a_wrapped_cause_stays_reachable_through_the_source_chain() {
    let error = CredentialError::Unreadable {
        path: "/run/secrets/typesafe".into(),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
    };

    let cause = error.source().expect("the io error is the cause");
    assert!(
        cause.downcast_ref::<std::io::Error>().is_some(),
        "the cause should still be the io error, got {cause}"
    );
    assert!(error.to_string().contains("/run/secrets/typesafe"));
    // The path is named, but the reason lives on the cause, not duplicated here.
    assert!(!error.to_string().contains("denied"));
}

/// An empty key is rejected, and says so.
#[test]
fn a_blank_credential_is_refused_with_a_message() {
    let error = ApiKey::new("  ").expect_err("blank is not a credential");
    assert!(matches!(error, CredentialError::Empty));
    assert!(!error.to_string().is_empty());
}

/// A refusal that carries no message is a refusal nobody can act on.
#[test]
fn a_full_choice_explains_itself() {
    let mut options = Options::default();
    for _ in 0..MAX_OPTIONS {
        options.push("a row").expect("within the limit");
    }

    let full = options
        .push("one too many")
        .expect_err("the choice is full");

    let message = full.to_string();
    assert!(message.contains(&MAX_OPTIONS.to_string()), "got {message}");
    // It is an Error, not merely a marker.
    let _: &dyn std::error::Error = &full;
}

/// An empty screen is legal: it simply offers nothing to tap.
#[test]
fn a_screen_with_nothing_on_it_is_not_an_error() {
    let snapshot = Snapshot::new(Vec::new()).expect("an empty screen is legal");
    assert!(snapshot.is_empty());
    assert_eq!(snapshot.len(), 0);
}
