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
        /// Where questions are written for another decider to answer.
        desk: Option<std::path::PathBuf>,
    },
    /// Report on the on-device helper, or install it.
    Helper {
        /// Which device, when more than one is attached.
        device: Option<String>,
        /// Whether to install and enable it, rather than only report.
        install: bool,
    },
    /// List the attached devices.
    Devices,
    /// Print what the next step would be offered, without acting.
    Observe {
        /// Which device, when more than one is attached.
        device: Option<String>,
    },
    /// Explain the usage.
    Help,
}

/// How many steps a run takes before giving up, unless told otherwise.
pub const DEFAULT_STEPS: u32 = 15;

/// The confidence below which a run asks rather than acts, unless told
/// otherwise.
pub const DEFAULT_FLOOR: f64 = 0.6;

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
}

impl core::fmt::Display for CliError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option {flag}"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::BadValue { flag, got } => write!(f, "{flag} cannot be {got:?}"),
            Self::NoGoal => f.write_str("no goal given: say what you want done, in quotes"),
            Self::TooManyGoals => {
                f.write_str("more than one goal given: put the whole goal in quotes")
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

    match args.peek().map(String::as_str) {
        Some("--help" | "-h" | "help") => return Ok(Invocation::Help),
        Some("devices") => {
            let _ = args.next();
            return Ok(Invocation::Devices);
        }
        Some("observe") => {
            let _ = args.next();
            let mut device = None;
            while let Some(word) = args.next() {
                match word.as_str() {
                    "--device" | "-d" => device = Some(value(&mut args, "--device")?),
                    other => return Err(CliError::UnknownFlag(other.to_owned())),
                }
            }
            return Ok(Invocation::Observe { device });
        }
        Some("helper") => {
            let _ = args.next();
            let mut install = false;
            let mut device = None;
            while let Some(word) = args.next() {
                match word.as_str() {
                    "install" => install = true,
                    "--device" | "-d" => device = Some(value(&mut args, "--device")?),
                    other => return Err(CliError::UnknownFlag(other.to_owned())),
                }
            }
            return Ok(Invocation::Helper { device, install });
        }
        _ => {}
    }

    let mut goal: Option<String> = None;
    let mut device = None;
    let mut accept = Vec::new();
    let mut steps = DEFAULT_STEPS;
    // Checked at construction, so the default is known good.
    let mut floor = Confidence::new(DEFAULT_FLOOR).unwrap_or(Confidence::ZERO);
    let mut desk = None;
    let mut app: Option<Box<str>> = None;
    let mut options_ended = false;

    while let Some(word) = args.next() {
        if options_ended || !word.starts_with('-') {
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

    let goal = goal.ok_or(CliError::NoGoal)?;
    Ok(Invocation::Run {
        floors: floors_from(floor),
        app,
        goal: goal.into_boxed_str(),
        device,
        accept,
        steps,
        desk,
    })
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
jev-pilot — drive an Android device toward a goal

USAGE
  jev-pilot [options] \"<goal>\"       pursue a goal
  jev-pilot devices                   list attached devices
  jev-pilot observe                   print what the next step would be offered
  jev-pilot helper [install]          report on, or install, the on-device helper

OPTIONS
  -d, --device <serial>   which device, when more than one is attached
  -a, --accept <claim>    a claim that must hold on screen before success is
                          accepted; repeatable. Only write claims about text
                          the final screen actually shows.
      --steps <n>         how many steps before giving up (default 15)
      --app <package>     the app the goal is about; it is brought to the
                          front first, and the run knows when it has left it
      --floor <0..1>      below this confidence the run asks you (default 0.6).
                          Lowering it applies to ordinary gestures only —
                          ending the run, and leaving the app, keep 0.6
      --desk <dir>        where questions are written, so another decider can
                          answer them by writing <dir>/answer.json
  -h, --help              this text

ENVIRONMENT
  TYPESAFE_API_KEY        the key Jev is called with
  TYPESAFE_API_KEY_FILE   a file holding it instead

The helper is strongly recommended: without it each screen read takes about
2.5s instead of about 60ms. `jev-pilot helper` says what a device needs.
";
