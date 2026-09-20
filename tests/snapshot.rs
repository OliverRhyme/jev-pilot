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
