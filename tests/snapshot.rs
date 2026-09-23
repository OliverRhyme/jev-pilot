//! Observations of a screen, and the references they issue.

use jev_pilot::platform::{Android, HierarchyError, Platform};
use jev_pilot::snapshot::{Bounds, Element, MAX_ELEMENTS, Point, Snapshot};
use std::fmt::Write as _;

const SETTINGS: &str = include_str!("fixtures/settings.xml");

/// A reference names a position on a screen that has since been replaced, even
/// when the replacement looks identical. Resolving it against a newer snapshot
/// must fail rather than silently tap whatever now occupies that slot.
#[test]
fn a_reference_from_an_earlier_snapshot_does_not_resolve_against_a_later_one() {
    let first = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let stale = first.refs().next().expect("fixture has elements").0;

    let second = Android.parse_hierarchy(SETTINGS).expect("fixture parses");

    assert!(first.resolve(stale).is_ok());
    assert!(
        second.resolve(stale).is_err(),
        "a ref must not cross snapshot boundaries even between identical screens"
    );
}

/// Bounds arrive from a dump the worker did not author. Averaging them by
/// `(a + b) / 2` overflows long before `i32` runs out of room, which panics a
/// debug build on a screen it merely failed to understand.
#[test]
fn extreme_bounds_do_not_overflow_the_centre_calculation() {
    let xml = format!(
        r#"<hierarchy><node clickable="true" text="Far" bounds="[{},{}][{},{}]"/></hierarchy>"#,
        i32::MAX - 1,
        i32::MAX - 1,
        i32::MAX,
        i32::MAX
    );
    let snapshot = Android.parse_hierarchy(&xml).expect("parses");
    let handle = snapshot.refs().next().expect("one element").0;

    let centre = snapshot.resolve(handle).expect("live ref").bounds.center();

    assert_eq!(
        centre,
        Point {
            x: i32::MAX - 1,
            y: i32::MAX - 1
        }
    );
}

/// An element that cannot be offered as a choice cannot be acted on, so a
/// screen holding more than can be addressed is refused rather than truncated.
#[test]
fn a_screen_with_more_rows_than_can_be_addressed_is_refused() {
    let mut rows = String::new();
    for n in 0..=MAX_ELEMENTS {
        write!(
            rows,
            r#"<node clickable="true" text="Row {n}" bounds="[0,0][10,10]"/>"#
        )
        .expect("writing to a String cannot fail");
    }

    let outcome = Android.parse_hierarchy(&format!("<hierarchy>{rows}</hierarchy>"));

    assert!(
        matches!(outcome, Err(HierarchyError::TooMany(_))),
        "{outcome:?}"
    );
}

/// A disabled control keeps `clickable="true"` — the flag says the view handles
/// taps, not that it will act on one. Offering it hands the model a choice that
/// silently does nothing, and a loop reads that as the tap having worked.
#[test]
fn a_disabled_control_is_not_offered_even_though_it_is_clickable() {
    const SCREEN: &str = r#"<hierarchy>
      <node clickable="true" enabled="true"  text="Continue" bounds="[0,0][100,50]"/>
      <node clickable="true" enabled="false" text="Submit"   bounds="[0,60][100,110]"/>
    </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(SCREEN).expect("parses");
    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();

    assert_eq!(
        labels,
        vec!["Continue"],
        "a greyed-out button is not a target"
    );
}

/// Some controls expose their action as a long press or a state toggle rather
/// than a click, so a click-only filter silently drops them from the catalog.
#[test]
fn long_clickable_and_checkable_controls_are_offered() {
    const SCREEN: &str = r#"<hierarchy>
      <node long-clickable="true" enabled="true" text="Shortcut" bounds="[0,0][100,50]"/>
      <node checkable="true" enabled="true" text="Wi-Fi toggle" bounds="[0,60][100,110]"/>
    </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(SCREEN).expect("parses");
    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();

    assert_eq!(labels, vec!["Shortcut", "Wi-Fi toggle"]);
}

/// An accessibility helper serving the hierarchy over a socket emits the same
/// document `uiautomator dump` does, so the reader is shared. Captured from the
/// helper on a physical Pixel 8 Pro; it is 50x faster to obtain, which is the
/// whole reason to have a second source at all.
#[test]
fn a_helper_served_hierarchy_reads_with_the_same_parser() {
    const HELPER: &str = include_str!("fixtures/helper-home.xml");

    let snapshot = Android
        .parse_hierarchy(HELPER)
        .expect("the helper document parses");

    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();
    assert!(!labels.is_empty(), "the home screen has actionable rows");
    assert!(
        labels.iter().any(|l| l.contains("YouTube")),
        "got {labels:?}"
    );
    // Geometry survives, which is what a tap depends on.
    let (handle, _) = snapshot.refs().next().expect("a row");
    let centre = snapshot.resolve(handle).expect("live").bounds.center();
    assert!(centre.x > 0 && centre.y > 0, "got {centre:?}");
}

/// A row can be reported at its full size while most of it sits behind
/// something drawn later. Tapping the centre of the reported box then lands on
/// whatever covers it.
///
/// Taken from a real YouTube results screen on a 1008x2244 Pixel 8 Pro: the
/// last result is reported as [0,1994][1008,2244], running to the bottom of the
/// display, while the navigation bar occupies [0,2061][1008,2183]. The naive
/// centre is (504, 2119) — inside the Create button, which is what a tap there
/// actually opened.
#[test]
fn a_row_half_hidden_behind_a_bar_is_tapped_where_it_is_visible() {
    let row = |label: &str, l, t, r, b| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds {
            left: l,
            top: t,
            right: r,
            bottom: b,
        },
    };
    let snapshot = Snapshot::new(vec![
        row("Jev Explained: Demos and Use Cases", 0, 1994, 1008, 2244),
        row("Home", 0, 2061, 201, 2183),
        row("Create", 402, 2061, 604, 2183),
        row("You", 806, 2061, 1008, 2183),
    ])
    .expect("a screen");

    let (result, _) = snapshot.refs().next().expect("the result row");
    let point = snapshot.tap_point(result).expect("a visible point");

    assert!(
        point.y < 2061,
        "must land above the navigation bar, got {point:?}"
    );
    assert!(point.y >= 1994, "and inside the row, got {point:?}");
}

/// An element with nothing on top of it is tapped in the middle, as before.
#[test]
fn an_unobstructed_row_is_still_tapped_at_its_centre() {
    let snapshot = Snapshot::new(vec![Element {
        label: "Network and Internet".into(),
        detail: None,
        editable: false,
        bounds: Bounds {
            left: 168,
            top: 596,
            right: 840,
            bottom: 744,
        },
    }])
    .expect("a screen");

    let (row, _) = snapshot.refs().next().expect("the row");
    assert_eq!(
        snapshot.tap_point(row).expect("visible"),
        Point { x: 504, y: 670 }
    );
}

/// A row completely covered cannot be tapped at all, and saying so beats
/// tapping whatever is on top of it.
#[test]
fn a_fully_covered_row_is_refused() {
    let snapshot = Snapshot::new(vec![
        Element {
            label: "behind a dialog".into(),
            detail: None,
            editable: false,
            bounds: Bounds {
                left: 0,
                top: 500,
                right: 1000,
                bottom: 600,
            },
        },
        Element {
            label: "the dialog".into(),
            detail: None,
            editable: false,
            bounds: Bounds {
                left: 0,
                top: 400,
                right: 1000,
                bottom: 900,
            },
        },
    ])
    .expect("a screen");

    let (hidden, _) = snapshot.refs().next().expect("the covered row");
    assert!(snapshot.tap_point(hidden).is_err());
}

/// A screen caught mid-transition parses to nothing actionable. That is not a
/// decision for a model to make — there is no row to choose and no operation
/// that helps — so a reader must look again rather than hand it over. Measured
/// on a real run: a YouTube settings screen read 60ms after a tap returned
/// zero rows, and the loop spent a step choosing `wait` to recover. Through
/// the 2.5s CLI the same transition was never visible.
#[test]
fn a_screen_with_nothing_on_it_is_not_worth_acting_on() {
    let empty = Snapshot::new(Vec::new()).expect("an empty screen is still a screen");
    assert!(!empty.worth_acting_on());

    let occupied = Snapshot::new(vec![Element {
        label: "General".into(),
        detail: None,
        editable: false,
        bounds: Bounds {
            left: 0,
            top: 0,
            right: 100,
            bottom: 50,
        },
    }])
    .expect("one row");
    assert!(occupied.worth_acting_on());
}

/// An empty text field is the one a form most needs acted on, and it was the
/// one the reader threw away: a node is kept only if it has text of its own,
/// and an empty field has none. Taken from a real Flutter form, where the
/// only input on screen reads
/// `class="android.widget.EditText" text="" content-desc=""
///  hint="Number to be loaded" editable="true"` — the name is right there, in
/// the hint, and was not being read.
///
/// Before this, the form parsed to two elements and neither was the field, so
/// `type_text` had no target and the loop could not say "type into that".
#[test]
fn an_empty_text_field_is_surfaced_and_named_by_its_hint() {
    const FLUTTER_FORM: &str = include_str!("fixtures/flutter-form.xml");

    let snapshot = Android.parse_hierarchy(FLUTTER_FORM).expect("a screen");
    let field = snapshot
        .refs()
        .map(|(_, element)| element)
        .find(|element| element.editable)
        .expect("the form's text field");

    assert_eq!(&*field.label, "Number to be loaded");
    assert_eq!(field.bounds.left, 41, "its own bounds, not a parent's");
}

/// The hint is what the field is called, not what it contains. A field
/// showing its hint holds no text, and the two must not be confused or a run
/// will think a form is already filled.
#[test]
fn a_hint_names_a_field_without_pretending_to_be_its_contents() {
    let raw = r#"<hierarchy rotation="0">
        <node index="0" text="" content-desc="" class="android.widget.EditText"
              editable="true" hint="Number to be loaded" clickable="true"
              bounds="[41,360][967,493]" />
        <node index="1" text="0917" content-desc="" class="android.widget.EditText"
              editable="true" hint="Number to be loaded" clickable="true"
              bounds="[41,560][967,693]" />
        </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(raw).expect("a screen");
    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();

    assert_eq!(
        labels,
        ["Number to be loaded", "0917"],
        "an empty field is named by its hint; a filled one by what it holds"
    );
}

/// Flutter does not build its accessibility tree out of Android widgets, so a
/// rule reading class names is a rule about one toolkit. The node says
/// `editable="true"` outright, and that is what should be believed.
#[test]
fn editability_is_taken_from_the_trait_not_from_a_class_name() {
    let raw = r#"<hierarchy rotation="0">
        <node index="0" text="typed" class="android.view.View" editable="true"
              clickable="true" bounds="[0,0][100,50]" />
        <node index="1" text="Submit" class="android.widget.Button"
              clickable="true" bounds="[0,60][100,110]" />
        </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(raw).expect("a screen");
    let editable: Vec<bool> = snapshot.refs().map(|(_, e)| e.editable).collect();

    assert_eq!(editable, [true, false]);
}

/// A dump from `uiautomator` carries no `editable` attribute at all, so the
/// class name is still the only signal there and must keep working.
#[test]
fn editability_falls_back_to_the_class_when_the_trait_is_absent() {
    let raw = r#"<hierarchy rotation="0">
        <node index="0" text="typed" class="android.widget.EditText"
              clickable="true" bounds="[0,0][100,50]" />
        </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(raw).expect("a screen");
    assert!(snapshot.refs().next().expect("one row").1.editable);
}

/// A node with nothing to call it and nothing to type into is noise. Keeping
/// it would put an unnameable row in front of the model.
#[test]
fn a_clickable_with_no_name_at_all_is_still_dropped() {
    let raw = r#"<hierarchy rotation="0">
        <node index="0" text="" content-desc="" class="android.view.View"
              clickable="true" bounds="[0,0][100,50]" />
        </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(raw).expect("a screen");
    assert_eq!(snapshot.refs().count(), 0);
}

/// An editable field with no hint either still has to be actable on, so it is
/// named for what it is rather than dropped.
#[test]
fn a_nameless_field_is_described_rather_than_discarded() {
    let raw = r#"<hierarchy rotation="0">
        <node index="0" text="" content-desc="" class="android.widget.EditText"
              editable="true" clickable="true" bounds="[0,0][100,50]" />
        </hierarchy>"#;

    let snapshot = Android.parse_hierarchy(raw).expect("a screen");
    let field = snapshot.refs().next().expect("the field").1;

    assert!(field.editable);
    assert!(
        !field.label.is_empty(),
        "it must be nameable to be targetable"
    );
}

/// The soft keyboard is a window of its own, and its keys are not rows of the
/// screen. Measured on a real search field: the input method's window held 54
/// nodes of which 52 were clickable and labelled, against 7 nodes and 2
/// clickable in the app's own. Offered as targets they drowned the screen —
/// `operation` 0.27 and `target` 0.31 against a floor of 0.6, stalling two
/// consecutive steps on a keyboard the loop had itself opened.
///
/// A key is never the thing to act on: it is the rendering of a field the run
/// already decided to type into, and typing goes through the field.
#[test]
fn the_soft_keyboard_is_not_offered_as_rows() {
    const KEYBOARD: &str = include_str!("fixtures/keyboard.xml");

    let snapshot = Android.parse_hierarchy(KEYBOARD).expect("a screen");
    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();

    for key in ["q", "w", "e", "Shift", "Delete", "Space"] {
        assert!(!labels.contains(&key), "a key is on offer: {labels:?}");
    }
    assert!(
        labels.len() < 10,
        "the app's own rows, not 52 keys: {labels:?}"
    );
}

/// Whether the keyboard is up is worth knowing in itself — it is the
/// difference between a form that is being filled and one that is not — so it
/// is reported rather than left to be inferred from a pile of single letters.
#[test]
fn the_screen_says_whether_the_keyboard_is_open() {
    const KEYBOARD: &str = include_str!("fixtures/keyboard.xml");
    const SETTLED: &str = include_str!("fixtures/settings.xml");

    assert!(
        Android
            .parse_hierarchy(KEYBOARD)
            .expect("a screen")
            .keyboard_open()
    );
    assert!(
        !Android
            .parse_hierarchy(SETTLED)
            .expect("a screen")
            .keyboard_open()
    );
}

/// After acting, a run has to know whether the screen it is looking at is the
/// result or still the screen it acted on. Comparing what is on them is enough
/// and costs nothing: a fingerprint over the rows and where they sit.
#[test]
fn a_screen_can_be_told_apart_from_the_one_before_it() {
    let row = |label: &str, top: i32| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds {
            left: 0,
            top,
            right: 100,
            bottom: top + 50,
        },
    };

    let before = Snapshot::new(vec![row("Continue", 0)]).expect("a screen");
    let same = Snapshot::new(vec![row("Continue", 0)]).expect("a screen");
    let moved = Snapshot::new(vec![row("Continue", 80)]).expect("a screen");
    let other = Snapshot::new(vec![row("Back", 0)]).expect("a screen");

    assert_eq!(
        before.fingerprint(),
        same.fingerprint(),
        "same rows, same place"
    );
    assert_ne!(before.fingerprint(), moved.fingerprint(), "the row moved");
    assert_ne!(before.fingerprint(), other.fingerprint(), "different rows");
}

/// A generation is not identity. Every read makes a new one, so comparing
/// those would call every screen different and defeat the purpose.
#[test]
fn a_fingerprint_is_about_what_is_on_screen_not_when_it_was_read() {
    let element = || Element {
        label: "Continue".into(),
        detail: None,
        editable: false,
        bounds: Bounds {
            left: 0,
            top: 0,
            right: 100,
            bottom: 50,
        },
    };

    let first = Snapshot::new(vec![element()]).expect("a screen");
    let second = Snapshot::new(vec![element()]).expect("a screen");

    assert_ne!(
        format!("{:?}", first.refs().next().expect("a row").0),
        format!("{:?}", second.refs().next().expect("a row").0),
        "the references differ, as they must"
    );
    assert_eq!(
        first.fingerprint(),
        second.fingerprint(),
        "the screen does not"
    );
}

/// The keyboard arriving or leaving changes what a run can do, so it counts as
/// the screen having changed even when the rows behind it have not.
#[test]
fn the_keyboard_coming_up_counts_as_a_change() {
    let rows = || {
        vec![Element {
            label: "Number to be loaded".into(),
            detail: None,
            editable: true,
            bounds: Bounds {
                left: 41,
                top: 360,
                right: 967,
                bottom: 493,
            },
        }]
    };
    let closed = Snapshot::new(rows()).expect("a screen");
    let open = Snapshot::new(rows())
        .expect("a screen")
        .with_keyboard_open(true);

    assert_ne!(closed.fingerprint(), open.fingerprint());
}

/// A run that presses Home, follows a notification, or is bounced into a
/// browser is no longer driving the app it was asked about — and until the
/// screen says whose it is, nothing downstream can tell. The launcher looks
/// like any other list of rows.
#[test]
fn the_screen_says_which_app_it_belongs_to() {
    const SETTINGS: &str = include_str!("fixtures/settings.xml");

    assert_eq!(
        Android.parse_hierarchy(SETTINGS).expect("a screen").app(),
        Some("com.android.settings"),
    );
}

/// Status bar and navigation bar belong to the system and are drawn over every
/// app. Naming the screen after them would call every screen the same one.
#[test]
fn system_chrome_does_not_get_to_name_the_screen() {
    const HOME: &str = include_str!("fixtures/helper-home.xml");

    assert_ne!(
        Android.parse_hierarchy(HOME).expect("a screen").app(),
        Some("com.android.systemui"),
    );
}

/// What a screen *says* is not what a screen *offers*, and a catalog of
/// actionable rows alone hands the judge a set of buttons with no account of
/// the state they act on. Measured on a PIN pad: twelve rows, all keys, while
/// the screen itself said "4 of 6 digits entered", "Step 3 of 3" and what the
/// transfer was for. Asked "is the goal met?", the run could only look at the
/// keys.
#[test]
fn the_screen_keeps_the_words_that_are_not_rows() {
    const PIN_PAD: &str = include_str!("fixtures/pin-pad.xml");

    let screen = Android.parse_hierarchy(PIN_PAD).expect("a screen");
    let notices: Vec<&str> = screen.notices().collect();

    assert!(
        notices.contains(&"4 of 6 digits entered, TPIN"),
        "progress through the PIN is the whole state of this screen: {notices:?}",
    );
    assert!(notices.contains(&"Step 3 of 3"), "{notices:?}");
    assert!(
        notices.iter().any(|n| n.contains("TEST PAYEE")),
        "{notices:?}",
    );
    // Keys are rows. Repeating them as text would say each one twice.
    assert!(!notices.contains(&"DEL"), "{notices:?}");
}

/// A screen whose only change is in its words has still changed. Without this
/// a PIN pad looks identical after every digit, and a run entering one is
/// stopped by its own guard against repeating itself.
#[test]
fn words_that_are_not_rows_still_make_a_screen_a_different_screen() {
    const PIN_PAD: &str = include_str!("fixtures/pin-pad.xml");

    let four = Android.parse_hierarchy(PIN_PAD).expect("a screen");
    let five = Android
        .parse_hierarchy(&PIN_PAD.replace("4 of 6 digits", "5 of 6 digits"))
        .expect("a screen");

    assert_ne!(four.fingerprint(), five.fingerprint());
}

/// A control that is present but switched off is a fact about what can be
/// done, not a line of prose. Kept among the screen's words it sits beside
/// captions and insurance notices, where the only thing distinguishing it is
/// how it happens to be worded — and wording alone once moved a login screen
/// from 0.03 to 0.59 on "is this an error screen?".
///
/// It belongs with the actions, named as one that is not available.
#[test]
fn a_disabled_control_is_listed_apart_from_what_the_screen_says() {
    const FORM: &str = include_str!("fixtures/disabled-continue.xml");

    let screen = Android.parse_hierarchy(FORM).expect("a screen");
    let unavailable: Vec<&str> = screen.unavailable().collect();
    let notices: Vec<&str> = screen.notices().collect();

    assert_eq!(unavailable, ["Continue"]);
    assert!(
        !notices.iter().any(|said| said.contains("Continue")),
        "a disabled control is not one of the screen's words: {notices:?}",
    );
    // Captions are unaffected: they were never actions.
    assert!(notices.contains(&"Step 1 of 3"), "{notices:?}");
}

/// A button becoming available is the most important change a form makes, and
/// a run that cannot see it has happened will not go looking for the row.
#[test]
fn a_control_becoming_available_makes_it_a_different_screen() {
    const FORM: &str = include_str!("fixtures/disabled-continue.xml");

    let off = Android.parse_hierarchy(FORM).expect("a screen");
    let on = Android
        .parse_hierarchy(&FORM.replace(r#"enabled="false""#, r#"enabled="true""#))
        .expect("a screen");

    assert_ne!(off.fingerprint(), on.fingerprint());
}

/// A screen that is still being drawn emits accessibility events; one that
/// has finished does not. The helper sees that stream, so how long the screen
/// has been quiet is a fact the device can state exactly — where a caller
/// polling the tree from outside can only watch for the answer to stop
/// changing, and cannot tell a screen that has finished from one that is
/// between two others and happens to be still for a moment.
#[test]
fn the_screen_says_how_long_it_has_been_quiet() {
    const SETTLED: &str = include_str!("fixtures/settings.xml");

    let quiet = SETTLED.replace(
        "<hierarchy rotation=\"0\"",
        "<hierarchy rotation=\"0\" quiet-ms=\"431\"",
    );
    assert_eq!(
        Android
            .parse_hierarchy(&quiet)
            .expect("a screen")
            .quiet_for_ms(),
        Some(431),
    );
}

/// A reader that cannot say is not the same as a screen that has just
/// changed, and treating the two alike would make every `uiautomator` dump
/// look like a screen mid-transition.
#[test]
fn a_reader_that_cannot_say_how_quiet_it_is_says_nothing() {
    const SETTLED: &str = include_str!("fixtures/settings.xml");

    assert_eq!(
        Android
            .parse_hierarchy(SETTLED)
            .expect("a screen")
            .quiet_for_ms(),
        None,
    );
}

/// "Did anything change?" and "have I been here before?" are different
/// questions, and one hash cannot answer both. A spinner moving a pixel is a
/// change; it is not a different place. A form re-entered after a detour is
/// the same place, and its rows sit at slightly different offsets.
///
/// Measured: three laps of one transfer flow, the same screen by rows, words
/// and unavailable controls on steps 6, 11 and 23 — and the run's own
/// revisit signal silent on every one of them, because the geometry had
/// shifted underneath.
#[test]
fn where_a_screen_is_does_not_change_which_screen_it_is() {
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let rows = |top| {
        vec![
            Element {
                label: "Account Number".into(),
                detail: None,
                editable: true,
                bounds: Bounds::from_origin_size(0, top, 400, 80),
            },
            Element {
                label: "Continue".into(),
                detail: None,
                editable: false,
                bounds: Bounds::from_origin_size(0, top + 200, 400, 80),
            },
        ]
    };
    let here = Snapshot::new(rows(100)).expect("a screen");
    let shifted = Snapshot::new(rows(140)).expect("a screen");

    assert_ne!(
        here.fingerprint(),
        shifted.fingerprint(),
        "moving is a change, which is what settling asks about",
    );
    assert_eq!(
        here.place(),
        shifted.place(),
        "and it is the same place, which is what revisiting asks about",
    );
}

/// A place is still what it offers and says, so a screen that gains a row is
/// somewhere else.
#[test]
fn a_screen_that_gains_a_row_is_a_different_place() {
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let row = |label: &str| Element {
        label: label.into(),
        detail: None,
        editable: false,
        bounds: Bounds::from_origin_size(0, 0, 400, 80),
    };
    let before = Snapshot::new(vec![row("Account Number")]).expect("a screen");
    let after = Snapshot::new(vec![row("Account Number"), row("Continue")]).expect("a screen");

    assert_ne!(before.place(), after.place());
}

/// A web page is read whole: every row on it is in the hierarchy whether or not
/// it is on screen, so scrolling changes no row and no word. What it changes is
/// which rows can be reached. Measured on a Google results page: a scroll that
/// brought the wanted result into reach counted as a fourth visit to the same
/// place, and the run ended for going in circles.
#[test]
fn a_page_scrolled_to_other_rows_is_a_different_place() {
    use jev_pilot::snapshot::{Bounds, Element, Snapshot};

    let page = |shift| {
        let row = |label: &str, top: i32| Element {
            label: label.into(),
            detail: None,
            editable: false,
            bounds: Bounds::from_origin_size(0, top - shift, 400, 80),
        };
        vec![
            row("Result A", 100),
            row("Result B", 600),
            // The toolbar is drawn last and covers everything beneath it.
            Element {
                label: "Toolbar".into(),
                detail: None,
                editable: false,
                bounds: Bounds::from_origin_size(0, 500, 400, 1000),
            },
        ]
    };
    let top = Snapshot::new(page(0)).expect("a screen");
    let scrolled = Snapshot::new(page(400)).expect("a screen");

    assert_ne!(top.place(), scrolled.place());
}
