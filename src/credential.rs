//! Holding an API key without leaking it.
//!
//! A key is a `String` that must never be printed. Relying on everyone to
//! remember that fails the first time a struct holding one derives `Debug` and
//! ends up in a panic message, a log line or an error report. [`ApiKey`] makes
//! the safe thing automatic: its `Debug` is redacted, it has no `Display` and
//! no `Serialize`, and reading the secret requires calling [`ApiKey::expose`],
//! which is deliberately awkward to write by accident.

use core::fmt;

/// The environment variable holding the key itself.
pub const ENV_KEY: &str = "TYPESAFE_API_KEY";

/// The environment variable naming a file that holds the key.
///
/// Preferred for services: a secret mounted at a path is not visible in the
/// process environment, which `ps` and crash handlers expose.
pub const ENV_KEY_FILE: &str = "TYPESAFE_API_KEY_FILE";

/// A TypeSafe API key.
///
/// Cloned sparingly and never logged; see the module documentation.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(Box<str>);

impl ApiKey {
    /// Wrap a key, rejecting one that is empty or only whitespace.
    ///
    /// # Errors
    /// Returns [`CredentialError::Empty`] for a blank key, which is almost
    /// always an unset variable rather than a real credential.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, CredentialError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(CredentialError::Empty);
        }
        Ok(Self(trimmed.into()))
    }

    /// Read the key from the environment.
    ///
    /// [`ENV_KEY_FILE`] takes precedence over [`ENV_KEY`]: when a deployment
    /// has gone to the trouble of mounting a secret file, silently falling back
    /// to an inherited variable would defeat the point. A configured file that
    /// cannot be read is an error, never a fallback.
    ///
    /// # Errors
    /// Returns [`CredentialError`] when neither variable is set, when the named
    /// file cannot be read, or when what was found is blank.
    pub fn from_env() -> Result<Self, CredentialError> {
        Self::from_sources(
            std::env::var(ENV_KEY_FILE).ok().as_deref(),
            std::env::var(ENV_KEY).ok().as_deref(),
        )
    }

    /// Choose between a secret file and a plain value.
    ///
    /// A blank path counts as unset. Declaring an environment variable with no
    /// value is routine in Compose files, Kubernetes manifests and CI
    /// templates, and treating that as "a secret file was configured" discards
    /// a perfectly good key set beside it, then reports a failure to read a
    /// file named "". A path that is genuinely set still wins, and still fails
    /// closed when it cannot be read.
    ///
    /// # Errors
    /// Returns [`CredentialError`] when neither source yields a key, when a
    /// configured file cannot be read, or when what was found is blank.
    pub fn from_sources(
        key_file: Option<&str>,
        key: Option<&str>,
    ) -> Result<Self, CredentialError> {
        if let Some(path) = key_file.map(str::trim).filter(|path| !path.is_empty()) {
            let contents =
                std::fs::read_to_string(path).map_err(|source| CredentialError::Unreadable {
                    path: path.into(),
                    source,
                })?;
            return Self::new(contents);
        }
        key.ok_or(CredentialError::Missing).and_then(Self::new)
    }

    /// Borrow the secret.
    ///
    /// Named to be conspicuous at the call site: every use is a place the key
    /// escapes the type that was protecting it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Redacted so a key cannot reach a log through a derived `Debug`.
impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// No key could be obtained.
#[derive(Debug)]
#[non_exhaustive]
pub enum CredentialError {
    /// Neither environment variable was set.
    Missing,
    /// A key was found but held nothing.
    Empty,
    /// A key file was named but could not be read.
    Unreadable {
        /// The path that was named.
        path: Box<str>,
        /// Why it could not be read.
        source: std::io::Error,
    },
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "set {ENV_KEY} or {ENV_KEY_FILE}"),
            Self::Empty => write!(f, "the configured API key is empty"),
            Self::Unreadable { path, .. } => write!(f, "cannot read key file {path}"),
        }
    }
}

impl core::error::Error for CredentialError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } => Some(source),
            Self::Missing | Self::Empty => None,
        }
    }
}
