//! Updating an installed copy to the latest release.
//!
//! What to do is decided here and is pure; doing it — asking GitHub for the
//! latest release, moving the running program aside, running the installer —
//! is the command line's.

use std::path::Path;

/// Where releases are published.
pub const RELEASES: &str = "https://github.com/OliverRhyme/jev-pilot/releases";

/// The API answer naming the latest release.
pub const LATEST_API: &str = "https://api.github.com/repos/OliverRhyme/jev-pilot/releases/latest";

/// How an installed copy is brought up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Plan {
    /// Run the installer the copy came from, which replaces it in place.
    Installer {
        /// The shell that runs it.
        program: String,
        /// Its arguments.
        args: Vec<String>,
    },
    /// A copy built from source, updated the way it was built.
    ///
    /// Not replaced by a download: the installer would put a second copy in
    /// its own directory, and whichever came first on the path would win.
    FromSource {
        /// What to run instead.
        hint: String,
    },
}

/// How to update the copy at `exe`, for a user whose home is `home`.
///
/// A copy under `~/.jev-pilot/bin` came from an installer, and is updated by
/// running it again. Anything else was built from source.
#[must_use]
pub fn plan(exe: &Path, home: &Path, windows: bool) -> Plan {
    if !exe.starts_with(home.join(".jev-pilot").join("bin")) {
        return Plan::FromSource {
            hint: format!(
                "this copy was not installed by the jev-pilot installer ({}). \
                 Update it the way it was built, for example from a clone: \
                 git pull && cargo install --path . --features mcp --locked",
                exe.display()
            ),
        };
    }
    if windows {
        Plan::Installer {
            program: "powershell".to_owned(),
            args: vec![
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-c".to_owned(),
                format!("irm {RELEASES}/latest/download/jev-pilot-installer.ps1 | iex"),
            ],
        }
    } else {
        Plan::Installer {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                format!(
                    "curl --proto '=https' --tlsv1.2 -LsSf \
                     {RELEASES}/latest/download/jev-pilot-installer.sh | sh"
                ),
            ],
        }
    }
}

/// Whether the release tagged `tag` is newer than version `current`.
///
/// A tag that is not a version is never newer: an update is not worth
/// starting on the strength of something that could not be read.
#[must_use]
pub fn is_newer(current: &str, tag: &str) -> bool {
    match (parse(current), parse(tag)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// `major.minor.patch`, with or without a leading `v`.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Where the running program is moved to while it is replaced.
///
/// A running program cannot be overwritten on Windows, and on Linux writing
/// over one can fail as busy; renaming it is allowed on both, and on macOS.
#[must_use]
pub fn set_aside(exe: &Path) -> std::path::PathBuf {
    exe.with_file_name(format!("jev-pilot.old{}", std::env::consts::EXE_SUFFIX))
}
