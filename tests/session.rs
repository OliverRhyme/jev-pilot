//! Starting a browser session: finding Chrome, launching it, choosing a tab,
//! and the device a run drives it through. Pure: nothing here starts a browser
//! or opens a socket.

#![cfg(feature = "browser")]

use jev_pilot::browser::discover::{
    ActivePort, AttachError, Channel, Os, TabChoice, choose_tab, is_chrome, user_data_dir,
};
use jev_pilot::browser::transport::ConnectError;
use jev_pilot::platform::page::PageTab;
use jev_pilot::snapshot::TabKey;
use serde_json::json;
use std::path::{Path, PathBuf};

// --- DevToolsActivePort ---

/// Chrome writes the port, a newline, then the browser websocket path, with no
/// trailing newline.
#[test]
fn an_active_port_file_gives_the_browser_endpoint() {
    let port = ActivePort::parse("61545\n/devtools/browser/0b9a7c4e-1f2d-4c3b-9a8e-5d6f7a8b9c0d")
        .expect("parses");

    assert_eq!(port.port, 61545);
    assert_eq!(
        port.endpoint(),
        "ws://127.0.0.1:61545/devtools/browser/0b9a7c4e-1f2d-4c3b-9a8e-5d6f7a8b9c0d"
    );
}

#[test]
fn a_trailing_newline_is_tolerated() {
    let port = ActivePort::parse("9222\n/devtools/browser/abc\n").expect("parses");
    assert_eq!(port.endpoint(), "ws://127.0.0.1:9222/devtools/browser/abc");
}

/// Read while Chrome was still writing it, or by hand afterwards.
#[test]
fn a_truncated_active_port_file_is_refused() {
    for text in [
        "",
        "9222",
        "9222\n",
        "9222\n/json/version",
        "port\n/devtools/browser/x",
    ] {
        assert!(
            matches!(ActivePort::parse(text), Err(AttachError::Truncated { .. })),
            "{text:?} is not a usable file"
        );
    }
}

/// A file left by a Chrome that has exited still parses; it is the connection
/// that shows it is stale, and that has its own message.
#[test]
fn a_stale_file_shows_as_a_refused_connection() {
    let port = ActivePort::parse("9222\n/devtools/browser/old").expect("parses");
    assert_eq!(port.port, 9222);

    let error = AttachError::from(ConnectError::Refused);
    assert!(matches!(error, AttachError::Stale));
    assert!(error.to_string().contains("stale"), "{error}");
}

// --- where Chrome keeps its profile ---

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn the_user_data_directory_on_macos_for_each_channel() {
    let home = Path::new("/Users/ada");
    let support = "/Users/ada/Library/Application Support/Google";
    for (channel, folder) in [
        (Channel::Stable, "Chrome"),
        (Channel::Beta, "Chrome Beta"),
        (Channel::Dev, "Chrome Dev"),
        (Channel::Canary, "Chrome Canary"),
    ] {
        assert_eq!(
            user_data_dir(Os::MacOs, channel, home, &no_env),
            PathBuf::from(format!("{support}/{folder}"))
        );
    }
}

#[test]
fn the_user_data_directory_on_linux_for_each_channel() {
    let home = Path::new("/home/ada");
    for (channel, folder) in [
        (Channel::Stable, "google-chrome"),
        (Channel::Beta, "google-chrome-beta"),
        (Channel::Dev, "google-chrome-unstable"),
        (Channel::Canary, "google-chrome-canary"),
    ] {
        assert_eq!(
            user_data_dir(Os::Linux, channel, home, &no_env),
            PathBuf::from(format!("/home/ada/.config/{folder}"))
        );
    }
}

/// `CHROME_CONFIG_HOME` wins over `XDG_CONFIG_HOME`, which wins over
/// `~/.config`.
#[test]
fn linux_config_homes_are_honoured_in_order() {
    let home = Path::new("/home/ada");
    let xdg = |name: &str| (name == "XDG_CONFIG_HOME").then(|| "/xdg".to_owned());
    assert_eq!(
        user_data_dir(Os::Linux, Channel::Stable, home, &xdg),
        PathBuf::from("/xdg/google-chrome")
    );
    let both = |name: &str| match name {
        "CHROME_CONFIG_HOME" => Some("/chrome-home".to_owned()),
        "XDG_CONFIG_HOME" => Some("/xdg".to_owned()),
        _ => None,
    };
    assert_eq!(
        user_data_dir(Os::Linux, Channel::Stable, home, &both),
        PathBuf::from("/chrome-home/google-chrome")
    );
}

#[test]
fn the_user_data_directory_on_windows_for_each_channel() {
    let home = Path::new(r"C:\Users\ada");
    let local =
        |name: &str| (name == "LOCALAPPDATA").then(|| r"C:\Users\ada\AppData\Local".to_owned());
    for (channel, folder) in [
        (Channel::Stable, "Chrome"),
        (Channel::Beta, "Chrome Beta"),
        (Channel::Dev, "Chrome Dev"),
        (Channel::Canary, "Chrome SxS"),
    ] {
        assert_eq!(
            user_data_dir(Os::Windows, channel, home, &local),
            Path::new(r"C:\Users\ada\AppData\Local")
                .join("Google")
                .join(folder)
                .join("User Data")
        );
    }
}

#[test]
fn channels_parse_by_name() {
    assert_eq!("stable".parse::<Channel>(), Ok(Channel::Stable));
    assert_eq!("canary".parse::<Channel>(), Ok(Channel::Canary));
    assert!("nightly".parse::<Channel>().is_err());
}

// --- attach errors ---

/// Each way attaching fails has its own advice, because each has its own fix.
#[test]
fn each_attach_error_says_what_to_do() {
    let dir = PathBuf::from("/Users/ada/Library/Application Support/Google/Chrome");
    let cases: Vec<(AttachError, &str)> = vec![
        (
            AttachError::NoFile { dir: dir.clone() },
            "remote debugging is off",
        ),
        (
            AttachError::Unreadable { dir: dir.clone() },
            "Files & Folders",
        ),
        (AttachError::from(ConnectError::Refused), "stale"),
        (
            AttachError::from(ConnectError::Rejected { status: 403 }),
            "Cancel",
        ),
        (
            AttachError::from(ConnectError::TimedOut),
            "Allow was not clicked",
        ),
        (
            AttachError::NotChrome {
                product: "Firefox/141".into(),
            },
            "not Chrome",
        ),
    ];
    for (error, advice) in cases {
        let message = error.to_string();
        assert!(
            message.contains(advice),
            "{message:?} should mention {advice:?}"
        );
    }
    assert!(
        AttachError::from(ConnectError::Rejected { status: 403 })
            .to_string()
            .contains("no open window"),
        "a 403 is also what Chrome with no window answers"
    );
}

/// Reading the file is where macOS privacy controls bite.
#[test]
fn reading_the_active_port_file_tells_missing_from_forbidden() {
    let dir = PathBuf::from("/x");
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    let forbidden = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(matches!(
        AttachError::reading(&dir, &missing),
        AttachError::NoFile { .. }
    ));
    assert!(matches!(
        AttachError::reading(&dir, &forbidden),
        AttachError::Unreadable { .. }
    ));
}

#[test]
fn only_chrome_or_chromium_counts_as_chrome() {
    assert!(is_chrome(&json!({"product": "Chrome/153.0.8010.12"})));
    assert!(is_chrome(&json!({"product": "HeadlessChrome/153.0.0.0"})));
    assert!(is_chrome(&json!({"product": "Chromium/140.0.1"})));
    assert!(
        is_chrome(&json!({"product": "Edg/142.0.3595.53"})),
        "Edge is Chromium"
    );
    assert!(is_chrome(&json!({"product": "HeadlessEdg/142.0.3595.53"})));
    assert!(!is_chrome(&json!({"product": "Firefox/141.0"})));
    assert!(!is_chrome(&json!({})));
}

/// A browser this run started is not a stale endpoint, so its refusal says
/// which binary to change rather than blaming a port file.
#[test]
fn a_fresh_browser_that_is_not_chrome_names_the_binary() {
    use jev_pilot::browser::launch::LaunchError;

    let message = LaunchError::NotChrome {
        binary: PathBuf::from("/opt/odd/browser"),
        product: "Firefox/141.0".into(),
    }
    .to_string();

    assert!(
        message.contains("/opt/odd/browser") && message.contains("Firefox/141.0"),
        "{message}"
    );
    assert!(message.contains("--chrome"), "{message}");
    assert!(!message.contains("DevToolsActivePort"), "{message}");
}

// --- choosing a tab ---

fn tab(id: &str, title: &str, url: &str) -> PageTab {
    PageTab {
        key: TabKey::new(id),
        title: title.into(),
        url: url.into(),
        opened_by_run: false,
    }
}

fn tabs() -> Vec<PageTab> {
    vec![
        tab("A1", "Inbox", "https://mail.example.test/inbox"),
        tab("B2", "Pull requests", "https://github.com/pulls"),
        tab("C3", "Issues", "https://github.com/issues"),
    ]
}

#[test]
fn a_tab_is_chosen_by_id_or_by_a_unique_piece_of_its_address() {
    let tabs = tabs();
    assert_eq!(
        choose_tab(&tabs, Some("B2")).map(|t| t.key.clone()),
        Ok(TabKey::new("B2"))
    );
    assert_eq!(
        choose_tab(&tabs, Some("mail.example")).map(|t| t.key.clone()),
        Ok(TabKey::new("A1"))
    );
}

#[test]
fn an_ambiguous_match_is_refused_with_the_candidates() {
    let tabs = tabs();
    let Err(TabChoice::Ambiguous(candidates)) = choose_tab(&tabs, Some("github.com")) else {
        panic!("two tabs match");
    };
    assert_eq!(candidates.len(), 2);
    let message = TabChoice::Ambiguous(candidates).to_string();
    assert!(
        message.contains("B2") && message.contains("C3") && !message.contains("A1"),
        "{message}"
    );
}

#[test]
fn no_match_is_refused_with_every_tab_listed() {
    let tabs = tabs();
    let Err(none) = choose_tab(&tabs, Some("gitlab")) else {
        panic!("nothing matches");
    };
    assert!(matches!(none, TabChoice::None(_)));
    let message = none.to_string();
    assert!(
        message.contains("A1") && message.contains("B2") && message.contains("C3"),
        "{message}"
    );
}

/// With no `--tab`, a browser with one tab drives it, and one with more asks.
#[test]
fn without_a_match_string_only_a_single_tab_is_chosen() {
    let one = vec![tab("A1", "Inbox", "https://mail.example.test/inbox")];
    assert_eq!(
        choose_tab(&one, None).map(|t| t.key.clone()),
        Ok(TabKey::new("A1"))
    );
    assert!(matches!(
        choose_tab(&tabs(), None),
        Err(TabChoice::Ambiguous(_))
    ));
}

// --- launching a fresh browser ---

use jev_pilot::browser::launch::{chrome_candidates, find_chrome, launch_args};

/// A fresh browser starts on a throwaway profile with nothing that would stop
/// at a first-run screen, ask for a keychain, or sync.
#[test]
fn the_launch_arguments_are_the_fresh_profile_flags() {
    let profile = Path::new("/tmp/jev-pilot-fresh-1234");

    assert_eq!(
        launch_args(profile, false),
        [
            "--remote-debugging-port=0",
            "--user-data-dir=/tmp/jev-pilot-fresh-1234",
            "--no-first-run",
            "--no-default-browser-check",
            "--enable-automation",
            "--disable-search-engine-choice-screen",
            "--password-store=basic",
            "--use-mock-keychain",
            "--disable-sync",
            "about:blank",
        ]
    );
    let headless = launch_args(profile, true);
    assert!(headless.contains(&"--headless".to_owned()));
    assert_eq!(headless.last().map(String::as_str), Some("about:blank"));
}

/// `--chrome` wins, then `JEV_PILOT_CHROME`, then the usual places.
#[test]
fn the_browser_binary_is_chosen_in_order() {
    let everywhere = |_: &Path| true;
    assert_eq!(
        find_chrome(
            Some(Path::new("/opt/cft/chrome")),
            Some("/env/chrome".into()),
            Os::Linux,
            &everywhere
        ),
        Some(PathBuf::from("/opt/cft/chrome"))
    );
    assert_eq!(
        find_chrome(None, Some("/env/chrome".into()), Os::Linux, &everywhere),
        Some(PathBuf::from("/env/chrome"))
    );
    assert_eq!(
        find_chrome(None, None, Os::Linux, &everywhere),
        chrome_candidates(Os::Linux).first().cloned()
    );
    assert_eq!(find_chrome(None, None, Os::Linux, &|_: &Path| false), None);
}

/// Chrome for Testing and Chromium come before Google Chrome, whose stable
/// build ignores remote debugging on its default profile from version 136.
#[test]
fn the_usual_places_prefer_chrome_for_testing_and_chromium() {
    let mac = chrome_candidates(Os::MacOs);
    let position = |needle: &str| {
        mac.iter()
            .position(|p| p.to_string_lossy().contains(needle))
            .unwrap_or_else(|| panic!("{needle} is a candidate: {mac:?}"))
    };
    assert!(position("Google Chrome for Testing") < position("Google Chrome.app"));
    assert!(position("Chromium") < position("Google Chrome.app"));
    assert!(
        chrome_candidates(Os::Linux)
            .iter()
            .any(|p| p == Path::new("/opt/google/chrome/chrome"))
    );
}

// --- the browser device ---

use jev_pilot::act::{DialogReply, Direction, SystemAct};
use jev_pilot::browser::device::{BrowserDevice, browser_command};
use jev_pilot::browser::engine::{BrowserCommand, BrowserEngine, DialogAnswer};
use jev_pilot::device::{Command, Device};
use jev_pilot::platform::page::{PageBox, PageNode, PageRead, Viewport};
use jev_pilot::snapshot::{NodeKey, Point};

#[test]
fn the_pilots_commands_become_the_engines() {
    let at = Point { x: 10, y: 20 };
    let node = NodeKey::new("7:1");
    let cases = [
        (
            Command::Press {
                at,
                node: node.clone(),
            },
            BrowserCommand::Press {
                at,
                node: node.clone(),
            },
        ),
        (
            Command::Fill {
                at,
                node: node.clone(),
                text: "Ada".into(),
            },
            BrowserCommand::Fill {
                at,
                node: node.clone(),
                text: "Ada".into(),
            },
        ),
        (Command::System(SystemAct::Submit), BrowserCommand::Submit),
        (Command::System(SystemAct::Back), BrowserCommand::Back),
        (
            Command::Scroll(Direction::Down),
            BrowserCommand::Scroll(Direction::Down),
        ),
        (
            Command::Navigate("https://example.test/".into()),
            BrowserCommand::Navigate("https://example.test/".into()),
        ),
        (
            Command::SwitchTab(TabKey::new("T2")),
            BrowserCommand::SwitchTab(TabKey::new("T2")),
        ),
        (
            Command::Dialog(DialogReply::Accept {
                words: Some("Ada".into()),
            }),
            BrowserCommand::Dialog(DialogAnswer::Accept(Some("Ada".into()))),
        ),
        (
            Command::Dialog(DialogReply::Dismiss),
            BrowserCommand::Dialog(DialogAnswer::Dismiss),
        ),
        (Command::Settle, BrowserCommand::Settle),
    ];
    for (command, expected) in cases {
        assert_eq!(browser_command(&command), Some(expected), "{command:?}");
    }
}

/// A gesture a browser has no equivalent for is refused, never approximated.
#[test]
fn phone_only_commands_have_no_browser_equivalent() {
    let at = Point { x: 10, y: 20 };
    for command in [
        Command::Tap(at),
        Command::TypeText {
            at,
            text: "x".into(),
        },
        Command::DoubleTap(at),
        Command::LongPress(at),
        Command::SwipeFrom {
            from: at,
            direction: jev_pilot::act::Swipe::Left,
        },
        Command::Peek(at),
        Command::Launch("com.example".into()),
        Command::System(SystemAct::Home),
        Command::System(SystemAct::AppSwitcher),
    ] {
        assert_eq!(browser_command(&command), None, "{command:?}");
    }
}

/// An engine that hands out one page read and remembers what it was told.
#[derive(Default)]
struct FakeEngine {
    read: Option<PageRead>,
    performed: Vec<BrowserCommand>,
    outcome: Option<&'static str>,
}

#[derive(Debug)]
struct FakeError;

impl core::fmt::Display for FakeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("fake")
    }
}

impl core::error::Error for FakeError {}

impl BrowserEngine for FakeEngine {
    type Error = FakeError;

    fn read(&mut self) -> Result<PageRead, FakeError> {
        self.read.clone().ok_or(FakeError)
    }

    fn perform(&mut self, command: &BrowserCommand) -> Result<(), FakeError> {
        self.performed.push(command.clone());
        Ok(())
    }

    fn outcome(&self) -> Option<&str> {
        self.outcome
    }

    fn quiet_for_ms(&self) -> Option<u64> {
        Some(420)
    }

    fn tabs(&mut self) -> Result<Vec<PageTab>, FakeError> {
        Ok(Vec::new())
    }

    fn select_tab(&mut self, _: &TabKey) -> Result<(), FakeError> {
        Ok(())
    }
}

fn button_page() -> PageRead {
    PageRead {
        url: "https://shop.example.test/cart".into(),
        title: "Cart".into(),
        viewport: Viewport {
            width: 800.0,
            height: 600.0,
        },
        dialog: None,
        accepted_alert: None,
        tabs: Vec::new(),
        current: None,
        unreadable_frames: 0,
        nodes: vec![PageNode {
            frame: "main".into(),
            role: "button".into(),
            name: "Check out".into(),
            value: None,
            protected: false,
            description: None,
            states: jev_pilot::platform::page::NodeStates::default(),
            clickable: true,
            options: Vec::new(),
            context: None,
            bounds: Some(PageBox {
                x: 100.0,
                y: 100.0,
                width: 120.0,
                height: 40.0,
            }),
            key: NodeKey::new("12:1"),
        }],
    }
}

/// Observing reads the page through the Browser platform, so the pilot sees
/// the same snapshot as any other screen, with the engine's quiet time.
#[test]
fn observing_reads_the_page_into_a_snapshot() {
    let mut device = BrowserDevice::new(FakeEngine {
        read: Some(button_page()),
        ..FakeEngine::default()
    });

    let screen = device.observe().expect("reads");

    let (handle, row) = screen.refs().next().expect("one row");
    assert_eq!(&*row.label, "Check out");
    assert_eq!(screen.node_key(handle), Ok(Some(&NodeKey::new("12:1"))));
    assert_eq!(screen.app(), Some("https://shop.example.test"));
    assert_eq!(screen.url(), Some("https://shop.example.test/cart"));
    assert_eq!(screen.quiet_for_ms(), Some(420));
}

#[test]
fn performing_hands_the_engine_its_command_and_passes_its_outcome_on() {
    let mut device = BrowserDevice::new(FakeEngine {
        outcome: Some("navigated to https://shop.example.test"),
        ..FakeEngine::default()
    });

    device
        .perform(&Command::Navigate("https://shop.example.test/".into()))
        .expect("performs");

    assert_eq!(
        device.engine().performed,
        [BrowserCommand::Navigate(
            "https://shop.example.test/".into()
        )]
    );
    assert_eq!(
        device.outcome().as_deref(),
        Some("navigated to https://shop.example.test")
    );
}

#[test]
fn a_phone_only_command_is_an_error_and_reaches_no_engine() {
    let mut device = BrowserDevice::new(FakeEngine::default());

    let error = device
        .perform(&Command::LongPress(Point { x: 1, y: 2 }))
        .expect_err("unsupported");

    assert!(error.to_string().contains("browser"), "{error}");
    assert_eq!(device.engine().performed, []);
}
