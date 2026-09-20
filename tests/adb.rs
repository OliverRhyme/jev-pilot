//! Building `adb` invocations. Pure: nothing here touches a device.

use jev_pilot::act::{Direction, Swipe, SystemAct};
use jev_pilot::device::adb::{Adb, DumpFailure, TextError};
use jev_pilot::snapshot::Point;

fn adb() -> Adb {
    Adb::new("SERIAL123")
}

/// `adb shell` joins its arguments into a string the *device's* shell parses,
/// so a metacharacter that survives the local exec is still interpreted there.
/// An unquoted `&` truncates the command and silently types half the text.
#[test]
fn text_with_shell_metacharacters_is_quoted_for_the_remote_shell() {
    let args = adb().type_text_args("Tom & Jerry's").expect("ascii");

    let typed = args.last().expect("the text argument");
    assert!(
        !typed.contains("& Jerry") || typed.starts_with('\''),
        "the ampersand must not reach the device shell unquoted: {typed}"
    );
    assert_eq!(typed, r"'Tom & Jerry'\''s'");
}

/// `input text` goes through the current IME's key-character map, which maps
/// ASCII. Emoji and CJK silently produce nothing, so refusing is honest.
#[test]
fn non_ascii_text_is_refused_rather_than_silently_dropped() {
    assert!(matches!(
        adb().type_text_args("こんにちは"),
        Err(TextError::NotAscii { .. })
    ));
    assert!(adb().type_text_args("plain ascii").is_ok());
}

/// Every invocation must name the device, or a second phone appearing changes
/// which one the run drives halfway through.
#[test]
fn every_invocation_targets_the_chosen_serial() {
    let point = Point { x: 504, y: 670 };
    for args in [
        adb().dump_args(),
        adb().tap_args(point),
        adb().system_args(SystemAct::Back),
    ] {
        assert_eq!(&args[..2], &["-s", "SERIAL123"], "got {args:?}");
    }
}

/// The dump is streamed to stdout rather than written to the device and pulled
/// back, which halves the round trips per observation.
#[test]
fn the_hierarchy_is_streamed_rather_than_written_and_pulled() {
    let args = adb().dump_args();

    assert!(args.contains(&"exec-out".to_owned()), "got {args:?}");
    assert!(!args.iter().any(|a| a.contains("/sdcard")), "got {args:?}");
}

/// Under gesture navigation there is no recents key binding, so the app
/// switcher is reached by swiping up from the bottom edge and pausing. That
/// needs the display size, which `wm size` reports as a labelled line.
#[test]
fn the_display_size_is_read_from_the_wm_size_line() {
    assert_eq!(
        Adb::parse_size("Physical size: 1080x2400\n"),
        Some((1080, 2400))
    );
    assert_eq!(
        Adb::parse_size("Physical size: 1080x2400\nOverride size: 720x1600\n"),
        Some((720, 1600)),
        "an override is what is actually displayed"
    );
    assert_eq!(Adb::parse_size("nonsense"), None);
}

/// The swipe has to start at the very bottom edge and pause partway up; a
/// short flick opens the home screen instead of the switcher.
#[test]
fn the_app_switcher_swipe_starts_at_the_bottom_edge_and_dwells() {
    let args = adb().app_switcher_swipe_args(1080, 2400);
    let tail: Vec<&str> = args.iter().map(String::as_str).collect();

    assert_eq!(&tail[2..5], &["shell", "input", "swipe"]);
    assert_eq!(tail[5], "540", "horizontally centred");
    assert_eq!(tail[6], "2399", "from the very bottom edge");
    let dwell: u32 = tail[9].parse().expect("a duration in ms");
    assert!(
        dwell >= 300,
        "a flick opens home, not the switcher: {dwell}ms"
    );
}

/// `uiautomator dump` prints a status line to the same stream as the document,
/// so the bytes read back are the hierarchy with a sentence glued to the end.
/// XML allows nothing after the root element, so the whole parse fails.
#[test]
fn the_status_line_uiautomator_appends_is_trimmed() {
    let raw = "<hierarchy rotation=\"0\"><node text=\"a\"/></hierarchy>\
               UI hierchary dumped to: /dev/tty\n";

    assert_eq!(
        Adb::extract_hierarchy(raw),
        Some("<hierarchy rotation=\"0\"><node text=\"a\"/></hierarchy>")
    );
    assert_eq!(Adb::extract_hierarchy("ERROR: something went wrong"), None);
}

/// A scroll is a swipe across the middle of the screen, inset from the edges
/// so it is not mistaken for a system navigation gesture.
#[test]
fn scrolling_swipes_within_the_screen_rather_than_from_its_edge() {
    let (width, height) = (1080, 2400);
    let down = adb().scroll_args(Direction::Down, width, height);
    let up = adb().scroll_args(Direction::Up, width, height);

    let nums = |args: &[String]| -> Vec<i32> {
        args[5..9]
            .iter()
            .map(|n| n.parse().expect("a coordinate"))
            .collect()
    };
    let (d, u) = (nums(&down), nums(&up));

    assert!(
        d[1] > d[3],
        "scrolling down drags the content upward: {d:?}"
    );
    assert!(
        u[1] < u[3],
        "scrolling up drags the content downward: {u:?}"
    );
    for edge in [d[1], d[3], u[1], u[3]] {
        assert!(edge > 0 && edge < height, "must stay off the edges: {edge}");
    }
}

/// A long press is a swipe that goes nowhere and takes its time. Android's
/// threshold is around half a second, so the duration must comfortably exceed
/// it or the gesture registers as an ordinary tap.
#[test]
fn a_long_press_holds_still_for_longer_than_the_system_threshold() {
    let args = adb().long_press_args(Point { x: 500, y: 700 });
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();

    assert_eq!(&parts[2..5], &["shell", "input", "swipe"]);
    assert_eq!(&parts[5..9], &["500", "700", "500", "700"], "must not move");
    let held: u32 = parts[9].parse().expect("a duration");
    assert!(
        held > 500,
        "must outlast the long-press threshold: {held}ms"
    );
}

/// Both taps go in one invocation. Each `adb shell` spawns a process on the
/// device, and two separate invocations can easily exceed the double-tap
/// window, leaving two unrelated single taps.
#[test]
fn a_double_tap_sends_both_taps_in_one_invocation() {
    let args = adb().double_tap_args(Point { x: 120, y: 240 });

    assert_eq!(args.len(), 4, "one shell argument, not two calls: {args:?}");
    assert_eq!(args[2], "shell");
    assert_eq!(
        args[3].matches("input tap 120 240").count(),
        2,
        "{}",
        args[3]
    );
}

/// A row swipe starts at the row and travels sideways across it.
#[test]
fn swiping_a_row_travels_sideways_from_it() {
    let at = Point { x: 540, y: 900 };
    let left = adb().swipe_from_args(at, Swipe::Left, 1080);
    let right = adb().swipe_from_args(at, Swipe::Right, 1080);

    let x_of = |args: &[String], i: usize| -> i32 { args[i].parse().expect("coordinate") };
    assert!(x_of(&left, 7) < x_of(&left, 5), "left goes leftward");
    assert!(x_of(&right, 7) > x_of(&right, 5), "right goes rightward");
    assert_eq!(x_of(&left, 6), 900, "stays on the row");
}

/// The stream this reads is untrusted device output, and may be truncated or
/// interleaved. A closing tag appearing before an opening one inverts the slice
/// range, which panics rather than rejecting the input it exists to sanitise.
#[test]
fn a_hierarchy_with_its_tags_reversed_is_rejected_not_panicked_on() {
    assert_eq!(
        Adb::extract_hierarchy("</hierarchy>\n<hierarchy rotation=\"0\"><node/>"),
        None
    );
    assert_eq!(Adb::extract_hierarchy("</hierarchy>"), None);
    assert_eq!(Adb::extract_hierarchy("<hierarchy"), None);
}

/// `uiautomator` reports the idle-state failure on stderr, but some builds also
/// exit non-zero for it. Judging the status first turns a retryable condition
/// into a fatal one, and the documented retry never runs.
#[test]
fn the_stderr_classification_is_not_shadowed_by_the_exit_status() {
    assert_eq!(
        Adb::classify_stderr("ERROR: could not get idle state."),
        Some(DumpFailure::NeverSettled)
    );
    assert_eq!(
        Adb::classify_stderr("ERROR: null root node returned by UiTestAutomationBridge."),
        Some(DumpFailure::NoActiveWindow)
    );
    assert_eq!(Adb::classify_stderr("device offline"), None);
}

/// Typing into a field leaves the text uncommitted. Without a way to submit it,
/// a search can be composed and never run, which reads as a loop that typed
/// correctly and then had nothing left it could do.
#[test]
fn submitting_presses_the_enter_key() {
    let args = adb().system_args(SystemAct::Submit);
    assert_eq!(args.last().map(String::as_str), Some("KEYCODE_ENTER"));
}

/// The helper listens on a loopback port on the phone, so a tunnel is needed.
/// `tcp:0` lets adb pick the host port, which is what makes two processes able
/// to attach to the same device without agreeing a number in advance.
#[test]
fn the_helper_tunnel_lets_adb_choose_the_host_port() {
    let args = adb().forward_args(18888);
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();

    assert_eq!(&parts[2..], &["forward", "tcp:0", "tcp:18888"]);
}

/// adb prints the port it allocated, and nothing else useful.
#[test]
fn the_allocated_host_port_is_read_back_from_adb() {
    assert_eq!(Adb::parse_forward_port("18899\n"), Some(18899));
    assert_eq!(Adb::parse_forward_port(""), None);
    assert_eq!(Adb::parse_forward_port("error: cannot bind"), None);
}

/// `adb forward tcp:0` allocates a *new* host port every call, and adb never
/// reclaims it. A loop that opened a tunnel per run accumulated fifteen
/// forwards to the same device port on one afternoon. The existing tunnel is
/// found first so a second run reuses it instead of stacking another.
#[test]
fn an_existing_tunnel_to_the_same_device_port_is_found() {
    let listing = "\
SERIAL123 tcp:18899 tcp:18888
SERIAL123 tcp:19000 tcp:9000
OTHER tcp:18700 tcp:18888
";

    assert_eq!(adb().parse_forward_reuse(listing, 18888), Some(18899));
}

/// The listing is global: `--list` ignores `-s` and prints every device's
/// forwards. Reusing another phone's tunnel would read the wrong screen.
#[test]
fn a_tunnel_belonging_to_another_device_is_not_reused() {
    let listing = "OTHER tcp:18700 tcp:18888\n";

    assert_eq!(adb().parse_forward_reuse(listing, 18888), None);
}

/// A forward to a different device port is a different service.
#[test]
fn a_tunnel_to_a_different_device_port_is_not_reused() {
    let listing = "SERIAL123 tcp:19000 tcp:9000\n";

    assert_eq!(adb().parse_forward_reuse(listing, 18888), None);
}

/// Listing and removing are how a caller cleans up tunnels it no longer wants.
#[test]
fn tunnels_can_be_listed_and_removed() {
    let listed: Vec<String> = adb().forward_list_args();
    assert_eq!(&listed[2..], &["forward", "--list"]);

    let removed = adb().forward_remove_args(18899);
    assert_eq!(&removed[2..], &["forward", "--remove", "tcp:18899"]);
}

/// With one device attached the tool should not make a person copy a serial
/// out of another command. With two it must, because picking one silently
/// would drive whichever phone happened to enumerate first.
#[test]
fn the_attached_devices_are_read_from_adb_devices() {
    let listing = "List of devices attached\n\
                   39081FDJG00G9A\tdevice\n\
                   emulator-5554\tdevice\n\
                   R5CT10ABCDE\tunauthorized\n\
                   ZY223KLMNO\toffline\n\n";

    assert_eq!(
        Adb::parse_devices(listing),
        vec!["39081FDJG00G9A".to_owned(), "emulator-5554".to_owned()],
        "only devices that are ready to be driven"
    );
    assert!(Adb::parse_devices("List of devices attached\n\n").is_empty());
}

/// A screen withheld from accessibility services and a screen caught
/// mid-transition both parse to no actionable rows, and they want opposite
/// treatment: the first will never improve and needs the 2.5s dump now, the
/// second improves in 150ms and needs only patience.
///
/// The helper's own answer tells them apart. A withheld screen leaves only the
/// system's own windows — on Settings' Wi-Fi panel, six nodes, every one of
/// them `com.android.systemui` — while the application window it is hiding is
/// listed by the platform as present and focused the whole time.
#[test]
fn a_document_without_an_application_window_means_one_is_being_withheld() {
    let withheld = r#"<hierarchy rotation="0">
        <node index="0" package="com.android.systemui" window-type="system" bounds="[0,0][1008,113]" />
        </hierarchy>"#;
    let ordinary = r#"<hierarchy rotation="0">
        <node index="0" package="com.android.systemui" window-type="system" bounds="[0,0][1008,113]" />
        <node index="1" package="com.android.settings" window-type="application" bounds="[0,0][1008,2244]" />
        </hierarchy>"#;

    assert!(!Adb::shows_an_application(withheld));
    assert!(Adb::shows_an_application(ordinary));
}

/// The CLI's own dumps carry no window metadata at all, and must not be read
/// as withholding anything.
#[test]
fn a_document_with_no_window_metadata_is_not_treated_as_withheld() {
    let cli_shaped = r#"<hierarchy rotation="0"><node index="0" class="x" /></hierarchy>"#;

    assert!(Adb::shows_an_application(cli_shaped));
}

/// Returning to an app is a launch, not a gesture. `monkey` is used rather
/// than `am start` because it needs only the package: the launchable activity
/// is the device's business, and naming one means knowing it for every app a
/// goal might be about.
#[test]
fn returning_to_an_app_launches_it_by_package() {
    assert_eq!(
        adb().launch_args("com.example.wallet"),
        [
            "-s",
            "SERIAL123",
            "shell",
            "monkey",
            "-p",
            "com.example.wallet",
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
    );
}

/// Asking the device which app draws the home screen, rather than carrying a
/// list of launcher package names that is wrong on the first device with a
/// vendor launcher or a replacement one.
#[test]
fn the_home_screen_app_is_resolved_from_the_device() {
    assert_eq!(
        adb().home_screen_args(),
        [
            "-s",
            "SERIAL123",
            "shell",
            "cmd",
            "package",
            "resolve-activity",
            "--brief",
            "-c",
            "android.intent.category.HOME",
            "-a",
            "android.intent.action.MAIN",
        ],
    );
}

/// `resolve-activity --brief` answers with a component on its last line, and
/// preamble above it. Only the package is wanted.
#[test]
fn the_home_screen_package_is_read_from_the_component_it_resolves_to() {
    let answer = "priority=0 preferredOrder=0 match=0x108000 isDefault=true\n\
                  com.example.launcher/.MainActivity\n";

    assert_eq!(
        Adb::parse_home_screen(answer).as_deref(),
        Some("com.example.launcher"),
    );
    assert_eq!(Adb::parse_home_screen("No activity found\n"), None);
}
