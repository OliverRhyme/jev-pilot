//! What differs between platforms: capability, not just parsing.

use jev_pilot::act::{Act, Catalog, Operation};
use jev_pilot::platform::{Android, Ios, Platform};

const SETTINGS: &str = include_str!("fixtures/settings.xml");

/// Platforms differ in what they can be told to do, not only in how their
/// hierarchy is spelled. Android has a system back; iOS has none, and offering
/// one would invite a choice the device cannot carry out. iOS has a force-touch
/// preview; Android has no equivalent.
#[test]
fn each_platform_offers_only_the_operations_it_has() {
    assert!(Android.operations().contains(&Operation::Back));
    assert!(!Ios.operations().contains(&Operation::Back));

    assert!(Ios.operations().contains(&Operation::Peek));
    assert!(!Android.operations().contains(&Operation::Peek));

    for platform in [&Android as &dyn Platform, &Ios] {
        for shared in [Operation::Tap, Operation::LongPress, Operation::Home] {
            assert!(
                platform.operations().contains(&shared),
                "{} should support {shared}",
                platform.name()
            );
        }
    }
}

/// The catalog offers exactly what the platform under test supports.
#[test]
fn a_catalog_never_offers_an_act_the_platform_lacks() {
    let snapshot = Android.parse_hierarchy(SETTINGS).expect("fixture parses");
    let catalog = Catalog::for_screen(&snapshot, &Ios);

    assert!(!catalog.operations().contains(&Operation::Back));
    let wire = serde_json::to_value(catalog.operation_question("anything")).expect("serializes");
    assert!(wire["criteria"].get("back").is_none(), "{wire}");
    assert!(wire["criteria"].get("peek").is_some(), "{wire}");

    // Act is still the vocabulary the device speaks.
    let _: fn(jev_pilot::snapshot::ElementRef) -> Act = Act::Tap;
}
