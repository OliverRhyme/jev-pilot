//! Talking to the on-device accessibility helper. Pure: nothing here touches a
//! device.

use jev_pilot::device::helper::{HelperInfo, Token};

/// What `GET /ping` answers before any token has been pushed.
const PING: &str = r#"{"success":true,"service":"PilotAccessibilityService",
    "version_code":1,"version_name":"0.1.0","protocol_version":2,"port":18877,
    "auth_required":true,"token_set":false,"authenticated":false}"#;

#[test]
fn the_ping_answer_describes_the_helper() {
    let info = HelperInfo::parse(PING).expect("a helper ping");

    assert_eq!(info.version_name(), "0.1.0");
    assert_eq!(info.protocol_version(), 2);
    assert!(!info.token_set(), "no token has been pushed yet");
    assert!(info.usable());
}

/// The loopback port is reachable by every app on the phone, so anything at
/// all can be listening on it. Reading a screen from whatever answers would
/// hand the run's decisions to an unknown program.
#[test]
fn a_reply_from_some_other_service_is_refused() {
    let impostor = r#"{"success":true,"service":"SomethingElse",
        "protocol_version":2,"version_name":"9.9.9","token_set":true}"#;

    let info = HelperInfo::parse(impostor).expect("well-formed json");
    assert!(!info.usable(), "only our own service may be read from");
}

/// A helper from a newer build can describe the screen in a shape this crate
/// does not understand. Failing the check sends the run down the CLI path,
/// which is slow and correct, rather than into a misparse.
#[test]
fn a_helper_speaking_another_protocol_is_refused() {
    let newer = PING.replace("\"protocol_version\":2", "\"protocol_version\":3");

    let info = HelperInfo::parse(&newer).expect("well-formed json");
    assert_eq!(info.protocol_version(), 3);
    assert!(!info.usable());
}

#[test]
fn a_reply_that_is_not_the_ping_shape_is_refused() {
    assert!(HelperInfo::parse("").is_none());
    assert!(HelperInfo::parse("<html>404</html>").is_none());
    assert!(HelperInfo::parse("{}").is_none());
}

/// The token is a secret that authorises reading every screen on the device.
/// It must not reach a log through the formatting that debugging adds.
#[test]
fn a_token_does_not_print_itself() {
    let token = Token::from("c0ffee".to_owned());

    assert!(!format!("{token:?}").contains("c0ffee"), "{token:?}");
}

/// Two runs on one device must not share a token: the earlier one would keep
/// working after it should have stopped.
#[test]
fn each_token_is_fresh() {
    let (first, second) = (
        Token::random().expect("urandom"),
        Token::random().expect("urandom"),
    );

    assert_eq!(first.expose().len(), 32, "128 bits, hex encoded");
    assert!(first.expose().chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(first.expose(), second.expose());
}

// ---------------------------------------------------------------- //
// Getting the helper onto a device
// ---------------------------------------------------------------- //

use jev_pilot::device::adb::Adb;
use jev_pilot::device::helper::{BUNDLED, Provision};

fn adb() -> Adb {
    Adb::new("SERIAL123")
}

/// The crate ships the APK it was built against, so it can say precisely
/// whether the device holds that build, an older one, or none. The constants
/// are read out of the manifest at compile time by a hand-written parser, so
/// what it produced is checked against the file it read.
#[test]
fn the_bundled_helper_matches_its_manifest() {
    const MANIFEST: &str = include_str!("../helper/helper_manifest.json");

    assert_eq!(BUNDLED.package, "dev.jevpilot.helper");
    assert_eq!(BUNDLED.sha256.len(), 64, "a sha256 is 64 hex digits");
    assert!(
        MANIFEST.contains(&format!("\"version_code\": {}", BUNDLED.version_code)),
        "parsed {} out of {MANIFEST}",
        BUNDLED.version_code
    );
    assert!(
        MANIFEST.contains(BUNDLED.sha256),
        "sha256 not found in {MANIFEST}"
    );
    assert!(MANIFEST.contains(BUNDLED.version_name));
}

#[test]
fn the_installed_version_is_read_from_dumpsys() {
    let dump = "    versionCode=7 minSdk=24 targetSdk=36\n    versionName=0.2.0\n";

    assert_eq!(Adb::parse_version_code(dump), Some(7));
    assert_eq!(Adb::parse_version_code("Unable to find package"), None);
}

/// Each state needs a different action, and telling them apart wrongly either
/// reinstalls on every run or leaves an old build serving a shape this crate
/// cannot read.
#[test]
fn what_the_device_needs_is_decided_from_what_it_has() {
    let enabled =
        "com.other/.Service:dev.jevpilot.helper/dev.jevpilot.helper.PilotAccessibilityService";

    assert_eq!(Provision::assess(None, ""), Provision::NotInstalled);
    assert_eq!(
        Provision::assess(Some(BUNDLED.version_code - 1), enabled),
        Provision::Outdated {
            installed: BUNDLED.version_code - 1
        }
    );
    assert_eq!(
        Provision::assess(Some(BUNDLED.version_code), "com.other/.Service"),
        Provision::NotEnabled
    );
    assert_eq!(
        Provision::assess(Some(BUNDLED.version_code), enabled),
        Provision::Ready
    );
}

/// A device already running someone's screen reader or switch-access service
/// must keep it. Writing only our own service into the secure setting turns
/// those off, which for a person who depends on one is doing real harm.
#[test]
fn enabling_the_helper_keeps_every_other_accessibility_service() {
    let existing = "com.cyb3rko.flashdim/.service.VolumeButtonService";

    let merged = Provision::enabled_services_with_helper(existing);

    assert!(merged.starts_with(existing), "{merged}");
    assert!(merged.contains(Provision::SERVICE_COMPONENT), "{merged}");
    assert_eq!(
        merged.matches(':').count(),
        1,
        "one separator, one addition"
    );
}

/// Enabling twice must not add the component twice: the setting is a plain
/// string and the framework does not deduplicate it.
#[test]
fn enabling_an_already_enabled_helper_changes_nothing() {
    let already = Provision::enabled_services_with_helper("com.other/.S");

    assert_eq!(Provision::enabled_services_with_helper(&already), already);
}

/// An empty setting must not gain a leading separator, which the framework
/// reads as an empty component name.
#[test]
fn enabling_on_a_device_with_no_services_yields_just_the_helper() {
    assert_eq!(
        Provision::enabled_services_with_helper(""),
        Provision::SERVICE_COMPONENT
    );
    assert_eq!(
        Provision::enabled_services_with_helper("null"),
        Provision::SERVICE_COMPONENT,
        "`settings get` prints `null` for an unset key"
    );
}

/// The token is delivered by a broadcast only a sender holding
/// WRITE_SECURE_SETTINGS can make, and it is shell-quoted like any other
/// argument that crosses into the device's shell.
#[test]
fn the_token_is_delivered_by_an_explicitly_targeted_broadcast() {
    let args = adb().push_token_args(&Token::from("abc123".to_owned()));
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();

    assert_eq!(&parts[2..5], &["shell", "am", "broadcast"]);
    assert!(parts.contains(&"-n"), "{parts:?}");
    assert!(
        parts
            .iter()
            .any(|p| p.contains("dev.jevpilot.helper/.TokenReceiver")),
        "the receiver is named, so no other app can be handed the token: {parts:?}"
    );
    assert!(parts.iter().any(|p| p.contains("'abc123'")), "{parts:?}");
}

// ---------------------------------------------------------------- //
// Falling back
// ---------------------------------------------------------------- //

use jev_pilot::device::helper::Reader;

/// The helper is an optimisation, never a requirement. A device without one
/// reads screens through the CLI and every other part of a run is unchanged.
#[test]
fn a_device_without_a_helper_still_reads_screens() {
    let reader = Reader::cli("no helper installed");

    assert!(!reader.uses_helper());
    assert_eq!(reader.why(), Some("no helper installed"));
}

/// A helper can stop answering mid-run: the service is killed by the ROM, the
/// phone is unplugged and replugged, someone turns it off in settings. None of
/// those should end a run that the CLI could finish.
#[test]
fn losing_the_helper_mid_run_falls_back_rather_than_failing() {
    let mut reader = Reader::helper();
    assert!(reader.uses_helper());

    let changed = reader.degrade("helper stopped answering");

    assert!(changed);
    assert!(!reader.uses_helper());
    assert_eq!(reader.why(), Some("helper stopped answering"));
}

/// Falling back is a one-way door, and deliberately so. `uiautomator dump`
/// opens a UiAutomation connection, and Android unbinds every accessibility
/// service while one is alive, so the first CLI read silences the helper.
/// Going back to it would fail, degrade, and pay both costs on every
/// observation for the rest of the run.
#[test]
fn falling_back_never_reverses() {
    let mut reader = Reader::helper();
    reader.degrade("first cause");
    let changed = reader.degrade("a later symptom");

    assert!(!changed, "already fallen back");
    assert_eq!(
        reader.why(),
        Some("first cause"),
        "the first cause is what explains the run, not what it led to"
    );
}
