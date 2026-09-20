//! Holding an API key without leaking it.

use jev_pilot::credential::{ApiKey, CredentialError};

/// The whole point of the newtype. A key reaches a log the moment some struct
/// holding one derives `Debug` and lands in a panic message or error report, so
/// the redaction has to be a property of the type, not of everyone's discipline.
#[test]
fn debug_output_never_contains_the_secret() {
    let key = ApiKey::new("ts-live-SUPERSECRET-0123456789").expect("valid key");

    let rendered = format!("{key:?}");

    assert!(
        !rendered.contains("SUPERSECRET"),
        "the key leaked into Debug output: {rendered}"
    );
    assert_eq!(rendered, "ApiKey(<redacted>)");

    // Still readable when explicitly asked for.
    assert_eq!(key.expose(), "ts-live-SUPERSECRET-0123456789");
}

/// A struct holding a key is the realistic leak path, not the key alone.
#[test]
fn a_derived_debug_on_an_enclosing_struct_stays_redacted() {
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Config {
        endpoint: &'static str,
        key: ApiKey,
    }

    let config = Config {
        endpoint: "https://api.typesafe.ai/v1/systemone",
        key: ApiKey::new("ts-live-LEAKME").expect("valid key"),
    };

    assert!(!format!("{config:?}").contains("LEAKME"));
}

/// An unset variable usually arrives as an empty string rather than an absence,
/// and treating that as a credential produces a confusing 401 much later.
#[test]
fn a_blank_key_is_rejected_rather_than_accepted() {
    assert!(matches!(ApiKey::new(""), Err(CredentialError::Empty)));
    assert!(matches!(ApiKey::new("   \n"), Err(CredentialError::Empty)));
}

/// Surrounding whitespace comes free with `$(cat secret)` and heredocs.
#[test]
fn surrounding_whitespace_is_trimmed() {
    let key = ApiKey::new("  ts-live-abc \n").expect("valid key");
    assert_eq!(key.expose(), "ts-live-abc");
}

/// Declaring an environment variable with no value is routine in Compose files,
/// Kubernetes manifests and CI templates. Treating a blank path as "a secret
/// file was configured" discards a perfectly good key that was set right next
/// to it, and reports a failure to read a file named "".
#[test]
fn a_blank_key_file_path_falls_back_instead_of_failing() {
    let key = ApiKey::from_sources(Some("   "), Some("ts-live-real")).expect("falls back");
    assert_eq!(key.expose(), "ts-live-real");

    let key = ApiKey::from_sources(None, Some("ts-live-real")).expect("plain variable");
    assert_eq!(key.expose(), "ts-live-real");

    // A path that is actually set still takes precedence, and still fails
    // closed when it cannot be read.
    assert!(matches!(
        ApiKey::from_sources(Some("/definitely/not/a/path"), Some("ts-live-real")),
        Err(CredentialError::Unreadable { .. })
    ));

    assert!(matches!(
        ApiKey::from_sources(None, None),
        Err(CredentialError::Missing)
    ));
}
