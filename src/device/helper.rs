//! The on-device accessibility helper: what it is, and how to get it there.
//!
//! `uiautomator dump` spawns a JVM and waits a hardcoded second for the
//! accessibility event stream to fall quiet, costing about 2.5s per
//! observation. The helper in `helper/` holds an accessibility session open
//! and answers in about 50ms over a loopback port.
//!
//! # These two cannot be mixed within a run
//!
//! Android unbinds every accessibility service for as long as a `UiAutomation`
//! connection is alive, and `uiautomator dump` opens one. Measured on a Pixel:
//! the helper's port stops accepting connections outright for the duration of
//! a dump, then answers again about 1.5s later. So a run that fell back to the
//! CLI for one observation would silence the helper and fall back again for
//! the next, paying both costs forever. The reader is therefore chosen once,
//! before the run, and held.
//!
//! # Installing it
//!
//! The APK the crate was built against is bundled, so a device can be
//! provisioned with nothing but `adb`. [`Provision::assess`] says what a
//! device needs; installing is never silent and never automatic, because it
//! puts a service that can read every screen onto someone's phone.

/// The helper's application id.
pub const PACKAGE: &str = "dev.jevpilot.helper";

/// The loopback port the helper listens on, on the device.
pub const DEVICE_PORT: u16 = 18877;

/// The path that answers with a UIAutomator-shaped hierarchy.
pub const DUMP_PATH: &str = "/dump_xml";

/// The path that carries out a gesture.
pub const ACTION_PATH: &str = "/action";

/// The header carrying the session token.
pub const TOKEN_HEADER: &str = "X-Jev-Token";

/// The wire contract this crate understands.
///
/// The helper bumps its own `protocol_version` whenever it changes shape in a
/// way an older host cannot read. A mismatch sends the run down the CLI path
/// rather than into a misparse.
pub const PROTOCOL_VERSION: u32 = 2;

/// The service name the helper reports, checked before reading from it.
const SERVICE_NAME: &str = "PilotAccessibilityService";

/// The APK this crate ships, as `helper/helper_manifest.json` describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Bundled {
    /// Application id.
    pub package: &'static str,
    /// Monotonic build number, compared against what a device holds.
    pub version_code: u32,
    /// Human-readable version.
    pub version_name: &'static str,
    /// Digest of the bundled APK.
    pub sha256: &'static str,
}

/// What `helper/helper_manifest.json` said when this crate was compiled.
pub const BUNDLED: Bundled = Bundled {
    package: PACKAGE,
    version_code: bundled_u32(
        include_str!("../../helper/helper_manifest.json"),
        "version_code",
    ),
    version_name: bundled_str(
        include_str!("../../helper/helper_manifest.json"),
        "version_name",
    ),
    sha256: bundled_str(include_str!("../../helper/helper_manifest.json"), "sha256"),
};

/// The bytes of the bundled APK, for writing somewhere `adb install` can read.
pub const BUNDLED_APK: &[u8] = include_bytes!("../../helper/JevPilotHelper.apk");

/// What a device needs before the helper can be read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Provision {
    /// Installed at the bundled version and enabled. Nothing to do.
    Ready,
    /// No helper on the device.
    NotInstalled,
    /// An older build is installed.
    Outdated {
        /// The `versionCode` the device holds.
        installed: u32,
    },
    /// Installed, but the accessibility service is switched off.
    NotEnabled,
}

impl Provision {
    /// The component name in `enabled_accessibility_services`.
    pub const SERVICE_COMPONENT: &'static str =
        "dev.jevpilot.helper/dev.jevpilot.helper.PilotAccessibilityService";

    /// What this device needs, from its installed version and enabled services.
    #[must_use]
    pub fn assess(installed: Option<u32>, enabled_services: &str) -> Self {
        match installed {
            None => Self::NotInstalled,
            Some(version) if version < BUNDLED.version_code => {
                Self::Outdated { installed: version }
            }
            Some(_) if !enabled_services.contains(Self::SERVICE_COMPONENT) => Self::NotEnabled,
            Some(_) => Self::Ready,
        }
    }

    /// Whether a run can read from the helper as the device stands.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// What `enabled_accessibility_services` should become.
    ///
    /// The existing value is kept in full. A device may be running a screen
    /// reader or switch access that someone depends on, and writing only our
    /// own component into the setting would switch it off.
    #[must_use]
    pub fn enabled_services_with_helper(current: &str) -> String {
        let current = current.trim();
        // `settings get` prints `null` for a key that was never set.
        let current = if current == "null" { "" } else { current };
        if current.is_empty() {
            return Self::SERVICE_COMPONENT.to_owned();
        }
        if current
            .split(':')
            .any(|service| service == Self::SERVICE_COMPONENT)
        {
            return current.to_owned();
        }
        format!("{current}:{}", Self::SERVICE_COMPONENT)
    }

    /// One line saying what the device needs, for a caller to show a person.
    #[must_use]
    pub fn advice(self) -> &'static str {
        match self {
            Self::Ready => "the helper is installed and enabled",
            Self::NotInstalled => {
                "no helper on this device: reading a screen will take about 2.5s \
                 instead of about 50ms, and every gesture will go through the shell"
            }
            Self::Outdated { .. } => {
                "an older helper is installed: it may not speak this crate's protocol"
            }
            Self::NotEnabled => {
                "the helper is installed but switched off in accessibility settings"
            }
        }
    }
}

/// The session token authorising reads and gestures for one run.
///
/// The helper's port is reachable by every app on the device, so this is the
/// only thing between a local app and a screen reader. It never prints itself:
/// see [`crate::credential::ApiKey`] for the same treatment of the other
/// secret in this crate.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A fresh 128-bit token, hex encoded.
    ///
    /// Read from the operating system's entropy source. A token that a second
    /// run could predict or reuse would keep authorising the first run's
    /// access after it should have ended.
    ///
    /// # Errors
    /// Returns [`std::io::Error`] when the entropy source cannot be read.
    pub fn random() -> std::io::Result<Self> {
        use std::io::Read as _;
        let mut bytes = [0_u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let mut hex = String::with_capacity(32);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        Ok(Self(hex))
    }

    /// The token itself, named so that every use of it is visible.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Token {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Redacted so a token cannot reach a log through a derived `Debug`.
impl core::fmt::Debug for Token {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// What the helper says about itself, from `GET /ping`.
///
/// `/ping` is the one endpoint served without a token, so this can be read
/// before anything has been pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HelperInfo {
    service: String,
    version_name: String,
    protocol_version: u32,
    token_set: bool,
}

impl HelperInfo {
    /// Reads a `/ping` body, or `None` when it is not one.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        Some(Self {
            service: value.get("service")?.as_str()?.to_owned(),
            version_name: value
                .get("version_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            protocol_version: u32::try_from(value.get("protocol_version")?.as_u64()?).ok()?,
            token_set: value
                .get("token_set")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// The helper's human-readable version.
    #[must_use]
    pub fn version_name(&self) -> &str {
        &self.version_name
    }

    /// The wire contract the helper is speaking.
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Whether the helper already holds a session token.
    #[must_use]
    pub const fn token_set(&self) -> bool {
        self.token_set
    }

    /// Whether a run may read screens from whatever answered.
    ///
    /// Two separate checks. The port is reachable by every app on the phone,
    /// so something else may be listening there, and a screen read from an
    /// unknown program would decide the run's next action. And a helper from a
    /// newer build can describe a screen in a shape this crate misreads.
    #[must_use]
    pub fn usable(&self) -> bool {
        self.service == SERVICE_NAME && self.protocol_version == PROTOCOL_VERSION
    }
}

/// Reads one unquoted integer field out of the bundled manifest, at compile time.
const fn bundled_u32(manifest: &'static str, field: &'static str) -> u32 {
    let bytes = manifest.as_bytes();
    let start = field_value_start(bytes, field.as_bytes());
    let mut index = start;
    let mut value = 0_u32;
    while index < bytes.len() && bytes[index] >= b'0' && bytes[index] <= b'9' {
        value = value * 10 + (bytes[index] - b'0') as u32;
        index += 1;
    }
    value
}

/// Reads one quoted string field out of the bundled manifest, at compile time.
const fn bundled_str(manifest: &'static str, field: &'static str) -> &'static str {
    let bytes = manifest.as_bytes();
    let start = field_value_start(bytes, field.as_bytes()) + 1;
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'"' {
        end += 1;
    }
    match core::str::from_utf8(unsafe_free_slice(bytes, start, end)) {
        Ok(text) => text,
        Err(_) => panic!("helper_manifest.json is not UTF-8"),
    }
}

/// The offset just past `"field":` and any spaces.
const fn field_value_start(bytes: &[u8], field: &[u8]) -> usize {
    let mut index = 0;
    while index + field.len() + 2 < bytes.len() {
        if bytes[index] == b'"' && starts_with_at(bytes, index + 1, field) {
            let mut cursor = index + 1 + field.len() + 1;
            while cursor < bytes.len() && (bytes[cursor] == b':' || bytes[cursor] == b' ') {
                cursor += 1;
            }
            return cursor;
        }
        index += 1;
    }
    panic!("helper_manifest.json is missing a field this crate needs")
}

const fn starts_with_at(bytes: &[u8], at: usize, needle: &[u8]) -> bool {
    let mut index = 0;
    while index < needle.len() {
        if at + index >= bytes.len() || bytes[at + index] != needle[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// `&bytes[start..end]` in a const context.
const fn unsafe_free_slice(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    bytes.split_at(end).0.split_at(start).1
}

/// Where a run reads screens from, and why.
///
/// The helper is an optimisation. Everything works without it — slower, and
/// with `uiautomator`'s occasional malformed boxes — so nothing here ever
/// fails a run for its absence.
///
/// Falling back is **one way**. `uiautomator dump` opens a `UiAutomation`
/// connection, and Android unbinds every accessibility service while one is
/// alive: the first CLI read silences the helper for about 1.5s. A reader that
/// tried to return would fail, fall back, and pay both costs on every
/// observation for the rest of the run. So the first failure settles it, and
/// the reason kept is the first one — what explains the run, rather than what
/// it led to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    uses_helper: bool,
    why: Option<Box<str>>,
    borrowed: u32,
    rebinding: bool,
}

impl Reader {
    /// Reading through the helper.
    #[must_use]
    pub const fn helper() -> Self {
        Self {
            uses_helper: true,
            why: None,
            borrowed: 0,
            rebinding: false,
        }
    }

    /// Reading through `uiautomator dump`, for the stated reason.
    #[must_use]
    pub fn cli(why: impl Into<Box<str>>) -> Self {
        Self {
            uses_helper: false,
            why: Some(why.into()),
            borrowed: 0,
            rebinding: false,
        }
    }

    /// Whether screens are coming from the helper.
    #[must_use]
    pub const fn uses_helper(&self) -> bool {
        self.uses_helper
    }

    /// Why the CLI is being used, when it is.
    #[must_use]
    pub fn why(&self) -> Option<&str> {
        self.why.as_deref()
    }

    /// Whether a CLI reading should replace the helper's and end its use.
    ///
    /// An empty screen from the helper is not proof of an empty screen. Some
    /// windows are simply not served to an accessibility service: measured on
    /// a Pixel, Settings' Internet panel gave the helper one window and four
    /// nodes while `uiautomator dump` gave 111, and the platform's own window
    /// list showed the application window present, focused and active the
    /// whole time. `UiAutomation` is a privileged connection; an accessibility
    /// service asking for that window's content is given nothing.
    ///
    /// A screen that neither can read is genuinely empty — mid-transition, or
    /// a video filling the display — and is no reason to give up the helper
    /// for the rest of the run.
    #[must_use]
    pub const fn cli_saw_more(helper_rows: usize, cli_rows: usize) -> bool {
        helper_rows == 0 && cli_rows > 0
    }

    /// The reason to record when the helper could not see a screen the CLI
    /// could.
    #[must_use]
    pub fn blind_to_this_screen(cli_rows: usize) -> String {
        format!(
            "the helper read no rows from a screen the uiautomator CLI read {cli_rows} from, \
             so that window is not served to an accessibility service"
        )
    }

    /// Read this one screen through the CLI, and keep the helper for the next.
    ///
    /// A screen the helper cannot see is a property of that screen, not of the
    /// helper: Settings' Wi-Fi panel is withheld from accessibility services
    /// while the rest of Settings is not. Giving the helper up for the whole
    /// run would pay 2.5s on every later read to solve a problem that ended
    /// with that screen.
    ///
    /// The dump this implies silences the helper for about 1.5s, so the next
    /// failure is forgiven once — see [`Self::forgives_a_failure`].
    pub fn borrow_cli(&mut self, why: impl Into<Box<str>>) {
        self.borrowed = self.borrowed.saturating_add(1);
        self.rebinding = true;
        self.why = Some(why.into());
    }

    /// How many screens were read through the CLI without giving the helper up.
    #[must_use]
    pub const fn borrowed(&self) -> u32 {
        self.borrowed
    }

    /// Whether a dump we did is still expected to be suppressing the helper.
    ///
    /// An answer with no application window in it means one of two things: the
    /// screen is withheld from accessibility services, or the helper has not
    /// finished rebinding after a dump. The first will never improve and the
    /// second improves in about 1.5s, so only the second is worth waiting for.
    #[must_use]
    pub const fn rebinding(&self) -> bool {
        self.rebinding
    }

    /// Note that the helper has answered with a screen, so it is back.
    pub const fn settled(&mut self) {
        self.rebinding = false;
    }

    /// Whether the next helper failure means "still rebinding" rather than
    /// "gone", and should be waited out instead of ending its use.
    ///
    /// True exactly once after a borrow: `uiautomator dump` opens a
    /// `UiAutomation` connection and Android unbinds every accessibility
    /// service while one is alive, so the read straight after a dump finds the
    /// helper mid-rebind. A failure with no dump behind it is a real loss, and
    /// waiting on it would slow every remaining step.
    pub const fn forgives_a_failure(&mut self) -> bool {
        let forgiven = self.rebinding;
        self.rebinding = false;
        forgiven
    }

    /// Give up on the helper for the rest of this run.
    ///
    /// Returns whether this call was the one that changed it, so a caller can
    /// report the switch once rather than on every observation after it.
    pub fn degrade(&mut self, why: impl Into<Box<str>>) -> bool {
        if !self.uses_helper {
            return false;
        }
        self.uses_helper = false;
        self.why = Some(why.into());
        true
    }
}

/// A gesture in the shape the helper's `/action` endpoint expects.
///
/// The shell path costs about 220ms per gesture — each `adb shell` spawns a
/// process on the device — and `input text` goes through the current IME's
/// key-character map, which silently produces nothing for anything outside
/// ASCII. The helper dispatches the gesture itself and sets a field's text
/// directly, so both problems go away.
///
/// Not every command has a form here, and what is missing is chosen to match
/// what the shell path does rather than what the helper is capable of: the
/// same command must not behave differently on two devices that differ only in
/// whether a helper is installed. Those stay on the shell path, which is
/// harmless — `input` does not open a `UiAutomation` connection, so it does not
/// suppress the helper the way a dump does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    body: String,
}

impl Action {
    /// The helper form of this command, or `None` when it has none.
    ///
    /// `size` is the display, needed for the commands that carry a direction
    /// rather than geometry.
    #[must_use]
    pub fn for_command(command: &crate::device::Command, size: (i32, i32)) -> Option<Self> {
        use crate::act::{Direction, Swipe, SystemAct};
        use crate::device::Command;
        use serde_json::json;

        let (width, height) = size;
        let body = match command {
            Command::Tap(at) => json!({"cmd": "tap", "x": at.x, "y": at.y}),
            Command::DoubleTap(at) => json!({"cmd": "double_tap", "x": at.x, "y": at.y}),
            Command::LongPress(at) => {
                json!({"cmd": "long_press", "x": at.x, "y": at.y, "duration": 800})
            }
            Command::TypeText(text) => json!({"cmd": "type", "text": text.as_ref()}),
            Command::Scroll(direction) => {
                let x = width / 2;
                // Inset from both edges: a drag that starts at one is claimed
                // by the system as a navigation gesture.
                let (near, far) = (height * 3 / 4, height / 4);
                // Dragging the content up reveals what is below it.
                let (y1, y2) = match direction {
                    Direction::Down => (near, far),
                    Direction::Up => (far, near),
                };
                json!({"cmd": "swipe", "x1": x, "y1": y1, "x2": x, "y2": y2, "duration": 300})
            }
            Command::SwipeFrom { from, direction } => {
                let travel = width / 3;
                let to = match direction {
                    Swipe::Left => (from.x - travel).max(1),
                    Swipe::Right => (from.x + travel).min(width - 1),
                };
                json!({
                    "cmd": "swipe",
                    "x1": from.x, "y1": from.y,
                    "x2": to, "y2": from.y,
                    "duration": 250,
                })
            }
            Command::System(act) => {
                let global = match act {
                    SystemAct::Back => "back",
                    SystemAct::Home => "home",
                    SystemAct::AppSwitcher => "recents",
                    // There is no global action for the keyboard's enter key.
                    SystemAct::Submit => return None,
                };
                json!({"cmd": "global", "action": global})
            }
            // Neither of these has a helper form, for different reasons.
            //
            // Waiting is not a gesture: reporting success would tell a caller
            // the screen had settled when nothing had happened at all.
            //
            // Peek the helper *could* serve, as a long press, and deliberately
            // does not. The shell path refuses it, and a command that works on
            // one backend and errors on the other differs by an accident of
            // what happens to be installed.
            Command::Settle | Command::Peek(_) => return None,
        };
        Some(Self {
            body: body.to_string(),
        })
    }

    /// The JSON to POST to `/action`.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}
