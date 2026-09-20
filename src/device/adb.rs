//! Driving an Android device through `adb`.
//!
//! Building an invocation is separated from running one: the argument vectors
//! are pure functions, so the escaping rules that actually bite — remote shell
//! quoting, the ASCII limit of `input text` — are testable without a phone.

use crate::act::{Direction, Swipe, SystemAct};
use crate::snapshot::Point;
use core::fmt;

/// A device reachable over `adb`.
#[derive(Debug, Clone)]
pub struct Adb {
    serial: Box<str>,
}

/// Text that `input text` cannot deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextError {
    /// The string contains characters outside ASCII.
    NotAscii {
        /// The first character that cannot be typed.
        offender: char,
    },
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAscii { offender } => write!(
                f,
                "`input text` cannot type {offender:?}: it maps keys through the \
                 active IME's key-character map, which covers ASCII only"
            ),
        }
    }
}

impl core::error::Error for TextError {}

impl Adb {
    /// Target a specific device by serial.
    #[must_use]
    pub fn new(serial: impl Into<Box<str>>) -> Self {
        Self {
            serial: serial.into(),
        }
    }

    /// The serial every invocation is pinned to.
    #[must_use]
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Always name the device explicitly.
    ///
    /// Without `-s`, `adb` picks the only attached device and fails once a
    /// second appears — including a long-running run that a colleague plugs
    /// into halfway through.
    fn targeted(&self, rest: &[&str]) -> Vec<String> {
        let mut args = vec!["-s".to_owned(), self.serial.to_string()];
        args.extend(rest.iter().map(|part| (*part).to_owned()));
        args
    }

    /// Read the UI hierarchy, streamed straight to stdout.
    ///
    /// `exec-out` avoids writing to `/sdcard` and pulling the file back, which
    /// halves the round trips per observation.
    #[must_use]
    pub fn dump_args(&self) -> Vec<String> {
        self.targeted(&["exec-out", "uiautomator", "dump", "/dev/tty"])
    }

    /// Tap a point.
    #[must_use]
    pub fn tap_args(&self, point: Point) -> Vec<String> {
        self.targeted(&[
            "shell",
            "input",
            "tap",
            &point.x.to_string(),
            &point.y.to_string(),
        ])
    }

    /// Press and hold at a point.
    ///
    /// A swipe that goes nowhere: `input` has no long-press verb for touches,
    /// and a zero-distance swipe with a long duration is how the gesture is
    /// expressed. The hold must outlast Android's threshold of roughly half a
    /// second, or it lands as an ordinary tap.
    #[must_use]
    pub fn long_press_args(&self, at: Point) -> Vec<String> {
        const HOLD_MS: u32 = 700;
        let (x, y) = (at.x.to_string(), at.y.to_string());
        self.targeted(&[
            "shell",
            "input",
            "swipe",
            &x,
            &y,
            &x,
            &y,
            &HOLD_MS.to_string(),
        ])
    }

    /// Tap twice in quick succession at a point.
    ///
    /// Both taps travel in a single invocation. Each `adb shell` spawns a
    /// process on the device, and two separate calls routinely exceed the
    /// double-tap window, arriving as two unrelated single taps.
    #[must_use]
    pub fn double_tap_args(&self, at: Point) -> Vec<String> {
        let tap = format!("input tap {} {}", at.x, at.y);
        self.targeted(&["shell", &format!("{tap}; {tap}")])
    }

    /// Drag sideways from a point, as for swipe-to-delete.
    #[must_use]
    pub fn swipe_from_args(&self, from: Point, direction: Swipe, width: i32) -> Vec<String> {
        const DURATION_MS: u32 = 250;
        // Far enough to pass the gesture threshold, short of the screen edge
        // where the system claims the touch for back navigation.
        let travel = width / 3;
        let to = match direction {
            Swipe::Left => (from.x - travel).max(1),
            Swipe::Right => (from.x + travel).min(width - 1),
        };
        self.targeted(&[
            "shell",
            "input",
            "swipe",
            &from.x.to_string(),
            &from.y.to_string(),
            &to.to_string(),
            &from.y.to_string(),
            &DURATION_MS.to_string(),
        ])
    }

    /// Type into whatever currently has focus.
    ///
    /// # Errors
    /// Returns [`TextError::NotAscii`] for text the IME cannot deliver.
    pub fn type_text_args(&self, text: &str) -> Result<Vec<String>, TextError> {
        if let Some(offender) = text.chars().find(|c| !c.is_ascii()) {
            return Err(TextError::NotAscii { offender });
        }
        Ok(self.targeted(&["shell", "input", "text", &shell_quote(text)]))
    }

    /// Perform a system gesture.
    ///
    /// `AppSwitcher` is expressed as a key event here, which is correct on
    /// button navigation and unreliable under gesture navigation; see
    /// [`Navigation`].
    #[must_use]
    pub fn system_args(&self, gesture: SystemAct) -> Vec<String> {
        let keycode = match gesture {
            SystemAct::Back => "KEYCODE_BACK",
            SystemAct::Home => "KEYCODE_HOME",
            SystemAct::AppSwitcher => "KEYCODE_APP_SWITCH",
        };
        self.targeted(&["shell", "input", "keyevent", keycode])
    }

    /// Read the display size.
    #[must_use]
    pub fn size_args(&self) -> Vec<String> {
        self.targeted(&["shell", "wm", "size"])
    }

    /// Swipe up from the bottom edge and dwell, opening the app switcher.
    ///
    /// The gesture-navigation equivalent of the recents key. The dwell matters:
    /// the same swipe without it goes to the home screen.
    #[must_use]
    pub fn app_switcher_swipe_args(&self, width: i32, height: i32) -> Vec<String> {
        const DWELL_MS: u32 = 400;
        let x = (width / 2).to_string();
        let bottom = (height - 1).to_string();
        let midway = (height / 2).to_string();
        self.targeted(&[
            "shell",
            "input",
            "swipe",
            &x,
            &bottom,
            &x,
            &midway,
            &DWELL_MS.to_string(),
        ])
    }

    /// The hierarchy document, without what `uiautomator` prints around it.
    ///
    /// The tool writes its own status line to the same stream as the document
    /// ("UI hierchary dumped to: ...", misspelling included). XML permits
    /// nothing after the root element, so leaving it attached fails the parse
    /// of an otherwise perfectly good screen.
    #[must_use]
    pub fn extract_hierarchy(raw: &str) -> Option<&str> {
        const CLOSE: &str = "</hierarchy>";
        let start = raw.find("<hierarchy")?;
        // Searched from the opening tag onward, not from the whole string: a
        // truncated or interleaved stream can carry a closing tag *before* the
        // opening one, and an unchecked range there is inverted rather than
        // empty, which panics on the slice.
        let tail = raw.get(start..)?;
        let end = start + tail.rfind(CLOSE)? + CLOSE.len();
        raw.get(start..end)
    }

    /// Swipe to scroll one view in `direction`.
    ///
    /// The gesture stays within the middle band of the screen. Starting at the
    /// very edge would be read as system navigation — a bottom-edge swipe is
    /// home, a side-edge swipe is back — rather than as scrolling content.
    #[must_use]
    pub fn scroll_args(&self, direction: Direction, width: i32, height: i32) -> Vec<String> {
        const DURATION_MS: u32 = 300;
        let x = (width / 2).to_string();
        let near = (height * 3 / 4).to_string();
        let far = (height / 4).to_string();
        // Dragging the content up reveals what is below it.
        let (from, to) = match direction {
            Direction::Down => (&near, &far),
            Direction::Up => (&far, &near),
        };
        self.targeted(&[
            "shell",
            "input",
            "swipe",
            &x,
            from,
            &x,
            to,
            &DURATION_MS.to_string(),
        ])
    }

    /// The display size `wm size` reports, in pixels.
    ///
    /// An `Override size` line wins when present: that is what is actually
    /// rendered, and so what the hierarchy's bounds are expressed in.
    #[must_use]
    pub fn parse_size(raw: &str) -> Option<(i32, i32)> {
        let read = |label: &str| {
            let line = raw
                .lines()
                .find(|line| line.trim_start().starts_with(label))?;
            let (width, height) = line.rsplit_once(':')?.1.trim().split_once('x')?;
            Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
        };
        read("Override size").or_else(|| read("Physical size"))
    }

    /// What `uiautomator` said went wrong, if anything.
    ///
    /// Read before the exit status, not after: the tool reports these on stderr
    /// and, depending on the build, may also exit non-zero for them. Judging
    /// the status first turns a retryable condition into a fatal one and loses
    /// the actionable "unlock the device" message.
    #[must_use]
    pub fn classify_stderr(stderr: &str) -> Option<DumpFailure> {
        if stderr.contains("null root node") {
            return Some(DumpFailure::NoActiveWindow);
        }
        if stderr.contains("could not get idle state") {
            return Some(DumpFailure::NeverSettled);
        }
        None
    }

    /// Read the device's navigation mode.
    #[must_use]
    pub fn navigation_args(&self) -> Vec<String> {
        self.targeted(&["shell", "settings", "get", "secure", "navigation_mode"])
    }
}

/// A dump failure `uiautomator` reports in prose rather than in its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpFailure {
    /// The screen never stopped animating long enough to be read.
    NeverSettled,
    /// There was no readable window, which usually means a locked screen.
    NoActiveWindow,
}

/// How the device is navigated, which decides how the app switcher is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    /// Three-button or two-button navigation.
    Buttons,
    /// Gesture navigation, where there is no recents key binding.
    Gesture,
}

impl Navigation {
    /// Read the value `settings get secure navigation_mode` reports.
    ///
    /// `2` is gesture navigation; `0` and `1` are button and pill navigation.
    /// An unreadable value is treated as gesture, because assuming the key
    /// event works when it does not leaves a step silently doing nothing.
    #[must_use]
    pub fn from_setting(raw: &str) -> Self {
        match raw.trim() {
            "0" | "1" => Self::Buttons,
            _ => Self::Gesture,
        }
    }

    /// Whether `KEYCODE_APP_SWITCH` actually opens the recents view.
    ///
    /// Under gesture navigation there is no recents key binding at all, so the
    /// key event is accepted and does nothing.
    #[must_use]
    pub const fn app_switcher_responds_to_keyevent(self) -> bool {
        matches!(self, Self::Buttons)
    }
}

/// Quote a string so the device's shell sees it as one literal argument.
///
/// `adb shell` joins its arguments into a command string that the *device*
/// parses, so a metacharacter surviving the local exec is still interpreted
/// there: an unquoted `&` truncates the command and types half the text.
/// Single quotes suppress every expansion; an embedded single quote is closed,
/// escaped, and reopened.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A live Android device driven through the `adb` binary.
#[derive(Debug)]
pub struct AdbDevice {
    adb: Adb,
    platform: crate::platform::Android,
    navigation: Option<Navigation>,
    size: Option<(i32, i32)>,
    dump_attempts: u32,
}

/// Something went wrong talking to the device.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdbError {
    /// The `adb` binary could not be run at all.
    Spawn(std::io::Error),
    /// An invocation failed.
    Failed {
        /// What was run.
        args: Box<str>,
        /// What it said.
        stderr: Box<str>,
    },
    /// The screen never stopped animating long enough to be read.
    NeverSettled {
        /// How many times the dump was attempted.
        attempts: u32,
    },
    /// There was no readable window, which usually means a locked screen.
    NoActiveWindow,
    /// The hierarchy came back but could not be understood.
    Hierarchy(crate::platform::HierarchyError),
    /// The text could not be typed.
    Text(TextError),
    /// The gesture has no Android equivalent.
    Unsupported {
        /// What was asked for.
        gesture: &'static str,
    },
}

impl fmt::Display for AdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "could not run adb: {error}"),
            Self::Failed { args, stderr } => write!(f, "adb {args} failed: {stderr}"),
            Self::NeverSettled { attempts } => write!(
                f,
                "the screen was still animating after {attempts} attempts; \
                 uiautomator needs a 1s quiet gap within 10s and has no flag to relax it"
            ),
            Self::NoActiveWindow => write!(
                f,
                "no readable window; the device is usually locked — unlock it and retry"
            ),
            Self::Hierarchy(inner) => write!(f, "{inner}"),
            Self::Text(inner) => write!(f, "{inner}"),
            Self::Unsupported { gesture } => {
                write!(f, "{gesture} has no Android equivalent")
            }
        }
    }
}

impl core::error::Error for AdbError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Spawn(inner) => Some(inner),
            Self::Hierarchy(inner) => Some(inner),
            Self::Text(inner) => Some(inner),
            _ => None,
        }
    }
}

impl AdbDevice {
    /// How many times a dump is retried before giving up.
    pub const DUMP_ATTEMPTS: u32 = 5;

    /// How long a deliberate wait lasts.
    pub const SETTLE_MS: u64 = 600;

    /// Drive the device with this serial.
    #[must_use]
    pub fn new(serial: impl Into<Box<str>>) -> Self {
        Self {
            adb: Adb::new(serial),
            platform: crate::platform::Android,
            navigation: None,
            size: None,
            dump_attempts: Self::DUMP_ATTEMPTS,
        }
    }

    /// The serial this device is pinned to.
    #[must_use]
    pub fn serial(&self) -> &str {
        self.adb.serial()
    }

    /// Run one invocation and return its stdout.
    fn run(args: &[String]) -> Result<String, AdbError> {
        let output = std::process::Command::new("adb")
            .args(args)
            .output()
            .map_err(AdbError::Spawn)?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        // Classified first: the status alone does not say whether a dump is
        // usable, and for these two the prose is the more reliable signal.
        match Adb::classify_stderr(&stderr) {
            Some(DumpFailure::NoActiveWindow) => return Err(AdbError::NoActiveWindow),
            Some(DumpFailure::NeverSettled) => {
                return Err(AdbError::NeverSettled { attempts: 1 });
            }
            None => {}
        }
        if !output.status.success() {
            return Err(AdbError::Failed {
                args: args.join(" ").into_boxed_str(),
                stderr: stderr.into_boxed_str(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Read the navigation mode once and remember it.
    fn navigation(&mut self) -> Result<Navigation, AdbError> {
        if let Some(known) = self.navigation {
            return Ok(known);
        }
        let raw = Self::run(&self.adb.navigation_args())?;
        let mode = Navigation::from_setting(&raw);
        self.navigation = Some(mode);
        Ok(mode)
    }

    /// Read the display size once and remember it.
    fn size(&mut self) -> Result<(i32, i32), AdbError> {
        if let Some(known) = self.size {
            return Ok(known);
        }
        let raw = Self::run(&self.adb.size_args())?;
        // Not defaulted. Every gesture is computed from this, so guessing a
        // size sends swipes to coordinates that belong to a different display —
        // off-screen, or into the system edge-back zone — while `perform` still
        // reports success, which a loop reads as an action that worked.
        let size = Adb::parse_size(&raw).ok_or_else(|| AdbError::Failed {
            args: "shell wm size".into(),
            stderr: format!("could not read a display size from {raw:?}").into_boxed_str(),
        })?;
        self.size = Some(size);
        Ok(size)
    }
}

impl super::Device for AdbDevice {
    type Error = AdbError;

    /// Read the screen, retrying while it is still animating.
    ///
    /// `uiautomator` waits for a one-second quiet gap in the accessibility
    /// event stream within a ten-second budget, and offers no flag to relax
    /// either. A screen with a spinner, a video, or a blinking cursor can miss
    /// that window repeatedly, so the only remedy is to ask again.
    fn observe(&mut self) -> Result<crate::snapshot::Snapshot, Self::Error> {
        use crate::platform::Platform as _;

        let args = self.adb.dump_args();
        let mut last_settled_failure = None;
        for attempt in 1..=self.dump_attempts {
            match Self::run(&args) {
                Ok(raw) => {
                    let document = Adb::extract_hierarchy(&raw).ok_or(AdbError::NoActiveWindow)?;
                    return self
                        .platform
                        .parse_hierarchy(document)
                        .map_err(AdbError::Hierarchy);
                }
                Err(AdbError::NeverSettled { .. }) => {
                    last_settled_failure = Some(attempt);
                    std::thread::sleep(std::time::Duration::from_millis(300 * u64::from(attempt)));
                }
                Err(other) => return Err(other),
            }
        }
        Err(AdbError::NeverSettled {
            attempts: last_settled_failure.unwrap_or(self.dump_attempts),
        })
    }

    fn perform(&mut self, command: &super::Command) -> Result<(), Self::Error> {
        let args = match command {
            super::Command::Tap(point) => self.adb.tap_args(*point),
            super::Command::TypeText(text) => {
                self.adb.type_text_args(text).map_err(AdbError::Text)?
            }
            // Gesture navigation has no recents key binding, so the key event
            // is accepted and does nothing. Swipe instead.
            super::Command::System(SystemAct::AppSwitcher)
                if !self.navigation()?.app_switcher_responds_to_keyevent() =>
            {
                let (width, height) = self.size()?;
                self.adb.app_switcher_swipe_args(width, height)
            }
            super::Command::System(gesture) => self.adb.system_args(*gesture),
            super::Command::Scroll(direction) => {
                let (width, height) = self.size()?;
                self.adb.scroll_args(*direction, width, height)
            }
            super::Command::DoubleTap(at) => self.adb.double_tap_args(*at),
            super::Command::LongPress(at) => self.adb.long_press_args(*at),
            super::Command::SwipeFrom { from, direction } => {
                let (width, _) = self.size()?;
                self.adb.swipe_from_args(*from, *direction, width)
            }
            // Sleeping rather than watching for quiescence: the dump already
            // retries around uiautomator's own idle wait, so this only has to
            // outlast a transition the model judged worth waiting out.
            super::Command::Settle => {
                std::thread::sleep(std::time::Duration::from_millis(Self::SETTLE_MS));
                return Ok(());
            }
            // Android offers no force-touch preview, and the platform does not
            // list Peek among its operations, so this cannot be reached by a
            // decision — only by a caller building the command by hand.
            super::Command::Peek(_) => {
                return Err(AdbError::Unsupported { gesture: "peek" });
            }
        };
        Self::run(&args).map(|_| ())
    }
}
