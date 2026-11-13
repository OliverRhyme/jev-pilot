//! The Browser platform: page reads in, snapshots out.
//!
//! Every page here is a recorded or hand-built JSON page read from
//! `tests/fixtures/browser/`; nothing opens a browser.

#![cfg(feature = "browser")]

use jev_pilot::act::Operation;
use jev_pilot::platform::browser::{BAND_VIEWPORTS, Browser};
use jev_pilot::platform::page::{NodeStates, PageBox, PageNode, PageRead, PageTab, Viewport};
use jev_pilot::platform::{HierarchyError, Platform};
use jev_pilot::snapshot::{MAX_ELEMENTS, NodeKey, Snapshot, TabKey, TapError};

fn fixture(name: &str) -> Snapshot {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/browser/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture exists");
    Browser::default()
        .parse_hierarchy(&raw)
        .expect("fixture parses")
}

fn rows(snapshot: &Snapshot) -> Vec<(String, Option<String>)> {
    snapshot
        .refs()
        .map(|(_, e)| {
            (
                e.label.to_string(),
                e.detail.as_ref().map(ToString::to_string),
            )
        })
        .collect()
}

fn labels(snapshot: &Snapshot) -> Vec<String> {
    snapshot.refs().map(|(_, e)| e.label.to_string()).collect()
}

fn notices(snapshot: &Snapshot) -> Vec<String> {
    snapshot.notices().map(ToString::to_string).collect()
}

fn unavailable(snapshot: &Snapshot) -> Vec<String> {
    snapshot.unavailable().map(ToString::to_string).collect()
}

fn row<'a>(
    snapshot: &'a Snapshot,
    label: &str,
) -> (
    jev_pilot::snapshot::ElementRef,
    &'a jev_pilot::snapshot::Element,
) {
    snapshot
        .refs()
        .find(|(_, e)| &*e.label == label)
        .unwrap_or_else(|| panic!("no row {label:?} in {:?}", labels(snapshot)))
}

fn node(role: &str, name: &str, y: f64) -> PageNode {
    PageNode {
        frame: "main".into(),
        role: role.into(),
        name: name.into(),
        value: None,
        protected: false,
        description: None,
        states: NodeStates::default(),
        clickable: false,
        options: Vec::new(),
        context: None,
        bounds: Some(PageBox {
            x: 100.0,
            y,
            width: 100.0,
            height: 20.0,
        }),
        key: NodeKey::new(format!("n:{name}:{y}")),
    }
}

fn read(nodes: Vec<PageNode>) -> PageRead {
    PageRead {
        url: "https://example.test/".into(),
        title: "Example".into(),
        viewport: Viewport {
            width: 1280.0,
            height: 720.0,
        },
        dialog: None,
        accepted_alert: None,
        tabs: Vec::new(),
        current: None,
        unreadable_frames: 0,
        nodes,
    }
}

// --- the platform ---

#[test]
fn the_browser_set_has_no_phone_only_operations() {
    let offered = Browser::default().operations();
    for wanted in [
        Operation::Tap,
        Operation::TypeText,
        Operation::Submit,
        Operation::ScrollUp,
        Operation::ScrollDown,
        Operation::Back,
        Operation::Navigate,
        Operation::SwitchTab,
        Operation::AcceptDialog,
        Operation::DismissDialog,
        Operation::Wait,
        Operation::Done,
        Operation::Blocked,
    ] {
        assert!(offered.contains(&wanted), "{wanted:?} is offered");
    }
    for refused in [
        Operation::DoubleTap,
        Operation::LongPress,
        Operation::SwipeLeft,
        Operation::SwipeRight,
        Operation::Peek,
        Operation::Home,
        Operation::AppSwitcher,
        Operation::CloseKeyboard,
        Operation::Return,
    ] {
        assert!(!offered.contains(&refused), "{refused:?} is not offered");
    }
}

#[test]
fn malformed_json_is_not_a_page_read() {
    let error = Browser::default().parse_hierarchy("not json").unwrap_err();
    assert!(matches!(error, HierarchyError::Malformed { .. }));
}

#[test]
fn parsing_json_and_reading_directly_agree() {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/browser/github-repo.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let page: PageRead = serde_json::from_str(&raw).unwrap();
    let direct = Browser::default().snapshot_from(&page);
    let parsed = Browser::default().parse_hierarchy(&raw).unwrap();
    assert_eq!(direct.fingerprint(), parsed.fingerprint());
}

// --- which nodes become rows ---

#[test]
fn a_repository_page_offers_its_links_and_buttons() {
    let snapshot = fixture("github-repo");
    assert_eq!(
        labels(&snapshot),
        [
            "Homepage",
            "Platform",
            "Solutions",
            "Search or jump to…",
            "Sign in",
            "Sign up",
            // "Issues 12k" only wraps this link, so it collapses into it.
            "Issues",
            "Pull requests",
            "Star",
            "Fork",
            "Code",
            "README.md",
            "Releases",
        ]
    );
}

#[test]
fn a_repository_page_says_what_it_shows_and_what_each_row_is_in() {
    let snapshot = fixture("github-repo");
    let by_label = rows(&snapshot);
    let detail = |label: &str| {
        by_label
            .iter()
            .find(|(l, _)| l == label)
            .unwrap_or_else(|| panic!("no row {label}"))
            .1
            .clone()
    };
    assert_eq!(detail("Platform").as_deref(), Some("collapsed"));
    assert_eq!(detail("Code").as_deref(), Some("collapsed"));
    assert_eq!(
        detail("Star").as_deref(),
        Some("Sign in to star this repository")
    );
    assert_eq!(detail("Sign in"), None);
    assert_eq!(
        notices(&snapshot),
        [
            "rust-lang/rust",
            "Public",
            "Empowering everyone to build reliable and efficient software.",
        ]
    );
    assert_eq!(snapshot.app(), Some("https://github.com"));
    assert_eq!(snapshot.url(), Some("https://github.com/rust-lang/rust"));
    assert!(!snapshot.keyboard_open());
}

#[test]
fn a_clickable_node_is_a_row_and_a_disabled_one_is_not() {
    let mut open = node("generic", "Open card", 100.0);
    open.clickable = true;
    let mut greyed = node("generic", "Greyed card", 200.0);
    greyed.clickable = true;
    greyed.states.disabled = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![open, greyed]));
    assert_eq!(labels(&snapshot), ["Open card"]);
    assert_eq!(unavailable(&snapshot), ["Greyed card"]);
}

#[test]
fn a_focusable_node_is_a_row_only_when_it_has_a_name() {
    let mut named = node("generic", "Skip to content", 100.0);
    named.states.focusable = true;
    let mut nameless = node("generic", "", 200.0);
    nameless.states.focusable = true;
    let inert = node("generic", "Just text", 300.0);
    let snapshot = Browser::default().snapshot_from(&read(vec![named, nameless, inert]));
    assert_eq!(labels(&snapshot), ["Skip to content"]);
}

#[test]
fn a_node_without_a_box_is_not_a_row() {
    let mut boxless = node("button", "Ghost", 100.0);
    boxless.bounds = None;
    let snapshot = Browser::default().snapshot_from(&read(vec![boxless]));
    assert!(snapshot.is_empty());
}

#[test]
fn a_clickable_wrapping_two_rows_stays_a_row() {
    let mut card = node("generic", "Card", 100.0);
    card.clickable = true;
    card.bounds = Some(PageBox {
        x: 50.0,
        y: 90.0,
        width: 400.0,
        height: 100.0,
    });
    let one = node("link", "One", 100.0);
    let two = node("link", "Two", 140.0);
    let snapshot = Browser::default().snapshot_from(&read(vec![card, one, two]));
    assert_eq!(labels(&snapshot), ["Card", "One", "Two"]);
}

#[test]
fn a_form_offers_its_fields_and_refuses_its_disabled_submit() {
    let snapshot = fixture("form");
    assert_eq!(unavailable(&snapshot), ["Submit"]);
    assert!(!labels(&snapshot).contains(&"Submit".to_string()));
    assert_eq!(notices(&snapshot), ["Create your account"]);
}

#[test]
fn fields_are_labelled_by_name_or_placeholder_and_say_what_they_hold() {
    let snapshot = fixture("form");
    let by_label = rows(&snapshot);
    assert_eq!(
        by_label[..4],
        [
            ("Email".into(), Some("empty".into())),
            ("Full name".into(), Some("empty".into())),
            ("text field".into(), None),
            ("Password".into(), Some("contents not shown".into())),
        ]
    );
    for (_, element) in snapshot.refs().take(4) {
        assert!(element.editable, "{} is a field", element.label);
    }
}

#[test]
fn a_fields_value_is_its_detail() {
    let snapshot = fixture("search-results");
    let (_, search) = row(&snapshot, "Search");
    assert_eq!(search.detail.as_deref(), Some("opus 5.5"));
    assert!(search.editable);
}

#[test]
fn a_password_value_is_never_read() {
    let snapshot = fixture("form");
    let all: String = snapshot.refs().map(|(_, e)| e.describe()).collect();
    assert!(!all.contains("hunter2"));

    // Should an engine ever pass a protected field's value on, it still does
    // not reach the snapshot.
    let mut leaked = node("textbox", "PIN", 100.0);
    leaked.value = Some("2468".into());
    leaked.protected = true;
    leaked.states.editable = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![leaked]));
    assert_eq!(
        rows(&snapshot),
        [("PIN".to_string(), Some("contents not shown".to_string()))]
    );
}

/// The snapshot carries which rows are password fields, so the run's own
/// account of what it typed can leave the words out.
#[test]
fn a_protected_field_is_marked_in_the_snapshot() {
    let mut secret = node("textbox", "Password", 100.0);
    secret.protected = true;
    secret.states.editable = true;
    let mut name = node("textbox", "Name", 200.0);
    name.states.editable = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![secret, name]));

    let marks: Vec<bool> = snapshot
        .refs()
        .map(|(handle, _)| snapshot.is_protected(handle).expect("current"))
        .collect();
    assert_eq!(marks, [true, false]);
}

/// The engine says which fields are passwords. A field merely named like one
/// is an ordinary field, whose value is its detail.
#[test]
fn only_a_protected_field_hides_its_contents() {
    let mut hint = node("textbox", "Password hint", 100.0);
    hint.value = Some("first pet".into());
    hint.states.editable = true;
    let mut secret = node("textbox", "Password", 200.0);
    secret.protected = true;
    secret.states.editable = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![hint, secret]));
    assert_eq!(
        rows(&snapshot),
        [
            ("Password hint".to_string(), Some("first pet".to_string())),
            (
                "Password".to_string(),
                Some("contents not shown".to_string())
            ),
        ]
    );
}

/// Chrome names an unlabelled field by its placeholder, and drops the
/// placeholder of a labelled one; a description is never a placeholder, so a
/// field with no name is a "text field" whatever it is described as.
#[test]
fn a_field_with_no_name_is_not_labelled_by_its_description() {
    let mut described = node("textbox", "", 100.0);
    described.description = Some("We never share it".into());
    described.states.editable = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![described]));
    assert_eq!(labels(&snapshot), ["text field"]);
}

/// A natively disabled button has no click listener the layout reports, but
/// it is still a control the page shows and will not let anything use.
#[test]
fn a_disabled_button_that_is_not_clickable_is_unavailable() {
    let mut submit = node("button", "Place order", 100.0);
    submit.states.disabled = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![submit]));
    assert!(snapshot.is_empty());
    assert_eq!(unavailable(&snapshot), ["Place order"]);
}

#[test]
fn a_native_select_is_a_field_only_row_listing_its_choices() {
    let snapshot = fixture("form");
    let (handle, country) = row(&snapshot, "Country");
    assert!(country.editable);
    assert!(snapshot.is_field_only(handle).unwrap());
    assert_eq!(
        country.detail.as_deref(),
        Some("Philippines (options: Japan, Philippines, Singapore)")
    );
    for (other, element) in snapshot.refs() {
        if &*element.label != "Country" {
            assert!(
                !snapshot.is_field_only(other).unwrap(),
                "{} is not field-only",
                element.label
            );
        }
    }
}

#[test]
fn a_label_around_a_hidden_checkbox_becomes_that_checkbox() {
    let snapshot = fixture("form");
    let remember = labels(&snapshot)
        .iter()
        .filter(|label| *label == "Remember me")
        .count();
    assert_eq!(remember, 1, "the label and its control are one row");
    let (handle, element) = row(&snapshot, "Remember me");
    assert_eq!(element.detail.as_deref(), Some("checked"));
    assert!(!element.editable);
    // The row is pressed where the label is drawn, and by the label's handle.
    assert!(snapshot.tap_point(handle).is_ok());
    assert_eq!(
        snapshot.node_key(handle).unwrap().map(NodeKey::as_str),
        Some("main:7")
    );
    assert_eq!(
        row(&snapshot, "Subscribe").1.detail.as_deref(),
        Some("not checked")
    );
}

#[test]
fn a_promoted_label_that_is_disabled_is_unavailable() {
    let label = node("LabelText", "Marketing", 100.0);
    let mut control = node("checkbox", "Marketing", 100.0);
    control.bounds = None;
    control.states.disabled = true;
    let snapshot = Browser::default().snapshot_from(&read(vec![label, control]));
    assert!(snapshot.is_empty());
    assert_eq!(unavailable(&snapshot), ["Marketing"]);
}

// --- the off-screen band ---

#[test]
fn rows_within_a_viewport_of_the_screen_are_offered_marked_off_screen() {
    let snapshot = fixture("search-results");
    let shown = labels(&snapshot);
    // Rows 1-5 start inside the 720px viewport, 6-10 within one viewport
    // below it, 11 and later are further than that.
    for i in 1..=10 {
        assert!(shown.contains(&format!("Result title {i}")), "row {i}");
    }
    for i in 11..=14 {
        assert!(!shown.contains(&format!("Result title {i}")), "row {i}");
    }
    let (_, inside) = row(&snapshot, "Result title 2");
    assert_eq!(inside.detail, None);
    let (_, below) = row(&snapshot, "Result title 7");
    assert_eq!(below.detail.as_deref(), Some("off screen"));
}

#[test]
fn a_band_of_zero_offers_only_the_viewport() {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/browser/search-results.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let snapshot = Browser::with_band(0.0).parse_hierarchy(&raw).unwrap();
    let shown = labels(&snapshot);
    assert!(shown.contains(&"Result title 5".to_string()));
    assert!(!shown.contains(&"Result title 6".to_string()));
    assert!(
        snapshot
            .refs()
            .all(|(_, e)| e.detail.as_deref() != Some("off screen"))
    );
}

#[test]
fn the_default_band_is_one_viewport() {
    assert!((BAND_VIEWPORTS - 1.0).abs() < f64::EPSILON);
}

#[test]
fn off_screen_joins_an_existing_detail() {
    let mut far = node("button", "Menu", 900.0);
    far.states.expanded = Some(true);
    let snapshot = Browser::default().snapshot_from(&read(vec![far]));
    assert_eq!(
        rows(&snapshot),
        [("Menu".to_string(), Some("expanded, off screen".to_string()))]
    );
}

#[test]
fn rows_above_the_viewport_are_in_the_band_too() {
    let snapshot = Browser::default().snapshot_from(&read(vec![
        node("link", "Above", -500.0),
        node("link", "Way above", -2000.0),
    ]));
    assert_eq!(labels(&snapshot), ["Above"]);
}

// --- covering ---

#[test]
fn a_sticky_header_covers_the_rows_under_it() {
    let snapshot = fixture("search-results");
    let (maps, _) = row(&snapshot, "Maps");
    assert!(matches!(
        snapshot.tap_point(maps),
        Err(TapError::Obscured(_))
    ));
    let (result, _) = row(&snapshot, "Result title 1");
    assert!(snapshot.tap_point(result).is_ok());
}

#[test]
fn a_cookie_banner_covers_the_footer_but_not_its_own_buttons() {
    let snapshot = fixture("cookie-banner");
    for covered in ["Privacy", "Terms"] {
        let (handle, _) = row(&snapshot, covered);
        assert!(snapshot.tap_point(handle).is_err(), "{covered} is covered");
    }
    for open in ["Top story", "Sports", "Accept all", "Reject all"] {
        let (handle, _) = row(&snapshot, open);
        assert!(snapshot.tap_point(handle).is_ok(), "{open} is reachable");
    }
}

// --- modals ---

#[test]
fn under_a_modal_only_its_rows_are_offered() {
    let snapshot = fixture("modal");
    assert_eq!(
        labels(&snapshot),
        ["Type the repository name", "Delete", "Cancel"]
    );
    assert_eq!(
        notices(&snapshot),
        ["Are you sure?", "This cannot be undone."]
    );
}

#[test]
fn a_modals_rows_survive_truncation() {
    let mut nodes: Vec<PageNode> = (0..300)
        .map(|i| node("link", &format!("Background {i}"), f64::from(i) * 2.0))
        .collect();
    let mut dialog = node("dialog", "Confirm", 0.0);
    dialog.states.modal = true;
    dialog.bounds = Some(PageBox {
        x: 500.0,
        y: 0.0,
        width: 600.0,
        height: 400.0,
    });
    nodes.push(dialog);
    for (i, name) in (0..2_u8).zip(["Yes", "No"]) {
        let mut inside = node("button", name, 100.0 + 40.0 * f64::from(i));
        inside.bounds = Some(PageBox {
            x: 520.0,
            y: 100.0 + 40.0 * f64::from(i),
            width: 80.0,
            height: 30.0,
        });
        nodes.push(inside);
    }
    let snapshot = Browser::default().snapshot_from(&read(nodes));
    assert_eq!(labels(&snapshot), ["Yes", "No"]);
}

// --- too many rows ---

#[test]
fn truncation_keeps_the_viewport_then_the_nearest_band_and_says_what_it_dropped() {
    // 200 rows in the viewport, then 200 below it at growing distances.
    let mut nodes = Vec::new();
    for i in 0..200 {
        nodes.push(node("link", &format!("In {i}"), f64::from(i) * 3.0));
    }
    for i in 0..200 {
        nodes.push(node(
            "link",
            &format!("Band {i}"),
            725.0 + f64::from(i) * 3.0,
        ));
    }
    let snapshot = Browser::default().snapshot_from(&read(nodes));
    assert_eq!(snapshot.rows(), MAX_ELEMENTS);
    let shown = labels(&snapshot);
    assert!(shown.contains(&"In 199".to_string()));
    let band_kept = MAX_ELEMENTS - 200;
    assert!(shown.contains(&format!("Band {}", band_kept - 1)));
    assert!(!shown.contains(&format!("Band {band_kept}")));
    let dropped = 400 - MAX_ELEMENTS;
    assert!(
        notices(&snapshot)
            .iter()
            .any(|n| n.contains(&dropped.to_string()) && n.contains("left out")),
        "{:?}",
        notices(&snapshot)
    );
    // Kept rows stay in paint order.
    assert_eq!(shown.first().map(String::as_str), Some("In 0"));
}

#[test]
fn a_page_within_the_limit_has_no_truncation_notice() {
    let snapshot = fixture("github-repo");
    assert!(notices(&snapshot).iter().all(|n| !n.contains("left out")));
}

// --- look-alikes ---

#[test]
fn three_alike_buttons_gain_the_context_they_sit_in() {
    let snapshot = fixture("edit-buttons");
    let edits: Vec<_> = rows(&snapshot)
        .into_iter()
        .filter(|(label, _)| label == "Edit")
        .map(|(_, detail)| detail)
        .collect();
    assert_eq!(
        edits,
        [
            Some("in Experience".to_string()),
            // No heading to name: the row's own container says where it is.
            Some("in Education: State University".to_string()),
            Some("in Skills".to_string()),
        ]
    );
    assert_eq!(row(&snapshot, "Save").1.detail, None);
}

#[test]
fn twenty_alike_rows_are_left_identical() {
    let snapshot = fixture("twenty-alike");
    assert_eq!(snapshot.rows(), 20);
    assert!(snapshot.refs().all(|(_, e)| e.detail.is_none()));
}

#[test]
fn four_alike_rows_are_left_identical_too() {
    let nodes: Vec<PageNode> = (0..4)
        .map(|i| {
            let mut n = node("button", "More", 100.0 + f64::from(i) * 30.0);
            n.context = Some(format!("Item {i}").into());
            n
        })
        .collect();
    let snapshot = Browser::default().snapshot_from(&read(nodes));
    assert!(snapshot.refs().all(|(_, e)| e.detail.is_none()));
}

// --- frames, dialogs, tabs ---

#[test]
fn nodes_from_a_frame_sit_under_their_iframe_in_order() {
    let snapshot = fixture("iframe");
    assert_eq!(labels(&snapshot), ["Home", "Card number", "Pay", "Help"]);
}

#[test]
fn an_open_confirm_is_carried_and_described() {
    let snapshot = fixture("confirm-dialog");
    let dialog = snapshot.dialog().expect("a dialog is open");
    assert_eq!(&*dialog.message, "Really delete this item?");
    assert!(snapshot.worth_acting_on());
    assert!(snapshot.is_empty());
    let said = notices(&snapshot);
    assert_eq!(said.len(), 1);
    assert!(said[0].contains("Really delete this item?"));
    assert!(said[0].contains("confirm"));
}

#[test]
fn an_accepted_alert_is_among_the_notices() {
    let mut page = read(vec![node("link", "Next", 100.0)]);
    page.accepted_alert = Some("Saved!".into());
    let snapshot = Browser::default().snapshot_from(&page);
    assert!(notices(&snapshot).iter().any(|n| n.contains("Saved!")));
}

#[test]
fn the_other_tabs_are_carried_and_the_driven_one_is_not() {
    let snapshot = fixture("two-tabs");
    let others: Vec<_> = snapshot
        .tabs()
        .map(|(_, tab)| (tab.key.as_str().to_string(), tab.title.to_string()))
        .collect();
    assert_eq!(others, [("tab-2".to_string(), "Page B".to_string())]);
    assert_eq!(snapshot.app(), Some("https://example.test"));
}

/// Two tabs can show the same page; the driven one is told apart by its key,
/// not by its address and title.
#[test]
fn the_driven_tab_is_known_by_its_key() {
    let tab = |key: &str| PageTab {
        key: TabKey::new(key),
        title: "Example".into(),
        url: "https://example.test/".into(),
        opened_by_run: true,
    };
    let mut page = read(vec![]);
    page.tabs = vec![tab("t1"), tab("t2")];
    page.current = Some(TabKey::new("t2"));

    let snapshot = Browser::default().snapshot_from(&page);

    let others: Vec<&str> = snapshot.tabs().map(|(_, t)| t.key.as_str()).collect();
    assert_eq!(others, ["t1"]);
}

/// Rows inside a frame from another site are not offered, so the page says
/// there is more to it than the rows show.
#[test]
fn a_frame_that_could_not_be_read_is_among_the_notices() {
    let mut page = read(vec![node("button", "Pay", 100.0)]);
    page.unreadable_frames = 1;
    assert!(
        notices(&Browser::default().snapshot_from(&page))
            .contains(&"A frame from another site on this page cannot be read".to_owned())
    );

    page.unreadable_frames = 2;
    assert!(
        notices(&Browser::default().snapshot_from(&page))
            .contains(&"2 frames from other sites on this page cannot be read".to_owned())
    );

    page.unreadable_frames = 0;
    assert!(
        !notices(&Browser::default().snapshot_from(&page))
            .iter()
            .any(|n| n.contains("frame")),
    );
}

#[test]
fn tabs_are_kept_when_the_driven_tab_is_not_among_them() {
    let mut page = read(vec![]);
    page.tabs = vec![PageTab {
        key: TabKey::new("t9"),
        title: "Elsewhere".into(),
        url: "https://elsewhere.test/".into(),
        opened_by_run: true,
    }];
    let snapshot = Browser::default().snapshot_from(&page);
    assert_eq!(snapshot.tabs().count(), 1);
}

#[test]
fn the_app_is_the_origin_with_its_port() {
    let mut page = read(vec![]);
    page.url = "https://user:pw@Host.test:8443/a/b?q=1#x".into();
    assert_eq!(
        Browser::default().snapshot_from(&page).app(),
        Some("https://Host.test:8443")
    );
    page.url = "about:blank".into();
    assert_eq!(
        Browser::default().snapshot_from(&page).app(),
        Some("about:")
    );
}

#[test]
fn rows_carry_the_engines_node_keys_in_order() {
    let snapshot = fixture("iframe");
    let keys: Vec<_> = snapshot
        .refs()
        .map(|(h, _)| {
            snapshot
                .node_key(h)
                .unwrap()
                .map(|k| k.as_str().to_string())
        })
        .collect();
    assert!(keys.iter().all(Option::is_some));
    assert_eq!(keys[1].as_deref().map(|k| k.starts_with("f1:")), Some(true));
}

#[test]
fn a_modal_with_too_many_rows_keeps_its_visible_ones_first() {
    let mut dialog = node("dialog", "Pick one", 0.0);
    dialog.states.modal = true;
    dialog.bounds = Some(PageBox {
        x: 0.0,
        y: -5000.0,
        width: 1280.0,
        height: 10000.0,
    });
    let mut nodes = vec![node("link", "Behind", 10.0), dialog];
    // Two hundred below the screen first in paint order, then 100 on it.
    for i in 0..200 {
        nodes.push(node("option", &format!("Far {i}"), 800.0 + f64::from(i)));
    }
    for i in 0..100 {
        nodes.push(node("option", &format!("Near {i}"), f64::from(i) * 5.0));
    }
    let snapshot = Browser::default().snapshot_from(&read(nodes));
    let shown = labels(&snapshot);
    assert_eq!(shown.len(), MAX_ELEMENTS);
    assert!(!shown.contains(&"Behind".to_string()));
    assert!((0..100).all(|i| shown.contains(&format!("Near {i}"))));
    assert!(
        notices(&snapshot)
            .iter()
            .any(|n| n.contains("50 more rows"))
    );
}

// --- a real Chrome's page ---

/// The recorded page's main frame, which is also its tab, and its iframe.
const SIGNUP_MAIN: &str = "933B271DD0EC81D3402A478AE60B40E5";
const SIGNUP_CHILD: &str = "4634BC906F07018102C158CE6AEB76E4";

/// The sign-up page recorded from Chrome 153 under `tests/fixtures/cdp/`,
/// translated by the DevTools engine's read, as a run would see it.
fn recorded_signup() -> Snapshot {
    use jev_pilot::browser::read::{Loaders, OwnedTabs, RawRead, translate};
    let load = |name: &str| -> serde_json::Value {
        let path = format!(
            "{}/tests/fixtures/cdp/signup/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture exists"))
            .expect("fixture is JSON")
    };
    let raw = RawRead {
        frame_tree: load("frame-tree"),
        ax_trees: vec![
            (SIGNUP_MAIN.into(), load("ax-0")),
            (SIGNUP_CHILD.into(), load("ax-1")),
        ],
        snapshot: load("snapshot"),
        layout_metrics: load("layout-metrics"),
        targets: load("targets"),
    };
    let mut owned = OwnedTabs::default();
    owned.start_on(SIGNUP_MAIN, true);
    let page = translate(&raw, SIGNUP_MAIN, &owned, &mut Loaders::default()).expect("translates");
    Browser::default().snapshot_from(&page)
}

/// Chrome reports each label around a field as a clickable node of its own,
/// drawn under the field and not quite containing it. It is the field, seen
/// from outside, so the field is offered once; a password field says its
/// contents are not shown rather than that it is empty, since the read never
/// carries them.
#[test]
fn a_recorded_page_offers_each_field_once() {
    let snapshot = recorded_signup();

    assert_eq!(
        rows(&snapshot),
        [
            ("Send me news".to_string(), Some("checked".to_string())),
            ("Pick the free plan".to_string(), None),
            (
                "Terms of service".to_string(),
                Some("off screen".to_string())
            ),
            ("Email".to_string(), Some("empty".to_string())),
            (
                "Password".to_string(),
                Some("contents not shown".to_string())
            ),
            (
                "Country".to_string(),
                Some("Philippines (options: Japan, Philippines, Spain)".to_string())
            ),
            ("Inside frame".to_string(), None),
            ("Home".to_string(), None),
            ("Help".to_string(), None),
        ]
    );
    assert_eq!(notices(&snapshot), ["Frame text"]);
    assert_eq!(unavailable(&snapshot), ["Create account"]);
    for (handle, element) in snapshot.refs() {
        assert!(
            snapshot.tap_point(handle).is_ok(),
            "{} can be tapped",
            element.label
        );
    }
}
