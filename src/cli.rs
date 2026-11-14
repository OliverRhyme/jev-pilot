//! Reading the command line of the `jev-pilot` tool.
//!
//! Hand-rolled rather than delegated to an argument-parsing crate: the whole
//! surface is four subcommands and five flags, and a screen reader that can
//! drive someone's phone is a thing to keep the dependency list of short.
//!
//! Parsing is separated from doing so that what a command line means can be
//! tested without a device attached.

use crate::judgment::Confidence;

/// What the tool was asked to do.
///
/// Deliberately not `#[non_exhaustive]`: the binary is a separate crate, so
/// marking it would force a wildcard arm in the one place that dispatches on
/// it — and that arm is what lets a new subcommand compile while silently
/// doing nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    /// Pursue a goal on a device.
    Run {
        /// What to achieve, in plain words.
        goal: Box<str>,
        /// Which device, when more than one is attached.
        device: Option<String>,
        /// Claims that must hold on screen before success is accepted.
        accept: Vec<String>,
        /// How many steps before giving up.
        steps: u32,
        /// Below these confidences the run asks rather than acts.
        floors: crate::act::Floors,
        /// The application the goal is about, when the caller named one.
        app: Option<Box<str>>,
        /// Text to type, by the field it belongs in.
        texts: Vec<(Box<str>, Box<str>)>,
        /// The goal's steps, in the order the app asks for them.
        plan: Vec<String>,
        /// Keys to press in order on a keypad, one character each.
        keys: Option<String>,
        /// Where questions are written for another decider to answer.
        desk: Option<std::path::PathBuf>,
        /// The browser to drive instead of a device.
        browser: Option<BrowserTarget>,
    },
    /// Serve the Model Context Protocol over stdin and stdout.
    ///
    /// A subcommand rather than a second program: each tool call runs this
    /// same executable, so there is one file to install and nothing beside it
    /// to find.
    Mcp,
    /// Report on the on-device helper, or install it.
    Helper {
        /// Which device, when more than one is attached.
        device: Option<String>,
        /// Whether to install and enable it, rather than only report.
        install: bool,
    },
    /// List the attached devices, or a browser's tabs.
    Devices {
        /// The browser whose tabs to list instead.
        browser: Option<BrowserTarget>,
    },
    /// Print what the next step would be offered, without acting.
    Observe {
        /// Which device, when more than one is attached.
        device: Option<String>,
        /// The browser to look at instead of a device.
        browser: Option<BrowserTarget>,
    },
    /// Explain the usage.
    Help,
    /// Say which version this is.
    Version,
    /// Bring this copy up to date with the latest release.
    Update,
    /// Set up, inspect or remove the bridge to Chrome's jev-pilot extension.
    Extension(ExtensionCommand),
    /// Serve as the bridge's host: Chrome started this program for the
    /// jev-pilot extension.
    Host,
}

/// What `jev-pilot extension` was asked to do.
///
/// Not `#[non_exhaustive]`, for the reason [`Invocation`] is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionCommand {
    /// Install the host, the extension and Chrome's policy.
    Install {
        /// A user-owned development install, not protected against other
        /// programs running as the owner.
        dev: bool,
    },
    /// Say what is installed and whether the bridge answers.
    Status,
    /// Remove what the install made.
    Uninstall,
    /// Show the audit log.
    Log,
    /// Add a site to the allowed sites, or remove one.
    Sites {
        /// Whether to add it, rather than remove it.
        add: bool,
        /// The site, as an origin.
        origin: Box<str>,
    },
    /// Say which of the owner's own tabs `--tab` may take.
    OwnerTabs(OwnerTabs),
}

/// Which of the owner's tabs, not opened by a run, a run may take with
/// `--tab` in extension mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerTabs {
    /// Only tabs the owner put in the "jev-pilot" tab group.
    Group,
    /// None.
    Never,
}

/// What Ctrl-C says in a run on the owner's own Chrome, through the
/// extension or attached: it lets go of every tab and closes nothing.
pub const LET_GO: &str = "interrupted: let go of Chrome, and closed nothing";

/// How many steps a run takes before giving up, unless told otherwise.
pub const DEFAULT_STEPS: u32 = 15;

/// The confidence below which a run asks rather than acts, unless told
/// otherwise.
pub const DEFAULT_FLOOR: f64 = 0.6;

/// How long attach mode waits for the owner to click Allow, unless told
/// otherwise, in seconds.
pub const DEFAULT_ALLOW_WAIT: u64 = 120;

/// The release channels whose running Chrome can be attached to.
pub const CHANNELS: &[&str] = &["stable", "beta", "dev", "canary"];

/// Which browser, and how it is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserMode {
    /// The owner's running Chrome, through its remote-debugging prompt.
    Attach {
        /// Which channel's Chrome: one of [`CHANNELS`].
        channel: Box<str>,
        /// A browser websocket to use instead of finding one.
        endpoint: Option<Box<str>>,
        /// How long to wait for the owner to click Allow, in seconds.
        allow_wait: u64,
        /// Name the owner's tabs by id and site alone, never by title or
        /// path, wherever they are listed: for a caller other than the
        /// owner, such as an agent over MCP.
        sites_only: bool,
    },
    /// The owner's running Chrome, through the jev-pilot extension: no
    /// prompt, and only what the extension's policy allows.
    Extension {
        /// Name the owner's tabs by id and site alone, as in attach mode.
        sites_only: bool,
    },
    /// A browser started for the run, on a throwaway profile.
    Fresh {
        /// The browser binary, when named.
        chrome: Option<std::path::PathBuf>,
        /// Whether it runs without a window.
        headless: bool,
    },
}

/// The browser a command drives, and which tab of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserTarget {
    /// Which browser.
    pub mode: BrowserMode,
    /// An existing tab, by id or a piece of its address.
    pub tab: Option<Box<str>>,
    /// An address to open in a new tab and drive.
    pub open: Option<Box<str>>,
    /// The addresses `navigate` may go to; the `--open` one first.
    pub urls: Vec<Box<str>>,
}

/// A command line that could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CliError {
    /// A flag that is not one of ours.
    ///
    /// Refused rather than taken as the goal: a mistyped `--device` would
    /// otherwise become the thing the run pursues, on whatever phone happened
    /// to be attached.
    UnknownFlag(String),
    /// A flag that needs a value did not get one.
    MissingValue(&'static str),
    /// A value that is not what that flag accepts.
    BadValue {
        /// The flag.
        flag: &'static str,
        /// What followed it.
        got: String,
    },
    /// No goal was given.
    NoGoal,
    /// More than one goal was given.
    TooManyGoals,
    /// Two flags that cannot be used together.
    Conflict {
        /// The first.
        flag: &'static str,
        /// The one it conflicts with.
        with: &'static str,
        /// What to do instead.
        instead: &'static str,
    },
    /// A browser flag without `--browser`, or with the other mode.
    NeedsMode {
        /// The flag.
        flag: &'static str,
        /// The mode it belongs to.
        mode: &'static str,
    },
    /// A single word that is not a command.
    ///
    /// Refused rather than taken as the goal, as an unknown flag is: a
    /// mistyped `version` would otherwise be pursued on whatever phone
    /// happened to be attached. A goal is a sentence; a one-word goal meant
    /// as one goes after `--`.
    NotACommand(String),
    /// Started as a native messaging host by an extension other than
    /// jev-pilot's.
    ForeignOrigin(String),
}

impl core::fmt::Display for CliError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option {flag}"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::BadValue { flag, got } => write!(f, "{flag} cannot be {got:?}"),
            Self::NoGoal => f.write_str("no goal given: say what you want done, in quotes"),
            Self::Conflict {
                flag,
                with,
                instead,
            } => write!(f, "{flag} cannot be used with {with}: {instead}"),
            Self::NeedsMode { flag, mode } => write!(f, "{flag} needs --browser {mode}"),
            Self::NotACommand(word) => write!(
                f,
                "{word:?} is not a command. A goal is a sentence in quotes, like \
                 jev-pilot \"open settings\"; a one-word goal goes after --, like \
                 jev-pilot -- {word}. See jev-pilot --help for the commands"
            ),
            Self::TooManyGoals => {
                f.write_str("more than one goal given: put the whole goal in quotes")
            }
            Self::ForeignOrigin(origin) => {
                write!(f, "jev-pilot serves only its own extension, not {origin}")
            }
        }
    }
}

impl core::error::Error for CliError {}

/// Read a command line, without its program name.
///
/// # Errors
/// Returns [`CliError`] when the arguments are not a command this tool has.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Invocation, CliError> {
    let mut args = args.into_iter().peekable();

    if let Some(command) = subcommand(&mut args) {
        return command;
    }

    let mut goal: Option<String> = None;
    // Whether the goal came after `--`, where a single word is meant as one.
    let mut goal_after_dashes = false;
    let mut device = None;
    let mut accept = Vec::new();
    let mut steps = DEFAULT_STEPS;
    // Checked at construction, so the default is known good.
    let mut floor = Confidence::new(DEFAULT_FLOOR).unwrap_or(Confidence::ZERO);
    let mut desk = None;
    let mut app: Option<Box<str>> = None;
    let mut texts: Vec<(Box<str>, Box<str>)> = Vec::new();
    let mut plan = Vec::new();
    let mut keys = None;
    let mut options_ended = false;
    let mut browser = BrowserFlags::default();

    while let Some(word) = args.next() {
        if !options_ended && browser.take(&word, &mut args)? {
            continue;
        }
        if options_ended || !word.starts_with('-') {
            goal_after_dashes = options_ended;
            if goal.replace(word).is_some() {
                return Err(CliError::TooManyGoals);
            }
            continue;
        }
        match word.as_str() {
            "--" => options_ended = true,
            "--device" | "-d" => device = Some(value(&mut args, "--device")?),
            "--accept" | "-a" => accept.push(value(&mut args, "--accept")?),
            "--desk" => desk = Some(std::path::PathBuf::from(value(&mut args, "--desk")?)),
            "--steps" => {
                let got = value(&mut args, "--steps")?;
                steps = got
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(CliError::BadValue {
                        flag: "--steps",
                        got,
                    })?;
            }
            "--app" => app = Some(value(&mut args, "--app")?.into_boxed_str()),
            "--then" => plan.push(value(&mut args, "--then")?),
            "--keys" => keys = Some(value(&mut args, "--keys")?),
            "--text" => texts.push(field_text(value(&mut args, "--text")?)?),
            "--floor" => {
                let got = value(&mut args, "--floor")?;
                let parsed = got.parse::<f64>().ok().and_then(Confidence::new);
                floor = parsed.ok_or(CliError::BadValue {
                    flag: "--floor",
                    got,
                })?;
            }
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        }
    }

    let browser = browser.finish(device.is_some(), app.is_some())?;
    let goal = goal.ok_or(CliError::NoGoal)?;
    if !goal_after_dashes && !goal.trim().contains(char::is_whitespace) {
        return Err(CliError::NotACommand(goal));
    }
    Ok(Invocation::Run {
        floors: floors_from(floor),
        app,
        texts,
        plan,
        keys,
        goal: goal.into_boxed_str(),
        device,
        accept,
        steps,
        desk,
        browser,
    })
}

/// A command other than pursuing a goal, when the first word names one.
fn subcommand(
    args: &mut core::iter::Peekable<impl Iterator<Item = String>>,
) -> Option<Result<Invocation, CliError>> {
    // A command configured once and then forgotten, as `mcp` is in a client,
    // refuses a stray argument rather than ignoring it.
    fn alone(
        args: &mut impl Iterator<Item = String>,
        command: Invocation,
    ) -> Result<Invocation, CliError> {
        let _ = args.next();
        match args.next() {
            Some(other) => Err(CliError::UnknownFlag(other)),
            None => Ok(command),
        }
    }

    Some(match args.peek().map(String::as_str)? {
        // Chrome passes the caller's origin first, and on Windows a
        // `--parent-window` after it, which the host has no use for.
        origin if origin.starts_with("chrome-extension://") => {
            let origin = args.next().unwrap_or_default();
            host(&origin, args)
        }
        "extension" => {
            let _ = args.next();
            extension(args)
        }
        "--help" | "-h" | "help" => Ok(Invocation::Help),
        "--version" | "-V" | "version" => Ok(Invocation::Version),
        "devices" => {
            let _ = args.next();
            devices(args)
        }
        "update" => alone(args, Invocation::Update),
        "mcp" => alone(args, Invocation::Mcp),
        "observe" => {
            let _ = args.next();
            observe(args)
        }
        "helper" => {
            let _ = args.next();
            helper(args)
        }
        _ => return None,
    })
}

/// Host mode, for the extension whose origin Chrome passed. On Windows
/// Chrome adds one `--parent-window=<handle>`; nothing else may follow.
fn host(origin: &str, rest: impl Iterator<Item = String>) -> Result<Invocation, CliError> {
    let mut parent_window = false;
    for word in rest {
        if word.starts_with("--parent-window=") && !parent_window {
            parent_window = true;
        } else {
            return Err(CliError::UnknownFlag(word));
        }
    }
    #[cfg(feature = "browser")]
    {
        let ours = format!(
            "chrome-extension://{}/",
            crate::browser::bridge::EXTENSION_ID
        );
        if origin != ours {
            return Err(CliError::ForeignOrigin(origin.to_owned()));
        }
    }
    // Without the browser, the binary cannot serve and says so when asked.
    let _ = origin;
    Ok(Invocation::Host)
}

/// The rest of `extension`.
fn extension(mut args: impl Iterator<Item = String>) -> Result<Invocation, CliError> {
    let word = |got: Option<String>| got.ok_or(CliError::MissingValue("extension"));
    let command = match word(args.next())?.as_str() {
        "install" => match args.next().as_deref() {
            None => ExtensionCommand::Install { dev: false },
            Some("--dev") => ExtensionCommand::Install { dev: true },
            Some(other) => return Err(CliError::UnknownFlag(other.to_owned())),
        },
        "status" => ExtensionCommand::Status,
        "uninstall" => ExtensionCommand::Uninstall,
        "log" => ExtensionCommand::Log,
        "policy" => match word(args.next())?.as_str() {
            "sites" => {
                let add = match word(args.next())?.as_str() {
                    "add" => true,
                    "remove" => false,
                    other => return Err(CliError::NotACommand(other.to_owned())),
                };
                let got = value(&mut args, "sites")?;
                let origin = site_origin(&got).ok_or(CliError::BadValue { flag: "sites", got })?;
                ExtensionCommand::Sites {
                    add,
                    origin: origin.into(),
                }
            }
            "owner-tabs" => {
                let got = value(&mut args, "owner-tabs")?;
                ExtensionCommand::OwnerTabs(match got.as_str() {
                    "group" => OwnerTabs::Group,
                    "never" => OwnerTabs::Never,
                    _ => {
                        return Err(CliError::BadValue {
                            flag: "owner-tabs",
                            got,
                        });
                    }
                })
            }
            other => return Err(CliError::NotACommand(other.to_owned())),
        },
        other => return Err(CliError::NotACommand(other.to_owned())),
    };
    match args.next() {
        Some(other) => Err(CliError::UnknownFlag(other)),
        None => Ok(Invocation::Extension(command)),
    }
}

/// A site for the allowed list, as its origin in lower case: `http` or
/// `https`, a host, and perhaps a port. A `*.` before the host also allows
/// its subdomains. `None` for anything else.
#[must_use]
pub fn site_origin(site: &str) -> Option<String> {
    let site = site.to_ascii_lowercase();
    let (scheme, rest) = site.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    if port.is_some_and(|p| p.is_empty() || p.parse::<u16>().is_err()) {
        return None;
    }
    let name = host.strip_prefix("*.").unwrap_or(host);
    let label_ok = |label: &str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !name.split('.').all(label_ok) {
        return None;
    }
    Some(site)
}

/// The rest of `observe`: which device, or which browser.
fn observe(mut args: impl Iterator<Item = String>) -> Result<Invocation, CliError> {
    let mut device = None;
    let mut browser = BrowserFlags::default();
    while let Some(word) = args.next() {
        if browser.take(&word, &mut args)? {
            continue;
        }
        match word.as_str() {
            "--device" | "-d" => device = Some(value(&mut args, "--device")?),
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        }
    }
    let browser = browser.finish(device.is_some(), false)?;
    Ok(Invocation::Observe { device, browser })
}

/// The rest of `devices`: which browser's tabs, if any.
fn devices(mut args: impl Iterator<Item = String>) -> Result<Invocation, CliError> {
    let mut browser = BrowserFlags::default();
    while let Some(word) = args.next() {
        if !browser.take(&word, &mut args)? {
            return Err(CliError::UnknownFlag(word));
        }
    }
    Ok(Invocation::Devices {
        browser: browser.finish(false, false)?,
    })
}

/// Whether a browser endpoint is a plain websocket on this machine.
///
/// The DevTools endpoint carries the page and everything typed into it, with
/// no encryption, and a browser on another machine is not one a run should
/// reach; so only `ws://` to 127.0.0.1, `localhost` or `[::1]` is accepted.
#[must_use]
pub fn is_local_endpoint(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("ws://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// The browser flags, as they are met, checked against each other at the end.
#[derive(Default)]
struct BrowserFlags {
    mode: Option<String>,
    channel: Option<String>,
    endpoint: Option<String>,
    allow_wait: Option<u64>,
    chrome: Option<String>,
    headless: bool,
    sites_only: bool,
    tab: Option<String>,
    open: Option<String>,
    urls: Vec<String>,
}

impl BrowserFlags {
    /// Take `word` and its value if it is a browser flag; `false` if it is not
    /// one.
    fn take(
        &mut self,
        word: &str,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<bool, CliError> {
        match word {
            "--browser" => {
                let got = value(args, "--browser")?;
                if !matches!(got.as_str(), "attach" | "fresh" | "extension") {
                    return Err(CliError::BadValue {
                        flag: "--browser",
                        got,
                    });
                }
                self.mode = Some(got);
            }
            "--channel" => {
                let got = value(args, "--channel")?;
                if !CHANNELS.contains(&got.as_str()) {
                    return Err(CliError::BadValue {
                        flag: "--channel",
                        got,
                    });
                }
                self.channel = Some(got);
            }
            "--endpoint" => {
                let got = value(args, "--endpoint")?;
                if !is_local_endpoint(&got) {
                    return Err(CliError::BadValue {
                        flag: "--endpoint",
                        got,
                    });
                }
                self.endpoint = Some(got);
            }
            "--allow-wait" => {
                let got = value(args, "--allow-wait")?;
                self.allow_wait = Some(got.parse().ok().filter(|s| *s > 0).ok_or(
                    CliError::BadValue {
                        flag: "--allow-wait",
                        got,
                    },
                )?);
            }
            "--chrome" => self.chrome = Some(value(args, "--chrome")?),
            "--headless" => self.headless = true,
            "--sites-only" => self.sites_only = true,
            "--tab" => self.tab = Some(value(args, "--tab")?),
            "--open" => self.open = Some(value(args, "--open")?),
            "--url" => self.urls.push(value(args, "--url")?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The browser target, checked against the rest of the command line.
    fn finish(self, device: bool, app: bool) -> Result<Option<BrowserTarget>, CliError> {
        let Some(mode) = self.mode.clone() else {
            let stray = [
                (self.channel.is_some(), "--channel", "attach"),
                (self.endpoint.is_some(), "--endpoint", "attach"),
                (self.allow_wait.is_some(), "--allow-wait", "attach"),
                (self.sites_only, "--sites-only", "extension|attach"),
                (self.chrome.is_some(), "--chrome", "fresh"),
                (self.headless, "--headless", "fresh"),
                (self.tab.is_some(), "--tab", "extension|attach|fresh"),
                (self.open.is_some(), "--open", "extension|attach|fresh"),
                (!self.urls.is_empty(), "--url", "extension|attach|fresh"),
            ];
            return match stray.into_iter().find(|(set, ..)| *set) {
                Some((_, flag, mode)) => Err(CliError::NeedsMode { flag, mode }),
                None => Ok(None),
            };
        };
        if device {
            return Err(CliError::Conflict {
                flag: "--device",
                with: "--browser",
                instead: "a browser run drives a tab; choose it with --tab or --open",
            });
        }
        if app {
            return Err(CliError::Conflict {
                flag: "--app",
                with: "--browser",
                instead: "say where the run may go with --open and --url",
            });
        }
        if self.tab.is_some() && self.open.is_some() {
            return Err(CliError::Conflict {
                flag: "--tab",
                with: "--open",
                instead: "drive an existing tab, or open a new one, not both",
            });
        }
        let mode = self.mode(&mode)?;
        let mut urls: Vec<Box<str>> = Vec::new();
        for url in self.open.iter().chain(&self.urls) {
            if !urls.iter().any(|u| **u == **url) {
                urls.push(url.as_str().into());
            }
        }
        Ok(Some(BrowserTarget {
            mode,
            tab: self.tab.map(Into::into),
            open: self.open.map(Into::into),
            urls,
        }))
    }

    /// The mode `--browser` named, refusing the flags of the other modes.
    fn mode(&self, mode: &str) -> Result<BrowserMode, CliError> {
        let attach_only = [
            (self.channel.is_some(), "--channel"),
            (self.endpoint.is_some(), "--endpoint"),
            (self.allow_wait.is_some(), "--allow-wait"),
        ];
        let fresh_only = [
            (self.chrome.is_some(), "--chrome"),
            (self.headless, "--headless"),
        ];
        Ok(if mode == "extension" {
            let stray = attach_only
                .into_iter()
                .map(|(set, flag)| (set, flag, "attach"))
                .chain(
                    fresh_only
                        .into_iter()
                        .map(|(set, flag)| (set, flag, "fresh")),
                );
            for (set, flag, mode) in stray {
                if set {
                    return Err(CliError::NeedsMode { flag, mode });
                }
            }
            BrowserMode::Extension {
                sites_only: self.sites_only,
            }
        } else if mode == "attach" {
            for (set, flag) in [
                (self.chrome.is_some(), "--chrome"),
                (self.headless, "--headless"),
            ] {
                if set {
                    return Err(CliError::NeedsMode {
                        flag,
                        mode: "fresh",
                    });
                }
            }
            BrowserMode::Attach {
                channel: self
                    .channel
                    .clone()
                    .unwrap_or_else(|| "stable".to_owned())
                    .into(),
                endpoint: self.endpoint.clone().map(Into::into),
                allow_wait: self.allow_wait.unwrap_or(DEFAULT_ALLOW_WAIT),
                sites_only: self.sites_only,
            }
        } else {
            for (set, flag) in [
                (self.channel.is_some(), "--channel"),
                (self.endpoint.is_some(), "--endpoint"),
                (self.allow_wait.is_some(), "--allow-wait"),
                (self.sites_only, "--sites-only"),
            ] {
                if set {
                    return Err(CliError::NeedsMode {
                        flag,
                        mode: "attach",
                    });
                }
            }
            BrowserMode::Fresh {
                chrome: self.chrome.clone().map(std::path::PathBuf::from),
                headless: self.headless,
            }
        })
    }
}

/// The rest of `helper`: whether to install, and on which device.
fn helper(mut args: impl Iterator<Item = String>) -> Result<Invocation, CliError> {
    let mut install = false;
    let mut device = None;
    while let Some(word) = args.next() {
        match word.as_str() {
            "install" => install = true,
            "--device" | "-d" => device = Some(value(&mut args, "--device")?),
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        }
    }
    Ok(Invocation::Helper { device, install })
}

/// A `--text` value: the field's name, then `=`, then the words.
///
/// Split once: a value may hold an `=`, and a field name that does is not
/// worth the ambiguity.
fn field_text(got: String) -> Result<(Box<str>, Box<str>), CliError> {
    match got.split_once('=') {
        Some((field, text)) => Ok((field.into(), text.into())),
        None => Err(CliError::BadValue {
            flag: "--text",
            got,
        }),
    }
}

/// What one number on the command line means for actions that cost differently.
///
/// `--floor` is reached for when a run keeps stopping to ask about ordinary
/// gestures, and lowering it is the right answer to that. It is never an answer
/// about the actions that end the run or leave the app: a run that gives up, or
/// walks out of the app it was asked about, on a 0.41 guess has not been made
/// cheaper, it has been made wrong. Those keep the default as their minimum.
///
/// Raising the floor raises all three. Asking for more care means more care
/// everywhere, never less of it somewhere.
fn floors_from(floor: Confidence) -> crate::act::Floors {
    use crate::act::{Consequence, Floors};

    // Checked at construction, so the default is known good.
    let default = Confidence::new(DEFAULT_FLOOR).unwrap_or(Confidence::ZERO);
    let careful = if floor > default { floor } else { default };
    Floors::new(floor)
        .requiring_for(Consequence::Terminal, careful)
        .requiring_for(Consequence::Destructive, careful)
}

fn value(args: &mut impl Iterator<Item = String>, flag: &'static str) -> Result<String, CliError> {
    args.next().ok_or(CliError::MissingValue(flag))
}

/// What to print for `--help`.
pub const USAGE: &str = "\
jev-pilot — drive an Android device, or a web browser, toward a goal

USAGE
  jev-pilot [options] \"<goal>\"       pursue a goal
  jev-pilot devices                   list attached devices (or, with
                                      --browser, a browser's tabs)
  jev-pilot observe                   print what the next step would be offered
  jev-pilot helper [install]          report on, or install, the on-device helper
  jev-pilot extension install [--dev] set up the bridge to Chrome's jev-pilot
                                      extension (as an administrator; --dev is a
                                      user-owned install, not protected against
                                      other programs running as you)
  jev-pilot extension status          say what is set up, and whether Chrome answers
  jev-pilot extension uninstall       remove the bridge (as an administrator)
  jev-pilot extension log             show the bridge's audit log
  jev-pilot extension policy sites add|remove <origin>
                                      allow a site, or stop allowing it; an empty
                                      list allows every site (as an administrator)
  jev-pilot extension policy owner-tabs group|never
                                      whether --tab may take your own tabs in the
                                      'jev-pilot' tab group (as an administrator)
  jev-pilot mcp                       serve MCP over stdio, for an agent to drive runs
  jev-pilot update                    update to the latest release
  jev-pilot --version                 say which version this is

OPTIONS
  -d, --device <serial>   which device, when more than one is attached
  -a, --accept <claim>    a claim that must hold on screen before success is
                          accepted; repeatable. Only write claims about text
                          the final screen actually shows.
      --steps <n>         how many steps before giving up (default 15)
      --app <package>     the app the goal is about; it is brought to the
                          front first, and the run knows when it has left it
      --text <field>=<words>
                          what to type into a field whose name contains
                          <field>; repeatable. Without it, a run stops and
                          asks for every field it means to fill
      --then <step>       one step of the goal, in the order the app asks for
                          it; repeatable. Each screen is then judged against
                          the next unfinished step rather than the whole goal
      --keys <keys>       keys to press in order on a keypad, such as a PIN;
                          the run keeps count and names the next one
      --floor <0..1>      below this confidence the run asks you (default 0.6).
                          Lowering it applies to ordinary gestures only —
                          ending the run, and leaving the app, keep 0.6
      --desk <dir>        where questions are written, so another decider can
                          answer them by writing <dir>/answer.json
  -h, --help              this text

BROWSER (runs, observe and devices; needs a build with the `browser` feature)
      --browser extension drive your running Chrome through the jev-pilot
                          extension, with no prompt, within the sites and tabs
                          its policy allows. Set it up once with
                          `jev-pilot extension install`
      --browser attach    drive your running Chrome. Turn on remote debugging
                          at chrome://inspect/#remote-debugging, then click
                          Allow when Chrome asks (Enter chooses Cancel)
      --browser fresh     start a browser on a throwaway profile, removed at
                          the end
      --channel <name>    attach: stable (default), beta, dev or canary
      --endpoint <ws-url> attach: this browser websocket, instead of finding it
      --allow-wait <s>    attach: how long to wait for Allow (default 120)
      --sites-only        extension, attach: list tabs by id and site, with no
                          titles or paths
      --chrome <path>     fresh: the browser binary (else JEV_PILOT_CHROME,
                          else Chrome for Testing, Chromium or Chrome)
      --headless          fresh: run without a window
      --tab <id|text>     drive the tab with this id, or the one tab whose
                          address contains this text
      --open <url>        open a new tab at this address and drive it
      --url <url>         another address the run may navigate to; repeatable.
                          The --open address is always one of them

ENVIRONMENT
  TYPESAFE_API_KEY        the key Jev is called with
  TYPESAFE_API_KEY_FILE   a file holding it instead
  JEV_PILOT_REQUESTS      a directory to keep every request in, as sent,
                          for replaying a step with its wording changed

The helper is strongly recommended: without it each screen read takes about
2.5s instead of about 60ms. `jev-pilot helper` says what a device needs.
";
