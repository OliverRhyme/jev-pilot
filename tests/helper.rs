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

// ---------------------------------------------------------------- //
// Acting through the helper
// ---------------------------------------------------------------- //

use jev_pilot::act::{Direction, Swipe, SystemAct};
use jev_pilot::device::Command;
use jev_pilot::device::helper::Action;
use jev_pilot::snapshot::Point;

fn body_of(command: &Command) -> String {
    Action::for_command(command, (1080, 2400))
        .expect("the helper serves this command")
        .body()
        .to_owned()
}

/// Every gesture the loop can issue must have a helper form, or the device
/// would silently do nothing for the ones it does not.
#[test]
fn every_gesture_has_a_helper_form() {
    let at = Point { x: 540, y: 1200 };
    for command in [
        Command::Tap(at),
        Command::DoubleTap(at),
        Command::LongPress(at),
        Command::TypeText("hello".into()),
        Command::Scroll(Direction::Down),
        Command::SwipeFrom {
            from: at,
            direction: Swipe::Left,
        },
        Command::System(SystemAct::Back),
    ] {
        assert!(
            Action::for_command(&command, (1080, 2400)).is_some(),
            "no helper form for {command:?}"
        );
    }
}

/// What the helper serves must not differ from what the shell serves, or the
/// same command would do different things on two devices that differ only in
/// whether a helper happens to be installed. Waiting is not a gesture; Android
/// has no force-touch preview and the shell path refuses `Peek`, so the helper
/// refuses it too even though it could dispatch a long press.
#[test]
fn the_helper_serves_no_command_the_shell_refuses() {
    let at = Point { x: 540, y: 1200 };

    assert!(Action::for_command(&Command::Settle, (1080, 2400)).is_none());
    assert!(Action::for_command(&Command::Peek(at), (1080, 2400)).is_none());
    assert!(
        Action::for_command(&Command::System(SystemAct::Submit), (1080, 2400)).is_none(),
        "there is no global action for the keyboard's enter key"
    );
}

#[test]
fn a_tap_carries_the_point_it_was_given() {
    let body = body_of(&Command::Tap(Point { x: 540, y: 1200 }));

    assert!(body.contains("\"cmd\":\"tap\""), "{body}");
    assert!(body.contains("\"x\":540"), "{body}");
    assert!(body.contains("\"y\":1200"), "{body}");
}

/// The shell path refuses non-ASCII: `input text` goes through the IME's
/// key-character map, which silently produces nothing for emoji and CJK. The
/// helper sets the field's text directly, so there is nothing to refuse.
#[test]
fn text_the_shell_could_not_type_goes_through_unchanged() {
    let body = body_of(&Command::TypeText("こんにちは 🎉".into()));

    assert!(body.contains("\"cmd\":\"type\""), "{body}");
    assert!(body.contains("こんにちは 🎉"), "{body}");
}

/// A quote or a backslash in a label must not end the JSON string early.
#[test]
fn text_with_json_punctuation_is_escaped() {
    let body = body_of(&Command::TypeText(r#"say "hi" \ bye"#.into()));

    assert!(body.contains(r#"\"hi\""#), "{body}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&body).is_ok(),
        "{body}"
    );
}

/// A scroll is a swipe the device computes from its own size, and it must stay
/// off the edges, where the system reads a drag as a navigation gesture.
#[test]
fn scrolling_stays_off_the_screen_edges() {
    let (width, height) = (1080, 2400);
    let body = body_of(&Command::Scroll(Direction::Down));
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");

    assert_eq!(json["cmd"], "swipe");
    for key in ["y1", "y2"] {
        let edge = json[key].as_i64().expect(key);
        assert!(edge > 0 && edge < i64::from(height), "{key} = {edge}");
    }
    assert!(
        json["y1"].as_i64() > json["y2"].as_i64(),
        "scrolling down drags the content upward: {body}"
    );
    assert!(json["x1"].as_i64().expect("x1") < i64::from(width));
}

/// The system gestures map to the helper's own global actions rather than to
/// key events, which is what lets them work under gesture navigation where
/// KEYCODE_APP_SWITCH is not bound to anything.
#[test]
fn system_gestures_use_the_helpers_global_actions() {
    for (act, expected) in [
        (SystemAct::Back, "back"),
        (SystemAct::Home, "home"),
        (SystemAct::AppSwitcher, "recents"),
    ] {
        let body = body_of(&Command::System(act));
        assert!(body.contains("\"cmd\":\"global\""), "{body}");
        assert!(
            body.contains(expected),
            "{act:?} should map to {expected}: {body}"
        );
    }
}

/// Some screens are readable through `uiautomator` and not through the helper
/// at all. Measured on a Pixel, on Settings' Internet panel: the helper and
/// ARTEMIS's own helper each reported one window and four nodes, while
/// `uiautomator dump` reported 111 — and the platform's own window list showed
/// the application window as present, focused and active. An accessibility
/// service asks for that window's content and is given nothing; `UiAutomation`
/// is a privileged connection and is not refused.
///
/// So an empty screen from the helper is not proof of an empty screen. One CLI
/// read settles it, and if that sees rows the helper cannot, the run belongs on
/// the CLI from then on.
#[test]
fn a_screen_only_the_cli_can_see_ends_the_helpers_use() {
    assert!(
        Reader::cli_saw_more(0, 111),
        "the helper was blind to this screen"
    );
    assert!(
        !Reader::cli_saw_more(0, 0),
        "a genuinely empty screen is not the helper's fault"
    );
    assert!(
        !Reader::cli_saw_more(23, 111),
        "a helper that sees the screen is kept, whatever the node counts are"
    );
}

/// The reason has to name what was actually observed, because it is the only
/// account of why a run went slow.
#[test]
fn giving_up_on_the_helper_says_what_the_cli_saw() {
    let mut reader = Reader::helper();
    reader.degrade(Reader::blind_to_this_screen(111));

    assert!(!reader.uses_helper());
    assert!(
        reader.why().expect("a reason").contains("111"),
        "{:?}",
        reader.why()
    );
}

/// A screen the helper cannot see is a property of that screen, not of the
/// helper. Settings' Wi-Fi panel is withheld from accessibility services while
/// the rest of Settings is not, so giving the helper up for the whole run
/// would pay 2.5s on every later read to solve a problem that ended with that
/// screen.
#[test]
fn borrowing_the_cli_for_one_screen_keeps_the_helper() {
    let mut reader = Reader::helper();

    reader.borrow_cli(Reader::blind_to_this_screen(106));

    assert!(reader.uses_helper(), "still the helper for the next screen");
    assert_eq!(reader.borrowed(), 1);
    assert!(reader.why().expect("a reason").contains("106"));
}

/// Reading through the CLI opens a UiAutomation connection, and Android
/// unbinds every accessibility service while one is alive. So the read right
/// after a borrow finds the helper still rebinding, and that one failure means
/// "not yet", not "gone".
#[test]
fn the_read_after_a_borrow_forgives_one_helper_failure() {
    let mut reader = Reader::helper();
    reader.borrow_cli("blind");

    assert!(
        reader.forgives_a_failure(),
        "the dump we just did silenced it"
    );
    assert!(!reader.forgives_a_failure(), "but only once");
}

/// A helper that fails without a dump having just silenced it is actually
/// gone, and waiting on it would slow every remaining step.
#[test]
fn a_failure_out_of_nowhere_is_not_forgiven() {
    let mut reader = Reader::helper();

    assert!(!reader.forgives_a_failure());
}

/// Borrowing repeatedly is still not giving up: a run that visits the same
/// withheld screen ten times pays for those ten screens and no others.
#[test]
fn many_borrows_still_do_not_end_the_helpers_use() {
    let mut reader = Reader::helper();
    for _ in 0..10 {
        reader.borrow_cli("blind");
        let _ = reader.forgives_a_failure();
    }

    assert!(reader.uses_helper());
    assert_eq!(reader.borrowed(), 10);
}
