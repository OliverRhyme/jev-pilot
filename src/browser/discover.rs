//! Finding a running Chrome, and the tab to drive in it.
//!
//! Chrome 144 and later can be switched into remote debugging from
//! `chrome://inspect/#remote-debugging`. It then writes a `DevToolsActivePort`
//! file into its user data directory: the port on the first line, the browser
//! websocket path on the second. There is no HTTP discovery in that mode, so
//! the file is the only way to learn the endpoint, and every websocket
//! handshake raises Chrome's "Allow remote debugging?" prompt. That is why the
//! connection is made once, as soon as the run starts, and never retried.
//!
//! Everything here but [`read_active_port`] is pure.

use crate::browser::transport::ConnectError;
use crate::platform::page::PageTab;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Which Chrome release channel, each with its own profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// Chrome.
    #[default]
    Stable,
    /// Chrome Beta.
    Beta,
    /// Chrome Dev.
    Dev,
    /// Chrome Canary.
    Canary,
}

impl Channel {
    /// The channel's name on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Dev => "dev",
            Self::Canary => "canary",
        }
    }
}

impl core::str::FromStr for Channel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "dev" => Ok(Self::Dev),
            "canary" => Ok(Self::Canary),
            _ => Err(()),
        }
    }
}

/// The operating systems Chrome keeps its profile differently on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// macOS.
    MacOs,
    /// Linux and the other Unixes.
    Linux,
    /// Windows.
    Windows,
}

impl Os {
    /// The system this was built for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Where a channel keeps its default profile, as Puppeteer resolves it.
///
/// `env` reads an environment variable, so this can be checked for any system
/// from any other.
#[must_use]
pub fn user_data_dir(
    os: Os,
    channel: Channel,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> PathBuf {
    match os {
        Os::MacOs => home
            .join("Library/Application Support/Google")
            .join(match channel {
                Channel::Stable => "Chrome",
                Channel::Beta => "Chrome Beta",
                Channel::Dev => "Chrome Dev",
                Channel::Canary => "Chrome Canary",
            }),
        Os::Linux => {
            let config = env("CHROME_CONFIG_HOME")
                .or_else(|| env("XDG_CONFIG_HOME"))
                .map_or_else(|| home.join(".config"), PathBuf::from);
            config.join(match channel {
                Channel::Stable => "google-chrome",
                Channel::Beta => "google-chrome-beta",
                Channel::Dev => "google-chrome-unstable",
                Channel::Canary => "google-chrome-canary",
            })
        }
        Os::Windows => {
            let local = env("LOCALAPPDATA")
                .map_or_else(|| home.join("AppData").join("Local"), PathBuf::from);
            local
                .join("Google")
                .join(match channel {
                    Channel::Stable => "Chrome",
                    Channel::Beta => "Chrome Beta",
                    Channel::Dev => "Chrome Dev",
                    Channel::Canary => "Chrome SxS",
                })
                .join("User Data")
        }
    }
}

/// What a `DevToolsActivePort` file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePort {
    /// The port on 127.0.0.1.
    pub port: u16,
    /// The browser websocket path, `/devtools/browser/<id>`.
    pub path: Box<str>,
}

impl ActivePort {
    /// Read the file's contents.
    ///
    /// # Errors
    /// Returns [`AttachError::Truncated`] when it does not hold a port and a
    /// browser websocket path.
    pub fn parse(text: &str) -> Result<Self, AttachError> {
        let truncated = || AttachError::Truncated {
            contents: text.chars().take(80).collect::<String>().into(),
        };
        let mut lines = text.lines();
        let port = lines
            .next()
            .and_then(|l| l.trim().parse::<u16>().ok())
            .filter(|p| *p > 0)
            .ok_or_else(truncated)?;
        let path = lines
            .next()
            .map(str::trim)
            .filter(|p| p.starts_with("/devtools/browser/") && p.len() > "/devtools/browser/".len())
            .ok_or_else(truncated)?;
        Ok(Self {
            port,
            path: path.into(),
        })
    }

    /// The browser websocket to connect to.
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, self.path)
    }
}

/// Read `DevToolsActivePort` from a user data directory.
///
/// # Errors
/// Returns [`AttachError`] telling a missing file from one the system will not
/// let this process read, and a file that does not hold an endpoint.
pub fn read_active_port(dir: &Path) -> Result<ActivePort, AttachError> {
    let text = std::fs::read_to_string(dir.join("DevToolsActivePort"))
        .map_err(|e| AttachError::reading(dir, &e))?;
    ActivePort::parse(&text)
}

/// Whether a `Browser.getVersion` reply comes from Chrome or Chromium.
///
/// Checked before anything is typed: a stale file can point at a port some
/// other program now holds.
#[must_use]
pub fn is_chrome(version: &Value) -> bool {
    let product = version
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or_default();
    ["Chrome/", "HeadlessChrome/", "Chromium/"]
        .iter()
        .any(|p| product.starts_with(p))
}

/// Why a run could not attach to Chrome.
#[derive(Debug)]
#[non_exhaustive]
pub enum AttachError {
    /// There is no `DevToolsActivePort` file.
    NoFile {
        /// The user data directory looked in.
        dir: PathBuf,
    },
    /// The file is there, but this process may not read it.
    Unreadable {
        /// The user data directory looked in.
        dir: PathBuf,
    },
    /// The file does not hold an endpoint.
    Truncated {
        /// What it holds, cut short.
        contents: Box<str>,
    },
    /// Nothing listens on the port the file names.
    Stale,
    /// Chrome refused the connection.
    Rejected {
        /// The HTTP status of the refusal.
        status: u16,
    },
    /// Nobody answered Chrome's prompt in time.
    TimedOut,
    /// What answered is not Chrome.
    NotChrome {
        /// What it says it is.
        product: Box<str>,
    },
    /// Anything else.
    Other(Box<str>),
}

impl AttachError {
    /// The error for a failed read of the file in `dir`.
    #[must_use]
    pub fn reading(dir: &Path, error: &std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::NoFile { dir: dir.into() },
            std::io::ErrorKind::PermissionDenied => Self::Unreadable { dir: dir.into() },
            _ => Self::Other(format!("{}: {error}", dir.display()).into()),
        }
    }
}

impl From<ConnectError> for AttachError {
    fn from(e: ConnectError) -> Self {
        match e {
            ConnectError::Refused => Self::Stale,
            ConnectError::Rejected { status } => Self::Rejected { status },
            ConnectError::TimedOut => Self::TimedOut,
            other => Self::Other(other.to_string().into()),
        }
    }
}

impl core::fmt::Display for AttachError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoFile { dir } => write!(
                f,
                "no DevToolsActivePort in {}: remote debugging is off, or Chrome is not running. \
                 Turn it on at chrome://inspect/#remote-debugging",
                dir.display()
            ),
            Self::Unreadable { dir } => write!(
                f,
                "cannot read DevToolsActivePort in {}: on macOS, grant this terminal access to \
                 Chrome's folder under System Settings > Privacy & Security > Files & Folders",
                dir.display()
            ),
            Self::Truncated { contents } => write!(
                f,
                "DevToolsActivePort does not hold an endpoint ({contents:?}); \
                 turn remote debugging off and on again"
            ),
            Self::Stale => f.write_str(
                "nothing answers on the port DevToolsActivePort names: the file is stale. \
                 Turn remote debugging on again at chrome://inspect/#remote-debugging",
            ),
            Self::Rejected { status } => write!(
                f,
                "Chrome refused the connection ({status}): Cancel was chosen (Enter chooses it), \
                 or Chrome has no open window, and then refuses without asking. \
                 Open a window and run again"
            ),
            Self::TimedOut => f.write_str(
                "Chrome's prompt timed out: Allow was not clicked. Run again and click Allow",
            ),
            Self::NotChrome { product } => write!(
                f,
                "what answered is not Chrome ({product}); the DevToolsActivePort file is stale, \
                 and nothing was typed"
            ),
            Self::Other(e) => write!(f, "could not attach to Chrome: {e}"),
        }
    }
}

impl core::error::Error for AttachError {}

/// Why no single tab was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabChoice {
    /// No tab matches; every tab is listed.
    None(Vec<PageTab>),
    /// More than one tab matches; the matches are listed.
    Ambiguous(Vec<PageTab>),
}

impl core::fmt::Display for TabChoice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (lead, tabs) = match self {
            Self::None(tabs) if tabs.is_empty() => {
                return f.write_str("Chrome has no tabs open");
            }
            Self::None(tabs) => ("no tab matches --tab; the tabs are:", tabs),
            Self::Ambiguous(tabs) => ("more than one tab matches; name one with --tab:", tabs),
        };
        f.write_str(lead)?;
        for tab in tabs {
            write!(f, "\n  {}  {}  {}", tab.key.as_str(), tab.title, tab.url)?;
        }
        Ok(())
    }
}

impl core::error::Error for TabChoice {}

/// The tab `wanted` names: by its id exactly, or else the one tab whose
/// address contains it. With nothing wanted, the only tab there is.
///
/// # Errors
/// Returns [`TabChoice`] when no tab, or more than one, matches.
pub fn choose_tab<'t>(tabs: &'t [PageTab], wanted: Option<&str>) -> Result<&'t PageTab, TabChoice> {
    if let Some(wanted) = wanted
        && let Some(tab) = tabs.iter().find(|t| t.key.as_str() == wanted)
    {
        return Ok(tab);
    }
    let matches: Vec<&PageTab> = tabs
        .iter()
        .filter(|t| wanted.is_none_or(|w| t.url.contains(w)))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(TabChoice::None(tabs.to_vec())),
        many => Err(TabChoice::Ambiguous(
            many.iter().map(|t| (*t).clone()).collect(),
        )),
    }
}
