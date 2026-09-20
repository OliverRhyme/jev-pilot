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
            SystemAct::Submit => "KEYCODE_ENTER",
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

    /// Open a tunnel from a host port to a port on the device.
    ///
    /// `tcp:0` asks adb to allocate the host port and print it. A fixed number
    /// would collide the moment a second process attached to the same phone.
    #[must_use]
    pub fn forward_args(&self, device_port: u16) -> Vec<String> {
        self.targeted(&["forward", "tcp:0", &format!("tcp:{device_port}")])
    }

    /// The host port adb allocated, as it prints it.
    #[must_use]
    pub fn parse_forward_port(raw: &str) -> Option<u16> {
        raw.trim().lines().next()?.trim().parse().ok()
    }

    /// Every forward adb currently holds, for every device.
    ///
    /// `--list` ignores `-s` and prints the whole table, so the serial is
    /// matched in [`Self::parse_forward_reuse`] rather than by adb.
    #[must_use]
    pub fn forward_list_args(&self) -> Vec<String> {
        self.targeted(&["forward", "--list"])
    }

    /// Close a tunnel by the host port it occupies.
    #[must_use]
    pub fn forward_remove_args(&self, local_port: u16) -> Vec<String> {
        self.targeted(&["forward", "--remove", &format!("tcp:{local_port}")])
    }

    /// The host port of a tunnel already reaching `device_port` on this device.
    ///
    /// adb allocates a fresh host port for every `tcp:0` and never reclaims
    /// one, so opening a tunnel per run leaks a port per run. Reusing the
    /// existing tunnel caps it at one per device port. Lines read
    /// `<serial> tcp:<local> tcp:<remote>`, and a row for another device
    /// reaches another phone's screen, so the serial must match.
    #[must_use]
    pub fn parse_forward_reuse(&self, raw: &str, device_port: u16) -> Option<u16> {
        let remote = format!("tcp:{device_port}");
        raw.lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                Some((fields.next()?, fields.next()?, fields.next()?))
            })
            .find(|&(serial, _, found)| serial == &*self.serial && found == remote)
            .and_then(|(_, local, _)| local.strip_prefix("tcp:")?.parse().ok())
    }

    /// What `dumpsys package` says about the helper, to read its version from.
    #[must_use]
    pub fn package_info_args(&self, package: &str) -> Vec<String> {
        self.targeted(&["shell", "dumpsys", "package", package])
    }

    /// The `versionCode` in a `dumpsys package` report.
    ///
    /// The word appears in several unrelated lines, so the number is taken
    /// from the first `versionCode=` and nothing else.
    #[must_use]
    pub fn parse_version_code(raw: &str) -> Option<u32> {
        raw.split("versionCode=")
            .nth(1)?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    }

    /// Read the accessibility services the device has switched on.
    #[must_use]
    pub fn enabled_services_args(&self) -> Vec<String> {
        self.targeted(&[
            "shell",
            "settings",
            "get",
            "secure",
            "enabled_accessibility_services",
        ])
    }

    /// Write the accessibility services the device should have switched on.
    ///
    /// The value must already include everything that was there: see
    /// [`Provision::enabled_services_with_helper`].
    ///
    /// [`Provision::enabled_services_with_helper`]: crate::device::helper::Provision::enabled_services_with_helper
    #[must_use]
    pub fn set_enabled_services_args(&self, services: &str) -> Vec<String> {
        self.targeted(&[
            "shell",
            "settings",
            "put",
            "secure",
            "enabled_accessibility_services",
            &shell_quote(services),
        ])
    }

    /// Switch the accessibility subsystem on, which `put` alone does not do.
    #[must_use]
    pub fn enable_accessibility_args(&self) -> Vec<String> {
        self.targeted(&[
            "shell",
            "settings",
            "put",
            "secure",
            "accessibility_enabled",
            "1",
        ])
    }

    /// Install an APK, replacing any older build of the same package.
    #[must_use]
    pub fn install_args(&self, apk_path: &str) -> Vec<String> {
        self.targeted(&["install", "-r", apk_path])
    }

    /// Hand the helper the session token for this run.
    ///
    /// The receiver is named explicitly rather than left to the action alone,
    /// so the broadcast cannot be picked up by another app that registered the
    /// same action. On the device side the receiver is guarded by
    /// `WRITE_SECURE_SETTINGS`, which only the adb shell user and the system
    /// hold.
    #[must_use]
    pub fn push_token_args(&self, token: &crate::device::helper::Token) -> Vec<String> {
        self.targeted(&[
            "shell",
            "am",
            "broadcast",
            "-n",
            &format!("{}/.TokenReceiver", crate::device::helper::PACKAGE),
            "-a",
            &format!("{}.SET_TOKEN", crate::device::helper::PACKAGE),
            "--es",
            "token",
            &shell_quote(token.expose()),
        ])
    }

    /// Ask adb which devices are attached.
    #[must_use]
    pub fn devices_args() -> Vec<String> {
        vec!["devices".to_owned()]
    }

    /// The serials `adb devices` reports as ready to be driven.
    ///
    /// A device that is `unauthorized` or `offline` is listed by adb but
    /// cannot be driven, and offering one as a choice only moves the failure
    /// somewhere less clear.
    #[must_use]
    pub fn parse_devices(raw: &str) -> Vec<String> {
        raw.lines()
            .skip_while(|line| line.starts_with("List of devices"))
            .filter_map(|line| {
                let (serial, state) = line.split_once('\t')?;
                (state.trim() == "device").then(|| serial.trim().to_owned())
            })
            .collect()
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

/// Where a screen is read from.
///
/// The two differ by roughly fifty times, and agree on what they see. `uiautomator dump` spawns a JVM and
/// then waits a hardcoded second for the accessibility event stream to fall
/// quiet, costing about 2.5s per observation with no flag to relax it. An
/// accessibility helper already holding that session open answers in about
/// 50ms, and emits the same document, so the reader is shared.
///
/// Checked rather than assumed: on a scrolled search-results list the two
/// backends matched 23 of 23 labelled elements in both directions, with no
/// negative or off-screen bounds on either side. Their *total* node counts
/// differ — the helper prunes unlabelled structural wrappers — which looks
/// alarming and is not, because only labelled elements are ever acted on.
/// Worth re-checking on a busy screen after either side changes: a reader that
/// silently drops a row is worse than a slow one, because the loop reads the
/// absence as the screen genuinely not offering it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Hierarchy {
    /// `uiautomator dump`, over adb. Needs nothing installed.
    Cli,
    /// A helper serving the hierarchy over a tunnelled HTTP port.
    #[cfg(feature = "http")]
    Helper {
        /// Where to GET the document, e.g. `http://127.0.0.1:18899/dump_xml`.
        endpoint: Box<str>,
        /// Sent as [`helper::TOKEN_HEADER`], when the helper requires one.
        ///
        /// [`helper::TOKEN_HEADER`]: crate::device::helper::TOKEN_HEADER
        token: Option<Box<str>>,
    },
}

/// A live Android device driven through the `adb` binary.
#[derive(Debug)]
pub struct AdbDevice {
    hierarchy: Hierarchy,
    reader: crate::device::helper::Reader,
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

    /// How many times enabling the helper is attempted before giving up.
    pub const ENABLE_ATTEMPTS: u32 = 4;

    /// How long to wait before believing the accessibility setting stuck.
    pub const ENABLE_SETTLE_MS: u64 = 500;

    /// How many times a screen with nothing on it is read again.
    pub const EMPTY_ATTEMPTS: u32 = 4;

    /// The first backoff between those reads; it grows with each attempt.
    pub const EMPTY_BACKOFF_MS: u64 = 150;

    /// Drive the device with this serial.
    #[must_use]
    pub fn new(serial: impl Into<Box<str>>) -> Self {
        Self {
            hierarchy: Hierarchy::Cli,
            reader: crate::device::helper::Reader::cli("the helper was not asked for"),
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

    /// Read screens from somewhere other than the `uiautomator` CLI.
    #[must_use]
    pub fn reading_from(mut self, hierarchy: Hierarchy) -> Self {
        self.hierarchy = hierarchy;
        self
    }

    /// Open a tunnel to a helper on the device and read screens through it.
    ///
    /// A tunnel already reaching `device_port` on this device is reused. adb
    /// allocates a fresh host port for every `tcp:0` and reclaims none, so
    /// opening one per run leaks a port per run; reusing caps it at one.
    ///
    /// # Errors
    /// Returns [`AdbError`] when the tunnel cannot be opened or adb does not
    /// report the port it allocated.
    #[cfg(feature = "http")]
    pub fn through_helper(
        self,
        device_port: u16,
        path: &str,
        token: Option<&str>,
    ) -> Result<Self, AdbError> {
        let existing = Self::run(&self.adb.forward_list_args())
            .ok()
            .and_then(|listing| self.adb.parse_forward_reuse(&listing, device_port));
        let local = match existing {
            Some(port) => port,
            None => Self::open_forward(&self.adb, device_port)?,
        };
        Ok(self.reading_from(Hierarchy::Helper {
            endpoint: format!("http://127.0.0.1:{local}{path}").into_boxed_str(),
            token: token.map(Box::from),
        }))
    }

    /// Ask adb for a fresh host port tunnelled to `device_port`.
    #[cfg(feature = "http")]
    fn open_forward(adb: &Adb, device_port: u16) -> Result<u16, AdbError> {
        let raw = Self::run(&adb.forward_args(device_port))?;
        Adb::parse_forward_port(&raw).ok_or_else(|| AdbError::Failed {
            args: "forward tcp:0".into(),
            stderr: format!("adb did not report a host port, said {raw:?}").into_boxed_str(),
        })
    }

    /// What this device needs before the helper can serve it.
    ///
    /// # Errors
    /// Returns [`AdbError`] when the device cannot be questioned.
    pub fn helper_provision(&self) -> Result<crate::device::helper::Provision, AdbError> {
        use crate::device::helper::{BUNDLED, Provision};
        let installed =
            Adb::parse_version_code(&Self::run(&self.adb.package_info_args(BUNDLED.package))?);
        let enabled = Self::run(&self.adb.enabled_services_args())?;
        Ok(Provision::assess(installed, &enabled))
    }

    /// Put the bundled helper on the device and switch it on.
    ///
    /// This installs a service that can read every screen on someone's phone,
    /// so it is never called as a side effect of anything else: a caller asks
    /// for it, having told the person what it is.
    ///
    /// Every accessibility service already enabled stays enabled — a device
    /// may be running a screen reader that someone depends on.
    ///
    /// # Errors
    /// Returns [`AdbError`] when the APK cannot be staged, installed, or
    /// enabled.
    pub fn install_helper(&self) -> Result<(), AdbError> {
        use crate::device::helper::{BUNDLED, BUNDLED_APK, Provision};

        let staged =
            std::env::temp_dir().join(format!("{}-{}.apk", BUNDLED.package, BUNDLED.version_code));
        std::fs::write(&staged, BUNDLED_APK).map_err(AdbError::Spawn)?;
        let staged = staged.to_string_lossy().into_owned();

        Self::run(&self.adb.install_args(&staged))?;
        let _ = std::fs::remove_file(&staged);

        // Right after `pm install` the accessibility subsystem has not yet
        // resolved the new component, and it prunes what it cannot resolve
        // back out of the setting. The write returns success either way, and
        // an immediate read sees a value that is gone a moment later, so each
        // attempt waits and re-reads before believing it.
        for _ in 0..Self::ENABLE_ATTEMPTS {
            let current = Self::run(&self.adb.enabled_services_args())?;
            let merged = Provision::enabled_services_with_helper(&current);
            Self::run(&self.adb.set_enabled_services_args(&merged))?;
            Self::run(&self.adb.enable_accessibility_args())?;
            std::thread::sleep(std::time::Duration::from_millis(Self::ENABLE_SETTLE_MS));
            if Self::run(&self.adb.enabled_services_args())?.contains(Provision::SERVICE_COMPONENT)
            {
                return Ok(());
            }
        }
        Err(AdbError::Failed {
            args: "settings put secure enabled_accessibility_services".into(),
            stderr: format!(
                "the helper would not stay enabled after {} attempts; \
                 enable it by hand in Settings > Accessibility",
                Self::ENABLE_ATTEMPTS,
            )
            .into_boxed_str(),
        })
    }

    /// Read screens through the helper for the rest of this run.
    ///
    /// Opens the tunnel, checks that what answers is our own helper speaking a
    /// protocol this crate understands, gives it a fresh session token, and
    /// confirms the token took.
    ///
    /// The reader is chosen here and held. Falling back to the CLI for a
    /// single observation would unbind the helper — Android suppresses every
    /// accessibility service while a `UiAutomation` connection is alive — and
    /// the next observation would fall back too, paying both costs for the
    /// rest of the run.
    ///
    /// # Errors
    /// Returns [`AdbError`] when the helper is absent, is something else, or
    /// speaks another protocol.
    #[cfg(feature = "http")]
    pub fn through_jev_helper(self) -> Result<Self, AdbError> {
        use crate::device::helper::{DEVICE_PORT, DUMP_PATH, HelperInfo, Token};

        let local = match Self::run(&self.adb.forward_list_args())
            .ok()
            .and_then(|listing| self.adb.parse_forward_reuse(&listing, DEVICE_PORT))
        {
            Some(port) => port,
            None => Self::open_forward(&self.adb, DEVICE_PORT)?,
        };
        let base = format!("http://127.0.0.1:{local}");

        let ping = Self::read_helper(&format!("{base}/ping"), None)?;
        let info = HelperInfo::parse(&ping).ok_or_else(|| AdbError::Failed {
            args: "helper ping".into(),
            stderr: "the loopback port answered with something that is not a helper ping".into(),
        })?;
        if !info.usable() {
            return Err(AdbError::Failed {
                args: "helper ping".into(),
                stderr: format!(
                    "refusing to read screens from what answered on port {DEVICE_PORT}: \
                     it reports protocol {} where this crate speaks {}",
                    info.protocol_version(),
                    crate::device::helper::PROTOCOL_VERSION,
                )
                .into_boxed_str(),
            });
        }

        let token = Token::random().map_err(AdbError::Spawn)?;
        Self::run(&self.adb.push_token_args(&token))?;

        let endpoint = format!("{base}{DUMP_PATH}").into_boxed_str();
        let mut device = self.reading_from(Hierarchy::Helper {
            endpoint,
            token: Some(Box::from(token.expose())),
        });
        device.reader = crate::device::helper::Reader::helper();
        Ok(device)
    }

    /// Read through the helper when this device has a usable one, and through
    /// the CLI when it does not.
    ///
    /// Never fails for the helper's absence: the helper is an optimisation,
    /// and a run without one is slower rather than broken. [`Self::reader`]
    /// afterwards says which was chosen, and why.
    ///
    /// # Errors
    /// Returns [`AdbError`] only when the device itself cannot be reached.
    #[cfg(feature = "http")]
    pub fn with_helper(self) -> Result<Self, AdbError> {
        use crate::device::helper::Reader;

        let provision = self.helper_provision()?;
        if !provision.is_ready() {
            let why = provision.advice();
            return Ok(Self {
                reader: Reader::cli(why),
                ..self
            });
        }
        let serial = self.adb.serial().to_owned();
        match self.through_jev_helper() {
            Ok(attached) => Ok(attached),
            Err(error) => {
                let why = format!("helper did not answer: {error}");
                Ok(Self {
                    reader: Reader::cli(why),
                    ..AdbDevice::new(serial)
                })
            }
        }
    }

    /// Where this device is reading screens from, and why.
    #[must_use]
    pub const fn reader(&self) -> &crate::device::helper::Reader {
        &self.reader
    }

    /// Send a gesture to the helper over the tunnel.
    #[cfg(feature = "http")]
    fn post_action(endpoint: &str, token: Option<&str>, body: &str) -> Result<(), AdbError> {
        let mut request = ureq::post(endpoint).header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header(crate::device::helper::TOKEN_HEADER, token);
        }
        let mut response = request.send(body).map_err(|error| AdbError::Failed {
            args: "helper action".into(),
            stderr: error.to_string().into_boxed_str(),
        })?;
        let answered = response
            .body_mut()
            .read_to_string()
            .map_err(|error| AdbError::Failed {
                args: "helper action".into(),
                stderr: error.to_string().into_boxed_str(),
            })?;
        // The helper answers 200 with `success: false` for a gesture the
        // system refused, so the status alone does not say whether it happened.
        if answered.contains("\"success\":true") {
            return Ok(());
        }
        Err(AdbError::Failed {
            args: "helper action".into(),
            stderr: format!("the helper did not perform it: {answered}").into_boxed_str(),
        })
    }

    /// Fetch a screen from a helper over the tunnel.
    #[cfg(feature = "http")]
    fn read_helper(endpoint: &str, token: Option<&str>) -> Result<String, AdbError> {
        let mut request = ureq::get(endpoint);
        if let Some(token) = token {
            request = request.header(crate::device::helper::TOKEN_HEADER, token);
        }
        let mut response = request.call().map_err(|error| AdbError::Failed {
            args: "helper".into(),
            stderr: error.to_string().into_boxed_str(),
        })?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|error| AdbError::Failed {
                args: "helper".into(),
                stderr: error.to_string().into_boxed_str(),
            })
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

    /// One reading of the screen, from whichever backend is in use.
    fn read_screen(&mut self) -> Result<crate::snapshot::Snapshot, AdbError> {
        // A helper that stops answering - killed by the ROM, switched off, or
        // cut off by a replug - must not end a run the CLI could finish. The
        // switch is permanent: see `Reader` for why going back is worse than
        // staying.
        #[cfg(feature = "http")]
        if let Hierarchy::Helper { endpoint, token } = &self.hierarchy {
            match Self::read_helper(endpoint, token.as_deref()) {
                Ok(raw) => return self.parse(&raw),
                Err(error) => {
                    self.reader
                        .degrade(format!("helper stopped answering: {error}"));
                    self.hierarchy = Hierarchy::Cli;
                }
            }
        }
        self.read_via_cli()
    }

    /// Parse a hierarchy document as this platform describes screens.
    fn parse(&self, raw: &str) -> Result<crate::snapshot::Snapshot, AdbError> {
        use crate::platform::Platform as _;
        let document = Adb::extract_hierarchy(raw).ok_or(AdbError::NoActiveWindow)?;
        self.platform
            .parse_hierarchy(document)
            .map_err(AdbError::Hierarchy)
    }

    /// One reading through `uiautomator dump`, whatever the helper is doing.
    fn read_via_cli(&mut self) -> Result<crate::snapshot::Snapshot, AdbError> {
        let args = self.adb.dump_args();
        let mut last_settled_failure = None;
        for attempt in 1..=self.dump_attempts {
            match Self::run(&args) {
                Ok(raw) => return self.parse(&raw),
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
        // A screen caught between two others parses to nothing, and handing
        // that to a model asks it to choose among no rows — which it answers
        // by waiting, spending a step to recover from a read that should have
        // looked again. Reading faster made this visible rather than causing
        // it: through the 2.5s CLI a transition was usually over before the
        // dump returned, where the helper reads straight into it.
        //
        // A screen that is genuinely empty is returned as it is, once the
        // attempts are spent.
        let mut blank = self.read_screen()?;
        for attempt in 1..Self::EMPTY_ATTEMPTS {
            if blank.worth_acting_on() {
                return Ok(blank);
            }
            std::thread::sleep(std::time::Duration::from_millis(
                Self::EMPTY_BACKOFF_MS * u64::from(attempt),
            ));
            blank = self.read_screen()?;
        }
        if blank.worth_acting_on() {
            return Ok(blank);
        }

        // Still nothing. An empty screen from the helper is not proof of an
        // empty screen: some windows are simply not served to an accessibility
        // service, and the CLI's privileged connection reads them anyway. One
        // dump settles which of the two this is.
        #[cfg(feature = "http")]
        if self.reader.uses_helper() {
            let via_cli = self.read_via_cli()?;
            let seen = via_cli.refs().count();
            if crate::device::helper::Reader::cli_saw_more(0, seen) {
                self.reader
                    .degrade(crate::device::helper::Reader::blind_to_this_screen(seen));
                self.hierarchy = Hierarchy::Cli;
            }
            // Whatever it saw is a better answer than the helper's nothing;
            // the helper is kept when the CLI saw nothing either, because then
            // the screen really is empty and that is no fault of the helper's.
            return Ok(via_cli);
        }
        Ok(blank)
    }

    fn perform(&mut self, command: &super::Command) -> Result<(), Self::Error> {
        // A gesture through the helper is dispatched in-process; through the
        // shell it spawns a process on the device, which measures at about
        // 220ms. Text goes to the field directly rather than through the IME's
        // key-character map, which silently drops everything outside ASCII.
        //
        // A failure here degrades the reader, like a failed read: the helper
        // is gone, and the rest of the run belongs on the shell path.
        #[cfg(feature = "http")]
        if let Hierarchy::Helper { endpoint, token } = &self.hierarchy {
            let endpoint = endpoint.replace(
                crate::device::helper::DUMP_PATH,
                crate::device::helper::ACTION_PATH,
            );
            let token = token.clone();
            let size = self.size()?;
            if let Some(action) = crate::device::helper::Action::for_command(command, size) {
                match Self::post_action(&endpoint, token.as_deref(), action.body()) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        self.reader
                            .degrade(format!("helper would not act: {error}"));
                        self.hierarchy = Hierarchy::Cli;
                    }
                }
            }
        }

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
