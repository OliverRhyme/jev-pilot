//! Observations of a screen, and the references they issue.

use jev_pilot::platform::{Android, HierarchyError, Platform};
use jev_pilot::snapshot::{MAX_ELEMENTS, Point};
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
