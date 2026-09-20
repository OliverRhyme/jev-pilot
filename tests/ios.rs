//! Reading an XCUITest hierarchy.
//!
//! The fixture here was written by hand from the documented XCUITest attribute
//! shape, not captured from a device. It pins the reader's behaviour but proves
//! nothing about real hardware; see the caveat on [`jev_pilot::platform::Ios`].

use jev_pilot::act::{Catalog, Operation};
use jev_pilot::platform::{Ios, Platform};

const SCREEN: &str = r#"
<XCUIElementTypeApplication type="XCUIElementTypeApplication" name="Settings"
    enabled="true" visible="true" x="0" y="0" width="390" height="844">
  <XCUIElementTypeCell type="XCUIElementTypeCell" enabled="true" visible="true"
      x="0" y="100" width="390" height="44">
    <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" label="Wi-Fi"
        enabled="true" visible="true" x="16" y="110" width="100" height="20"/>
    <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" label="Not Connected"
        enabled="true" visible="true" x="200" y="110" width="150" height="20"/>
  </XCUIElementTypeCell>
  <XCUIElementTypeButton type="XCUIElementTypeButton" label="Done"
      enabled="true" visible="true" x="300" y="20" width="60" height="30"/>
  <XCUIElementTypeButton type="XCUIElementTypeButton" label="Hidden"
      enabled="true" visible="false" x="0" y="0" width="10" height="10"/>
  <XCUIElementTypeButton type="XCUIElementTypeButton" label="Greyed"
      enabled="false" visible="true" x="0" y="0" width="10" height="10"/>
</XCUIElementTypeApplication>
"#;

/// A cell carries its title and subtitle as child text, the same shape Android
/// uses, so the row keeps its own label rather than the leaves being listed.
///
/// This is the custom-cell and SwiftUI `List` shape, where each `Text` becomes
/// its own accessibility element under the row.
#[test]
fn a_cell_takes_the_label_of_the_text_it_contains() {
    let snapshot = Ios.parse_hierarchy(SCREEN).expect("fixture parses");

    let (_, cell) = snapshot
        .refs()
        .find(|(_, element)| &*element.label == "Wi-Fi")
        .expect("the Wi-Fi row is present");

    assert_eq!(cell.detail.as_deref(), Some("Not Connected"));
    assert_eq!(cell.bounds.center().y, 122);
}

/// Offscreen and disabled elements are in the hierarchy but cannot be acted on.
/// Offering them would be offering a choice that is certain to fail.
#[test]
fn invisible_and_disabled_elements_are_not_offered() {
    let snapshot = Ios.parse_hierarchy(SCREEN).expect("fixture parses");

    let labels: Vec<&str> = snapshot
        .refs()
        .map(|(_, element)| &*element.label)
        .collect();

    assert!(labels.contains(&"Done"));
    assert!(!labels.contains(&"Hidden"), "got {labels:?}");
    assert!(!labels.contains(&"Greyed"), "got {labels:?}");
}

/// iOS has no system Back, so a catalog built for it must not offer one.
#[test]
fn an_ios_catalog_offers_no_system_back() {
    let snapshot = Ios.parse_hierarchy(SCREEN).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Ios);

    assert!(!catalog.operations().contains(&Operation::Back));
    assert!(catalog.operations().contains(&Operation::Home));
    assert!(catalog.operations().contains(&Operation::Peek));
}

/// A default system cell style merges its title and subtitle into the cell's
/// own label, leaving no accessible text children to absorb. The row still has
/// to be described, so the cell's own label is used directly.
#[test]
fn a_cell_that_already_merged_its_text_keeps_that_label() {
    const MERGED: &str = r#"
    <XCUIElementTypeApplication type="XCUIElementTypeApplication" enabled="true" visible="true"
        x="0" y="0" width="390" height="844">
      <XCUIElementTypeCell type="XCUIElementTypeCell" label="Bluetooth, On"
          enabled="true" visible="true" x="0" y="200" width="390" height="44"/>
    </XCUIElementTypeApplication>
    "#;

    let snapshot = Ios.parse_hierarchy(MERGED).expect("fixture parses");
    let (_, cell) = snapshot.refs().next().expect("the row is present");

    assert_eq!(&*cell.label, "Bluetooth, On");
    assert_eq!(cell.detail, None);
}

/// Static text is a label, not a target: a `UILabel` is not user-interactive by
/// default. Offering one as a tap would be offering a choice that does nothing.
#[test]
fn bare_static_text_is_not_offered_as_an_action() {
    const HEADING: &str = r#"
    <XCUIElementTypeApplication type="XCUIElementTypeApplication" enabled="true" visible="true"
        x="0" y="0" width="390" height="844">
      <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" label="Settings"
          enabled="true" visible="true" x="16" y="60" width="200" height="34"/>
      <XCUIElementTypeButton type="XCUIElementTypeButton" label="Edit"
          enabled="true" visible="true" x="300" y="60" width="60" height="34"/>
    </XCUIElementTypeApplication>
    "#;

    let snapshot = Ios.parse_hierarchy(HEADING).expect("fixture parses");
    let labels: Vec<&str> = snapshot.refs().map(|(_, e)| &*e.label).collect();

    assert_eq!(labels, vec!["Edit"], "static text must not be a target");
}
